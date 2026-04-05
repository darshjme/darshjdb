//! Fine-grained permission engine for DarshanDB.
//!
//! Permissions are expressed as composable rules that can:
//! - Allow or deny operations outright.
//! - Inject `WHERE` clauses into read queries (row-level security).
//! - Restrict which fields are visible or writable.
//! - Compose via `And`/`Or` combinators.
//!
//! # Permission DSL
//!
//! Rules can be loaded from a JSON/YAML configuration:
//!
//! ```json
//! {
//!   "type": "composite",
//!   "operator": "and",
//!   "rules": [
//!     { "type": "role_check", "required_role": "editor" },
//!     { "type": "where_clause", "sql": "owner_id = $user_id" }
//!   ]
//! }
//! ```
//!
//! # Evaluation
//!
//! For **reads**, the engine produces a SQL `WHERE` clause fragment that the
//! query planner injects, ensuring the database only returns permitted rows.
//!
//! For **writes**, the engine evaluates permission *before* the transaction
//! opens, returning an allow/deny decision.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{AuthContext, AuthError};

// ---------------------------------------------------------------------------
// Permission rule types
// ---------------------------------------------------------------------------

/// The fundamental operations that can be performed on an entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    /// Read / query entities.
    Read,
    /// Create new entities.
    Create,
    /// Update existing entities.
    Update,
    /// Delete entities.
    Delete,
    /// Subscribe to real-time changes.
    Subscribe,
}

/// A composable permission rule.
///
/// Rules are evaluated against an [`AuthContext`], an entity type, an
/// [`Operation`], and optionally the entity data itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PermissionRule {
    /// Unconditionally allow the operation.
    Allow,

    /// Unconditionally deny the operation.
    Deny,

    /// Require the caller to have a specific role.
    RoleCheck {
        /// The role that must be present in `AuthContext.roles`.
        required_role: String,
    },

    /// Inject a WHERE clause for read queries.
    ///
    /// The SQL fragment can reference `$user_id` which will be substituted
    /// with the authenticated user's ID.
    WhereClause {
        /// SQL fragment, e.g., `"owner_id = $user_id"`.
        sql: String,
    },

    /// Restrict visible or writable fields.
    FieldRestriction {
        /// Fields the caller is allowed to access. If empty, all fields
        /// are allowed. If non-empty, only these fields are visible.
        allowed_fields: Vec<String>,
        /// Fields explicitly denied. Applied after `allowed_fields`.
        denied_fields: Vec<String>,
    },

    /// Combine multiple rules with a logical operator.
    Composite {
        /// How to combine the child rules.
        operator: CompositeOperator,
        /// The child rules to evaluate.
        rules: Vec<PermissionRule>,
    },
}

/// Logical combinator for composite permission rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompositeOperator {
    /// All child rules must evaluate to allowed.
    And,
    /// At least one child rule must evaluate to allowed.
    Or,
}

// ---------------------------------------------------------------------------
// Permission result
// ---------------------------------------------------------------------------

/// The outcome of evaluating a permission rule.
#[derive(Debug, Clone)]
pub struct PermissionResult {
    /// Whether the operation is allowed.
    pub allowed: bool,
    /// SQL WHERE clause fragments to inject for read queries.
    /// Multiple fragments are ANDed together.
    pub where_clauses: Vec<String>,
    /// Fields that the caller may not see or write.
    pub restricted_fields: Vec<String>,
    /// Fields the caller is explicitly allowed to access.
    /// Empty means all fields are accessible (subject to `restricted_fields`).
    pub allowed_fields: Vec<String>,
    /// Human-readable reason if denied.
    pub denial_reason: Option<String>,
}

impl PermissionResult {
    /// A fully-permitted result with no restrictions.
    pub fn allow() -> Self {
        Self {
            allowed: true,
            where_clauses: Vec::new(),
            restricted_fields: Vec::new(),
            allowed_fields: Vec::new(),
            denial_reason: None,
        }
    }

    /// A denied result with a reason.
    pub fn deny(reason: impl Into<String>) -> Self {
        Self {
            allowed: false,
            where_clauses: Vec::new(),
            restricted_fields: Vec::new(),
            allowed_fields: Vec::new(),
            denial_reason: Some(reason.into()),
        }
    }

    /// Merge two results with AND semantics.
    ///
    /// Both must be allowed; WHERE clauses and field restrictions accumulate.
    pub fn and(self, other: Self) -> Self {
        if !self.allowed {
            return self;
        }
        if !other.allowed {
            return other;
        }

        let mut where_clauses = self.where_clauses;
        where_clauses.extend(other.where_clauses);

        let mut restricted_fields = self.restricted_fields;
        restricted_fields.extend(other.restricted_fields);
        restricted_fields.sort();
        restricted_fields.dedup();

        // For allowed_fields, take the intersection if both specify.
        let allowed_fields = match (
            self.allowed_fields.is_empty(),
            other.allowed_fields.is_empty(),
        ) {
            (true, true) => Vec::new(),
            (true, false) => other.allowed_fields,
            (false, true) => self.allowed_fields,
            (false, false) => {
                // Intersection.
                self.allowed_fields
                    .into_iter()
                    .filter(|f| other.allowed_fields.contains(f))
                    .collect()
            }
        };

        Self {
            allowed: true,
            where_clauses,
            restricted_fields,
            allowed_fields,
            denial_reason: None,
        }
    }

    /// Merge two results with OR semantics.
    ///
    /// At least one must be allowed. Takes the most permissive combination.
    pub fn or(self, other: Self) -> Self {
        if self.allowed && other.allowed {
            // Take union of where clauses (most permissive = fewest clauses).
            // For OR, if either has no where clause, the result has none.
            let where_clauses = if self.where_clauses.is_empty() || other.where_clauses.is_empty() {
                Vec::new()
            } else {
                // Combine with OR in SQL.
                let left = self.where_clauses.join(" AND ");
                let right = other.where_clauses.join(" AND ");
                vec![format!("({left}) OR ({right})")]
            };

            // Restricted fields: intersection (most permissive).
            let restricted_fields: Vec<String> = self
                .restricted_fields
                .iter()
                .filter(|f| other.restricted_fields.contains(f))
                .cloned()
                .collect();

            // Allowed fields: union (most permissive).
            let mut allowed_fields = self.allowed_fields;
            for f in other.allowed_fields {
                if !allowed_fields.contains(&f) {
                    allowed_fields.push(f);
                }
            }

            Self {
                allowed: true,
                where_clauses,
                restricted_fields,
                allowed_fields,
                denial_reason: None,
            }
        } else if self.allowed {
            self
        } else if other.allowed {
            other
        } else {
            Self::deny("all alternatives denied")
        }
    }

    /// Build the combined WHERE clause for SQL injection.
    ///
    /// Substitutes `$user_id` with the actual user ID parameter placeholder.
    pub fn build_where_clause(&self, user_id: Uuid) -> Option<String> {
        if self.where_clauses.is_empty() {
            return None;
        }
        let combined = self.where_clauses.join(" AND ");
        let substituted = combined.replace("$user_id", &format!("'{}'", user_id));
        Some(substituted)
    }
}

// ---------------------------------------------------------------------------
// Permission engine
// ---------------------------------------------------------------------------

/// The permission engine holds entity-type permission configurations and
/// evaluates them against request contexts.
pub struct PermissionEngine {
    /// Map from `(entity_type, operation)` to the rule tree.
    rules: std::collections::HashMap<(String, Operation), PermissionRule>,
}

impl PermissionEngine {
    /// Create an empty permission engine.
    pub fn new() -> Self {
        Self {
            rules: std::collections::HashMap::new(),
        }
    }

    /// Register a permission rule for a specific entity type and operation.
    pub fn add_rule(
        &mut self,
        entity_type: impl Into<String>,
        operation: Operation,
        rule: PermissionRule,
    ) {
        self.rules.insert((entity_type.into(), operation), rule);
    }

    /// Load rules from a JSON configuration object.
    ///
    /// Expected format:
    /// ```json
    /// {
    ///   "entity_type": {
    ///     "read": { "type": "allow" },
    ///     "create": { "type": "role_check", "required_role": "editor" }
    ///   }
    /// }
    /// ```
    pub fn load_from_config(&mut self, config: &serde_json::Value) -> Result<(), AuthError> {
        let obj = config
            .as_object()
            .ok_or_else(|| AuthError::Internal("permission config must be an object".into()))?;

        for (entity_type, ops) in obj {
            let ops_obj = ops.as_object().ok_or_else(|| {
                AuthError::Internal(format!("operations for '{entity_type}' must be an object"))
            })?;

            for (op_str, rule_value) in ops_obj {
                let operation: Operation = serde_json::from_value(serde_json::Value::String(
                    op_str.clone(),
                ))
                .map_err(|e| AuthError::Internal(format!("invalid operation '{op_str}': {e}")))?;

                let rule: PermissionRule =
                    serde_json::from_value(rule_value.clone()).map_err(|e| {
                        AuthError::Internal(format!("invalid rule for {entity_type}.{op_str}: {e}"))
                    })?;

                self.add_rule(entity_type.clone(), operation, rule);
            }
        }

        Ok(())
    }

    /// Look up the rule for a given entity type and operation.
    ///
    /// Returns `None` if no rule is configured, which the caller should
    /// treat as deny-by-default.
    pub fn get_rule(&self, entity_type: &str, operation: Operation) -> Option<&PermissionRule> {
        self.rules.get(&(entity_type.to_string(), operation))
    }
}

impl Default for PermissionEngine {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Evaluation
// ---------------------------------------------------------------------------

/// Evaluate a permission rule against a request context.
///
/// This is the primary entry point for the permission system. It
/// recursively evaluates the rule tree and produces a [`PermissionResult`]
/// that includes:
/// - Whether the operation is allowed.
/// - SQL WHERE fragments for read-path injection.
/// - Field restrictions for response filtering.
///
/// # Arguments
///
/// - `ctx`: The authenticated user context.
/// - `entity_type`: The type of entity being accessed (e.g., "document").
/// - `operation`: The operation being performed.
/// - `entity`: Optional entity data for field-level checks (pass `None`
///   for collection-level operations).
/// - `engine`: The permission engine with loaded rules.
pub fn evaluate_permission(
    ctx: &AuthContext,
    entity_type: &str,
    operation: Operation,
    _entity: Option<&serde_json::Value>,
    engine: &PermissionEngine,
) -> PermissionResult {
    let rule = match engine.get_rule(entity_type, operation) {
        Some(r) => r,
        None => {
            return PermissionResult::deny(format!(
                "no permission rule configured for {entity_type}.{operation:?}"
            ));
        }
    };

    evaluate_rule(ctx, rule)
}

/// Recursively evaluate a single permission rule.
fn evaluate_rule(ctx: &AuthContext, rule: &PermissionRule) -> PermissionResult {
    match rule {
        PermissionRule::Allow => PermissionResult::allow(),

        PermissionRule::Deny => PermissionResult::deny("explicitly denied"),

        PermissionRule::RoleCheck { required_role } => {
            if ctx.roles.iter().any(|r| r == required_role) {
                PermissionResult::allow()
            } else {
                PermissionResult::deny(format!("requires role '{required_role}'"))
            }
        }

        PermissionRule::WhereClause { sql } => {
            let mut result = PermissionResult::allow();
            result.where_clauses.push(sql.clone());
            result
        }

        PermissionRule::FieldRestriction {
            allowed_fields,
            denied_fields,
        } => {
            let mut result = PermissionResult::allow();
            result.allowed_fields = allowed_fields.clone();
            result.restricted_fields = denied_fields.clone();
            result
        }

        PermissionRule::Composite { operator, rules } => {
            if rules.is_empty() {
                return PermissionResult::deny("empty composite rule");
            }

            let mut iter = rules.iter();
            let first = evaluate_rule(ctx, iter.next().expect("non-empty checked above"));

            iter.fold(first, |acc, r| {
                let next = evaluate_rule(ctx, r);
                match operator {
                    CompositeOperator::And => acc.and(next),
                    CompositeOperator::Or => acc.or(next),
                }
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_ctx(roles: Vec<&str>) -> AuthContext {
        AuthContext {
            user_id: Uuid::new_v4(),
            session_id: Uuid::new_v4(),
            roles: roles.into_iter().map(String::from).collect(),
            ip: "127.0.0.1".into(),
            user_agent: "test".into(),
            device_fingerprint: "test-fp".into(),
        }
    }

    #[test]
    fn allow_rule() {
        let ctx = test_ctx(vec![]);
        let result = evaluate_rule(&ctx, &PermissionRule::Allow);
        assert!(result.allowed);
    }

    #[test]
    fn deny_rule() {
        let ctx = test_ctx(vec![]);
        let result = evaluate_rule(&ctx, &PermissionRule::Deny);
        assert!(!result.allowed);
    }

    #[test]
    fn role_check_passes() {
        let ctx = test_ctx(vec!["admin"]);
        let rule = PermissionRule::RoleCheck {
            required_role: "admin".into(),
        };
        assert!(evaluate_rule(&ctx, &rule).allowed);
    }

    #[test]
    fn role_check_fails() {
        let ctx = test_ctx(vec!["viewer"]);
        let rule = PermissionRule::RoleCheck {
            required_role: "admin".into(),
        };
        assert!(!evaluate_rule(&ctx, &rule).allowed);
    }

    #[test]
    fn where_clause_injected() {
        let ctx = test_ctx(vec![]);
        let rule = PermissionRule::WhereClause {
            sql: "owner_id = $user_id".into(),
        };
        let result = evaluate_rule(&ctx, &rule);
        assert!(result.allowed);
        assert_eq!(result.where_clauses, vec!["owner_id = $user_id"]);

        let clause = result.build_where_clause(ctx.user_id);
        assert!(clause.is_some());
        assert!(
            clause
                .as_ref()
                .is_some_and(|c| c.contains(&ctx.user_id.to_string()))
        );
    }

    #[test]
    fn composite_and() {
        let ctx = test_ctx(vec!["editor"]);
        let rule = PermissionRule::Composite {
            operator: CompositeOperator::And,
            rules: vec![
                PermissionRule::RoleCheck {
                    required_role: "editor".into(),
                },
                PermissionRule::WhereClause {
                    sql: "team_id = $user_id".into(),
                },
            ],
        };
        let result = evaluate_rule(&ctx, &rule);
        assert!(result.allowed);
        assert_eq!(result.where_clauses.len(), 1);
    }

    #[test]
    fn composite_and_denied() {
        let ctx = test_ctx(vec!["viewer"]);
        let rule = PermissionRule::Composite {
            operator: CompositeOperator::And,
            rules: vec![
                PermissionRule::RoleCheck {
                    required_role: "editor".into(),
                },
                PermissionRule::Allow,
            ],
        };
        assert!(!evaluate_rule(&ctx, &rule).allowed);
    }

    #[test]
    fn composite_or() {
        let ctx = test_ctx(vec!["viewer"]);
        let rule = PermissionRule::Composite {
            operator: CompositeOperator::Or,
            rules: vec![
                PermissionRule::RoleCheck {
                    required_role: "admin".into(),
                },
                PermissionRule::Allow,
            ],
        };
        assert!(evaluate_rule(&ctx, &rule).allowed);
    }

    #[test]
    fn field_restriction() {
        let ctx = test_ctx(vec![]);
        let rule = PermissionRule::FieldRestriction {
            allowed_fields: vec!["name".into(), "email".into()],
            denied_fields: vec!["password_hash".into()],
        };
        let result = evaluate_rule(&ctx, &rule);
        assert!(result.allowed);
        assert_eq!(result.allowed_fields, vec!["name", "email"]);
        assert_eq!(result.restricted_fields, vec!["password_hash"]);
    }

    #[test]
    fn engine_load_from_config() {
        let config = serde_json::json!({
            "document": {
                "read": { "type": "allow" },
                "create": { "type": "role_check", "required_role": "editor" },
                "delete": { "type": "deny" }
            }
        });

        let mut engine = PermissionEngine::new();
        engine.load_from_config(&config).expect("load");

        let ctx = test_ctx(vec!["editor"]);
        let result = evaluate_permission(&ctx, "document", Operation::Create, None, &engine);
        assert!(result.allowed);

        let result = evaluate_permission(&ctx, "document", Operation::Delete, None, &engine);
        assert!(!result.allowed);
    }
}
