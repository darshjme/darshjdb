//! Action definitions and executors for the automation engine.
//!
//! Each action is a unit of work executed as part of a workflow step.
//! Built-in action kinds cover common BaaS operations (CRUD, webhooks,
//! notifications). Custom actions can be plugged in via the `Custom`
//! variant and the [`ActionExecutor`] trait.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::error::{DarshJError, Result};

// ── IDs ───────────────────────────────────────────────────────────

/// Unique identifier for an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ActionId(pub Uuid);

impl ActionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for ActionId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for ActionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ── Action kinds ──────────────────────────────────────────────────

/// The type of action to execute.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ActionKind {
    /// Create a new record in the triple store.
    CreateRecord,
    /// Update an existing record.
    UpdateRecord,
    /// Delete a record.
    DeleteRecord,
    /// Send an HTTP webhook to an external URL.
    SendWebhook,
    /// Send an email notification.
    SendEmail,
    /// Execute a registered server-side function.
    RunFunction,
    /// Set a specific field value on the triggering record.
    SetFieldValue,
    /// Add the record to a view/collection.
    AddToView,
    /// Send an in-app notification.
    Notify,
    /// Custom action — extensibility point.
    Custom { name: String },
}

// ── Action config ─────────────────────────────────────────────────

/// Configuration for a single action in a workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionConfig {
    /// Unique action identifier.
    pub id: ActionId,
    /// What kind of action to perform.
    pub kind: ActionKind,
    /// Action-specific configuration parameters.
    ///
    /// For `CreateRecord`: `{ "entity_type": "...", "data": { ... } }`
    /// For `UpdateRecord`: `{ "entity_id": "...", "data": { ... } }`
    /// For `SendWebhook`:  `{ "url": "...", "method": "POST", "headers": { ... } }`
    /// For `SendEmail`:    `{ "to": "...", "subject": "...", "body": "..." }`
    /// For `RunFunction`:  `{ "function_name": "...", "args": { ... } }`
    /// For `SetFieldValue`: `{ "field": "...", "value": ... }`
    pub config: Value,
    /// Maximum execution time in milliseconds before timeout.
    pub timeout_ms: u64,
}

impl ActionConfig {
    /// Create a new action config with a 30-second default timeout.
    pub fn new(kind: ActionKind, config: Value) -> Self {
        Self {
            id: ActionId::new(),
            kind,
            config,
            timeout_ms: 30_000,
        }
    }

    /// Set a custom timeout.
    pub fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }
}

// ── Action context ────────────────────────────────────────────────

/// Runtime context passed to action executors.
///
/// Contains the trigger event data, the record that caused the trigger,
/// and the user who owns the automation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionContext {
    /// The entity ID of the triggering record (if applicable).
    pub entity_id: Option<Uuid>,
    /// The entity type of the triggering record.
    pub entity_type: String,
    /// Current attributes of the triggering record.
    pub record_data: HashMap<String, Value>,
    /// Fields that changed (for update triggers).
    pub changed_fields: Vec<String>,
    /// Transaction ID of the triggering mutation.
    pub tx_id: i64,
    /// ID of the user who created the automation.
    pub automation_owner: Option<String>,
    /// Output from previous workflow steps (keyed by step index).
    pub previous_outputs: HashMap<String, Value>,
}

impl ActionContext {
    /// Create a minimal context for testing or manual triggers.
    pub fn manual(entity_type: impl Into<String>) -> Self {
        Self {
            entity_id: None,
            entity_type: entity_type.into(),
            record_data: HashMap::new(),
            changed_fields: Vec::new(),
            tx_id: 0,
            automation_owner: None,
            previous_outputs: HashMap::new(),
        }
    }
}

// ── Action result ─────────────────────────────────────────────────

/// Outcome of executing a single action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionResult {
    /// Whether the action succeeded.
    pub success: bool,
    /// Output data from the action (e.g., created entity ID, webhook response).
    pub output: Value,
    /// Error message if the action failed.
    pub error: Option<String>,
    /// Wall-clock duration of execution in milliseconds.
    pub duration_ms: u64,
}

impl ActionResult {
    /// Create a successful result.
    pub fn ok(output: Value, duration_ms: u64) -> Self {
        Self {
            success: true,
            output,
            error: None,
            duration_ms,
        }
    }

    /// Create a failed result.
    pub fn err(error: impl Into<String>, duration_ms: u64) -> Self {
        Self {
            success: false,
            output: Value::Null,
            error: Some(error.into()),
            duration_ms,
        }
    }
}

// ── Executor trait ────────────────────────────────────────────────

/// Trait for executing an action.
///
/// Implement this for custom action types. Built-in executors are
/// provided for all standard [`ActionKind`] variants.
pub trait ActionExecutor: Send + Sync {
    /// Execute the action with the given config and context.
    fn execute(
        &self,
        config: &ActionConfig,
        context: &ActionContext,
    ) -> Pin<Box<dyn Future<Output = ActionResult> + Send + '_>>;

    /// Human-readable name for logging.
    fn name(&self) -> &str;
}

// ── Built-in executors ────────────────────────────────────────────

/// Routes action execution to the appropriate built-in executor.
pub struct BuiltinExecutor {
    http_client: reqwest::Client,
}

impl BuiltinExecutor {
    pub fn new() -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("failed to build HTTP client for action executor");
        Self { http_client }
    }

    /// Execute an action, dispatching by kind.
    pub async fn execute(&self, config: &ActionConfig, context: &ActionContext) -> ActionResult {
        let start = Instant::now();
        let timeout = Duration::from_millis(config.timeout_ms);

        let result = tokio::time::timeout(timeout, self.dispatch(config, context)).await;

        let elapsed = start.elapsed().as_millis() as u64;

        match result {
            Ok(Ok(output)) => {
                info!(
                    action_id = %config.id,
                    kind = ?config.kind,
                    duration_ms = elapsed,
                    "action executed successfully"
                );
                ActionResult::ok(output, elapsed)
            }
            Ok(Err(e)) => {
                error!(
                    action_id = %config.id,
                    kind = ?config.kind,
                    error = %e,
                    duration_ms = elapsed,
                    "action execution failed"
                );
                ActionResult::err(e.to_string(), elapsed)
            }
            Err(_) => {
                warn!(
                    action_id = %config.id,
                    kind = ?config.kind,
                    timeout_ms = config.timeout_ms,
                    "action timed out"
                );
                ActionResult::err(
                    format!("action timed out after {}ms", config.timeout_ms),
                    elapsed,
                )
            }
        }
    }

    /// Dispatch to the correct executor based on action kind.
    async fn dispatch(&self, config: &ActionConfig, context: &ActionContext) -> Result<Value> {
        match &config.kind {
            ActionKind::CreateRecord => self.exec_create_record(config, context).await,
            ActionKind::UpdateRecord => self.exec_update_record(config, context).await,
            ActionKind::DeleteRecord => self.exec_delete_record(config, context).await,
            ActionKind::SendWebhook => self.exec_send_webhook(config, context).await,
            ActionKind::SendEmail => self.exec_send_email(config, context).await,
            ActionKind::RunFunction => self.exec_run_function(config, context).await,
            ActionKind::SetFieldValue => self.exec_set_field_value(config, context).await,
            ActionKind::AddToView => self.exec_add_to_view(config, context).await,
            ActionKind::Notify => self.exec_notify(config, context).await,
            ActionKind::Custom { name } => self.exec_custom(name, config, context).await,
        }
    }

    // -- Individual executors ------------------------------------------------

    async fn exec_create_record(
        &self,
        config: &ActionConfig,
        _context: &ActionContext,
    ) -> Result<Value> {
        let entity_type = config.config["entity_type"]
            .as_str()
            .ok_or_else(|| DarshJError::InvalidQuery("CreateRecord: missing entity_type".into()))?;
        let data = config
            .config
            .get("data")
            .cloned()
            .unwrap_or(Value::Object(Default::default()));
        let entity_id = Uuid::new_v4();

        info!(
            entity_type = entity_type,
            entity_id = %entity_id,
            "automation: creating record"
        );

        // Return the intended mutation — the workflow engine writes via triple store.
        Ok(serde_json::json!({
            "action": "create_record",
            "entity_id": entity_id.to_string(),
            "entity_type": entity_type,
            "data": data,
        }))
    }

    async fn exec_update_record(
        &self,
        config: &ActionConfig,
        context: &ActionContext,
    ) -> Result<Value> {
        let entity_id = config.config["entity_id"]
            .as_str()
            .or_else(|| context.entity_id.map(|_| "from_context"))
            .ok_or_else(|| DarshJError::InvalidQuery("UpdateRecord: missing entity_id".into()))?;
        let data = config
            .config
            .get("data")
            .cloned()
            .unwrap_or(Value::Object(Default::default()));

        info!(entity_id = entity_id, "automation: updating record");

        Ok(serde_json::json!({
            "action": "update_record",
            "entity_id": entity_id,
            "data": data,
        }))
    }

    async fn exec_delete_record(
        &self,
        config: &ActionConfig,
        context: &ActionContext,
    ) -> Result<Value> {
        let entity_id = config.config["entity_id"]
            .as_str()
            .map(|s| s.to_string())
            .or_else(|| context.entity_id.map(|id| id.to_string()))
            .ok_or_else(|| DarshJError::InvalidQuery("DeleteRecord: missing entity_id".into()))?;

        info!(entity_id = %entity_id, "automation: deleting record");

        Ok(serde_json::json!({
            "action": "delete_record",
            "entity_id": entity_id,
        }))
    }

    async fn exec_send_webhook(
        &self,
        config: &ActionConfig,
        context: &ActionContext,
    ) -> Result<Value> {
        let url = config.config["url"]
            .as_str()
            .ok_or_else(|| DarshJError::InvalidQuery("SendWebhook: missing url".into()))?;
        let method = config.config["method"].as_str().unwrap_or("POST");

        // Build the payload from context + any extra data.
        let payload = serde_json::json!({
            "trigger": {
                "entity_id": context.entity_id,
                "entity_type": &context.entity_type,
                "record_data": &context.record_data,
                "changed_fields": &context.changed_fields,
            },
            "extra": config.config.get("payload"),
        });

        let body = serde_json::to_vec(&payload).map_err(|e| {
            DarshJError::Internal(format!("failed to serialize webhook payload: {e}"))
        })?;

        let mut request = match method.to_uppercase().as_str() {
            "GET" => self.http_client.get(url),
            "PUT" => self.http_client.put(url),
            "PATCH" => self.http_client.patch(url),
            _ => self.http_client.post(url),
        };

        request = request
            .header("Content-Type", "application/json")
            .header("User-Agent", "DarshJDB-Automation/1.0")
            .body(body);

        // Apply custom headers.
        if let Some(headers) = config.config.get("headers").and_then(|h| h.as_object()) {
            for (key, value) in headers {
                if let Some(val) = value.as_str() {
                    request = request.header(key.as_str(), val);
                }
            }
        }

        let response = request
            .send()
            .await
            .map_err(|e| DarshJError::Internal(format!("webhook request failed: {e}")))?;

        let status = response.status().as_u16();
        let response_body = response.text().await.unwrap_or_default();

        if status >= 400 {
            return Err(DarshJError::Internal(format!(
                "webhook returned status {status}: {response_body}"
            )));
        }

        Ok(serde_json::json!({
            "action": "send_webhook",
            "status": status,
            "response": response_body,
        }))
    }

    async fn exec_send_email(
        &self,
        config: &ActionConfig,
        _context: &ActionContext,
    ) -> Result<Value> {
        let to = config.config["to"]
            .as_str()
            .ok_or_else(|| DarshJError::InvalidQuery("SendEmail: missing 'to'".into()))?;
        let subject = config.config["subject"]
            .as_str()
            .unwrap_or("DarshJDB Notification");
        let body = config.config["body"].as_str().unwrap_or("");

        info!(to = to, subject = subject, "automation: email queued");

        // Email delivery is async — return a receipt. Actual SMTP/API
        // integration is pluggable via the connector system.
        Ok(serde_json::json!({
            "action": "send_email",
            "to": to,
            "subject": subject,
            "body_length": body.len(),
            "status": "queued",
        }))
    }

    async fn exec_run_function(
        &self,
        config: &ActionConfig,
        context: &ActionContext,
    ) -> Result<Value> {
        let function_name = config.config["function_name"].as_str().ok_or_else(|| {
            DarshJError::InvalidQuery("RunFunction: missing function_name".into())
        })?;
        let args = config
            .config
            .get("args")
            .cloned()
            .unwrap_or(Value::Object(Default::default()));

        info!(
            function_name = function_name,
            "automation: invoking function"
        );

        // Function invocation is deferred to the workflow engine which
        // has access to the function runtime.
        Ok(serde_json::json!({
            "action": "run_function",
            "function_name": function_name,
            "args": args,
            "context_entity_id": context.entity_id,
        }))
    }

    async fn exec_set_field_value(
        &self,
        config: &ActionConfig,
        context: &ActionContext,
    ) -> Result<Value> {
        let field = config.config["field"]
            .as_str()
            .ok_or_else(|| DarshJError::InvalidQuery("SetFieldValue: missing field".into()))?;
        let value = config.config.get("value").cloned().unwrap_or(Value::Null);
        let entity_id = context.entity_id.ok_or_else(|| {
            DarshJError::InvalidQuery("SetFieldValue: no entity in context".into())
        })?;

        info!(
            entity_id = %entity_id,
            field = field,
            "automation: setting field value"
        );

        Ok(serde_json::json!({
            "action": "set_field_value",
            "entity_id": entity_id.to_string(),
            "field": field,
            "value": value,
        }))
    }

    async fn exec_add_to_view(
        &self,
        config: &ActionConfig,
        context: &ActionContext,
    ) -> Result<Value> {
        let view_id = config.config["view_id"]
            .as_str()
            .ok_or_else(|| DarshJError::InvalidQuery("AddToView: missing view_id".into()))?;
        let entity_id = context
            .entity_id
            .ok_or_else(|| DarshJError::InvalidQuery("AddToView: no entity in context".into()))?;

        Ok(serde_json::json!({
            "action": "add_to_view",
            "view_id": view_id,
            "entity_id": entity_id.to_string(),
        }))
    }

    async fn exec_notify(&self, config: &ActionConfig, _context: &ActionContext) -> Result<Value> {
        let channel = config.config["channel"].as_str().unwrap_or("default");
        let message = config.config["message"].as_str().unwrap_or("");
        let recipients = config
            .config
            .get("recipients")
            .cloned()
            .unwrap_or(Value::Array(vec![]));

        info!(channel = channel, "automation: notification queued");

        Ok(serde_json::json!({
            "action": "notify",
            "channel": channel,
            "message": message,
            "recipients": recipients,
            "status": "queued",
        }))
    }

    async fn exec_custom(
        &self,
        name: &str,
        config: &ActionConfig,
        _context: &ActionContext,
    ) -> Result<Value> {
        warn!(
            action_name = name,
            "custom action executed — no handler registered, returning config as output"
        );

        Ok(serde_json::json!({
            "action": "custom",
            "name": name,
            "config": config.config,
        }))
    }
}

impl Default for BuiltinExecutor {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn action_result_ok() {
        let result = ActionResult::ok(json!({"id": "abc"}), 42);
        assert!(result.success);
        assert_eq!(result.duration_ms, 42);
        assert!(result.error.is_none());
    }

    #[test]
    fn action_result_err() {
        let result = ActionResult::err("timeout", 100);
        assert!(!result.success);
        assert_eq!(result.error.as_deref(), Some("timeout"));
    }

    #[test]
    fn action_config_default_timeout() {
        let config = ActionConfig::new(ActionKind::CreateRecord, json!({}));
        assert_eq!(config.timeout_ms, 30_000);
    }

    #[test]
    fn action_config_custom_timeout() {
        let config = ActionConfig::new(ActionKind::SendWebhook, json!({})).with_timeout(5_000);
        assert_eq!(config.timeout_ms, 5_000);
    }

    #[test]
    fn action_kind_serde_roundtrip() {
        let kinds = vec![
            ActionKind::CreateRecord,
            ActionKind::UpdateRecord,
            ActionKind::DeleteRecord,
            ActionKind::SendWebhook,
            ActionKind::SendEmail,
            ActionKind::RunFunction,
            ActionKind::SetFieldValue,
            ActionKind::AddToView,
            ActionKind::Notify,
            ActionKind::Custom {
                name: "my_action".to_string(),
            },
        ];

        for kind in &kinds {
            let json = serde_json::to_string(kind).unwrap();
            let restored: ActionKind = serde_json::from_str(&json).unwrap();
            assert_eq!(*kind, restored);
        }
    }

    #[test]
    fn action_context_manual() {
        let ctx = ActionContext::manual("users");
        assert_eq!(ctx.entity_type, "users");
        assert!(ctx.entity_id.is_none());
        assert!(ctx.record_data.is_empty());
    }

    #[tokio::test]
    async fn builtin_executor_create_record() {
        let executor = BuiltinExecutor::new();
        let config = ActionConfig::new(
            ActionKind::CreateRecord,
            json!({
                "entity_type": "tasks",
                "data": { "title": "Auto-created" }
            }),
        );
        let context = ActionContext::manual("tasks");
        let result = executor.execute(&config, &context).await;
        assert!(result.success);
        assert_eq!(result.output["action"], "create_record");
        assert_eq!(result.output["entity_type"], "tasks");
    }

    #[tokio::test]
    async fn builtin_executor_create_record_missing_entity_type() {
        let executor = BuiltinExecutor::new();
        let config = ActionConfig::new(ActionKind::CreateRecord, json!({}));
        let context = ActionContext::manual("tasks");
        let result = executor.execute(&config, &context).await;
        assert!(!result.success);
        assert!(result.error.unwrap().contains("missing entity_type"));
    }

    #[tokio::test]
    async fn builtin_executor_set_field_value() {
        let executor = BuiltinExecutor::new();
        let config = ActionConfig::new(
            ActionKind::SetFieldValue,
            json!({
                "field": "status",
                "value": "completed"
            }),
        );
        let mut context = ActionContext::manual("tasks");
        context.entity_id = Some(Uuid::new_v4());
        let result = executor.execute(&config, &context).await;
        assert!(result.success);
        assert_eq!(result.output["field"], "status");
    }

    #[tokio::test]
    async fn builtin_executor_set_field_value_no_entity() {
        let executor = BuiltinExecutor::new();
        let config = ActionConfig::new(
            ActionKind::SetFieldValue,
            json!({ "field": "status", "value": "done" }),
        );
        let context = ActionContext::manual("tasks");
        let result = executor.execute(&config, &context).await;
        assert!(!result.success);
    }

    #[tokio::test]
    async fn builtin_executor_notify() {
        let executor = BuiltinExecutor::new();
        let config = ActionConfig::new(
            ActionKind::Notify,
            json!({
                "channel": "slack",
                "message": "Record created",
                "recipients": ["user@example.com"]
            }),
        );
        let context = ActionContext::manual("tasks");
        let result = executor.execute(&config, &context).await;
        assert!(result.success);
        assert_eq!(result.output["status"], "queued");
    }

    #[tokio::test]
    async fn builtin_executor_timeout() {
        let executor = BuiltinExecutor::new();
        // SendWebhook to a non-routable address with a very short timeout.
        let config = ActionConfig::new(
            ActionKind::SendWebhook,
            json!({ "url": "http://192.0.2.1:1/timeout" }),
        )
        .with_timeout(50);
        let context = ActionContext::manual("test");
        let result = executor.execute(&config, &context).await;
        assert!(!result.success);
    }
}
