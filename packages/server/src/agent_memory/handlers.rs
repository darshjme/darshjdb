// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
//! REST handlers + sub-router for the agent-memory subsystem.
//!
//! Mounted by `api::rest::build_router` under the protected route tree
//! so every endpoint inherits JWT authentication via the standard
//! `require_auth_middleware` layer.

use axum::{
    Extension, Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::AuthContext;

use super::context::{ContextBuildOptions, ContextBuilder};
use super::repo::AgentMemoryRepo;
use super::tokens::TiktokenCounter;
use super::types::{
    AgentFact, AgentSession, ContextMessage, MemoryEntry, MemoryRole, MemoryTier, TimelineFilter,
};
use super::working::WorkingMemory;

// ---------------------------------------------------------------------------
// Sub-router state
// ---------------------------------------------------------------------------

/// State threaded into every agent-memory handler.
#[derive(Clone)]
pub struct AgentMemoryState {
    /// Postgres pool — shared with the rest of the server.
    pub pool: PgPool,
    /// Persistent repo handle.
    pub repo: AgentMemoryRepo,
    /// In-memory working tier shared across requests.
    pub working: WorkingMemory,
}

impl AgentMemoryState {
    /// Build a fresh state from a pool.
    pub fn new(pool: PgPool) -> Self {
        let repo = AgentMemoryRepo::new(pool.clone());
        Self {
            pool,
            repo,
            working: WorkingMemory::default(),
        }
    }
}

/// Build the agent-memory sub-router. Mount at `/api/agent` from
/// `build_router`.
pub fn agent_memory_routes() -> Router<AgentMemoryState> {
    Router::new()
        .route("/sessions", post(create_session))
        .route("/sessions/{id}", delete(close_session))
        .route("/sessions/{id}/messages", post(insert_message))
        .route("/sessions/{id}/context", get(get_context))
        .route("/sessions/{id}/context/export", get(export_context))
        .route("/sessions/{id}/search", post(search_session))
        .route("/sessions/{id}/timeline", get(get_timeline))
        .route("/sessions/{id}/stats", get(get_stats))
        .route("/facts", post(upsert_fact).get(list_facts))
}

// ---------------------------------------------------------------------------
// Request / response DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CreateSessionRequest {
    pub agent_id: String,
    pub model: Option<String>,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub metadata: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct CreateSessionResponse {
    pub session: AgentSession,
}

#[derive(Debug, Deserialize)]
pub struct InsertMessageRequest {
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub metadata: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct InsertMessageResponse {
    pub message: MemoryEntry,
    pub tier: String,
    pub evicted_to_episodic: Option<MemoryEntry>,
}

#[derive(Debug, Deserialize)]
pub struct ContextQuery {
    pub max_tokens: Option<usize>,
    pub current_query: Option<String>,
    pub include_facts: Option<bool>,
    pub recall_top_k: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct ContextResponse {
    pub session_id: Uuid,
    pub messages: Vec<ContextMessage>,
    pub total_tokens: usize,
    pub budget_remaining: usize,
}

#[derive(Debug, Deserialize)]
pub struct SearchRequest {
    pub query: String,
    #[serde(default = "default_top_k")]
    pub top_k: i64,
}

fn default_top_k() -> i64 {
    10
}

#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub results: Vec<MemoryEntry>,
}

#[derive(Debug, Deserialize)]
pub struct TimelineQuery {
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    pub tier: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct TimelineResponse {
    pub entries: Vec<MemoryEntry>,
}

#[derive(Debug, Deserialize)]
pub struct UpsertFactRequest {
    pub agent_id: String,
    pub key: String,
    pub value: String,
    #[serde(default = "default_confidence")]
    pub confidence: f32,
    #[serde(default = "default_source")]
    pub source: String,
}

fn default_confidence() -> f32 {
    1.0
}

fn default_source() -> String {
    "explicit".into()
}

#[derive(Debug, Serialize)]
pub struct UpsertFactResponse {
    pub fact: AgentFact,
}

#[derive(Debug, Deserialize)]
pub struct ListFactsQuery {
    pub agent_id: String,
    pub user_id: Option<Uuid>,
    pub query: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ListFactsResponse {
    pub facts: Vec<AgentFact>,
}

#[derive(Debug, Deserialize)]
pub struct CloseSessionRequest {
    #[serde(default)]
    pub final_summary: Option<String>,
}

// ---------------------------------------------------------------------------
// Auth helper
// ---------------------------------------------------------------------------

#[allow(clippy::result_large_err)] // Response carries the full body via IntoResponse; boxing would just move the allocation
fn require_auth(auth: Option<Extension<AuthContext>>) -> Result<AuthContext, Response> {
    match auth {
        Some(Extension(ctx)) => Ok(ctx),
        None => Err(reject(StatusCode::UNAUTHORIZED, "authentication required")),
    }
}

type Response = axum::response::Response;

fn reject(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

fn db_err(err: sqlx::Error) -> Response {
    tracing::error!(?err, "agent_memory db error");
    reject(
        StatusCode::INTERNAL_SERVER_ERROR,
        &format!("database error: {err}"),
    )
}

fn counter_for(model: Option<&str>) -> TiktokenCounter {
    match model {
        Some(m) if !m.is_empty() => TiktokenCounter::for_model(m),
        _ => TiktokenCounter::default_counter(),
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /api/agent/sessions` — create a new session.
async fn create_session(
    State(state): State<AgentMemoryState>,
    auth: Option<Extension<AuthContext>>,
    Json(body): Json<CreateSessionRequest>,
) -> Response {
    let auth = match require_auth(auth) {
        Ok(a) => a,
        Err(r) => return r,
    };

    if body.agent_id.trim().is_empty() {
        return reject(StatusCode::BAD_REQUEST, "agent_id is required");
    }

    let mut metadata = body.metadata.unwrap_or_else(|| json!({}));
    if let Some(prompt) = body.system_prompt.as_ref()
        && let Some(obj) = metadata.as_object_mut()
    {
        obj.insert("system".to_string(), json!(prompt));
    }

    match state
        .repo
        .create_session(
            auth.user_id,
            &body.agent_id,
            body.model.as_deref(),
            metadata,
        )
        .await
    {
        Ok(session) => {
            (StatusCode::CREATED, Json(CreateSessionResponse { session })).into_response()
        }
        Err(e) => db_err(e),
    }
}

/// `POST /api/agent/sessions/:id/messages` — append a message to the
/// working tier. If the working window overflows, the oldest entry is
/// promoted to the episodic tier in Postgres.
async fn insert_message(
    State(state): State<AgentMemoryState>,
    auth: Option<Extension<AuthContext>>,
    Path(session_id): Path<Uuid>,
    Json(body): Json<InsertMessageRequest>,
) -> Response {
    let auth = match require_auth(auth) {
        Ok(a) => a,
        Err(r) => return r,
    };

    let session = match state.repo.get_session(session_id, auth.user_id).await {
        Ok(Some(s)) => s,
        Ok(None) => return reject(StatusCode::NOT_FOUND, "session not found"),
        Err(e) => return db_err(e),
    };

    let role = match MemoryRole::parse(&body.role) {
        Some(r) => r,
        None => return reject(StatusCode::BAD_REQUEST, "invalid role"),
    };

    let counter = counter_for(session.model.as_deref());
    let token_count = counter.count(&body.content) as i32;
    let metadata = body.metadata.unwrap_or_else(|| json!({}));

    // Persist into the working tier first (Postgres row marked
    // `working`) so callers can audit the message even before eviction.
    let stored = match state
        .repo
        .insert_message(
            session_id,
            MemoryTier::Working,
            role,
            &body.content,
            token_count,
            metadata,
        )
        .await
    {
        Ok(m) => m,
        Err(e) => return db_err(e),
    };

    // Push into the in-memory working window. If something falls out
    // the back, promote it to episodic by writing a fresh row marked
    // `episodic` and deleting the original `working` row.
    let evicted = state.working.push(session_id, stored.clone());
    let evicted_response = if let Some(old) = evicted {
        match state
            .repo
            .insert_message(
                session_id,
                MemoryTier::Episodic,
                old.role,
                &old.content,
                old.token_count,
                old.metadata.clone(),
            )
            .await
        {
            Ok(promoted) => Some(promoted),
            Err(e) => return db_err(e),
        }
    } else {
        None
    };

    let response = InsertMessageResponse {
        message: stored,
        tier: MemoryTier::Working.as_str().into(),
        evicted_to_episodic: evicted_response,
    };
    (StatusCode::CREATED, Json(response)).into_response()
}

/// `GET /api/agent/sessions/:id/context` — run the [`ContextBuilder`]
/// and return the assembled prompt.
async fn get_context(
    State(state): State<AgentMemoryState>,
    auth: Option<Extension<AuthContext>>,
    Path(session_id): Path<Uuid>,
    Query(params): Query<ContextQuery>,
) -> Response {
    let auth = match require_auth(auth) {
        Ok(a) => a,
        Err(r) => return r,
    };

    let session = match state.repo.get_session(session_id, auth.user_id).await {
        Ok(Some(s)) => s,
        Ok(None) => return reject(StatusCode::NOT_FOUND, "session not found"),
        Err(e) => return db_err(e),
    };

    let counter = counter_for(session.model.as_deref());
    let mut builder = ContextBuilder::new(
        session_id,
        counter,
        state.repo.clone(),
        state.working.clone(),
    );

    let opts = ContextBuildOptions {
        max_tokens: params.max_tokens.unwrap_or(4096),
        current_query: params.current_query,
        include_facts: params.include_facts.unwrap_or(false),
        recall_top_k: params.recall_top_k.unwrap_or(5),
        ..ContextBuildOptions::default()
    };

    match builder.build(&opts).await {
        Ok(bundle) => Json(ContextResponse {
            session_id,
            messages: bundle.messages,
            total_tokens: bundle.total_tokens,
            budget_remaining: bundle.budget_remaining,
        })
        .into_response(),
        Err(e) => db_err(e),
    }
}

/// `GET /api/agent/sessions/:id/context/export` — same as `/context`
/// but emits a self-contained JSON document suitable for offline
/// debugging or replay.
async fn export_context(
    State(state): State<AgentMemoryState>,
    auth: Option<Extension<AuthContext>>,
    Path(session_id): Path<Uuid>,
) -> Response {
    let auth = match require_auth(auth) {
        Ok(a) => a,
        Err(r) => return r,
    };

    let session = match state.repo.get_session(session_id, auth.user_id).await {
        Ok(Some(s)) => s,
        Ok(None) => return reject(StatusCode::NOT_FOUND, "session not found"),
        Err(e) => return db_err(e),
    };

    let timeline = match state
        .repo
        .timeline(
            session_id,
            &TimelineFilter {
                limit: Some(1000),
                ..Default::default()
            },
        )
        .await
    {
        Ok(t) => t,
        Err(e) => return db_err(e),
    };

    let counter = counter_for(session.model.as_deref());
    let mut builder = ContextBuilder::new(
        session_id,
        counter,
        state.repo.clone(),
        state.working.clone(),
    );
    let bundle = match builder.build(&ContextBuildOptions::default()).await {
        Ok(b) => b,
        Err(e) => return db_err(e),
    };

    Json(json!({
        "session": session,
        "timeline": timeline,
        "context": {
            "messages": bundle.messages,
            "total_tokens": bundle.total_tokens,
            "budget_remaining": bundle.budget_remaining,
        }
    }))
    .into_response()
}

/// `POST /api/agent/sessions/:id/search` — keyword/semantic recall.
async fn search_session(
    State(state): State<AgentMemoryState>,
    auth: Option<Extension<AuthContext>>,
    Path(session_id): Path<Uuid>,
    Json(body): Json<SearchRequest>,
) -> Response {
    let auth = match require_auth(auth) {
        Ok(a) => a,
        Err(r) => return r,
    };

    if state
        .repo
        .get_session(session_id, auth.user_id)
        .await
        .ok()
        .flatten()
        .is_none()
    {
        return reject(StatusCode::NOT_FOUND, "session not found");
    }

    let limit = body.top_k.clamp(1, 100);
    match state
        .repo
        .semantic_recall(session_id, &body.query, limit)
        .await
    {
        Ok(results) => Json(SearchResponse { results }).into_response(),
        Err(e) => db_err(e),
    }
}

/// `GET /api/agent/sessions/:id/timeline` — filtered chronological view.
async fn get_timeline(
    State(state): State<AgentMemoryState>,
    auth: Option<Extension<AuthContext>>,
    Path(session_id): Path<Uuid>,
    Query(params): Query<TimelineQuery>,
) -> Response {
    let auth = match require_auth(auth) {
        Ok(a) => a,
        Err(r) => return r,
    };

    if state
        .repo
        .get_session(session_id, auth.user_id)
        .await
        .ok()
        .flatten()
        .is_none()
    {
        return reject(StatusCode::NOT_FOUND, "session not found");
    }

    let filter = TimelineFilter {
        from: params.from,
        to: params.to,
        tier: params.tier,
        limit: params.limit,
    };

    match state.repo.timeline(session_id, &filter).await {
        Ok(entries) => Json(TimelineResponse { entries }).into_response(),
        Err(e) => db_err(e),
    }
}

/// `GET /api/agent/sessions/:id/stats` — aggregate counts.
async fn get_stats(
    State(state): State<AgentMemoryState>,
    auth: Option<Extension<AuthContext>>,
    Path(session_id): Path<Uuid>,
) -> Response {
    let auth = match require_auth(auth) {
        Ok(a) => a,
        Err(r) => return r,
    };

    if state
        .repo
        .get_session(session_id, auth.user_id)
        .await
        .ok()
        .flatten()
        .is_none()
    {
        return reject(StatusCode::NOT_FOUND, "session not found");
    }

    match state.repo.session_stats(session_id).await {
        Ok(Some(mut stats)) => {
            stats.working_messages = state.working.len(session_id);
            Json(stats).into_response()
        }
        Ok(None) => reject(StatusCode::NOT_FOUND, "session not found"),
        Err(e) => db_err(e),
    }
}

/// `DELETE /api/agent/sessions/:id` — close + summarise + drop the
/// in-memory working window.
async fn close_session(
    State(state): State<AgentMemoryState>,
    auth: Option<Extension<AuthContext>>,
    Path(session_id): Path<Uuid>,
    body: Option<Json<CloseSessionRequest>>,
) -> Response {
    let auth = match require_auth(auth) {
        Ok(a) => a,
        Err(r) => return r,
    };

    let summary = body
        .and_then(|Json(b)| b.final_summary)
        .unwrap_or_else(|| build_auto_summary(&state, session_id));

    match state
        .repo
        .close_session(session_id, auth.user_id, &summary)
        .await
    {
        Ok(true) => {
            state.working.clear(session_id);
            Json(json!({"closed": true, "session_id": session_id, "final_summary": summary }))
                .into_response()
        }
        Ok(false) => reject(StatusCode::NOT_FOUND, "session not found"),
        Err(e) => db_err(e),
    }
}

fn build_auto_summary(state: &AgentMemoryState, session_id: Uuid) -> String {
    let snap = state.working.snapshot(session_id);
    if snap.is_empty() {
        return "Session closed — no in-memory working window.".into();
    }
    let lines: Vec<String> = snap
        .iter()
        .map(|m| format!("- {}: {}", m.role.as_str(), preview(&m.content, 120)))
        .collect();
    format!(
        "Auto-summary of last {} messages:\n{}",
        snap.len(),
        lines.join("\n")
    )
}

fn preview(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}

/// `POST /api/agent/facts` — upsert an agent fact.
async fn upsert_fact(
    State(state): State<AgentMemoryState>,
    auth: Option<Extension<AuthContext>>,
    Json(body): Json<UpsertFactRequest>,
) -> Response {
    let auth = match require_auth(auth) {
        Ok(a) => a,
        Err(r) => return r,
    };

    if body.agent_id.trim().is_empty() || body.key.trim().is_empty() {
        return reject(StatusCode::BAD_REQUEST, "agent_id and key are required");
    }

    match state
        .repo
        .upsert_fact(
            &body.agent_id,
            auth.user_id,
            &body.key,
            &body.value,
            body.confidence,
            &body.source,
        )
        .await
    {
        Ok(fact) => (StatusCode::CREATED, Json(UpsertFactResponse { fact })).into_response(),
        Err(e) => db_err(e),
    }
}

/// `GET /api/agent/facts` — list facts for `(agent_id, user_id)`.
async fn list_facts(
    State(state): State<AgentMemoryState>,
    auth: Option<Extension<AuthContext>>,
    Query(params): Query<ListFactsQuery>,
) -> Response {
    let auth = match require_auth(auth) {
        Ok(a) => a,
        Err(r) => return r,
    };

    let user_id = params.user_id.unwrap_or(auth.user_id);
    // Non-admin callers may only query their own facts.
    if user_id != auth.user_id && !auth.roles.iter().any(|r| r == "admin") {
        return reject(StatusCode::FORBIDDEN, "cannot query facts for another user");
    }

    match state
        .repo
        .list_facts(&params.agent_id, user_id, params.query.as_deref())
        .await
    {
        Ok(facts) => Json(ListFactsResponse { facts }).into_response(),
        Err(e) => db_err(e),
    }
}
