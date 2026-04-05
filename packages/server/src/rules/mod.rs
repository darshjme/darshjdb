//! Forward-chaining rule engine for DarshJDB.
//!
//! Inspired by GraphDB's TRREE engine, this module implements automatic
//! triple inference: when a triple is inserted, rules evaluate against
//! the new data and produce implied triples. Implied triples are written
//! in the same transaction, and can themselves trigger further rules up
//! to a configurable depth limit (default: 3) to prevent infinite loops.
//!
//! # Architecture
//!
//! Rules are loaded from a JSON configuration file (`darshan/rules.json`)
//! at startup and held in memory by [`RuleEngine`]. The engine is invoked
//! after every successful triple write, receiving the batch of new triples
//! and returning any implied triples that should be persisted.
//!
//! # Rule Structure
//!
//! Each [`Rule`] has a [`TriplePattern`] that matches against inserted
//! triples and a [`RuleAction`] that describes what to compute when the
//! pattern fires. Supported actions include computed attributes (concat,
//! copy, literal), value propagation across references, and counter
//! updates on related entities.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;
use uuid::Uuid;

use crate::error::{DarshJError, Result};
use crate::query::WhereOp;
use crate::triple_store::schema::ValueType;
use crate::triple_store::{PgTripleStore, Triple, TripleInput, TripleStore};

/// Maximum depth of chained rule evaluation to prevent infinite loops.
const MAX_CHAIN_DEPTH: u32 = 3;

// ── Core types ─────────────────────────────────────────────────────

/// A forward-chaining rule that fires when matching triples are inserted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    /// Human-readable rule name.
    pub name: String,
    /// Pattern to match against inserted triples.
    pub pattern: TriplePattern,
    /// Action to execute when pattern matches.
    pub action: RuleAction,
}

/// Pattern for matching incoming triples.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriplePattern {
    /// Match any triple with this attribute.
    Attribute(String),
    /// Match any triple with this entity type prefix (e.g. "users").
    EntityType(String),
    /// Match attribute + value condition.
    AttributeValue {
        attribute: String,
        condition: WhereOp,
        value: serde_json::Value,
    },
}

/// Action to execute when a rule's pattern fires.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuleAction {
    /// Set a computed attribute on the same entity.
    Compute {
        target: String,
        computation: Computation,
    },
    /// Copy/propagate a value to related entities.
    Propagate {
        follow_reference: String,
        target_attribute: String,
    },
    /// Increment/decrement a counter on a related entity.
    UpdateCounter {
        follow_reference: String,
        target_attribute: String,
        delta: i64,
    },
}

/// Computation types for derived attributes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Computation {
    /// Concatenate string attributes: e.g. fullName = firstName + " " + lastName.
    Concat {
        fields: Vec<String>,
        separator: String,
    },
    /// Count related entities by a reference attribute.
    CountRelated { reference_attribute: String },
    /// Copy value from another attribute on the same entity.
    Copy { source_attribute: String },
    /// Set a fixed value.
    Literal { value: serde_json::Value },
}

// ── Rule Engine ────────────────────────────────────────────────────

/// The forward-chaining rule engine. Holds rules in memory and evaluates
/// them against batches of newly inserted triples.
pub struct RuleEngine {
    rules: Vec<Rule>,
    triple_store: Arc<PgTripleStore>,
}

impl RuleEngine {
    /// Create a new rule engine with the given rules and triple store.
    pub fn new(rules: Vec<Rule>, triple_store: Arc<PgTripleStore>) -> Self {
        tracing::info!(rule_count = rules.len(), "rule engine initialized");
        for rule in &rules {
            tracing::debug!(name = %rule.name, "loaded rule");
        }
        Self {
            rules,
            triple_store,
        }
    }

    /// Return the number of loaded rules.
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// Evaluate all rules against a batch of newly inserted triples.
    ///
    /// Returns any implied triples that should be written. This method
    /// handles chaining: implied triples are re-evaluated up to
    /// [`MAX_CHAIN_DEPTH`] levels.
    pub async fn evaluate(&self, triples: &[TripleInput]) -> Result<Vec<TripleInput>> {
        if self.rules.is_empty() || triples.is_empty() {
            return Ok(Vec::new());
        }

        let mut all_implied: Vec<TripleInput> = Vec::new();
        let mut current_batch = triples.to_vec();
        // Track (entity_id, attribute) pairs we have already produced to avoid duplicates.
        let mut produced: HashSet<(Uuid, String)> = HashSet::new();

        for depth in 0..MAX_CHAIN_DEPTH {
            let mut new_implied: Vec<TripleInput> = Vec::new();

            for triple in &current_batch {
                for rule in &self.rules {
                    if self.matches_pattern(&rule.pattern, triple) {
                        let implied = self.execute_action(&rule.action, triple).await?;

                        for imp in implied {
                            let key = (imp.entity_id, imp.attribute.clone());
                            if !produced.contains(&key) {
                                produced.insert(key);
                                new_implied.push(imp);
                            }
                        }
                    }
                }
            }

            if new_implied.is_empty() {
                break;
            }

            tracing::debug!(
                depth = depth,
                implied_count = new_implied.len(),
                "rule chain level produced implied triples"
            );

            all_implied.extend(new_implied.clone());
            current_batch = new_implied;
        }

        Ok(all_implied)
    }

    /// Evaluate rules within an existing database transaction.
    ///
    /// This is the transactional variant used by the mutation pipeline.
    /// It evaluates rules and writes implied triples in the same transaction.
    pub async fn evaluate_and_write_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        triples: &[TripleInput],
        tx_id: i64,
    ) -> Result<Vec<TripleInput>> {
        if self.rules.is_empty() || triples.is_empty() {
            return Ok(Vec::new());
        }

        let mut all_implied: Vec<TripleInput> = Vec::new();
        let mut current_batch = triples.to_vec();
        let mut produced: HashSet<(Uuid, String)> = HashSet::new();

        for depth in 0..MAX_CHAIN_DEPTH {
            let mut new_implied: Vec<TripleInput> = Vec::new();

            for triple in &current_batch {
                for rule in &self.rules {
                    if self.matches_pattern(&rule.pattern, triple) {
                        let implied = self.execute_action_in_tx(tx, &rule.action, triple).await?;

                        for imp in implied {
                            let key = (imp.entity_id, imp.attribute.clone());
                            if !produced.contains(&key) {
                                produced.insert(key);
                                new_implied.push(imp);
                            }
                        }
                    }
                }
            }

            if new_implied.is_empty() {
                break;
            }

            tracing::debug!(
                depth = depth,
                implied_count = new_implied.len(),
                "rule chain level produced implied triples (in tx)"
            );

            // Retract old values for implied attributes, then write new ones.
            for imp in &new_implied {
                PgTripleStore::retract_in_tx(tx, imp.entity_id, &imp.attribute).await?;
            }
            PgTripleStore::set_triples_in_tx(tx, &new_implied, tx_id).await?;

            all_implied.extend(new_implied.clone());
            current_batch = new_implied;
        }

        Ok(all_implied)
    }

    /// Check whether a triple matches a rule's pattern.
    fn matches_pattern(&self, pattern: &TriplePattern, triple: &TripleInput) -> bool {
        pattern_matches(pattern, triple)
    }

    /// Execute a rule action and produce implied triples.
    ///
    /// Uses the triple store directly (not in a transaction).
    async fn execute_action(
        &self,
        action: &RuleAction,
        trigger: &TripleInput,
    ) -> Result<Vec<TripleInput>> {
        match action {
            RuleAction::Compute {
                target,
                computation,
            } => {
                let value = self.compute_value(computation, trigger.entity_id).await?;
                match value {
                    Some(v) => Ok(vec![TripleInput {
                        entity_id: trigger.entity_id,
                        attribute: target.clone(),
                        value: v,
                        value_type: ValueType::String as i16,
                        ttl_seconds: None,
                    }]),
                    None => Ok(Vec::new()),
                }
            }
            RuleAction::Propagate {
                follow_reference,
                target_attribute,
            } => {
                self.propagate_value(trigger, follow_reference, target_attribute)
                    .await
            }
            RuleAction::UpdateCounter {
                follow_reference,
                target_attribute,
                delta,
            } => {
                self.update_counter(trigger, follow_reference, target_attribute, *delta)
                    .await
            }
        }
    }

    /// Execute a rule action within an existing transaction.
    async fn execute_action_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        action: &RuleAction,
        trigger: &TripleInput,
    ) -> Result<Vec<TripleInput>> {
        match action {
            RuleAction::Compute {
                target,
                computation,
            } => {
                let value = self
                    .compute_value_in_tx(tx, computation, trigger.entity_id)
                    .await?;
                match value {
                    Some(v) => Ok(vec![TripleInput {
                        entity_id: trigger.entity_id,
                        attribute: target.clone(),
                        value: v,
                        value_type: ValueType::String as i16,
                        ttl_seconds: None,
                    }]),
                    None => Ok(Vec::new()),
                }
            }
            RuleAction::Propagate {
                follow_reference,
                target_attribute,
            } => {
                self.propagate_value_in_tx(tx, trigger, follow_reference, target_attribute)
                    .await
            }
            RuleAction::UpdateCounter {
                follow_reference,
                target_attribute,
                delta,
            } => {
                self.update_counter_in_tx(tx, trigger, follow_reference, target_attribute, *delta)
                    .await
            }
        }
    }

    /// Compute a derived value from an entity's current attributes.
    async fn compute_value(
        &self,
        computation: &Computation,
        entity_id: Uuid,
    ) -> Result<Option<serde_json::Value>> {
        let entity_triples = self.triple_store.get_entity(entity_id).await?;
        Ok(compute_from_triples(computation, &entity_triples))
    }

    /// Compute a derived value within an existing transaction.
    async fn compute_value_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        computation: &Computation,
        entity_id: Uuid,
    ) -> Result<Option<serde_json::Value>> {
        let entity_triples = PgTripleStore::get_entity_in_tx(tx, entity_id).await?;
        Ok(compute_from_triples(computation, &entity_triples))
    }

    /// Follow a reference attribute to propagate a value.
    async fn propagate_value(
        &self,
        trigger: &TripleInput,
        follow_reference: &str,
        target_attribute: &str,
    ) -> Result<Vec<TripleInput>> {
        let entity_triples = self.triple_store.get_entity(trigger.entity_id).await?;
        let ref_targets = extract_reference_targets(&entity_triples, follow_reference);

        Ok(ref_targets
            .into_iter()
            .map(|target_id| TripleInput {
                entity_id: target_id,
                attribute: target_attribute.to_string(),
                value: trigger.value.clone(),
                value_type: trigger.value_type,
                ttl_seconds: None,
            })
            .collect())
    }

    /// Follow a reference attribute to propagate a value (in transaction).
    async fn propagate_value_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        trigger: &TripleInput,
        follow_reference: &str,
        target_attribute: &str,
    ) -> Result<Vec<TripleInput>> {
        let entity_triples = PgTripleStore::get_entity_in_tx(tx, trigger.entity_id).await?;
        let ref_targets = extract_reference_targets(&entity_triples, follow_reference);

        Ok(ref_targets
            .into_iter()
            .map(|target_id| TripleInput {
                entity_id: target_id,
                attribute: target_attribute.to_string(),
                value: trigger.value.clone(),
                value_type: trigger.value_type,
                ttl_seconds: None,
            })
            .collect())
    }

    /// Update a counter on a related entity.
    async fn update_counter(
        &self,
        trigger: &TripleInput,
        follow_reference: &str,
        target_attribute: &str,
        delta: i64,
    ) -> Result<Vec<TripleInput>> {
        let entity_triples = self.triple_store.get_entity(trigger.entity_id).await?;
        let ref_targets = extract_reference_targets(&entity_triples, follow_reference);

        let mut results = Vec::new();
        for target_id in ref_targets {
            let target_triples = self.triple_store.get_entity(target_id).await?;
            let current = target_triples
                .iter()
                .find(|t| t.attribute == target_attribute)
                .and_then(|t| t.value.as_i64())
                .unwrap_or(0);

            results.push(TripleInput {
                entity_id: target_id,
                attribute: target_attribute.to_string(),
                value: serde_json::Value::Number(serde_json::Number::from(current + delta)),
                value_type: ValueType::Integer as i16,
                ttl_seconds: None,
            });
        }

        Ok(results)
    }

    /// Update a counter on a related entity (in transaction).
    async fn update_counter_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        trigger: &TripleInput,
        follow_reference: &str,
        target_attribute: &str,
        delta: i64,
    ) -> Result<Vec<TripleInput>> {
        let entity_triples = PgTripleStore::get_entity_in_tx(tx, trigger.entity_id).await?;
        let ref_targets = extract_reference_targets(&entity_triples, follow_reference);

        let mut results = Vec::new();
        for target_id in ref_targets {
            let target_triples = PgTripleStore::get_entity_in_tx(tx, target_id).await?;
            let current = target_triples
                .iter()
                .find(|t| t.attribute == target_attribute)
                .and_then(|t| t.value.as_i64())
                .unwrap_or(0);

            results.push(TripleInput {
                entity_id: target_id,
                attribute: target_attribute.to_string(),
                value: serde_json::Value::Number(serde_json::Number::from(current + delta)),
                value_type: ValueType::Integer as i16,
                ttl_seconds: None,
            });
        }

        Ok(results)
    }
}

// ── Pattern matching (standalone for testability) ──────────────────

/// Check whether a triple matches a pattern.
///
/// Extracted as a free function so it can be tested without constructing
/// a full [`RuleEngine`] (which requires a Tokio runtime for the PgPool).
fn pattern_matches(pattern: &TriplePattern, triple: &TripleInput) -> bool {
    match pattern {
        TriplePattern::Attribute(attr) => triple.attribute == *attr,
        TriplePattern::EntityType(prefix) => triple.attribute.starts_with(&format!("{}/", prefix)),
        TriplePattern::AttributeValue {
            attribute,
            condition,
            value,
        } => {
            if triple.attribute != *attribute {
                return false;
            }
            match condition {
                WhereOp::Eq => triple.value == *value,
                WhereOp::Neq => triple.value != *value,
                WhereOp::Gt => {
                    json_compare(&triple.value, value) == Some(std::cmp::Ordering::Greater)
                }
                WhereOp::Gte => {
                    matches!(
                        json_compare(&triple.value, value),
                        Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)
                    )
                }
                WhereOp::Lt => json_compare(&triple.value, value) == Some(std::cmp::Ordering::Less),
                WhereOp::Lte => {
                    matches!(
                        json_compare(&triple.value, value),
                        Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
                    )
                }
                WhereOp::Like | WhereOp::Contains => {
                    // Like/Contains not meaningful for single-value match; treat as no-match.
                    false
                }
            }
        }
    }
}

// ─��� Helper functions ───────────────────────────────────────────────

/// Compute a derived value from an entity's triples.
fn compute_from_triples(
    computation: &Computation,
    entity_triples: &[Triple],
) -> Option<serde_json::Value> {
    match computation {
        Computation::Concat { fields, separator } => {
            let parts: Vec<String> = fields
                .iter()
                .filter_map(|field| {
                    entity_triples
                        .iter()
                        .find(|t| t.attribute == *field)
                        .and_then(|t| t.value.as_str().map(|s| s.to_string()))
                })
                .collect();

            if parts.is_empty() {
                None
            } else {
                Some(serde_json::Value::String(parts.join(separator)))
            }
        }
        Computation::CountRelated {
            reference_attribute,
        } => {
            let count = entity_triples
                .iter()
                .filter(|t| t.attribute == *reference_attribute)
                .count();
            Some(serde_json::Value::Number(serde_json::Number::from(
                count as i64,
            )))
        }
        Computation::Copy { source_attribute } => entity_triples
            .iter()
            .find(|t| t.attribute == *source_attribute)
            .map(|t| t.value.clone()),
        Computation::Literal { value } => Some(value.clone()),
    }
}

/// Extract UUIDs from reference-type triples matching a given attribute.
fn extract_reference_targets(triples: &[Triple], reference_attribute: &str) -> Vec<Uuid> {
    triples
        .iter()
        .filter(|t| t.attribute == reference_attribute)
        .filter_map(|t| t.value.as_str().and_then(|s| Uuid::parse_str(s).ok()))
        .collect()
}

/// Compare two JSON values for ordering (numbers and strings only).
fn json_compare(a: &serde_json::Value, b: &serde_json::Value) -> Option<std::cmp::Ordering> {
    match (a, b) {
        (serde_json::Value::Number(a), serde_json::Value::Number(b)) => {
            let af = a.as_f64()?;
            let bf = b.as_f64()?;
            af.partial_cmp(&bf)
        }
        (serde_json::Value::String(a), serde_json::Value::String(b)) => Some(a.cmp(b)),
        _ => None,
    }
}

/// Load rules from a JSON file.
///
/// Returns an empty vec if the file does not exist (rules are optional).
/// Returns an error only if the file exists but cannot be parsed.
pub fn load_rules_from_file(path: &std::path::Path) -> Result<Vec<Rule>> {
    if !path.exists() {
        tracing::info!(path = %path.display(), "no rules file found, rule engine will be idle");
        return Ok(Vec::new());
    }

    let content = std::fs::read_to_string(path).map_err(|e| {
        DarshJError::Internal(format!("failed to read rules file {}: {e}", path.display()))
    })?;

    let rules: Vec<Rule> = serde_json::from_str(&content)?;
    tracing::info!(
        path = %path.display(),
        count = rules.len(),
        "loaded rules from file"
    );
    Ok(rules)
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── Pattern matching ───────────────────────────────────────────

    fn make_triple_input(
        entity_id: Uuid,
        attribute: &str,
        value: serde_json::Value,
    ) -> TripleInput {
        TripleInput {
            entity_id,
            attribute: attribute.to_string(),
            value,
            value_type: ValueType::String as i16,
            ttl_seconds: None,
        }
    }

    #[test]
    fn pattern_attribute_matches() {
        let pattern = TriplePattern::Attribute("users/firstName".to_string());
        let triple = make_triple_input(Uuid::new_v4(), "users/firstName", json!("Alice"));
        assert!(pattern_matches(&pattern, &triple));
    }

    #[test]
    fn pattern_attribute_no_match() {
        let pattern = TriplePattern::Attribute("users/firstName".to_string());
        let triple = make_triple_input(Uuid::new_v4(), "users/lastName", json!("Smith"));
        assert!(!pattern_matches(&pattern, &triple));
    }

    #[test]
    fn pattern_entity_type_matches() {
        let pattern = TriplePattern::EntityType("users".to_string());
        let triple = make_triple_input(Uuid::new_v4(), "users/email", json!("a@b.com"));
        assert!(pattern_matches(&pattern, &triple));
    }

    #[test]
    fn pattern_entity_type_no_match() {
        let pattern = TriplePattern::EntityType("posts".to_string());
        let triple = make_triple_input(Uuid::new_v4(), "users/email", json!("a@b.com"));
        assert!(!pattern_matches(&pattern, &triple));
    }

    #[test]
    fn pattern_attribute_value_eq_matches() {
        let pattern = TriplePattern::AttributeValue {
            attribute: "users/role".to_string(),
            condition: WhereOp::Eq,
            value: json!("admin"),
        };
        let triple = make_triple_input(Uuid::new_v4(), "users/role", json!("admin"));
        assert!(pattern_matches(&pattern, &triple));
    }

    #[test]
    fn pattern_attribute_value_eq_no_match() {
        let pattern = TriplePattern::AttributeValue {
            attribute: "users/role".to_string(),
            condition: WhereOp::Eq,
            value: json!("admin"),
        };
        let triple = make_triple_input(Uuid::new_v4(), "users/role", json!("user"));
        assert!(!pattern_matches(&pattern, &triple));
    }

    #[test]
    fn pattern_attribute_value_gt_matches() {
        let pattern = TriplePattern::AttributeValue {
            attribute: "users/age".to_string(),
            condition: WhereOp::Gt,
            value: json!(18),
        };
        let triple = make_triple_input(Uuid::new_v4(), "users/age", json!(25));
        assert!(pattern_matches(&pattern, &triple));
    }

    #[test]
    fn pattern_attribute_value_gt_no_match() {
        let pattern = TriplePattern::AttributeValue {
            attribute: "users/age".to_string(),
            condition: WhereOp::Gt,
            value: json!(18),
        };
        let triple = make_triple_input(Uuid::new_v4(), "users/age", json!(15));
        assert!(!pattern_matches(&pattern, &triple));
    }

    // ── Computation helpers ────────────────────────────────────────

    fn make_triple(attribute: &str, value: serde_json::Value) -> Triple {
        Triple {
            id: 1,
            entity_id: Uuid::new_v4(),
            attribute: attribute.to_string(),
            value,
            value_type: ValueType::String as i16,
            tx_id: 1,
            created_at: chrono::Utc::now(),
            retracted: false,
            expires_at: None,
        }
    }

    #[test]
    fn compute_concat() {
        let triples = vec![
            make_triple("users/firstName", json!("John")),
            make_triple("users/lastName", json!("Doe")),
        ];
        let comp = Computation::Concat {
            fields: vec!["users/firstName".to_string(), "users/lastName".to_string()],
            separator: " ".to_string(),
        };
        let result = compute_from_triples(&comp, &triples);
        assert_eq!(result, Some(json!("John Doe")));
    }

    #[test]
    fn compute_concat_partial() {
        let triples = vec![make_triple("users/firstName", json!("John"))];
        let comp = Computation::Concat {
            fields: vec!["users/firstName".to_string(), "users/lastName".to_string()],
            separator: " ".to_string(),
        };
        // Only one field present — should still produce a value with just that part.
        let result = compute_from_triples(&comp, &triples);
        assert_eq!(result, Some(json!("John")));
    }

    #[test]
    fn compute_concat_no_fields() {
        let triples = vec![];
        let comp = Computation::Concat {
            fields: vec!["users/firstName".to_string()],
            separator: " ".to_string(),
        };
        let result = compute_from_triples(&comp, &triples);
        assert_eq!(result, None);
    }

    #[test]
    fn compute_copy() {
        let triples = vec![make_triple("users/displayName", json!("Alice"))];
        let comp = Computation::Copy {
            source_attribute: "users/displayName".to_string(),
        };
        let result = compute_from_triples(&comp, &triples);
        assert_eq!(result, Some(json!("Alice")));
    }

    #[test]
    fn compute_literal() {
        let triples = vec![];
        let comp = Computation::Literal {
            value: json!("default"),
        };
        let result = compute_from_triples(&comp, &triples);
        assert_eq!(result, Some(json!("default")));
    }

    #[test]
    fn compute_count_related() {
        let triples = vec![
            make_triple("posts/tag", json!("rust")),
            make_triple("posts/tag", json!("db")),
        ];
        let comp = Computation::CountRelated {
            reference_attribute: "posts/tag".to_string(),
        };
        let result = compute_from_triples(&comp, &triples);
        assert_eq!(result, Some(json!(2)));
    }

    // ── JSON comparison ────────────────────────────────────────────

    #[test]
    fn json_compare_numbers() {
        assert_eq!(
            json_compare(&json!(10), &json!(5)),
            Some(std::cmp::Ordering::Greater)
        );
        assert_eq!(
            json_compare(&json!(3), &json!(7)),
            Some(std::cmp::Ordering::Less)
        );
        assert_eq!(
            json_compare(&json!(4), &json!(4)),
            Some(std::cmp::Ordering::Equal)
        );
    }

    #[test]
    fn json_compare_strings() {
        assert_eq!(
            json_compare(&json!("b"), &json!("a")),
            Some(std::cmp::Ordering::Greater)
        );
    }

    #[test]
    fn json_compare_mixed_returns_none() {
        assert_eq!(json_compare(&json!(1), &json!("a")), None);
    }

    // ── Rule deserialization ───────────────────────────────────────

    #[test]
    fn deserialize_rule_from_json() {
        let json_str = r#"{
            "name": "compute_fullName",
            "pattern": { "attribute": "users/firstName" },
            "action": {
                "type": "compute",
                "target": "users/fullName",
                "computation": {
                    "type": "concat",
                    "fields": ["users/firstName", "users/lastName"],
                    "separator": " "
                }
            }
        }"#;

        let rule: Rule = serde_json::from_str(json_str).unwrap();
        assert_eq!(rule.name, "compute_fullName");
        assert!(matches!(rule.pattern, TriplePattern::Attribute(ref a) if a == "users/firstName"));
        assert!(matches!(rule.action, RuleAction::Compute { .. }));
    }

    #[test]
    fn deserialize_rules_array() {
        let json_str = r#"[
            {
                "name": "r1",
                "pattern": { "attribute": "users/firstName" },
                "action": {
                    "type": "compute",
                    "target": "users/fullName",
                    "computation": { "type": "literal", "value": "test" }
                }
            },
            {
                "name": "r2",
                "pattern": { "entity_type": "posts" },
                "action": {
                    "type": "update_counter",
                    "follow_reference": "posts/authorId",
                    "target_attribute": "users/postCount",
                    "delta": 1
                }
            }
        ]"#;

        let rules: Vec<Rule> = serde_json::from_str(json_str).unwrap();
        assert_eq!(rules.len(), 2);
    }

    // ── Engine evaluate (empty rules) ──────────────────────────────

    /// Build an engine with no rules for testing. Requires a Tokio runtime.
    fn make_engine() -> RuleEngine {
        let pool = sqlx::PgPool::connect_lazy("postgres://localhost/test").expect("test pool");
        let store = Arc::new(PgTripleStore::new_lazy(pool));
        RuleEngine::new(Vec::new(), store)
    }

    #[tokio::test]
    async fn evaluate_empty_rules_returns_empty() {
        let engine = make_engine();
        let triples = vec![make_triple_input(
            Uuid::new_v4(),
            "users/name",
            json!("Alice"),
        )];
        let result = engine.evaluate(&triples).await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn evaluate_empty_triples_returns_empty() {
        let engine = make_engine();
        let result = engine.evaluate(&[]).await.unwrap();
        assert!(result.is_empty());
    }

    // ── Load rules from file ───────────────────────────────────────

    #[test]
    fn load_rules_nonexistent_file() {
        let result = load_rules_from_file(std::path::Path::new("/nonexistent/rules.json"));
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }
}
