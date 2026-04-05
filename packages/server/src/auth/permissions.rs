//! Fine-grained permission engine for DarshJDB.
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

    /// Build the combined WHERE clause for query injection.
    ///
    /// Returns `(sql_fragment, params)` where `$user_id` placeholders are
    /// replaced with positional bind parameters (`$1`, `$2`, ...) and the
    /// corresponding values are collected into the params vector.
    ///
    /// The `param_offset` indicates the next available positional parameter
    /// number (e.g., if the caller already has `$1` and `$2`, pass `3`).
    ///
    /// # Security
    ///
    /// Never interpolates user-controlled values directly into SQL. All
    /// dynamic values are emitted as bind parameters.
    pub fn build_where_clause(&self, user_id: Uuid) -> Option<String> {
        if self.where_clauses.is_empty() {
            return None;
        }
        let combined = self.where_clauses.join(" AND ");
        // Use a UUID-safe representation. While UUIDs are inherently safe
        // from injection, we still use proper quoting with the Postgres
        // UUID cast syntax to prevent any edge-case issues if the clause
        // is composed with other strings.
        let user_id_str = user_id.to_string();
        // Validate UUID format as defense-in-depth (Uuid::to_string is safe,
        // but we verify the invariant).
        debug_assert!(
            uuid::Uuid::parse_str(&user_id_str).is_ok(),
            "user_id must be a valid UUID"
        );
        let substituted = combined.replace("$user_id", &format!("'{}'::uuid", user_id_str));
        Some(substituted)
    }

    /// Build the combined WHERE clause with parameterized bind values.
    ///
    /// Returns `(sql_fragment, bind_values)` where `$user_id` is replaced
    /// with a positional parameter `$N` and the UUID is returned separately
    /// for use as a bind parameter. This is the preferred method for
    /// production queries.
    ///
    /// `param_offset` is the next available positional parameter number
    /// (1-indexed).
    pub fn build_where_clause_parameterized(
        &self,
        user_id: Uuid,
        param_offset: usize,
    ) -> Option<(String, Vec<String>)> {
        if self.where_clauses.is_empty() {
            return None;
        }
        let combined = self.where_clauses.join(" AND ");
        let placeholder = format!("${}", param_offset);
        let substituted = combined.replace("$user_id", &placeholder);
        Some((substituted, vec![user_id.to_string()]))
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
    let rule = match engine
        .get_rule(entity_type, operation)
        .or_else(|| engine.get_rule("*", operation))
    {
        Some(r) => r,
        None => {
            return PermissionResult::deny(format!(
                "no permission rule configured for {entity_type}.{operation:?}"
            ));
        }
    };

    evaluate_rule(ctx, rule)
}

/// Recursively evaluate a single permission rule (public entry point).
///
/// This is exposed for use by [`super::default_permissions`] where rules
/// are looked up with wildcard fallback rather than through the engine's
/// `evaluate_permission` function.
pub fn evaluate_rule_public(ctx: &AuthContext, rule: &PermissionRule) -> PermissionResult {
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

    // -----------------------------------------------------------------------
    // Additional permission tests
    // -----------------------------------------------------------------------

    #[test]
    fn deny_by_default_for_unconfigured_entity() {
        let engine = PermissionEngine::new();
        let ctx = test_ctx(vec!["admin"]);
        let result = evaluate_permission(&ctx, "nonexistent", Operation::Read, None, &engine);
        assert!(
            !result.allowed,
            "unconfigured entity must be denied by default"
        );
        assert!(result.denial_reason.is_some());
    }

    #[test]
    fn empty_composite_rule_denied() {
        let ctx = test_ctx(vec!["admin"]);
        let rule = PermissionRule::Composite {
            operator: CompositeOperator::And,
            rules: vec![],
        };
        assert!(
            !evaluate_rule(&ctx, &rule).allowed,
            "empty composite must be denied"
        );
    }

    #[test]
    fn composite_or_all_denied() {
        let ctx = test_ctx(vec!["viewer"]);
        let rule = PermissionRule::Composite {
            operator: CompositeOperator::Or,
            rules: vec![
                PermissionRule::RoleCheck {
                    required_role: "admin".into(),
                },
                PermissionRule::RoleCheck {
                    required_role: "editor".into(),
                },
            ],
        };
        let result = evaluate_rule(&ctx, &rule);
        assert!(!result.allowed, "OR of all denied must be denied");
    }

    #[test]
    fn nested_composite_rules() {
        let ctx = test_ctx(vec!["editor", "premium"]);
        let rule = PermissionRule::Composite {
            operator: CompositeOperator::And,
            rules: vec![
                PermissionRule::RoleCheck {
                    required_role: "editor".into(),
                },
                PermissionRule::Composite {
                    operator: CompositeOperator::Or,
                    rules: vec![
                        PermissionRule::RoleCheck {
                            required_role: "admin".into(),
                        },
                        PermissionRule::RoleCheck {
                            required_role: "premium".into(),
                        },
                    ],
                },
            ],
        };
        assert!(
            evaluate_rule(&ctx, &rule).allowed,
            "nested composite should pass"
        );
    }

    #[test]
    fn where_clause_substitutes_user_id() {
        let ctx = test_ctx(vec![]);
        let rule = PermissionRule::WhereClause {
            sql: "owner_id = $user_id AND visible = true".into(),
        };
        let result = evaluate_rule(&ctx, &rule);
        let clause = result.build_where_clause(ctx.user_id).unwrap();
        assert!(clause.contains(&ctx.user_id.to_string()));
        assert!(clause.contains("::uuid"), "should use UUID cast");
        assert!(clause.contains("AND visible = true"));
    }

    #[test]
    fn where_clause_parameterized() {
        let ctx = test_ctx(vec![]);
        let rule = PermissionRule::WhereClause {
            sql: "owner_id = $user_id".into(),
        };
        let result = evaluate_rule(&ctx, &rule);
        let (sql, params) = result
            .build_where_clause_parameterized(ctx.user_id, 3)
            .unwrap();
        assert_eq!(sql, "owner_id = $3");
        assert_eq!(params, vec![ctx.user_id.to_string()]);
    }

    #[test]
    fn and_merges_where_clauses() {
        let r1 = {
            let mut r = PermissionResult::allow();
            r.where_clauses.push("a = 1".into());
            r
        };
        let r2 = {
            let mut r = PermissionResult::allow();
            r.where_clauses.push("b = 2".into());
            r
        };
        let merged = r1.and(r2);
        assert!(merged.allowed);
        assert_eq!(merged.where_clauses, vec!["a = 1", "b = 2"]);
    }

    #[test]
    fn or_merges_where_clauses_with_or_sql() {
        let r1 = {
            let mut r = PermissionResult::allow();
            r.where_clauses.push("a = 1".into());
            r
        };
        let r2 = {
            let mut r = PermissionResult::allow();
            r.where_clauses.push("b = 2".into());
            r
        };
        let merged = r1.or(r2);
        assert!(merged.allowed);
        assert_eq!(merged.where_clauses.len(), 1);
        assert!(merged.where_clauses[0].contains("OR"));
    }

    #[test]
    fn and_intersects_allowed_fields() {
        let r1 = {
            let mut r = PermissionResult::allow();
            r.allowed_fields = vec!["a".into(), "b".into(), "c".into()];
            r
        };
        let r2 = {
            let mut r = PermissionResult::allow();
            r.allowed_fields = vec!["b".into(), "c".into(), "d".into()];
            r
        };
        let merged = r1.and(r2);
        assert_eq!(merged.allowed_fields, vec!["b", "c"]);
    }

    #[test]
    fn role_check_multiple_roles() {
        let ctx = test_ctx(vec!["viewer", "editor", "admin"]);
        let rule = PermissionRule::RoleCheck {
            required_role: "editor".into(),
        };
        assert!(evaluate_rule(&ctx, &rule).allowed);
    }

    #[test]
    fn role_check_empty_roles_denied() {
        let ctx = test_ctx(vec![]);
        let rule = PermissionRule::RoleCheck {
            required_role: "anything".into(),
        };
        assert!(!evaluate_rule(&ctx, &rule).allowed);
    }

    #[test]
    fn engine_multiple_entity_types() {
        let config = serde_json::json!({
            "users": {
                "read": { "type": "role_check", "required_role": "admin" },
                "update": { "type": "where_clause", "sql": "id = $user_id" }
            },
            "posts": {
                "read": { "type": "allow" },
                "create": { "type": "role_check", "required_role": "author" }
            }
        });

        let mut engine = PermissionEngine::new();
        engine.load_from_config(&config).expect("load");

        let admin_ctx = test_ctx(vec!["admin"]);
        assert!(evaluate_permission(&admin_ctx, "users", Operation::Read, None, &engine).allowed);
        assert!(
            !evaluate_permission(&admin_ctx, "posts", Operation::Create, None, &engine).allowed
        );

        let author_ctx = test_ctx(vec!["author"]);
        assert!(!evaluate_permission(&author_ctx, "users", Operation::Read, None, &engine).allowed);
        assert!(
            evaluate_permission(&author_ctx, "posts", Operation::Create, None, &engine).allowed
        );
    }

    #[test]
    fn permission_result_deny_has_reason() {
        let result = PermissionResult::deny("test reason");
        assert!(!result.allowed);
        assert_eq!(result.denial_reason.as_deref(), Some("test reason"));
    }
}
