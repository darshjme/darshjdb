//! Trigger definitions and evaluation for the automation engine.
//!
//! A trigger watches for specific events (record mutations, scheduled
//! times, webhook invocations) and fires when conditions are met,
//! kicking off the associated workflow.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::query::WhereClause;

use super::event_bus::DdbEvent;

// ── IDs ───────────────────────────────────────────────────────────

/// Unique identifier for a trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TriggerId(pub Uuid);

impl TriggerId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for TriggerId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for TriggerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ── Cron expression wrapper ───────────────────────────────────────

/// Validated cron expression for scheduled triggers.
///
/// Wraps the `cron` crate's `Schedule` type with serde support.
#[derive(Debug, Clone)]
pub struct CronExpr {
    /// The raw cron expression string.
    pub expression: String,
    /// Parsed schedule (not serialized — rebuilt on deserialize).
    schedule: cron::Schedule,
}

impl CronExpr {
    /// Parse a cron expression string.
    ///
    /// Uses the standard 7-field cron format: sec min hour day month weekday year.
    pub fn parse(expr: &str) -> Result<Self, String> {
        let schedule: cron::Schedule = expr
            .parse()
            .map_err(|e| format!("invalid cron expression '{expr}': {e}"))?;
        Ok(Self {
            expression: expr.to_string(),
            schedule,
        })
    }

    /// Returns the next occurrence after `after`.
    pub fn next_after(&self, after: chrono::DateTime<chrono::Utc>) -> Option<chrono::DateTime<chrono::Utc>> {
        self.schedule.after(&after).next()
    }
}

impl Serialize for CronExpr {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.expression)
    }
}

impl<'de> Deserialize<'de> for CronExpr {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let expr = String::deserialize(deserializer)?;
        Self::parse(&expr).map_err(serde::de::Error::custom)
    }
}

impl PartialEq for CronExpr {
    fn eq(&self, other: &Self) -> bool {
        self.expression == other.expression
    }
}

// ── Trigger kinds ─────────────────────────────────────────────────

/// The type of event that fires a trigger.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TriggerKind {
    /// Fires when a new record is created in the target entity type.
    OnRecordCreate,
    /// Fires when any record in the target entity type is updated.
    OnRecordUpdate,
    /// Fires when a record is deleted from the target entity type.
    OnRecordDelete,
    /// Fires when a specific field changes on any record.
    OnFieldChange { field: String },
    /// Fires on a cron schedule.
    OnSchedule { cron: CronExpr },
    /// Fires when an external webhook hits the automation endpoint.
    OnWebhook,
    /// Fires when a form submission creates a record.
    OnFormSubmit,
    /// Manual trigger — only fires via the API.
    Manual,
}

// ── Trigger config ────────────────────────────────────────────────

/// Full configuration for a trigger attached to an automation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerConfig {
    /// Unique trigger identifier.
    pub id: TriggerId,
    /// What kind of event fires this trigger.
    pub kind: TriggerKind,
    /// The entity type (table) this trigger watches.
    /// For Manual and OnWebhook triggers, this may be empty.
    pub table_entity_type: String,
    /// Optional filter condition — the trigger only fires when the
    /// record/event matches all clauses.
    pub condition: Option<Vec<WhereClause>>,
    /// Whether this trigger is currently active.
    pub enabled: bool,
}

impl TriggerConfig {
    /// Create a new enabled trigger with no condition filter.
    pub fn new(kind: TriggerKind, table_entity_type: impl Into<String>) -> Self {
        Self {
            id: TriggerId::new(),
            kind,
            table_entity_type: table_entity_type.into(),
            condition: None,
            enabled: true,
        }
    }
}

// ── Trigger evaluator ─────────────────────────────────────────────

/// Evaluates incoming events against registered triggers.
///
/// The evaluator holds all active trigger configs and checks each
/// event against them. When a match is found, it returns the trigger
/// IDs that should fire.
pub struct TriggerEvaluator {
    triggers: Vec<TriggerConfig>,
}

impl TriggerEvaluator {
    /// Create a new evaluator with the given trigger configs.
    pub fn new(triggers: Vec<TriggerConfig>) -> Self {
        Self { triggers }
    }

    /// Replace all triggers (used when automations are updated).
    pub fn set_triggers(&mut self, triggers: Vec<TriggerConfig>) {
        self.triggers = triggers;
    }

    /// Add a single trigger.
    pub fn add_trigger(&mut self, trigger: TriggerConfig) {
        self.triggers.push(trigger);
    }

    /// Remove a trigger by ID.
    pub fn remove_trigger(&mut self, id: TriggerId) {
        self.triggers.retain(|t| t.id != id);
    }

    /// Evaluate an event against all active triggers.
    ///
    /// Returns the IDs of triggers that matched.
    pub fn evaluate(&self, event: &DdbEvent) -> Vec<TriggerId> {
        self.triggers
            .iter()
            .filter(|t| t.enabled && self.matches(t, event))
            .map(|t| t.id)
            .collect()
    }

    /// Check whether a single trigger matches an event.
    fn matches(&self, trigger: &TriggerConfig, event: &DdbEvent) -> bool {
        match (&trigger.kind, event) {
            // Record creation
            (TriggerKind::OnRecordCreate, DdbEvent::RecordCreated { entity_type, .. }) => {
                trigger.table_entity_type == *entity_type
                    && self.check_condition(trigger, event)
            }

            // Record update
            (TriggerKind::OnRecordUpdate, DdbEvent::RecordUpdated { entity_type, .. }) => {
                trigger.table_entity_type == *entity_type
                    && self.check_condition(trigger, event)
            }

            // Record deletion
            (TriggerKind::OnRecordDelete, DdbEvent::RecordDeleted { entity_type, .. }) => {
                trigger.table_entity_type == *entity_type
            }

            // Specific field change
            (
                TriggerKind::OnFieldChange { field },
                DdbEvent::FieldChanged {
                    entity_type,
                    field_name,
                    ..
                },
            ) => {
                trigger.table_entity_type == *entity_type
                    && field == field_name
                    && self.check_condition(trigger, event)
            }

            // Webhook triggers match webhook events
            (TriggerKind::OnWebhook, DdbEvent::Custom { kind, .. }) => kind == "webhook",

            // Form submit
            (TriggerKind::OnFormSubmit, DdbEvent::RecordCreated { entity_type, .. }) => {
                // Form submissions create records — match if the entity type matches
                // and the event metadata indicates a form source.
                trigger.table_entity_type == *entity_type
            }

            // Manual triggers never match events — they fire via the API only.
            (TriggerKind::Manual, _) => false,

            // Schedule triggers are handled by the cron scheduler, not events.
            (TriggerKind::OnSchedule { .. }, _) => false,

            _ => false,
        }
    }

    /// Check optional condition clauses against event data.
    ///
    /// If no condition is configured, returns `true`.
    fn check_condition(&self, trigger: &TriggerConfig, event: &DdbEvent) -> bool {
        let conditions = match &trigger.condition {
            Some(c) if !c.is_empty() => c,
            _ => return true,
        };

        let attributes = match event {
            DdbEvent::RecordCreated { attributes, .. }
            | DdbEvent::RecordUpdated { attributes, .. } => attributes,
            DdbEvent::FieldChanged { attributes, .. } => attributes,
            _ => return true,
        };

        // All conditions must match (AND semantics).
        conditions.iter().all(|clause| {
            attributes
                .get(&clause.attribute)
                .map(|val| match_where_clause(val, clause))
                .unwrap_or(false)
        })
    }
}

/// Evaluate a single where clause against a value.
fn match_where_clause(value: &serde_json::Value, clause: &WhereClause) -> bool {
    use crate::query::WhereOp;

    match clause.op {
        WhereOp::Eq => *value == clause.value,
        WhereOp::Neq => *value != clause.value,
        WhereOp::Gt => json_cmp(value, &clause.value).map_or(false, |o| o.is_gt()),
        WhereOp::Gte => json_cmp(value, &clause.value).map_or(false, |o| o.is_ge()),
        WhereOp::Lt => json_cmp(value, &clause.value).map_or(false, |o| o.is_lt()),
        WhereOp::Lte => json_cmp(value, &clause.value).map_or(false, |o| o.is_le()),
        WhereOp::Contains => {
            if let (Some(haystack), Some(needle)) = (value.as_str(), clause.value.as_str()) {
                haystack.contains(needle)
            } else {
                false
            }
        }
        WhereOp::Like => {
            if let (Some(haystack), Some(pattern)) = (value.as_str(), clause.value.as_str()) {
                haystack.starts_with(pattern.trim_end_matches('%'))
            } else {
                false
            }
        }
    }
}

/// Compare two JSON values for ordering.
fn json_cmp(a: &serde_json::Value, b: &serde_json::Value) -> Option<std::cmp::Ordering> {
    match (a, b) {
        (serde_json::Value::Number(a), serde_json::Value::Number(b)) => {
            a.as_f64()?.partial_cmp(&b.as_f64()?)
        }
        (serde_json::Value::String(a), serde_json::Value::String(b)) => Some(a.cmp(b)),
        _ => None,
    }
}

// ── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    fn make_create_event(entity_type: &str, attrs: HashMap<String, serde_json::Value>) -> DdbEvent {
        DdbEvent::RecordCreated {
            entity_id: Uuid::new_v4(),
            entity_type: entity_type.to_string(),
            attributes: attrs,
            tx_id: 1,
        }
    }

    fn make_update_event(entity_type: &str, changed: Vec<String>) -> DdbEvent {
        DdbEvent::RecordUpdated {
            entity_id: Uuid::new_v4(),
            entity_type: entity_type.to_string(),
            attributes: HashMap::new(),
            changed_fields: changed,
            tx_id: 2,
        }
    }

    fn make_delete_event(entity_type: &str) -> DdbEvent {
        DdbEvent::RecordDeleted {
            entity_id: Uuid::new_v4(),
            entity_type: entity_type.to_string(),
            tx_id: 3,
        }
    }

    fn make_field_change_event(
        entity_type: &str,
        field: &str,
        old: serde_json::Value,
        new: serde_json::Value,
    ) -> DdbEvent {
        DdbEvent::FieldChanged {
            entity_id: Uuid::new_v4(),
            entity_type: entity_type.to_string(),
            field_name: field.to_string(),
            old_value: Some(old),
            new_value: new,
            attributes: HashMap::new(),
            tx_id: 4,
        }
    }

    #[test]
    fn trigger_matches_on_record_create() {
        let trigger = TriggerConfig::new(TriggerKind::OnRecordCreate, "users");
        let evaluator = TriggerEvaluator::new(vec![trigger.clone()]);

        let event = make_create_event("users", HashMap::new());
        let matched = evaluator.evaluate(&event);
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0], trigger.id);
    }

    #[test]
    fn trigger_no_match_wrong_entity_type() {
        let trigger = TriggerConfig::new(TriggerKind::OnRecordCreate, "users");
        let evaluator = TriggerEvaluator::new(vec![trigger]);

        let event = make_create_event("posts", HashMap::new());
        let matched = evaluator.evaluate(&event);
        assert!(matched.is_empty());
    }

    #[test]
    fn trigger_matches_on_record_update() {
        let trigger = TriggerConfig::new(TriggerKind::OnRecordUpdate, "orders");
        let evaluator = TriggerEvaluator::new(vec![trigger.clone()]);

        let event = make_update_event("orders", vec!["status".to_string()]);
        let matched = evaluator.evaluate(&event);
        assert_eq!(matched.len(), 1);
    }

    #[test]
    fn trigger_matches_on_record_delete() {
        let trigger = TriggerConfig::new(TriggerKind::OnRecordDelete, "tasks");
        let evaluator = TriggerEvaluator::new(vec![trigger.clone()]);

        let event = make_delete_event("tasks");
        let matched = evaluator.evaluate(&event);
        assert_eq!(matched.len(), 1);
    }

    #[test]
    fn trigger_matches_on_field_change() {
        let trigger = TriggerConfig::new(
            TriggerKind::OnFieldChange {
                field: "status".to_string(),
            },
            "orders",
        );
        let evaluator = TriggerEvaluator::new(vec![trigger.clone()]);

        let event = make_field_change_event("orders", "status", json!("pending"), json!("shipped"));
        let matched = evaluator.evaluate(&event);
        assert_eq!(matched.len(), 1);
    }

    #[test]
    fn trigger_field_change_wrong_field() {
        let trigger = TriggerConfig::new(
            TriggerKind::OnFieldChange {
                field: "status".to_string(),
            },
            "orders",
        );
        let evaluator = TriggerEvaluator::new(vec![trigger]);

        let event = make_field_change_event("orders", "price", json!(100), json!(200));
        let matched = evaluator.evaluate(&event);
        assert!(matched.is_empty());
    }

    #[test]
    fn disabled_trigger_does_not_fire() {
        let mut trigger = TriggerConfig::new(TriggerKind::OnRecordCreate, "users");
        trigger.enabled = false;
        let evaluator = TriggerEvaluator::new(vec![trigger]);

        let event = make_create_event("users", HashMap::new());
        let matched = evaluator.evaluate(&event);
        assert!(matched.is_empty());
    }

    #[test]
    fn manual_trigger_never_matches_events() {
        let trigger = TriggerConfig::new(TriggerKind::Manual, "users");
        let evaluator = TriggerEvaluator::new(vec![trigger]);

        let event = make_create_event("users", HashMap::new());
        let matched = evaluator.evaluate(&event);
        assert!(matched.is_empty());
    }

    #[test]
    fn condition_filter_passes() {
        use crate::query::WhereOp;

        let mut trigger = TriggerConfig::new(TriggerKind::OnRecordCreate, "orders");
        trigger.condition = Some(vec![WhereClause {
            attribute: "amount".to_string(),
            op: WhereOp::Gt,
            value: json!(100),
        }]);

        let evaluator = TriggerEvaluator::new(vec![trigger.clone()]);

        let mut attrs = HashMap::new();
        attrs.insert("amount".to_string(), json!(250));
        let event = make_create_event("orders", attrs);
        let matched = evaluator.evaluate(&event);
        assert_eq!(matched.len(), 1);
    }

    #[test]
    fn condition_filter_rejects() {
        use crate::query::WhereOp;

        let mut trigger = TriggerConfig::new(TriggerKind::OnRecordCreate, "orders");
        trigger.condition = Some(vec![WhereClause {
            attribute: "amount".to_string(),
            op: WhereOp::Gt,
            value: json!(100),
        }]);

        let evaluator = TriggerEvaluator::new(vec![trigger]);

        let mut attrs = HashMap::new();
        attrs.insert("amount".to_string(), json!(50));
        let event = make_create_event("orders", attrs);
        let matched = evaluator.evaluate(&event);
        assert!(matched.is_empty());
    }

    #[test]
    fn multiple_triggers_can_fire() {
        let t1 = TriggerConfig::new(TriggerKind::OnRecordCreate, "users");
        let t2 = TriggerConfig::new(TriggerKind::OnRecordCreate, "users");
        let evaluator = TriggerEvaluator::new(vec![t1.clone(), t2.clone()]);

        let event = make_create_event("users", HashMap::new());
        let matched = evaluator.evaluate(&event);
        assert_eq!(matched.len(), 2);
    }

    #[test]
    fn cron_expr_parse_valid() {
        let expr = CronExpr::parse("0 0 * * * * *");
        assert!(expr.is_ok());
    }

    #[test]
    fn cron_expr_parse_invalid() {
        let expr = CronExpr::parse("not a cron");
        assert!(expr.is_err());
    }

    #[test]
    fn cron_expr_next_after() {
        let expr = CronExpr::parse("0 0 * * * * *").unwrap();
        let now = chrono::Utc::now();
        let next = expr.next_after(now);
        assert!(next.is_some());
        assert!(next.unwrap() > now);
    }

    #[test]
    fn cron_expr_serde_roundtrip() {
        let expr = CronExpr::parse("0 30 9 * * * *").unwrap();
        let json = serde_json::to_string(&expr).unwrap();
        let restored: CronExpr = serde_json::from_str(&json).unwrap();
        assert_eq!(expr.expression, restored.expression);
    }

    #[test]
    fn add_and_remove_triggers() {
        let mut evaluator = TriggerEvaluator::new(vec![]);
        assert!(evaluator.triggers.is_empty());

        let t = TriggerConfig::new(TriggerKind::OnRecordCreate, "users");
        let id = t.id;
        evaluator.add_trigger(t);
        assert_eq!(evaluator.triggers.len(), 1);

        evaluator.remove_trigger(id);
        assert!(evaluator.triggers.is_empty());
    }
}
