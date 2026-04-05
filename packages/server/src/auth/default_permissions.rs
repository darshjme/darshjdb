//! Default permission configuration for DarshanDB.
//!
//! Provides sensible defaults that enforce:
//! - All operations require authentication.
//! - Users can only read/update/delete their own data (row-level security via `owner_id = $user_id`).
//! - Admin role bypasses all permission restrictions.
//! - The "users" entity has stricter rules: users can only read their own record.

use super::permissions::{CompositeOperator, Operation, PermissionEngine, PermissionRule};

/// Build a [`PermissionEngine`] with the default DarshanDB permission rules.
///
/// # Default rules
///
/// | Entity   | Operation | Rule                                                    |
/// |----------|-----------|---------------------------------------------------------|
/// | `*`      | read      | Authenticated + `owner_id = $user_id` OR admin          |
/// | `*`      | create    | Authenticated OR admin                                  |
/// | `*`      | update    | Authenticated + `owner_id = $user_id` OR admin          |
/// | `*`      | delete    | Authenticated + `owner_id = $user_id` OR admin          |
/// | `users`  | read      | `id = $user_id` OR admin (users can only see themselves) |
/// | `users`  | update    | `id = $user_id` OR admin                                |
/// | `users`  | delete    | Admin only                                              |
/// | `users`  | create    | Admin only                                              |
pub fn build_default_engine() -> PermissionEngine {
    let mut engine = PermissionEngine::new();

    // -- Wildcard rules (applied to any entity not explicitly configured) ------

    // Read: admin bypass OR owner filter
    engine.add_rule(
        "*",
        Operation::Read,
        admin_or(PermissionRule::WhereClause {
            sql: "owner_id = $user_id".into(),
        }),
    );

    // Create: admin bypass OR allow (authenticated users can create)
    engine.add_rule("*", Operation::Create, admin_or(PermissionRule::Allow));

    // Update: admin bypass OR owner filter
    engine.add_rule(
        "*",
        Operation::Update,
        admin_or(PermissionRule::WhereClause {
            sql: "owner_id = $user_id".into(),
        }),
    );

    // Delete: admin bypass OR owner filter
    engine.add_rule(
        "*",
        Operation::Delete,
        admin_or(PermissionRule::WhereClause {
            sql: "owner_id = $user_id".into(),
        }),
    );

    // Subscribe: same as read
    engine.add_rule(
        "*",
        Operation::Subscribe,
        admin_or(PermissionRule::WhereClause {
            sql: "owner_id = $user_id".into(),
        }),
    );

    // -- Users entity (stricter) -----------------------------------------------

    // Users read: admin OR id = $user_id (can only see own record)
    engine.add_rule(
        "users",
        Operation::Read,
        admin_or(PermissionRule::WhereClause {
            sql: "id = $user_id".into(),
        }),
    );

    // Users update: admin OR id = $user_id
    engine.add_rule(
        "users",
        Operation::Update,
        admin_or(PermissionRule::WhereClause {
            sql: "id = $user_id".into(),
        }),
    );

    // Users create: admin only
    engine.add_rule(
        "users",
        Operation::Create,
        PermissionRule::RoleCheck {
            required_role: "admin".into(),
        },
    );

    // Users delete: admin only
    engine.add_rule(
        "users",
        Operation::Delete,
        PermissionRule::RoleCheck {
            required_role: "admin".into(),
        },
    );

    engine
}

/// Convenience: wrap a rule with "admin OR <rule>".
///
/// If the user has the "admin" role, the inner rule is bypassed entirely
/// (no WHERE clauses, no field restrictions).
fn admin_or(inner: PermissionRule) -> PermissionRule {
    PermissionRule::Composite {
        operator: CompositeOperator::Or,
        rules: vec![
            PermissionRule::RoleCheck {
                required_role: "admin".into(),
            },
            inner,
        ],
    }
}

/// Look up a permission rule, falling back to the wildcard (`*`) entity
/// if no entity-specific rule is configured.
///
/// Returns `None` only if neither the specific entity nor the wildcard
/// has a rule for the given operation, which should be treated as
/// deny-by-default.
pub fn get_rule_with_fallback<'a>(
    engine: &'a PermissionEngine,
    entity_type: &str,
    operation: Operation,
) -> Option<&'a PermissionRule> {
    engine
        .get_rule(entity_type, operation)
        .or_else(|| engine.get_rule("*", operation))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthContext;
    use crate::auth::permissions::PermissionResult;
    use uuid::Uuid;

    fn ctx_with_roles(roles: &[&str]) -> AuthContext {
        AuthContext {
            user_id: Uuid::new_v4(),
            session_id: Uuid::new_v4(),
            roles: roles.iter().map(|r| r.to_string()).collect(),
            ip: "127.0.0.1".into(),
            user_agent: "test".into(),
            device_fingerprint: "test-fp".into(),
        }
    }

    fn evaluate_with_fallback(
        ctx: &AuthContext,
        entity_type: &str,
        operation: Operation,
        engine: &PermissionEngine,
    ) -> PermissionResult {
        match get_rule_with_fallback(engine, entity_type, operation) {
            Some(rule) => crate::auth::permissions::evaluate_rule_public(ctx, rule),
            None => PermissionResult::deny(format!(
                "no permission rule for {entity_type}.{operation:?}"
            )),
        }
    }

    #[test]
    fn admin_bypasses_all() {
        let engine = build_default_engine();
        let ctx = ctx_with_roles(&["admin"]);

        // Admin can read any entity without WHERE clause.
        let result = evaluate_with_fallback(&ctx, "posts", Operation::Read, &engine);
        assert!(result.allowed);
        assert!(
            result.where_clauses.is_empty(),
            "admin should have no WHERE clauses"
        );

        // Admin can create users.
        let result = evaluate_with_fallback(&ctx, "users", Operation::Create, &engine);
        assert!(result.allowed);

        // Admin can delete users.
        let result = evaluate_with_fallback(&ctx, "users", Operation::Delete, &engine);
        assert!(result.allowed);
    }

    #[test]
    fn regular_user_gets_owner_filter() {
        let engine = build_default_engine();
        let ctx = ctx_with_roles(&["user"]);

        // Regular user reading posts gets WHERE owner_id = $user_id.
        let result = evaluate_with_fallback(&ctx, "posts", Operation::Read, &engine);
        assert!(result.allowed);
        assert_eq!(result.where_clauses.len(), 1);
        assert!(result.where_clauses[0].contains("owner_id = $user_id"));
    }

    #[test]
    fn regular_user_can_create() {
        let engine = build_default_engine();
        let ctx = ctx_with_roles(&["user"]);

        // Regular user can create generic entities.
        let result = evaluate_with_fallback(&ctx, "posts", Operation::Create, &engine);
        assert!(result.allowed);
    }

    #[test]
    fn regular_user_cannot_create_users() {
        let engine = build_default_engine();
        let ctx = ctx_with_roles(&["user"]);

        // Regular user cannot create user records (admin only).
        let result = evaluate_with_fallback(&ctx, "users", Operation::Create, &engine);
        assert!(!result.allowed);
    }

    #[test]
    fn regular_user_cannot_delete_users() {
        let engine = build_default_engine();
        let ctx = ctx_with_roles(&["user"]);

        let result = evaluate_with_fallback(&ctx, "users", Operation::Delete, &engine);
        assert!(!result.allowed);
    }

    #[test]
    fn users_entity_filters_by_id() {
        let engine = build_default_engine();
        let ctx = ctx_with_roles(&["user"]);

        // Reading users should filter by id = $user_id.
        let result = evaluate_with_fallback(&ctx, "users", Operation::Read, &engine);
        assert!(result.allowed);
        assert_eq!(result.where_clauses.len(), 1);
        assert!(result.where_clauses[0].contains("id = $user_id"));
    }

    #[test]
    fn wildcard_fallback_works() {
        let engine = build_default_engine();
        let ctx = ctx_with_roles(&["user"]);

        // An entity with no specific rules falls back to wildcard.
        let result = evaluate_with_fallback(&ctx, "documents", Operation::Read, &engine);
        assert!(result.allowed);
        assert!(result.where_clauses[0].contains("owner_id = $user_id"));
    }

    #[test]
    fn where_clause_substitutes_user_id() {
        let engine = build_default_engine();
        let ctx = ctx_with_roles(&["user"]);

        let result = evaluate_with_fallback(&ctx, "posts", Operation::Read, &engine);
        let clause = result.build_where_clause(ctx.user_id).unwrap();
        assert!(clause.contains(&ctx.user_id.to_string()));
    }
}
