//! REST API handlers for the automation engine.
//!
//! Provides CRUD endpoints for automations (trigger + workflow pairs),
//! manual trigger invocation, and execution history retrieval.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::RwLock;
use tracing::info;
use uuid::Uuid;

use super::action::ActionContext;
use super::workflow::{Workflow, WorkflowEngine, WorkflowId, WorkflowRun};

// ── In-memory automation store ────────────────────────────────────
//
// In production this would be backed by EAV triples
// (`automation:{uuid}` with attributes). For now, an in-memory store
// enables the full API contract while the persistence layer is wired.

/// Shared state for the automation subsystem.
#[derive(Clone)]
pub struct AutomationState {
    /// All registered workflows keyed by ID.
    pub workflows: Arc<RwLock<HashMap<WorkflowId, Workflow>>>,
    /// Execution history keyed by workflow ID.
    pub runs: Arc<RwLock<HashMap<WorkflowId, Vec<WorkflowRun>>>>,
    /// The workflow execution engine.
    pub engine: Arc<WorkflowEngine>,
}

impl AutomationState {
    /// Create a new empty automation state.
    pub fn new() -> Self {
        Self {
            workflows: Arc::new(RwLock::new(HashMap::new())),
            runs: Arc::new(RwLock::new(HashMap::new())),
            engine: Arc::new(WorkflowEngine::new()),
        }
    }
}

impl Default for AutomationState {
    fn default() -> Self {
        Self::new()
    }
}

// ── Request / Response types ──────────────────────────────────────

/// Request body for creating a new automation.
#[derive(Debug, Deserialize)]
pub struct CreateAutomationRequest {
    /// Human-readable name.
    pub name: String,
    /// Optional description.
    #[serde(default)]
    pub description: String,
    /// Trigger configuration.
    pub trigger: super::trigger::TriggerConfig,
    /// Workflow steps.
    #[serde(default)]
    pub steps: Vec<super::workflow::WorkflowStep>,
    /// Whether to enable immediately (default: true).
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

/// Request body for updating an automation.
#[derive(Debug, Deserialize)]
pub struct UpdateAutomationRequest {
    /// Updated name.
    pub name: Option<String>,
    /// Updated description.
    pub description: Option<String>,
    /// Toggle enabled/disabled.
    pub enabled: Option<bool>,
    /// Replace trigger config.
    pub trigger: Option<super::trigger::TriggerConfig>,
    /// Replace workflow steps.
    pub steps: Option<Vec<super::workflow::WorkflowStep>>,
}

/// Query parameters for listing runs.
#[derive(Debug, Deserialize)]
pub struct ListRunsQuery {
    /// Maximum number of runs to return.
    #[serde(default = "default_limit")]
    pub limit: usize,
    /// Offset for pagination.
    #[serde(default)]
    pub offset: usize,
}

fn default_limit() -> usize {
    50
}

/// Standard API response envelope.
#[derive(Debug, Serialize)]
pub struct AutomationResponse {
    pub success: bool,
    pub data: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl AutomationResponse {
    fn ok(data: Value) -> (StatusCode, Json<Self>) {
        (
            StatusCode::OK,
            Json(Self {
                success: true,
                data,
                error: None,
            }),
        )
    }

    fn created(data: Value) -> (StatusCode, Json<Self>) {
        (
            StatusCode::CREATED,
            Json(Self {
                success: true,
                data,
                error: None,
            }),
        )
    }

    fn not_found(msg: impl Into<String>) -> (StatusCode, Json<Self>) {
        (
            StatusCode::NOT_FOUND,
            Json(Self {
                success: false,
                data: Value::Null,
                error: Some(msg.into()),
            }),
        )
    }

    fn bad_request(msg: impl Into<String>) -> (StatusCode, Json<Self>) {
        (
            StatusCode::BAD_REQUEST,
            Json(Self {
                success: false,
                data: Value::Null,
                error: Some(msg.into()),
            }),
        )
    }

    fn internal_error(msg: impl Into<String>) -> (StatusCode, Json<Self>) {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(Self {
                success: false,
                data: Value::Null,
                error: Some(msg.into()),
            }),
        )
    }
}

// ── Router ────────────────────────────────────────────────────────

/// Build the automation routes sub-router.
///
/// Mount under `/api/automations` in the main router.
pub fn automation_routes(state: AutomationState) -> Router {
    Router::new()
        .route("/", post(create_automation).get(list_automations))
        .route(
            "/{id}",
            get(get_automation)
                .patch(update_automation)
                .delete(delete_automation),
        )
        .route("/{id}/run", post(manual_trigger))
        .route("/{id}/runs", get(list_runs))
        .route("/{id}/runs/{run_id}", get(get_run))
        .with_state(state)
}

// ── Handlers ──────────────────────────────────────────────────────

/// `POST /api/automations` — Create a new automation.
async fn create_automation(
    State(state): State<AutomationState>,
    Json(req): Json<CreateAutomationRequest>,
) -> impl IntoResponse {
    let mut workflow = Workflow::new(req.name, req.trigger, req.steps);
    workflow.description = req.description;
    workflow.enabled = req.enabled;

    let id = workflow.id;

    info!(
        workflow_id = %id,
        name = %workflow.name,
        "creating automation"
    );

    let response_data = serde_json::to_value(&workflow).unwrap_or(Value::Null);

    {
        let mut workflows = state.workflows.write().await;
        workflows.insert(id, workflow);
    }

    AutomationResponse::created(response_data)
}

/// `GET /api/automations` — List all automations.
async fn list_automations(State(state): State<AutomationState>) -> impl IntoResponse {
    let workflows = state.workflows.read().await;
    let list: Vec<Value> = workflows
        .values()
        .map(|w| {
            json!({
                "id": w.id,
                "name": w.name,
                "description": w.description,
                "enabled": w.enabled,
                "trigger_kind": w.trigger.kind,
                "table_entity_type": w.trigger.table_entity_type,
                "step_count": w.steps.len(),
                "created_at": w.created_at,
                "updated_at": w.updated_at,
            })
        })
        .collect();

    AutomationResponse::ok(json!({
        "automations": list,
        "total": list.len(),
    }))
}

/// `GET /api/automations/{id}` — Get automation details.
async fn get_automation(
    State(state): State<AutomationState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let workflow_id = match parse_workflow_id(&id) {
        Ok(id) => id,
        Err(resp) => return resp,
    };

    let workflows = state.workflows.read().await;
    match workflows.get(&workflow_id) {
        Some(workflow) => {
            let data = serde_json::to_value(workflow).unwrap_or(Value::Null);
            AutomationResponse::ok(data)
        }
        None => AutomationResponse::not_found(format!("automation {id} not found")),
    }
}

/// `PATCH /api/automations/{id}` — Update an automation.
async fn update_automation(
    State(state): State<AutomationState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateAutomationRequest>,
) -> impl IntoResponse {
    let workflow_id = match parse_workflow_id(&id) {
        Ok(id) => id,
        Err(resp) => return resp,
    };

    let mut workflows = state.workflows.write().await;
    match workflows.get_mut(&workflow_id) {
        Some(workflow) => {
            if let Some(name) = req.name {
                workflow.name = name;
            }
            if let Some(description) = req.description {
                workflow.description = description;
            }
            if let Some(enabled) = req.enabled {
                workflow.enabled = enabled;
            }
            if let Some(trigger) = req.trigger {
                workflow.trigger = trigger;
            }
            if let Some(steps) = req.steps {
                workflow.steps = steps;
            }
            workflow.updated_at = chrono::Utc::now();

            info!(workflow_id = %workflow_id, "automation updated");

            let data = serde_json::to_value(&*workflow).unwrap_or(Value::Null);
            AutomationResponse::ok(data)
        }
        None => AutomationResponse::not_found(format!("automation {id} not found")),
    }
}

/// `DELETE /api/automations/{id}` — Delete an automation.
async fn delete_automation(
    State(state): State<AutomationState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let workflow_id = match parse_workflow_id(&id) {
        Ok(id) => id,
        Err(resp) => return resp,
    };

    let mut workflows = state.workflows.write().await;
    match workflows.remove(&workflow_id) {
        Some(workflow) => {
            info!(
                workflow_id = %workflow_id,
                name = %workflow.name,
                "automation deleted"
            );
            AutomationResponse::ok(json!({
                "deleted": true,
                "id": workflow_id,
            }))
        }
        None => AutomationResponse::not_found(format!("automation {id} not found")),
    }
}

/// `POST /api/automations/{id}/run` — Manually trigger an automation.
async fn manual_trigger(
    State(state): State<AutomationState>,
    Path(id): Path<String>,
    body: Option<Json<Value>>,
) -> impl IntoResponse {
    let workflow_id = match parse_workflow_id(&id) {
        Ok(id) => id,
        Err(resp) => return resp,
    };

    let workflow = {
        let workflows = state.workflows.read().await;
        match workflows.get(&workflow_id) {
            Some(w) => w.clone(),
            None => {
                return AutomationResponse::not_found(format!("automation {id} not found"));
            }
        }
    };

    if !workflow.enabled {
        return AutomationResponse::bad_request("automation is disabled");
    }

    // Build context from the request body or use defaults.
    let mut context = ActionContext::manual(&workflow.trigger.table_entity_type);
    if let Some(Json(body)) = body {
        if let Some(data) = body.get("data").and_then(|d| d.as_object()) {
            for (k, v) in data {
                context.record_data.insert(k.clone(), v.clone());
            }
        }
        if let Some(entity_id) = body.get("entity_id").and_then(|e| e.as_str()) {
            context.entity_id = Uuid::parse_str(entity_id).ok();
        }
    }

    info!(
        workflow_id = %workflow_id,
        name = %workflow.name,
        "manual trigger invoked"
    );

    let run = state.engine.execute(&workflow, None, context).await;

    // Store the run.
    {
        let mut runs = state.runs.write().await;
        runs.entry(workflow_id).or_default().push(run.clone());
    }

    let data = serde_json::to_value(&run).unwrap_or(Value::Null);
    AutomationResponse::ok(data)
}

/// `GET /api/automations/{id}/runs` — List execution history.
async fn list_runs(
    State(state): State<AutomationState>,
    Path(id): Path<String>,
    Query(params): Query<ListRunsQuery>,
) -> impl IntoResponse {
    let workflow_id = match parse_workflow_id(&id) {
        Ok(id) => id,
        Err(resp) => return resp,
    };

    let runs = state.runs.read().await;
    let workflow_runs = runs.get(&workflow_id);

    let (list, total) = match workflow_runs {
        Some(runs) => {
            let total = runs.len();
            let slice: Vec<Value> = runs
                .iter()
                .rev() // Most recent first.
                .skip(params.offset)
                .take(params.limit)
                .map(|r| {
                    json!({
                        "id": r.id,
                        "workflow_id": r.workflow_id,
                        "status": r.status,
                        "started_at": r.started_at,
                        "completed_at": r.completed_at,
                        "duration_ms": r.duration_ms,
                        "steps_executed": r.step_results.len(),
                    })
                })
                .collect();
            (slice, total)
        }
        None => (Vec::new(), 0),
    };

    AutomationResponse::ok(json!({
        "runs": list,
        "total": total,
        "limit": params.limit,
        "offset": params.offset,
    }))
}

/// `GET /api/automations/{id}/runs/{run_id}` — Get run details.
async fn get_run(
    State(state): State<AutomationState>,
    Path((id, run_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let workflow_id = match parse_workflow_id(&id) {
        Ok(id) => id,
        Err(resp) => return resp,
    };

    let run_uuid = match Uuid::parse_str(&run_id) {
        Ok(id) => id,
        Err(_) => return AutomationResponse::bad_request("invalid run ID format"),
    };

    let runs = state.runs.read().await;
    let found = runs
        .get(&workflow_id)
        .and_then(|runs| runs.iter().find(|r| r.id == run_uuid));

    match found {
        Some(run) => {
            let data = serde_json::to_value(run).unwrap_or(Value::Null);
            AutomationResponse::ok(data)
        }
        None => AutomationResponse::not_found(format!("run {run_id} not found")),
    }
}

// ── Helpers ───────────────────────────────────────────────────────

fn parse_workflow_id(
    id: &str,
) -> Result<WorkflowId, (StatusCode, Json<AutomationResponse>)> {
    Uuid::parse_str(id)
        .map(WorkflowId)
        .map_err(|_| AutomationResponse::bad_request("invalid automation ID format"))
}

// ── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    fn build_test_app() -> Router {
        let state = AutomationState::new();
        automation_routes(state)
    }

    #[tokio::test]
    async fn list_automations_empty() {
        let app = build_test_app();

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .method("GET")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), 1024 * 64)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert!(json["success"].as_bool().unwrap());
        assert_eq!(json["data"]["total"], 0);
    }

    #[tokio::test]
    async fn create_and_get_automation() {
        let state = AutomationState::new();
        let app = automation_routes(state);

        // Create.
        let create_body = json!({
            "name": "Welcome email",
            "trigger": {
                "id": Uuid::new_v4(),
                "kind": { "type": "on_record_create" },
                "table_entity_type": "users",
                "condition": null,
                "enabled": true,
            },
            "steps": [
                {
                    "id": "send_email",
                    "action": {
                        "id": Uuid::new_v4(),
                        "kind": { "type": "send_email" },
                        "config": {
                            "to": "{{record.email}}",
                            "subject": "Welcome!",
                            "body": "Hello!"
                        },
                        "timeout_ms": 10000,
                    },
                    "on_error": "stop",
                }
            ],
        });

        let create_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .method("POST")
                    .header("Content-Type", "application/json")
                    .body(Body::from(serde_json::to_vec(&create_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(create_resp.status(), StatusCode::CREATED);

        let body = axum::body::to_bytes(create_resp.into_body(), 1024 * 64)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        let automation_id = json["data"]["id"].as_array().unwrap()[1]
            .as_str()
            .unwrap()
            .to_string();

        // Get.
        let get_resp = app
            .oneshot(
                Request::builder()
                    .uri(&format!("/{automation_id}"))
                    .method("GET")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(get_resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn get_nonexistent_automation() {
        let app = build_test_app();
        let fake_id = Uuid::new_v4();

        let response = app
            .oneshot(
                Request::builder()
                    .uri(&format!("/{fake_id}"))
                    .method("GET")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn invalid_id_returns_bad_request() {
        let app = build_test_app();

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/not-a-uuid")
                    .method("GET")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
