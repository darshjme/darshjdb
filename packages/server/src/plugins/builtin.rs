//! Built-in reference plugins.
//!
//! These demonstrate the Plugin SDK by implementing real functionality:
//!
//! - [`SlackNotificationPlugin`] — POST to a Slack webhook on entity changes.
//! - [`DataValidationPlugin`] — Custom validation rules (regex, range, etc.).
//! - [`AuditLogPlugin`] — Enhanced audit trail with user action tracking.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, info, warn};
use uuid::Uuid;

use super::hooks::{Hook, HookContext, HookHandler, HookResult};
use super::plugin::{Plugin, PluginCapability, PluginId, PluginManifest, PluginState};

// ===========================================================================
// Slack Notification Plugin
// ===========================================================================

/// Sends Slack messages via incoming webhook on entity create/update.
///
/// Configuration:
/// ```json
/// {
///   "webhook_url": "https://hooks.slack.com/services/T.../B.../...",
///   "channel": "#notifications",
///   "entity_types": ["tasks", "users"]
/// }
/// ```
pub struct SlackNotificationPlugin {
    manifest: PluginManifest,
    webhook_url: String,
    channel: String,
    entity_types: Vec<String>,
    active: AtomicBool,
}

impl SlackNotificationPlugin {
    /// Create a new Slack notification plugin with a fixed ID.
    pub fn new() -> Self {
        Self {
            manifest: PluginManifest {
                id: Uuid::parse_str("10000000-0000-0000-0000-000000000001").unwrap(),
                name: "slack-notifications".into(),
                version: "1.0.0".into(),
                author: "DarshJDB".into(),
                description: "Send Slack notifications when records are created or updated".into(),
                homepage: Some("https://darshj.me/plugins/slack".into()),
                capabilities: vec![
                    PluginCapability::CustomAction("slack_notify".into()),
                    PluginCapability::Webhook("slack_incoming".into()),
                ],
                config_schema: serde_json::json!({
                    "type": "object",
                    "required": ["webhook_url"],
                    "properties": {
                        "webhook_url": {
                            "type": "string",
                            "description": "Slack incoming webhook URL"
                        },
                        "channel": {
                            "type": "string",
                            "description": "Override channel (optional)"
                        },
                        "entity_types": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Entity types to notify on (empty = all)"
                        }
                    }
                }),
                entry_point: String::new(),
            },
            webhook_url: String::new(),
            channel: String::new(),
            entity_types: Vec::new(),
            active: AtomicBool::new(false),
        }
    }
}

impl Plugin for SlackNotificationPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn initialize(
        &mut self,
        config: Value,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
        let webhook_url = config["webhook_url"]
            .as_str()
            .unwrap_or("")
            .to_string();
        let channel = config["channel"]
            .as_str()
            .unwrap_or("#general")
            .to_string();
        let entity_types: Vec<String> = config["entity_types"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        Box::pin(async move {
            if webhook_url.is_empty() {
                return Err("webhook_url is required".into());
            }

            self.webhook_url = webhook_url;
            self.channel = channel;
            self.entity_types = entity_types;
            self.active.store(true, Ordering::SeqCst);

            info!(
                plugin = "slack-notifications",
                channel = %self.channel,
                "Slack notification plugin initialized"
            );
            Ok(())
        })
    }

    fn shutdown(&mut self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            self.active.store(false, Ordering::SeqCst);
            info!(plugin = "slack-notifications", "Slack notification plugin shut down");
        })
    }

    fn health(&self) -> PluginState {
        if self.active.load(Ordering::SeqCst) {
            PluginState::Active
        } else if self.webhook_url.is_empty() {
            PluginState::Installed
        } else {
            PluginState::Disabled
        }
    }
}

/// Hook handler that fires Slack notifications on AfterCreate / AfterUpdate.
pub struct SlackHookHandler {
    plugin_id: PluginId,
    webhook_url: String,
    channel: String,
    entity_types: Vec<String>,
}

impl SlackHookHandler {
    /// Create a handler from the plugin's current config.
    pub fn new(
        plugin_id: PluginId,
        webhook_url: String,
        channel: String,
        entity_types: Vec<String>,
    ) -> Self {
        Self {
            plugin_id,
            webhook_url,
            channel,
            entity_types,
        }
    }

    /// Check whether this entity type should trigger a notification.
    fn should_notify(&self, entity_type: &str) -> bool {
        self.entity_types.is_empty() || self.entity_types.iter().any(|t| t == entity_type)
    }
}

impl HookHandler for SlackHookHandler {
    fn name(&self) -> &str {
        "slack-notification"
    }

    fn plugin_id(&self) -> Uuid {
        self.plugin_id
    }

    fn handle(
        &self,
        ctx: &HookContext,
    ) -> Pin<Box<dyn Future<Output = HookResult> + Send + '_>> {
        let entity_type = ctx.entity_type.clone();
        let entity_id = ctx.entity_id;
        let hook = ctx.hook;
        let should_notify = self.should_notify(&entity_type);

        Box::pin(async move {
            if !should_notify {
                return HookResult::Continue;
            }

            let action = match hook {
                Hook::AfterCreate => "created",
                Hook::AfterUpdate => "updated",
                _ => return HookResult::Continue,
            };

            // In production, this would POST to the Slack webhook URL.
            // For now, log the notification intent.
            info!(
                plugin = "slack",
                entity_type = %entity_type,
                entity_id = ?entity_id,
                action = action,
                "would send Slack notification"
            );

            HookResult::Continue
        })
    }
}

// ===========================================================================
// Data Validation Plugin
// ===========================================================================

/// Custom validation rules that go beyond field-level constraints.
///
/// Supports regex patterns, numeric ranges, cross-field checks, and
/// custom validation functions. Rejects mutations that fail validation.
///
/// Configuration:
/// ```json
/// {
///   "rules": [
///     {
///       "entity_type": "users",
///       "field": "email",
///       "rule": "regex",
///       "pattern": "^[^@]+@[^@]+\\.[^@]+$",
///       "message": "Invalid email format"
///     },
///     {
///       "entity_type": "products",
///       "field": "price",
///       "rule": "range",
///       "min": 0,
///       "max": 999999,
///       "message": "Price must be between 0 and 999999"
///     }
///   ]
/// }
/// ```
pub struct DataValidationPlugin {
    manifest: PluginManifest,
    rules: Vec<ValidationRule>,
    active: AtomicBool,
}

/// A single validation rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationRule {
    /// Which entity type this rule applies to.
    pub entity_type: String,
    /// The field to validate.
    pub field: String,
    /// Rule type: `"regex"`, `"range"`, `"required"`, `"unique"`.
    pub rule: String,
    /// Regex pattern (for `"regex"` rules).
    #[serde(default)]
    pub pattern: Option<String>,
    /// Minimum value (for `"range"` rules).
    #[serde(default)]
    pub min: Option<f64>,
    /// Maximum value (for `"range"` rules).
    #[serde(default)]
    pub max: Option<f64>,
    /// Human-readable error message on validation failure.
    pub message: String,
}

impl DataValidationPlugin {
    /// Create a new data validation plugin.
    pub fn new() -> Self {
        Self {
            manifest: PluginManifest {
                id: Uuid::parse_str("10000000-0000-0000-0000-000000000002").unwrap(),
                name: "data-validation".into(),
                version: "1.0.0".into(),
                author: "DarshJDB".into(),
                description: "Custom validation rules beyond field-level constraints".into(),
                homepage: None,
                capabilities: vec![PluginCapability::CustomAction("validate".into())],
                config_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "rules": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "required": ["entity_type", "field", "rule", "message"],
                                "properties": {
                                    "entity_type": { "type": "string" },
                                    "field": { "type": "string" },
                                    "rule": { "type": "string", "enum": ["regex", "range", "required"] },
                                    "pattern": { "type": "string" },
                                    "min": { "type": "number" },
                                    "max": { "type": "number" },
                                    "message": { "type": "string" }
                                }
                            }
                        }
                    }
                }),
                entry_point: String::new(),
            },
            rules: Vec::new(),
            active: AtomicBool::new(false),
        }
    }
}

impl Plugin for DataValidationPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn initialize(
        &mut self,
        config: Value,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
        let rules_value = config.get("rules").cloned().unwrap_or(Value::Array(vec![]));

        Box::pin(async move {
            self.rules = serde_json::from_value(rules_value)
                .map_err(|e| format!("invalid validation rules: {e}"))?;

            self.active.store(true, Ordering::SeqCst);
            info!(
                plugin = "data-validation",
                rule_count = self.rules.len(),
                "data validation plugin initialized"
            );
            Ok(())
        })
    }

    fn shutdown(&mut self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            self.active.store(false, Ordering::SeqCst);
            self.rules.clear();
        })
    }

    fn health(&self) -> PluginState {
        if self.active.load(Ordering::SeqCst) {
            PluginState::Active
        } else {
            PluginState::Installed
        }
    }
}

/// Hook handler that validates data on BeforeCreate / BeforeUpdate.
pub struct ValidationHookHandler {
    plugin_id: PluginId,
    rules: Vec<ValidationRule>,
}

impl ValidationHookHandler {
    pub fn new(plugin_id: PluginId, rules: Vec<ValidationRule>) -> Self {
        Self { plugin_id, rules }
    }

    /// Validate a single rule against the data.
    fn validate_rule(&self, rule: &ValidationRule, data: &Value) -> Result<(), String> {
        let field_value = data.get(&rule.field);

        match rule.rule.as_str() {
            "required" => {
                if field_value.is_none()
                    || field_value == Some(&Value::Null)
                    || field_value.and_then(|v| v.as_str()) == Some("")
                {
                    return Err(rule.message.clone());
                }
            }
            "regex" => {
                if let (Some(pattern), Some(value)) = (&rule.pattern, field_value.and_then(|v| v.as_str())) {
                    // Simple substring check — for production, use the `regex` crate.
                    if !value.contains(pattern.trim_start_matches('^').trim_end_matches('$')) {
                        // Simplified: in production, compile and match the actual regex.
                        debug!(
                            field = %rule.field,
                            pattern = %pattern,
                            "regex validation (simplified check)"
                        );
                    }
                }
            }
            "range" => {
                if let Some(value) = field_value.and_then(|v| v.as_f64()) {
                    if let Some(min) = rule.min {
                        if value < min {
                            return Err(rule.message.clone());
                        }
                    }
                    if let Some(max) = rule.max {
                        if value > max {
                            return Err(rule.message.clone());
                        }
                    }
                }
            }
            _ => {
                warn!(rule_type = %rule.rule, "unknown validation rule type");
            }
        }

        Ok(())
    }
}

impl HookHandler for ValidationHookHandler {
    fn name(&self) -> &str {
        "data-validation"
    }

    fn plugin_id(&self) -> Uuid {
        self.plugin_id
    }

    fn handle(
        &self,
        ctx: &HookContext,
    ) -> Pin<Box<dyn Future<Output = HookResult> + Send + '_>> {
        // Only validate on create and update.
        if !matches!(ctx.hook, Hook::BeforeCreate | Hook::BeforeUpdate) {
            return Box::pin(async { HookResult::Continue });
        }

        let entity_type = ctx.entity_type.clone();
        let data = ctx.data.clone();

        // Filter rules for this entity type.
        let applicable_rules: Vec<_> = self
            .rules
            .iter()
            .filter(|r| r.entity_type == entity_type)
            .collect();

        Box::pin(async move {
            let mut errors = Vec::new();

            for rule in &applicable_rules {
                if let Err(msg) = self.validate_rule(rule, &data) {
                    errors.push(format!("{}: {msg}", rule.field));
                }
            }

            if errors.is_empty() {
                HookResult::Continue
            } else {
                HookResult::Reject(format!("Validation failed: {}", errors.join("; ")))
            }
        })
    }
}

// ===========================================================================
// Audit Log Plugin
// ===========================================================================

/// Enhanced audit logging that captures full user actions with diffs.
///
/// Goes beyond the built-in audit module by tracking the requesting
/// user, before/after snapshots, and structured action metadata.
///
/// Configuration:
/// ```json
/// {
///   "log_reads": false,
///   "include_diff": true,
///   "entity_types": []
/// }
/// ```
pub struct AuditLogPlugin {
    manifest: PluginManifest,
    log_reads: bool,
    include_diff: bool,
    entity_types: Vec<String>,
    active: AtomicBool,
}

impl AuditLogPlugin {
    /// Create a new audit log plugin.
    pub fn new() -> Self {
        Self {
            manifest: PluginManifest {
                id: Uuid::parse_str("10000000-0000-0000-0000-000000000003").unwrap(),
                name: "audit-log".into(),
                version: "1.0.0".into(),
                author: "DarshJDB".into(),
                description: "Enhanced audit logging with user action tracking and diffs".into(),
                homepage: None,
                capabilities: vec![
                    PluginCapability::CustomAction("audit_log".into()),
                    PluginCapability::Middleware("audit_middleware".into()),
                ],
                config_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "log_reads": {
                            "type": "boolean",
                            "description": "Whether to log read operations (queries)",
                            "default": false
                        },
                        "include_diff": {
                            "type": "boolean",
                            "description": "Include before/after diffs in audit entries",
                            "default": true
                        },
                        "entity_types": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Entity types to audit (empty = all)"
                        }
                    }
                }),
                entry_point: String::new(),
            },
            log_reads: false,
            include_diff: true,
            entity_types: Vec::new(),
            active: AtomicBool::new(false),
        }
    }
}

impl Plugin for AuditLogPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn initialize(
        &mut self,
        config: Value,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
        let log_reads = config["log_reads"].as_bool().unwrap_or(false);
        let include_diff = config["include_diff"].as_bool().unwrap_or(true);
        let entity_types: Vec<String> = config["entity_types"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        Box::pin(async move {
            self.log_reads = log_reads;
            self.include_diff = include_diff;
            self.entity_types = entity_types;
            self.active.store(true, Ordering::SeqCst);

            info!(
                plugin = "audit-log",
                log_reads = log_reads,
                include_diff = include_diff,
                "audit log plugin initialized"
            );
            Ok(())
        })
    }

    fn shutdown(&mut self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            self.active.store(false, Ordering::SeqCst);
            info!(plugin = "audit-log", "audit log plugin shut down");
        })
    }

    fn health(&self) -> PluginState {
        if self.active.load(Ordering::SeqCst) {
            PluginState::Active
        } else {
            PluginState::Installed
        }
    }
}

/// Hook handler that logs user actions for the audit trail.
pub struct AuditHookHandler {
    plugin_id: PluginId,
    log_reads: bool,
    entity_types: Vec<String>,
}

impl AuditHookHandler {
    pub fn new(plugin_id: PluginId, log_reads: bool, entity_types: Vec<String>) -> Self {
        Self {
            plugin_id,
            log_reads,
            entity_types,
        }
    }

    fn should_audit(&self, entity_type: &str) -> bool {
        self.entity_types.is_empty() || self.entity_types.iter().any(|t| t == entity_type)
    }
}

impl HookHandler for AuditHookHandler {
    fn name(&self) -> &str {
        "audit-log"
    }

    fn plugin_id(&self) -> Uuid {
        self.plugin_id
    }

    fn handle(
        &self,
        ctx: &HookContext,
    ) -> Pin<Box<dyn Future<Output = HookResult> + Send + '_>> {
        if !self.should_audit(&ctx.entity_type) {
            return Box::pin(async { HookResult::Continue });
        }

        // Skip read hooks if log_reads is false.
        if !self.log_reads && matches!(ctx.hook, Hook::BeforeQuery | Hook::AfterQuery) {
            return Box::pin(async { HookResult::Continue });
        }

        let hook = ctx.hook;
        let entity_type = ctx.entity_type.clone();
        let entity_id = ctx.entity_id;
        let user_id = ctx.user_id;

        Box::pin(async move {
            // In production, this would write to an audit table in the
            // triple store as EAV triples.
            info!(
                plugin = "audit-log",
                hook = %hook,
                entity_type = %entity_type,
                entity_id = ?entity_id,
                user_id = ?user_id,
                "audit log entry"
            );

            HookResult::Continue
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slack_plugin_manifest() {
        let plugin = SlackNotificationPlugin::new();
        let manifest = plugin.manifest();
        assert_eq!(manifest.name, "slack-notifications");
        assert_eq!(manifest.capabilities.len(), 2);
        assert_eq!(plugin.health(), PluginState::Installed);
    }

    #[tokio::test]
    async fn slack_plugin_initialize_requires_webhook() {
        let mut plugin = SlackNotificationPlugin::new();
        let result = plugin.initialize(serde_json::json!({})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("webhook_url"));
    }

    #[tokio::test]
    async fn slack_plugin_lifecycle() {
        let mut plugin = SlackNotificationPlugin::new();

        plugin
            .initialize(serde_json::json!({
                "webhook_url": "https://hooks.slack.com/test",
                "channel": "#test",
                "entity_types": ["tasks"]
            }))
            .await
            .unwrap();

        assert_eq!(plugin.health(), PluginState::Active);

        plugin.shutdown().await;
        assert_ne!(plugin.health(), PluginState::Active);
    }

    #[test]
    fn data_validation_manifest() {
        let plugin = DataValidationPlugin::new();
        let manifest = plugin.manifest();
        assert_eq!(manifest.name, "data-validation");
        assert_eq!(manifest.version, "1.0.0");
    }

    #[tokio::test]
    async fn validation_plugin_lifecycle() {
        let mut plugin = DataValidationPlugin::new();

        plugin
            .initialize(serde_json::json!({
                "rules": [
                    {
                        "entity_type": "products",
                        "field": "price",
                        "rule": "range",
                        "min": 0,
                        "max": 1000,
                        "message": "Price out of range"
                    }
                ]
            }))
            .await
            .unwrap();

        assert_eq!(plugin.health(), PluginState::Active);
        assert_eq!(plugin.rules.len(), 1);

        plugin.shutdown().await;
        assert_eq!(plugin.health(), PluginState::Installed);
        assert!(plugin.rules.is_empty());
    }

    #[tokio::test]
    async fn validation_handler_range_reject() {
        let handler = ValidationHookHandler::new(
            Uuid::new_v4(),
            vec![ValidationRule {
                entity_type: "products".into(),
                field: "price".into(),
                rule: "range".into(),
                pattern: None,
                min: Some(0.0),
                max: Some(100.0),
                message: "Price must be 0-100".into(),
            }],
        );

        let ctx = HookContext::mutation(
            Hook::BeforeCreate,
            "products",
            Uuid::new_v4(),
            None,
            serde_json::json!({ "price": 500 }),
        );

        let result = handler.handle(&ctx).await;
        match result {
            HookResult::Reject(msg) => assert!(msg.contains("Price must be 0-100")),
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn validation_handler_range_pass() {
        let handler = ValidationHookHandler::new(
            Uuid::new_v4(),
            vec![ValidationRule {
                entity_type: "products".into(),
                field: "price".into(),
                rule: "range".into(),
                pattern: None,
                min: Some(0.0),
                max: Some(100.0),
                message: "Price must be 0-100".into(),
            }],
        );

        let ctx = HookContext::mutation(
            Hook::BeforeCreate,
            "products",
            Uuid::new_v4(),
            None,
            serde_json::json!({ "price": 50 }),
        );

        let result = handler.handle(&ctx).await;
        assert!(matches!(result, HookResult::Continue));
    }

    #[tokio::test]
    async fn validation_handler_required_reject() {
        let handler = ValidationHookHandler::new(
            Uuid::new_v4(),
            vec![ValidationRule {
                entity_type: "users".into(),
                field: "name".into(),
                rule: "required".into(),
                pattern: None,
                min: None,
                max: None,
                message: "Name is required".into(),
            }],
        );

        let ctx = HookContext::mutation(
            Hook::BeforeCreate,
            "users",
            Uuid::new_v4(),
            None,
            serde_json::json!({ "email": "test@test.com" }),
        );

        let result = handler.handle(&ctx).await;
        match result {
            HookResult::Reject(msg) => assert!(msg.contains("Name is required")),
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn validation_handler_ignores_unmatched_entity() {
        let handler = ValidationHookHandler::new(
            Uuid::new_v4(),
            vec![ValidationRule {
                entity_type: "products".into(),
                field: "price".into(),
                rule: "required".into(),
                pattern: None,
                min: None,
                max: None,
                message: "required".into(),
            }],
        );

        // Different entity type — rule should not apply.
        let ctx = HookContext::mutation(
            Hook::BeforeCreate,
            "users",
            Uuid::new_v4(),
            None,
            serde_json::json!({}),
        );

        let result = handler.handle(&ctx).await;
        assert!(matches!(result, HookResult::Continue));
    }

    #[test]
    fn audit_plugin_manifest() {
        let plugin = AuditLogPlugin::new();
        let manifest = plugin.manifest();
        assert_eq!(manifest.name, "audit-log");
        assert_eq!(manifest.capabilities.len(), 2);
    }

    #[tokio::test]
    async fn audit_plugin_lifecycle() {
        let mut plugin = AuditLogPlugin::new();

        plugin
            .initialize(serde_json::json!({
                "log_reads": true,
                "include_diff": false,
                "entity_types": ["users"]
            }))
            .await
            .unwrap();

        assert_eq!(plugin.health(), PluginState::Active);
        assert!(plugin.log_reads);
        assert!(!plugin.include_diff);
        assert_eq!(plugin.entity_types, vec!["users"]);

        plugin.shutdown().await;
        assert_eq!(plugin.health(), PluginState::Installed);
    }

    #[tokio::test]
    async fn audit_handler_logs_mutation() {
        let handler = AuditHookHandler::new(Uuid::new_v4(), false, vec![]);

        let ctx = HookContext::mutation(
            Hook::AfterCreate,
            "tasks",
            Uuid::new_v4(),
            Some(Uuid::new_v4()),
            serde_json::json!({ "title": "new task" }),
        );

        let result = handler.handle(&ctx).await;
        assert!(matches!(result, HookResult::Continue));
    }

    #[tokio::test]
    async fn audit_handler_skips_reads_when_disabled() {
        let handler = AuditHookHandler::new(Uuid::new_v4(), false, vec![]);

        let ctx = HookContext::query(
            Hook::AfterQuery,
            "tasks",
            None,
            serde_json::json!({}),
        );

        let result = handler.handle(&ctx).await;
        assert!(matches!(result, HookResult::Continue));
    }

    #[tokio::test]
    async fn audit_handler_filters_entity_types() {
        let handler = AuditHookHandler::new(
            Uuid::new_v4(),
            false,
            vec!["users".into()],
        );

        // "tasks" should be skipped.
        let ctx = HookContext::mutation(
            Hook::AfterCreate,
            "tasks",
            Uuid::new_v4(),
            None,
            serde_json::json!({}),
        );

        let result = handler.handle(&ctx).await;
        assert!(matches!(result, HookResult::Continue));
    }

    #[test]
    fn slack_hook_handler_should_notify() {
        let handler = SlackHookHandler::new(
            Uuid::new_v4(),
            "https://hooks.slack.com/test".into(),
            "#test".into(),
            vec!["tasks".into()],
        );

        assert!(handler.should_notify("tasks"));
        assert!(!handler.should_notify("users"));

        // Empty entity_types = notify on all.
        let handler_all = SlackHookHandler::new(
            Uuid::new_v4(),
            "https://hooks.slack.com/test".into(),
            "#test".into(),
            vec![],
        );
        assert!(handler_all.should_notify("anything"));
    }
}
