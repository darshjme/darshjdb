//! WebSocket handler for DarshJDB real-time sync protocol.
//!
//! Handles connection upgrade at `/ws`, automatic MessagePack/JSON codec
//! detection, authentication with a 5-second timeout, subscription lifecycle,
//! mutations, presence, and keepalive pings.
//!
//! # Protocol Messages (Client -> Server)
//!
//! ```json
//! { "type": "auth",         "token": "<jwt>" }
//! { "type": "sub",          "id": "<req_id>", "query": { ... } }
//! { "type": "unsub",        "id": "<req_id>", "sub_id": "<sub_id>" }
//! { "type": "mut",          "id": "<req_id>", "ops": [ ... ] }
//! { "type": "live-select",  "id": "<req_id>", "query": "LIVE SELECT * FROM users WHERE age > 18" }
//! { "type": "kill",         "id": "<req_id>", "live_id": "<uuid>" }
//! { "type": "pres-join",    "room": "<room_id>", "state": { ... } }
//! { "type": "pres-state",   "room": "<room_id>", "state": { ... } }
//! { "type": "pres-leave",   "room": "<room_id>" }
//! { "type": "pub-sub",      "id": "<sub_id>", "channel": "entity:users:*" }
//! { "type": "pub-unsub",    "id": "<sub_id>" }
//! { "type": "ping" }
//! ```
//!
//! # Protocol Messages (Server -> Client)
//!
//! ```json
//! { "type": "auth-ok",        "session_id": "<uuid>" }
//! { "type": "auth-err",       "error": "<reason>" }
//! { "type": "sub-ok",         "id": "<req_id>", "sub_id": "<sub_id>", "initial": [ ... ] }
//! { "type": "sub-err",        "id": "<req_id>", "error": "<reason>" }
//! { "type": "diff",           "sub_id": "<sub_id>", "tx": N, "changes": { ... } }
//! { "type": "unsub-ok",       "id": "<req_id>" }
//! { "type": "mut-ok",         "id": "<req_id>", "tx": N }
//! { "type": "mut-err",        "id": "<req_id>", "error": "<reason>" }
//! { "type": "live-select-ok", "id": "<req_id>", "live_id": "<uuid>" }
//! { "type": "live-select-err","id": "<req_id>", "error": "<reason>" }
//! { "type": "live-event",     "live_id": "<uuid>", "action": "CREATE|UPDATE|DELETE", "result": { ... }, "tx_id": N }
//! { "type": "kill-ok",        "id": "<req_id>", "live_id": "<uuid>" }
//! { "type": "kill-err",       "id": "<req_id>", "error": "<reason>" }
//! { "type": "pres-snap",      "room": "<room_id>", "members": [ ... ] }
//! { "type": "pres-diff",      "room": "<room_id>", "joined": [...], "left": [...], "updated": [...] }
//! { "type": "pub-sub-ok",     "id": "<sub_id>", "channel": "entity:users:*" }
//! { "type": "pub-unsub-ok",   "id": "<sub_id>" }
//! { "type": "pub-event",      "id": "<sub_id>", "event": "updated", "entity_type": "users", "entity_id": "<uuid>", "changed": [...], "tx_id": N }
//! { "type": "pong" }
//! { "type": "error",          "error": "<reason>" }
//! ```

use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::time::timeout;
use tracing::{debug, info};

use crate::cache::QueryCache;
use crate::query;
use crate::rules::RuleEngine;
use crate::sync::broadcaster::{ChangeEvent, OutboundDiff};
use crate::sync::change_feed::ChangeFeed;
use crate::sync::live_query::{LiveAction, LiveQueryId, LiveQueryManager};
use crate::sync::presence::PresenceManager;
use crate::sync::pubsub::PubSubEngine;
use crate::sync::registry::SubscriptionRegistry;
use crate::sync::session::{SessionId, SessionManager, SubId};
use crate::triple_store::{PgTripleStore, TripleInput};

/// Auth timeout: clients must send an auth message within this window.
const AUTH_TIMEOUT: Duration = Duration::from_secs(5);

/// Keepalive interval: server sends ping if no message received.
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(30);

/// Maximum inbound message size (1 MiB).
const MAX_MESSAGE_SIZE: usize = 1024 * 1024;

/// Codec format detected from the first client message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Codec {
    Json,
    MessagePack,
}

/// Shared application state injected into the WebSocket handler.
#[derive(Clone)]
pub struct WsState {
    /// Shared session manager for all connections.
    pub sessions: Arc<SessionManager>,
    /// Shared subscription registry for fan-out deduplication.
    pub registry: Arc<SubscriptionRegistry>,
    /// Shared presence manager for room tracking.
    pub presence: Arc<PresenceManager>,
    /// Channel for receiving diffs from the broadcaster (unused sender kept for cloning).
    pub diff_tx: mpsc::Sender<OutboundDiff>,
    /// Postgres connection pool for query execution.
    pub pool: sqlx::PgPool,
    /// Triple store for query execution.
    pub triple_store: Arc<PgTripleStore>,
    /// Broadcast sender for change events (subscribe to receive mutations).
    pub change_tx: tokio::sync::broadcast::Sender<ChangeEvent>,
    /// Pub/sub engine for keyspace notification subscriptions.
    pub pubsub: Arc<PubSubEngine>,
    /// Live query manager for LIVE SELECT subscriptions.
    pub live_queries: Arc<LiveQueryManager>,
    /// Change feed for mutation logging and cursor-based replay.
    pub change_feed: Arc<ChangeFeed>,
    /// Optional forward-chaining rule engine — when present, mutations fire
    /// inference inside the same DB transaction so derived triples are atomic
    /// with the user's writes. Mirrors the REST mutation path for parity.
    pub rule_engine: Option<Arc<RuleEngine>>,
    /// Hot query cache shared with the REST handler. Mutations invalidate
    /// touched entity types post-commit so subsequent reads see fresh data.
    pub query_cache: Arc<QueryCache>,
}

/// Inbound client message (deserialized from JSON or MessagePack).
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum ClientMessage {
    Auth {
        token: String,
    },
    Sub {
        id: String,
        query: Value,
    },
    Unsub {
        id: String,
        sub_id: String,
    },
    Mut {
        id: String,
        ops: Value,
    },
    PresJoin {
        room: String,
        #[serde(default)]
        state: Value,
    },
    PresState {
        room: String,
        state: Value,
    },
    PresLeave {
        room: String,
    },
    /// LIVE SELECT: register a live query with SQL-like syntax.
    LiveSelect {
        id: String,
        query: String,
    },
    /// KILL: unsubscribe from a live query.
    Kill {
        id: String,
        live_id: String,
    },
    PubSub {
        id: String,
        channel: String,
    },
    PubUnsub {
        id: String,
    },
    /// Batch: execute multiple operations in a single WebSocket frame.
    #[allow(dead_code)]
    Batch {
        #[serde(default)]
        id: String,
        ops: Vec<Value>,
    },
    Ping,
}

/// Outbound server message (serialized to JSON or MessagePack).
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum ServerMessage {
    AuthOk {
        session_id: String,
    },
    AuthErr {
        error: String,
    },
    SubOk {
        id: String,
        sub_id: String,
        initial: Vec<Value>,
    },
    SubErr {
        id: String,
        error: String,
    },
    #[allow(dead_code)] // used by client protocol
    Diff {
        sub_id: String,
        tx: i64,
        changes: Value,
    },
    UnsubOk {
        id: String,
    },
    MutOk {
        id: String,
        tx: i64,
    },
    #[allow(dead_code)] // used by client protocol
    MutErr {
        id: String,
        error: String,
    },
    PresSnap {
        room: String,
        members: Vec<Value>,
    },
    #[allow(dead_code)] // used by client protocol
    PresDiff {
        room: String,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        joined: Vec<Value>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        left: Vec<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        updated: Vec<Value>,
    },
    /// LIVE SELECT registered successfully.
    LiveSelectOk {
        id: String,
        live_id: String,
    },
    /// LIVE SELECT registration failed.
    LiveSelectErr {
        id: String,
        error: String,
    },
    /// A live query event pushed when a matching change occurs.
    #[serde(rename = "live-event")]
    LiveEventMsg {
        live_id: String,
        action: String,
        result: Value,
        tx_id: i64,
    },
    /// KILL acknowledged.
    KillOk {
        id: String,
        live_id: String,
    },
    /// KILL failed.
    KillErr {
        id: String,
        error: String,
    },
    PubSubOk {
        id: String,
        channel: String,
    },
    PubUnsubOk {
        id: String,
    },
    PubEvent {
        id: String,
        event: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        entity_type: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        entity_id: Option<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        changed: Vec<String>,
        tx_id: i64,
        #[serde(skip_serializing_if = "Option::is_none")]
        payload: Option<Value>,
    },
    /// Batch result: contains results for all operations in a batch frame.
    BatchResult {
        id: String,
        results: Vec<Value>,
        duration_ms: f64,
    },
    Pong,
    Error {
        error: String,
    },
}

/// Axum handler for WebSocket upgrade at `/ws`.
///
/// Accepts the upgrade, extracts the peer address, and spawns the
/// connection handler as a background task.
pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<WsState>) -> impl IntoResponse {
    ws.max_message_size(MAX_MESSAGE_SIZE)
        .on_upgrade(move |socket| handle_connection(socket, state, None))
}

/// Main connection handler. Runs the full lifecycle:
/// auth -> message loop -> cleanup.
///
/// Uses a channel-based architecture: an internal `mpsc` channel buffers
/// outbound messages so the reader and writer halves of the WebSocket
/// operate independently without holding locks across awaits.
async fn handle_connection(
    mut socket: WebSocket,
    state: WsState,
    peer_addr: Option<std::net::SocketAddr>,
) {
    let session_id = state.sessions.create_session(peer_addr);

    info!(
        session_id = %session_id,
        peer_addr = ?peer_addr,
        "WebSocket connected"
    );

    // Phase 1: Authentication with timeout.
    let codec = match timeout(AUTH_TIMEOUT, authenticate(&mut socket, &state, session_id)).await {
        Ok(Ok(codec)) => codec,
        Ok(Err(e)) => {
            let err_msg = ServerMessage::AuthErr {
                error: e.to_string(),
            };
            let _ = send_message(&mut socket, &err_msg, Codec::Json).await;
            cleanup(session_id, &state);
            return;
        }
        Err(_) => {
            let err_msg = ServerMessage::AuthErr {
                error: "authentication timeout".to_string(),
            };
            let _ = send_message(&mut socket, &err_msg, Codec::Json).await;
            cleanup(session_id, &state);
            return;
        }
    };

    // Phase 2: Main message loop with keepalive and change notification.
    let mut keepalive = tokio::time::interval(KEEPALIVE_INTERVAL);
    keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut change_rx = state.change_tx.subscribe();

    loop {
        tokio::select! {
            biased;

            msg = socket.recv() => {
                match msg {
                    Some(Ok(msg)) => {
                        let should_close = process_inbound(
                            msg, &mut socket, &state, session_id, codec,
                        ).await;
                        if should_close {
                            break;
                        }
                        // Reset keepalive on any received message.
                        keepalive.reset();
                    }
                    Some(Err(e)) => {
                        debug!(session_id = %session_id, error = %e, "WebSocket read error");
                        break;
                    }
                    None => {
                        debug!(session_id = %session_id, "client closed connection");
                        break;
                    }
                }
            }
            // Listen for triple-store change events and push diffs to subscribed clients.
            change = change_rx.recv() => {
                match change {
                    Ok(event) => {
                        // Process live query subscriptions for this change event.
                        if handle_live_query_change(&event, &mut socket, &state, session_id, codec).await {
                            debug!(session_id = %session_id, "send failed during live query event, closing");
                            break;
                        }
                        // Process pub/sub subscriptions for this change event.
                        if handle_pubsub_change(&event, &mut socket, &state, session_id, codec).await {
                            debug!(session_id = %session_id, "send failed during pub/sub event, closing");
                            break;
                        }
                        // Process query subscriptions (existing behavior).
                        if handle_change_event(&event, &mut socket, &state, session_id, codec).await {
                            debug!(session_id = %session_id, "send failed during change event, closing");
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        debug!(session_id = %session_id, skipped = n, "change receiver lagged");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        debug!(session_id = %session_id, "change broadcast closed");
                        break;
                    }
                }
            }
            _ = keepalive.tick() => {
                // Send a WebSocket-level ping for liveness detection.
                if socket.send(Message::Ping(vec![].into())).await.is_err() {
                    debug!(session_id = %session_id, "keepalive ping failed, closing");
                    break;
                }
            }
        }
    }

    // Phase 3: Cleanup.
    cleanup(session_id, &state);
    info!(session_id = %session_id, "WebSocket disconnected");
}

/// Authenticate the client by waiting for an auth message.
async fn authenticate(
    socket: &mut WebSocket,
    state: &WsState,
    session_id: SessionId,
) -> Result<Codec, WsError> {
    loop {
        let msg = socket
            .recv()
            .await
            .ok_or_else(|| WsError::Transport("connection closed before auth".into()))?
            .map_err(|e| WsError::Transport(e.to_string()))?;

        let (parsed, codec) = match &msg {
            Message::Text(text) => {
                let parsed: ClientMessage = serde_json::from_str(text)
                    .map_err(|e| WsError::Protocol(format!("invalid auth message: {e}")))?;
                (parsed, Codec::Json)
            }
            Message::Binary(data) => {
                let parsed: ClientMessage = rmp_serde::from_slice(data)
                    .map_err(|e| WsError::Protocol(format!("invalid msgpack auth: {e}")))?;
                (parsed, Codec::MessagePack)
            }
            Message::Close(_) => {
                return Err(WsError::Transport("connection closed during auth".into()));
            }
            Message::Ping(_) | Message::Pong(_) => continue,
        };

        match parsed {
            ClientMessage::Auth { token } => match validate_token(&token) {
                Ok(user_id) => {
                    state.sessions.with_session_mut(&session_id, |s| {
                        s.authenticate(user_id.clone());
                    });

                    let ok_msg = ServerMessage::AuthOk {
                        session_id: session_id.to_string(),
                    };
                    send_message(socket, &ok_msg, codec).await?;

                    info!(
                        session_id = %session_id,
                        user_id = %user_id,
                        codec = ?codec,
                        "WebSocket authenticated"
                    );

                    return Ok(codec);
                }
                Err(reason) => {
                    return Err(WsError::AuthFailed(reason));
                }
            },
            _ => {
                return Err(WsError::Protocol("first message must be auth".into()));
            }
        }
    }
}

/// Process a single inbound WebSocket message. Returns `true` if the
/// connection should be closed.
async fn process_inbound(
    msg: Message,
    socket: &mut WebSocket,
    state: &WsState,
    session_id: SessionId,
    codec: Codec,
) -> bool {
    let parsed = match &msg {
        Message::Text(text) => match serde_json::from_str::<ClientMessage>(text) {
            Ok(m) => m,
            Err(e) => {
                let _ = send_message(
                    socket,
                    &ServerMessage::Error {
                        error: format!("invalid message: {e}"),
                    },
                    codec,
                )
                .await;
                return false;
            }
        },
        Message::Binary(data) => match rmp_serde::from_slice::<ClientMessage>(data) {
            Ok(m) => m,
            Err(e) => {
                let _ = send_message(
                    socket,
                    &ServerMessage::Error {
                        error: format!("invalid msgpack message: {e}"),
                    },
                    codec,
                )
                .await;
                return false;
            }
        },
        Message::Close(_) => return true,
        Message::Ping(_) | Message::Pong(_) => return false,
    };

    handle_message(parsed, socket, state, session_id, codec).await;
    false
}

/// Dispatch a parsed client message to the appropriate handler.
async fn handle_message(
    msg: ClientMessage,
    socket: &mut WebSocket,
    state: &WsState,
    session_id: SessionId,
    codec: Codec,
) {
    match msg {
        ClientMessage::Auth { .. } => {
            let _ = send_message(
                socket,
                &ServerMessage::Error {
                    error: "already authenticated".into(),
                },
                codec,
            )
            .await;
        }

        ClientMessage::Sub { id, query } => {
            handle_subscribe(id, query, socket, state, session_id, codec).await;
        }

        ClientMessage::Unsub { id, sub_id } => {
            handle_unsubscribe(id, sub_id, socket, state, session_id, codec).await;
        }

        ClientMessage::Mut { id, ops } => {
            handle_mutation(id, ops, socket, state, session_id, codec).await;
        }

        ClientMessage::PresJoin {
            room,
            state: pres_state,
        } => {
            handle_presence_join(room, pres_state, socket, state, session_id, codec).await;
        }

        ClientMessage::PresState {
            room,
            state: pres_state,
        } => {
            handle_presence_state(room, pres_state, state, session_id);
        }

        ClientMessage::PresLeave { room } => {
            handle_presence_leave(room, state, session_id);
        }

        ClientMessage::LiveSelect { id, query } => {
            handle_live_select(id, query, socket, state, session_id, codec).await;
        }

        ClientMessage::Kill { id, live_id } => {
            handle_kill(id, live_id, socket, state, session_id, codec).await;
        }

        ClientMessage::PubSub { id, channel } => {
            handle_pub_sub(id, channel, socket, state, session_id, codec).await;
        }

        ClientMessage::PubUnsub { id } => {
            handle_pub_unsub(id, socket, state, session_id, codec).await;
        }

        ClientMessage::Ping => {
            let _ = send_message(socket, &ServerMessage::Pong, codec).await;
        }

        ClientMessage::Batch { id: _, ops: _ } => {
            let _ = send_message(
                socket,
                &ServerMessage::Error {
                    error: "batch via WebSocket not yet implemented".into(),
                },
                codec,
            )
            .await;
        }
    }
}

/// Handle a subscribe request: register the subscription and send initial results.
async fn handle_subscribe(
    req_id: String,
    query: Value,
    socket: &mut WebSocket,
    state: &WsState,
    session_id: SessionId,
    codec: Codec,
) {
    // Compute query hash for deduplication.
    let query_hash = {
        use std::hash::{DefaultHasher, Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        let canonical = serde_json::to_string(&query).unwrap_or_default();
        canonical.hash(&mut hasher);
        hasher.finish()
    };

    // Add subscription to the session.
    let sub_id = match state.sessions.with_session_mut(&session_id, |s| {
        s.add_subscription(query_hash, query.clone())
    }) {
        Some(id) => id,
        None => {
            let _ = send_message(
                socket,
                &ServerMessage::SubErr {
                    id: req_id,
                    error: "session not found".into(),
                },
                codec,
            )
            .await;
            return;
        }
    };

    // Register in the global registry for fan-out.
    state.registry.register(query_hash, session_id, sub_id);

    // Execute the initial query against the real query engine.
    let initial_results: Vec<Value> = match query::parse_darshan_ql(&query) {
        Ok(ast) => match query::plan_query(&ast) {
            Ok(plan) => match query::execute_query(&state.pool, &plan).await {
                Ok(rows) => rows
                    .into_iter()
                    .map(|row| {
                        let mut obj = serde_json::Map::new();
                        obj.insert("_id".to_string(), Value::String(row.entity_id.to_string()));
                        for (k, v) in row.attributes {
                            obj.insert(k, v);
                        }
                        Value::Object(obj)
                    })
                    .collect(),
                Err(e) => {
                    debug!(error = %e, "initial query execution failed");
                    Vec::new()
                }
            },
            Err(e) => {
                debug!(error = %e, "query planning failed");
                Vec::new()
            }
        },
        Err(e) => {
            debug!(error = %e, "query parsing failed");
            Vec::new()
        }
    };

    let _ = send_message(
        socket,
        &ServerMessage::SubOk {
            id: req_id,
            sub_id: sub_id.to_string(),
            initial: initial_results,
        },
        codec,
    )
    .await;

    debug!(
        session_id = %session_id,
        sub_id = %sub_id,
        query_hash = query_hash,
        "subscription registered"
    );
}

/// Handle an unsubscribe request.
async fn handle_unsubscribe(
    req_id: String,
    sub_id_str: String,
    socket: &mut WebSocket,
    state: &WsState,
    session_id: SessionId,
    codec: Codec,
) {
    let sub_id: SubId = match sub_id_str.parse() {
        Ok(id) => id,
        Err(_) => {
            let _ = send_message(
                socket,
                &ServerMessage::Error {
                    error: "invalid sub_id format".into(),
                },
                codec,
            )
            .await;
            return;
        }
    };

    // Get the query hash before removing, for registry cleanup.
    let query_hash = state.sessions.with_session_mut(&session_id, |s| {
        s.remove_subscription(&sub_id).map(|sub| sub.query_hash)
    });

    if let Some(Some(hash)) = query_hash {
        state.registry.unregister(hash, session_id, sub_id);
    }

    let _ = send_message(socket, &ServerMessage::UnsubOk { id: req_id }, codec).await;

    debug!(session_id = %session_id, sub_id = %sub_id, "subscription removed");
}

/// Handle a mutation request via the triple store transaction engine.
async fn handle_mutation(
    req_id: String,
    ops: Value,
    socket: &mut WebSocket,
    state: &WsState,
    session_id: SessionId,
    codec: Codec,
) {
    let ops_array = match ops.as_array() {
        Some(a) if !a.is_empty() => a.clone(),
        _ => {
            let _ = send_message(
                socket,
                &ServerMessage::MutErr {
                    id: req_id,
                    error: "ops must be a non-empty array".into(),
                },
                codec,
            )
            .await;
            return;
        }
    };

    let mut db_tx = match state.triple_store.begin_tx().await {
        Ok(t) => t,
        Err(e) => {
            let _ = send_message(
                socket,
                &ServerMessage::MutErr {
                    id: req_id,
                    error: format!("begin tx: {e}"),
                },
                codec,
            )
            .await;
            return;
        }
    };
    let tx_id = match PgTripleStore::next_tx_id_in_tx(&mut db_tx).await {
        Ok(i) => i,
        Err(e) => {
            let _ = send_message(
                socket,
                &ServerMessage::MutErr {
                    id: req_id,
                    error: format!("alloc tx_id: {e}"),
                },
                codec,
            )
            .await;
            return;
        }
    };

    let mut all_triples: Vec<TripleInput> = Vec::new();
    let mut entity_ids: Vec<String> = Vec::new();
    let mut entity_types: Vec<String> = Vec::new();

    for op_val in &ops_array {
        let op_str = op_val.get("op").and_then(|v| v.as_str()).unwrap_or("");
        let entity = op_val.get("entity").and_then(|v| v.as_str()).unwrap_or("");
        let id_str = op_val.get("id").and_then(|v| v.as_str());
        let data = op_val.get("data");
        if entity.is_empty() {
            let _ = send_message(
                socket,
                &ServerMessage::MutErr {
                    id: req_id,
                    error: "each op requires 'entity'".into(),
                },
                codec,
            )
            .await;
            return;
        }
        if !entity_types.contains(&entity.to_string()) {
            entity_types.push(entity.to_string());
        }

        match op_str {
            "insert" => {
                let eid = id_str
                    .and_then(|s| uuid::Uuid::parse_str(s).ok())
                    .unwrap_or_else(uuid::Uuid::new_v4);
                entity_ids.push(eid.to_string());
                all_triples.push(TripleInput {
                    entity_id: eid,
                    attribute: ":db/type".into(),
                    value: Value::String(entity.into()),
                    value_type: 0,
                    ttl_seconds: None,
                });
                if let Some(obj) = data.and_then(|d| d.as_object()) {
                    for (k, v) in obj {
                        all_triples.push(TripleInput {
                            entity_id: eid,
                            attribute: format!("{entity}/{k}"),
                            value: v.clone(),
                            value_type: ws_vtype(v),
                            ttl_seconds: None,
                        });
                    }
                }
            }
            "update" => {
                let eid = match id_str.and_then(|s| uuid::Uuid::parse_str(s).ok()) {
                    Some(i) => i,
                    None => {
                        let _ = send_message(
                            socket,
                            &ServerMessage::MutErr {
                                id: req_id,
                                error: "update requires 'id'".into(),
                            },
                            codec,
                        )
                        .await;
                        return;
                    }
                };
                entity_ids.push(eid.to_string());
                if let Some(obj) = data.and_then(|d| d.as_object()) {
                    for (k, _) in obj {
                        if let Err(e) =
                            PgTripleStore::retract_in_tx(&mut db_tx, eid, &format!("{entity}/{k}"))
                                .await
                        {
                            let _ = send_message(
                                socket,
                                &ServerMessage::MutErr {
                                    id: req_id,
                                    error: format!("retract: {e}"),
                                },
                                codec,
                            )
                            .await;
                            return;
                        }
                    }
                    for (k, v) in obj {
                        all_triples.push(TripleInput {
                            entity_id: eid,
                            attribute: format!("{entity}/{k}"),
                            value: v.clone(),
                            value_type: ws_vtype(v),
                            ttl_seconds: None,
                        });
                    }
                }
            }
            "delete" => {
                let eid = match id_str.and_then(|s| uuid::Uuid::parse_str(s).ok()) {
                    Some(i) => i,
                    None => {
                        let _ = send_message(
                            socket,
                            &ServerMessage::MutErr {
                                id: req_id,
                                error: "delete requires 'id'".into(),
                            },
                            codec,
                        )
                        .await;
                        return;
                    }
                };
                entity_ids.push(eid.to_string());
                let existing = match PgTripleStore::get_entity_in_tx(&mut db_tx, eid).await {
                    Ok(t) => t,
                    Err(e) => {
                        let _ = send_message(
                            socket,
                            &ServerMessage::MutErr {
                                id: req_id,
                                error: format!("fetch: {e}"),
                            },
                            codec,
                        )
                        .await;
                        return;
                    }
                };
                for t in &existing {
                    if let Err(e) =
                        PgTripleStore::retract_in_tx(&mut db_tx, eid, &t.attribute).await
                    {
                        let _ = send_message(
                            socket,
                            &ServerMessage::MutErr {
                                id: req_id,
                                error: format!("retract: {e}"),
                            },
                            codec,
                        )
                        .await;
                        return;
                    }
                }
            }
            _ => {
                let _ = send_message(
                    socket,
                    &ServerMessage::MutErr {
                        id: req_id,
                        error: format!("unknown op '{op_str}'"),
                    },
                    codec,
                )
                .await;
                return;
            }
        }
    }

    if !all_triples.is_empty()
        && let Err(e) = PgTripleStore::set_triples_in_tx(&mut db_tx, &all_triples, tx_id).await
    {
        let _ = send_message(
            socket,
            &ServerMessage::MutErr {
                id: req_id,
                error: format!("write: {e}"),
            },
            codec,
        )
        .await;
        return;
    }

    // Forward-chaining rule engine: implied triples are written in the same
    // transaction so the user's mutation + inferred facts commit atomically.
    // Mirrors the REST mutation path to keep WS clients and REST clients in
    // parity with respect to derived knowledge. Dropping `db_tx` on the
    // error path causes an implicit rollback of the user writes above.
    let mut implied_triples: Vec<TripleInput> = Vec::new();
    if !all_triples.is_empty()
        && let Some(ref rule_engine) = state.rule_engine
    {
        match rule_engine
            .evaluate_and_write_in_tx(&mut db_tx, &all_triples, tx_id)
            .await
        {
            Ok(implied) => implied_triples = implied,
            Err(e) => {
                let _ = send_message(
                    socket,
                    &ServerMessage::MutErr {
                        id: req_id,
                        error: format!("rule engine: {e}"),
                    },
                    codec,
                )
                .await;
                return;
            }
        }
    }

    if let Err(e) = db_tx.commit().await {
        let _ = send_message(
            socket,
            &ServerMessage::MutErr {
                id: req_id,
                error: format!("commit: {e}"),
            },
            codec,
        )
        .await;
        return;
    }

    // Invalidate hot cache for every entity type touched by this mutation so
    // subsequent reads on those types recompute against fresh triples.
    for et in &entity_types {
        state.query_cache.invalidate_by_entity_type(et);
    }

    // Collect attributes touched (including rule-implied) for the change
    // notification so live queries see the full effect of the transaction.
    let mut touched_attributes: Vec<String> = all_triples
        .iter()
        .chain(implied_triples.iter())
        .map(|t| t.attribute.clone())
        .collect();
    touched_attributes.sort();
    touched_attributes.dedup();

    let change_event = ChangeEvent {
        tx_id,
        entity_ids: entity_ids.clone(),
        attributes: touched_attributes,
        entity_type: entity_types.into_iter().next(),
        actor_id: None,
    };

    // Determine action for change feed logging.
    let feed_action = ops_array
        .first()
        .and_then(|op| op.get("op").and_then(|v| v.as_str()))
        .unwrap_or("UPDATE");
    state.change_feed.append(&change_event, feed_action);

    let _ = state.change_tx.send(change_event);
    debug!(session_id = %session_id, tx_id = tx_id, "ws mutation committed");
    let _ = send_message(
        socket,
        &ServerMessage::MutOk {
            id: req_id,
            tx: tx_id,
        },
        codec,
    )
    .await;
}

/// Infer triple value_type from JSON (WebSocket context).
fn ws_vtype(v: &Value) -> i16 {
    match v {
        Value::String(s) if s.len() == 36 && uuid::Uuid::parse_str(s).is_ok() => 5,
        Value::String(_) => 0,
        Value::Number(n) if n.is_f64() && !n.is_i64() && !n.is_u64() => 2,
        Value::Number(_) => 1,
        Value::Bool(_) => 3,
        Value::Object(_) | Value::Array(_) => 6,
        Value::Null => 0,
    }
}

/// Handle a presence join request.
async fn handle_presence_join(
    room: String,
    pres_state: Value,
    socket: &mut WebSocket,
    state: &WsState,
    session_id: SessionId,
    codec: Codec,
) {
    let user_id = match get_user_id(state, session_id) {
        Some(uid) => uid,
        None => {
            let _ = send_message(
                socket,
                &ServerMessage::Error {
                    error: "not authenticated".into(),
                },
                codec,
            )
            .await;
            return;
        }
    };

    let accepted = state.presence.join(&room, &user_id, pres_state);

    if !accepted {
        let _ = send_message(
            socket,
            &ServerMessage::Error {
                error: "presence update rate-limited".into(),
            },
            codec,
        )
        .await;
        return;
    }

    // Send current room snapshot.
    let members: Vec<Value> = state
        .presence
        .room_snapshot(&room)
        .into_iter()
        .map(|(uid, st)| {
            serde_json::json!({
                "user_id": uid,
                "state": st,
            })
        })
        .collect();

    let _ = send_message(socket, &ServerMessage::PresSnap { room, members }, codec).await;
}

/// Handle a presence state update.
fn handle_presence_state(room: String, pres_state: Value, state: &WsState, session_id: SessionId) {
    if let Some(user_id) = get_user_id(state, session_id) {
        state.presence.update_state(&room, &user_id, pres_state);
    }
}

/// Handle a presence leave.
fn handle_presence_leave(room: String, state: &WsState, session_id: SessionId) {
    if let Some(user_id) = get_user_id(state, session_id) {
        state.presence.leave(&room, &user_id);
    }
}

/// Extract user_id from the session.
fn get_user_id(state: &WsState, session_id: SessionId) -> Option<String> {
    state
        .sessions
        .with_session(&session_id, |s| s.user_id.clone())
        .flatten()
}

/// Handle a change event from the triple store: for each of this session's
/// subscriptions, check if the change is relevant, re-execute the query,
/// compute a diff, and send it to the client.
///
/// Returns `true` if a send failed and the connection should be closed.
async fn handle_change_event(
    event: &ChangeEvent,
    socket: &mut WebSocket,
    state: &WsState,
    session_id: SessionId,
    codec: Codec,
) -> bool {
    // Get all subscriptions for this session.
    let subs: Vec<(SubId, Value, u64)> = state
        .sessions
        .with_session(&session_id, |s| {
            s.subscriptions
                .iter()
                .map(|(sub_id, sub)| (*sub_id, sub.query_ast.clone(), sub.query_hash))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if subs.is_empty() {
        return false;
    }

    for (sub_id, query_ast, _query_hash) in subs {
        // Check if this change could affect this subscription by matching entity type.
        // A simple heuristic: if the query's "type" field matches the event's entity_type.
        let query_type = query_ast.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if let Some(ref event_type) = event.entity_type
            && !query_type.is_empty()
            && query_type != event_type
        {
            continue;
        }

        // Re-execute the query.
        let new_results: Vec<Value> = match query::parse_darshan_ql(&query_ast) {
            Ok(ast) => match query::plan_query(&ast) {
                Ok(plan) => match query::execute_query(&state.pool, &plan).await {
                    Ok(rows) => rows
                        .into_iter()
                        .map(|row| {
                            let mut obj = serde_json::Map::new();
                            obj.insert("_id".to_string(), Value::String(row.entity_id.to_string()));
                            for (k, v) in row.attributes {
                                obj.insert(k, v);
                            }
                            Value::Object(obj)
                        })
                        .collect(),
                    Err(_) => continue,
                },
                Err(_) => continue,
            },
            Err(_) => continue,
        };

        // Compute diff against empty (first notification) or previous results.
        // For simplicity, we send the full new result set as "added" on the
        // first change. A production implementation would cache previous results
        // per (session, sub) in the Broadcaster's result_cache.
        let diff_value = serde_json::json!({
            "added": new_results,
            "removed": [],
            "updated": []
        });

        if send_message(
            socket,
            &ServerMessage::Diff {
                sub_id: sub_id.to_string(),
                tx: event.tx_id,
                changes: diff_value,
            },
            codec,
        )
        .await
        .is_err()
        {
            return true;
        }
    }

    false
}

/// Clean up all resources for a disconnected session.
fn cleanup(session_id: SessionId, state: &WsState) {
    // Unregister all query subscriptions.
    let removed_hashes = state.registry.unregister_session(&session_id);
    debug!(
        session_id = %session_id,
        removed_queries = removed_hashes.len(),
        "cleaned up subscriptions"
    );

    // Kill all live queries for this session.
    let removed_live = state.live_queries.kill_session(&session_id);
    if removed_live > 0 {
        debug!(
            session_id = %session_id,
            removed_live = removed_live,
            "cleaned up live queries"
        );
    }

    // Unregister all pub/sub subscriptions.
    let removed_pubsub = state.pubsub.unsubscribe_all(&session_id.to_string());
    if removed_pubsub > 0 {
        debug!(
            session_id = %session_id,
            removed_pubsub = removed_pubsub,
            "cleaned up pub/sub subscriptions"
        );
    }

    // Leave all presence rooms.
    if let Some(user_id) = get_user_id(state, session_id) {
        state.presence.leave_all(&user_id);
    }

    // Remove session.
    state.sessions.remove_session(&session_id);
}

/// Handle a LIVE SELECT request: parse the query, register the live subscription,
/// and return the assigned live query ID.
async fn handle_live_select(
    req_id: String,
    query_str: String,
    socket: &mut WebSocket,
    state: &WsState,
    session_id: SessionId,
    codec: Codec,
) {
    match state.live_queries.register(session_id, &query_str) {
        Ok(live_id) => {
            let _ = send_message(
                socket,
                &ServerMessage::LiveSelectOk {
                    id: req_id,
                    live_id: live_id.to_string(),
                },
                codec,
            )
            .await;

            debug!(
                session_id = %session_id,
                live_id = %live_id,
                query = %query_str,
                "live query registered"
            );
        }
        Err(e) => {
            let _ = send_message(
                socket,
                &ServerMessage::LiveSelectErr {
                    id: req_id,
                    error: e,
                },
                codec,
            )
            .await;
        }
    }
}

/// Handle a KILL request: unsubscribe from a live query.
async fn handle_kill(
    req_id: String,
    live_id_str: String,
    socket: &mut WebSocket,
    state: &WsState,
    session_id: SessionId,
    codec: Codec,
) {
    let live_id: LiveQueryId = match live_id_str.parse() {
        Ok(id) => id,
        Err(_) => {
            let _ = send_message(
                socket,
                &ServerMessage::KillErr {
                    id: req_id,
                    error: "invalid live_id format".into(),
                },
                codec,
            )
            .await;
            return;
        }
    };

    if state.live_queries.kill(&live_id, &session_id) {
        let _ = send_message(
            socket,
            &ServerMessage::KillOk {
                id: req_id,
                live_id: live_id.to_string(),
            },
            codec,
        )
        .await;

        debug!(
            session_id = %session_id,
            live_id = %live_id,
            "live query killed"
        );
    } else {
        let _ = send_message(
            socket,
            &ServerMessage::KillErr {
                id: req_id,
                error: format!("live query '{live_id}' not found or not owned by this session"),
            },
            codec,
        )
        .await;
    }
}

/// Process a change event through the live query engine and push matching
/// events to this WebSocket client.
///
/// Fetches post-mutation entity data from the triple store, evaluates each
/// live query's filter, and sends [`LiveEvent`] messages for matches.
///
/// Returns `true` if a send failed and the connection should be closed.
async fn handle_live_query_change(
    event: &ChangeEvent,
    socket: &mut WebSocket,
    state: &WsState,
    session_id: SessionId,
    codec: Codec,
) -> bool {
    // Check if this session has any live queries at all (fast path).
    let session_live_ids = state.live_queries.session_queries(&session_id);
    if session_live_ids.is_empty() {
        return false;
    }

    // Fetch post-mutation entity data for filter evaluation.
    // Uses a fresh transaction per entity to read committed state.
    let mut entity_data = std::collections::HashMap::new();
    for entity_id_str in &event.entity_ids {
        if let Ok(eid) = uuid::Uuid::parse_str(entity_id_str) {
            match state.triple_store.begin_tx().await {
                Ok(mut tx) => {
                    match PgTripleStore::get_entity_in_tx(&mut tx, eid).await {
                        Ok(triples) => {
                            let mut obj = serde_json::Map::new();
                            obj.insert("_id".to_string(), Value::String(eid.to_string()));
                            for t in &triples {
                                // Strip the entity-type prefix from attribute names.
                                let attr_name =
                                    t.attribute.split('/').next_back().unwrap_or(&t.attribute);
                                if attr_name != ":db/type" && !t.attribute.starts_with(":db/") {
                                    obj.insert(attr_name.to_string(), t.value.clone());
                                }
                            }
                            entity_data.insert(entity_id_str.clone(), Value::Object(obj));
                        }
                        Err(_) => {
                            // Entity may have been deleted; provide a minimal record.
                            let obj = serde_json::json!({"_id": entity_id_str});
                            entity_data.insert(entity_id_str.clone(), obj);
                        }
                    }
                    // Read-only tx, just drop it (implicit rollback is fine).
                    let _ = tx.rollback().await;
                }
                Err(_) => {
                    let obj = serde_json::json!({"_id": entity_id_str});
                    entity_data.insert(entity_id_str.clone(), obj);
                }
            }
        }
    }

    // Determine the action type from the event heuristics.
    let action = if entity_data
        .values()
        .all(|v| v.as_object().is_none_or(|o| o.len() <= 1))
    {
        LiveAction::Delete
    } else if event.tx_id > 0 && event.entity_ids.len() == 1 {
        // Heuristic: single entity with data is likely create or update.
        LiveAction::Update
    } else {
        LiveAction::Update
    };

    // Evaluate live queries and push events.
    let events = state
        .live_queries
        .process_change(event, &entity_data, action);

    for (target_session, live_event) in events {
        if target_session != session_id {
            continue;
        }

        if send_message(
            socket,
            &ServerMessage::LiveEventMsg {
                live_id: live_event.live_id.to_string(),
                action: live_event.action.to_string(),
                result: live_event.result,
                tx_id: live_event.tx_id,
            },
            codec,
        )
        .await
        .is_err()
        {
            return true;
        }
    }

    false
}

/// Handle a pub/sub subscribe request.
async fn handle_pub_sub(
    id: String,
    channel: String,
    socket: &mut WebSocket,
    state: &WsState,
    session_id: SessionId,
    codec: Codec,
) {
    if channel.is_empty() {
        let _ = send_message(
            socket,
            &ServerMessage::Error {
                error: "channel pattern is required".into(),
            },
            codec,
        )
        .await;
        return;
    }

    let subscriber = session_id.to_string();
    let pattern = state.pubsub.subscribe(&subscriber, &id, &channel);

    let _ = send_message(
        socket,
        &ServerMessage::PubSubOk {
            id: id.clone(),
            channel: pattern.raw,
        },
        codec,
    )
    .await;

    debug!(
        session_id = %session_id,
        sub_id = %id,
        channel = %channel,
        "pub/sub subscription registered"
    );
}

/// Handle a pub/sub unsubscribe request.
async fn handle_pub_unsub(
    id: String,
    socket: &mut WebSocket,
    state: &WsState,
    session_id: SessionId,
    codec: Codec,
) {
    let subscriber = session_id.to_string();
    let removed = state.pubsub.unsubscribe(&subscriber, &id);

    if !removed {
        let _ = send_message(
            socket,
            &ServerMessage::Error {
                error: format!("pub/sub subscription '{id}' not found"),
            },
            codec,
        )
        .await;
        return;
    }

    let _ = send_message(socket, &ServerMessage::PubUnsubOk { id: id.clone() }, codec).await;

    debug!(
        session_id = %session_id,
        sub_id = %id,
        "pub/sub subscription removed"
    );
}

/// Process a change event through the pub/sub engine and send matching events
/// to this WebSocket client.
///
/// Returns `true` if a send failed and the connection should be closed.
async fn handle_pubsub_change(
    event: &ChangeEvent,
    socket: &mut WebSocket,
    state: &WsState,
    session_id: SessionId,
    codec: Codec,
) -> bool {
    let subscriber = session_id.to_string();
    let matches = state.pubsub.process_change_event(event);

    for (sub, sub_id, pub_event) in matches {
        if sub != subscriber {
            continue;
        }

        if send_message(
            socket,
            &ServerMessage::PubEvent {
                id: sub_id,
                event: pub_event.event,
                entity_type: pub_event.entity_type,
                entity_id: pub_event.entity_id,
                changed: pub_event.changed,
                tx_id: pub_event.tx_id,
                payload: pub_event.payload,
            },
            codec,
        )
        .await
        .is_err()
        {
            return true;
        }
    }

    false
}

/// Handle a batch of operations sent in a single WebSocket frame.
///
/// Each operation in `ops` is a JSON object with a `"t"` field identifying
/// the operation type. Operations are executed sequentially; results are
/// collected and returned as a single `batch-result` message.
#[allow(dead_code)] // called from handle_message match arm
async fn handle_ws_batch(
    batch_id: String,
    ops: Vec<Value>,
    socket: &mut WebSocket,
    state: &WsState,
    session_id: SessionId,
    codec: Codec,
) {
    let start = std::time::Instant::now();

    if ops.is_empty() {
        let _ = send_message(
            socket,
            &ServerMessage::Error {
                error: "batch ops array is empty".into(),
            },
            codec,
        )
        .await;
        return;
    }

    if ops.len() > 50 {
        let _ = send_message(
            socket,
            &ServerMessage::Error {
                error: format!("batch exceeds 50 ops limit (got {})", ops.len()),
            },
            codec,
        )
        .await;
        return;
    }

    let mut results: Vec<Value> = Vec::with_capacity(ops.len());

    for op in &ops {
        let op_type = op
            .get("t")
            .or_else(|| op.get("type"))
            .and_then(|t| t.as_str());

        match op_type {
            Some("sub") => {
                let id = op
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let query_val = op.get("query").cloned().unwrap_or(Value::Null);
                let query_result = match crate::query::parse_darshan_ql(&query_val) {
                    Ok(ast) => match crate::query::plan_query(&ast) {
                        Ok(plan) => match crate::query::execute_query(&state.pool, &plan).await {
                            Ok(rows) => {
                                let initial: Vec<Value> = rows
                                    .into_iter()
                                    .map(|row| {
                                        let mut obj = serde_json::Map::new();
                                        obj.insert(
                                            "_id".to_string(),
                                            Value::String(row.entity_id.to_string()),
                                        );
                                        for (k, v) in row.attributes {
                                            obj.insert(k, v);
                                        }
                                        Value::Object(obj)
                                    })
                                    .collect();
                                serde_json::json!({
                                    "t": "sub-ok", "id": id, "initial": initial
                                })
                            }
                            Err(e) => serde_json::json!({
                                "t": "sub-err", "id": id,
                                "error": format!("query failed: {e}")
                            }),
                        },
                        Err(e) => serde_json::json!({
                            "t": "sub-err", "id": id,
                            "error": format!("plan failed: {e}")
                        }),
                    },
                    Err(e) => serde_json::json!({
                        "t": "sub-err", "id": id,
                        "error": format!("parse failed: {e}")
                    }),
                };
                results.push(query_result);
            }
            Some("mut") => {
                let id = op
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let ops_val = op.get("ops").cloned().unwrap_or(Value::Null);
                handle_mutation(id.clone(), ops_val, socket, state, session_id, codec).await;
                results.push(serde_json::json!({ "t": "mut-ok", "id": id }));
            }
            Some("unsub") => {
                let id = op
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let sub_id = op
                    .get("sub_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                handle_unsubscribe(id.clone(), sub_id, socket, state, session_id, codec).await;
                results.push(serde_json::json!({ "t": "unsub-ok", "id": id }));
            }
            Some("ping") => {
                results.push(serde_json::json!({ "t": "pong" }));
            }
            Some(unknown) => {
                results.push(serde_json::json!({
                    "t": "error",
                    "error": format!("unknown batch op type: {unknown}")
                }));
            }
            None => {
                results.push(serde_json::json!({
                    "t": "error",
                    "error": "missing 't' (type) field in batch op"
                }));
            }
        }
    }

    let duration_ms = start.elapsed().as_secs_f64() * 1000.0;
    let _ = send_message(
        socket,
        &ServerMessage::BatchResult {
            id: batch_id,
            results,
            duration_ms,
        },
        codec,
    )
    .await;
}

/// Send a server message over the WebSocket using the detected codec.
async fn send_message(
    socket: &mut WebSocket,
    msg: &ServerMessage,
    codec: Codec,
) -> Result<(), WsError> {
    let ws_msg = match codec {
        Codec::Json => {
            let payload = serde_json::to_string(msg).map_err(|e| WsError::Codec(e.to_string()))?;
            Message::Text(payload.into())
        }
        Codec::MessagePack => {
            let payload = rmp_serde::to_vec(msg).map_err(|e| WsError::Codec(e.to_string()))?;
            Message::Binary(payload.into())
        }
    };

    socket
        .send(ws_msg)
        .await
        .map_err(|e| WsError::Transport(e.to_string()))
}

/// Validate a JWT token and extract the user ID.
///
/// In production, this will be wired to the auth subsystem's [`KeyManager`]
/// and [`SessionManager`] for full JWT validation (signature, expiry, revocation).
/// During development, it does lenient parsing: it attempts to extract the `sub`
/// claim from the JWT payload, falling back to treating the token as a user ID.
fn validate_token(token: &str) -> Result<String, String> {
    if token.is_empty() {
        return Err("empty token".to_string());
    }

    // Attempt to decode as a JWT and extract the `sub` claim.
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() == 3
        && let Ok(decoded) =
            base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, parts[1])
        && let Ok(claims) = serde_json::from_slice::<Value>(&decoded)
        && let Some(sub) = claims.get("sub").and_then(|v| v.as_str())
    {
        return Ok(sub.to_string());
    }

    // Fallback: treat the raw token as a user identifier (dev mode only).
    Ok(token.to_string())
}

/// Errors specific to the WebSocket subsystem.
#[derive(Debug, thiserror::Error)]
pub enum WsError {
    /// Client did not authenticate within the timeout window.
    #[error("authentication timed out")]
    AuthTimeout,

    /// Authentication credentials were rejected.
    #[error("authentication failed: {0}")]
    AuthFailed(String),

    /// Protocol violation (wrong message sequence, malformed data).
    #[error("protocol error: {0}")]
    Protocol(String),

    /// Underlying transport error.
    #[error("transport error: {0}")]
    Transport(String),

    /// Serialization/deserialization error.
    #[error("codec error: {0}")]
    Codec(String),
}

/// Build the WebSocket route for inclusion in the Axum router.
///
/// # Example
///
/// ```rust,ignore
/// use axum::Router;
/// use ddb_server::api::ws::{ws_routes, WsState};
///
/// let ws_state = WsState { /* ... */ };
/// let app = Router::new()
///     .merge(ws_routes(ws_state));
/// ```
pub fn ws_routes(state: WsState) -> axum::Router {
    use axum::routing::any;

    axum::Router::new()
        .route("/ws", any(ws_handler))
        .with_state(state)
}
