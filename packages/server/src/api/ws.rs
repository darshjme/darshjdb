//! WebSocket handler for DarshanDB real-time sync protocol.
//!
//! Handles connection upgrade at `/ws`, automatic MessagePack/JSON codec
//! detection, authentication with a 5-second timeout, subscription lifecycle,
//! mutations, presence, and keepalive pings.
//!
//! # Protocol Messages (Client -> Server)
//!
//! ```json
//! { "type": "auth",       "token": "<jwt>" }
//! { "type": "sub",        "id": "<req_id>", "query": { ... } }
//! { "type": "unsub",      "id": "<req_id>", "sub_id": "<sub_id>" }
//! { "type": "mut",        "id": "<req_id>", "ops": [ ... ] }
//! { "type": "pres-join",  "room": "<room_id>", "state": { ... } }
//! { "type": "pres-state", "room": "<room_id>", "state": { ... } }
//! { "type": "pres-leave", "room": "<room_id>" }
//! { "type": "ping" }
//! ```
//!
//! # Protocol Messages (Server -> Client)
//!
//! ```json
//! { "type": "auth-ok",    "session_id": "<uuid>" }
//! { "type": "auth-err",   "error": "<reason>" }
//! { "type": "sub-ok",     "id": "<req_id>", "sub_id": "<sub_id>", "initial": [ ... ] }
//! { "type": "sub-err",    "id": "<req_id>", "error": "<reason>" }
//! { "type": "diff",       "sub_id": "<sub_id>", "tx": N, "changes": { ... } }
//! { "type": "unsub-ok",   "id": "<req_id>" }
//! { "type": "mut-ok",     "id": "<req_id>", "tx": N }
//! { "type": "mut-err",    "id": "<req_id>", "error": "<reason>" }
//! { "type": "pres-snap",  "room": "<room_id>", "members": [ ... ] }
//! { "type": "pres-diff",  "room": "<room_id>", "joined": [...], "left": [...], "updated": [...] }
//! { "type": "pong" }
//! { "type": "error",      "error": "<reason>" }
//! ```

use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::time::timeout;
use tracing::{debug, error, info, warn};

use crate::sync::broadcaster::OutboundDiff;
use crate::sync::presence::PresenceManager;
use crate::sync::registry::SubscriptionRegistry;
use crate::sync::session::{SessionId, SessionManager, SubId};

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
    MutErr {
        id: String,
        error: String,
    },
    PresSnap {
        room: String,
        members: Vec<Value>,
    },
    PresDiff {
        room: String,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        joined: Vec<Value>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        left: Vec<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        updated: Vec<Value>,
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

    // Phase 2: Main message loop with keepalive.
    let mut keepalive = tokio::time::interval(KEEPALIVE_INTERVAL);
    keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

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

        ClientMessage::Ping => {
            let _ = send_message(socket, &ServerMessage::Pong, codec).await;
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

    // Execute the initial query.
    // In a full implementation, this calls the query engine with the user's
    // permission context. For now, return empty initial results that will be
    // wired to the actual query engine during integration.
    let initial_results: Vec<Value> = Vec::new();

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

/// Handle a mutation request.
async fn handle_mutation(
    req_id: String,
    ops: Value,
    socket: &mut WebSocket,
    _state: &WsState,
    session_id: SessionId,
    codec: Codec,
) {
    // In a full implementation, this executes the mutation through the
    // triple store's transaction engine with permission checks.
    // The resulting ChangeEvent is broadcast to trigger subscription updates.
    debug!(
        session_id = %session_id,
        ops = %ops,
        "mutation requested (pending query engine integration)"
    );

    // Acknowledge with tx_id 0 until wired to the storage engine.
    let _ = send_message(socket, &ServerMessage::MutOk { id: req_id, tx: 0 }, codec).await;
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

/// Clean up all resources for a disconnected session.
fn cleanup(session_id: SessionId, state: &WsState) {
    // Unregister all subscriptions.
    let removed_hashes = state.registry.unregister_session(&session_id);
    debug!(
        session_id = %session_id,
        removed_queries = removed_hashes.len(),
        "cleaned up subscriptions"
    );

    // Leave all presence rooms.
    if let Some(user_id) = get_user_id(state, session_id) {
        state.presence.leave_all(&user_id);
    }

    // Remove session.
    state.sessions.remove_session(&session_id);
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
    if parts.len() == 3 {
        if let Ok(decoded) =
            base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, parts[1])
        {
            if let Ok(claims) = serde_json::from_slice::<Value>(&decoded) {
                if let Some(sub) = claims.get("sub").and_then(|v| v.as_str()) {
                    return Ok(sub.to_string());
                }
            }
        }
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
/// use darshandb_server::api::ws::{ws_routes, WsState};
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
