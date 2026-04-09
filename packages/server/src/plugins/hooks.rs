//! Plugin hook system.
//!
//! Hooks allow plugins to intercept and modify the request lifecycle
//! at well-defined points. Each hook has an ordered list of handlers;
//! execution proceeds in registration order until a handler rejects
//! the operation or the list is exhausted.

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::RwLock;
use tracing::{debug, warn};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Hook variants
// ---------------------------------------------------------------------------

/// Lifecycle points where plugins can intercept operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Hook {
    /// Before a new entity is created.
    BeforeCreate,
    /// After a new entity has been created.
    AfterCreate,
    /// Before an entity is updated.
    BeforeUpdate,
    /// After an entity has been updated.
    AfterUpdate,
    /// Before an entity is deleted.
    BeforeDelete,
    /// After an entity has been deleted.
    AfterDelete,
    /// Before a query is executed.
    BeforeQuery,
    /// After a query has returned results.
    AfterQuery,
    /// On authentication events (login, token refresh).
    OnAuth,
    /// When an unhandled error occurs.
    OnError,
}

impl Hook {
    /// All hook variants, useful for iteration.
    pub const ALL: &'static [Hook] = &[
        Hook::BeforeCreate,
        Hook::AfterCreate,
        Hook::BeforeUpdate,
        Hook::AfterUpdate,
        Hook::BeforeDelete,
        Hook::AfterDelete,
        Hook::BeforeQuery,
        Hook::AfterQuery,
        Hook::OnAuth,
        Hook::OnError,
    ];
}

impl fmt::Display for Hook {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::BeforeCreate => "before_create",
            Self::AfterCreate => "after_create",
            Self::BeforeUpdate => "before_update",
            Self::AfterUpdate => "after_update",
            Self::BeforeDelete => "before_delete",
            Self::AfterDelete => "after_delete",
            Self::BeforeQuery => "before_query",
            Self::AfterQuery => "after_query",
            Self::OnAuth => "on_auth",
            Self::OnError => "on_error",
        };
        f.write_str(s)
    }
}

// ---------------------------------------------------------------------------
// Hook context & result
// ---------------------------------------------------------------------------

/// Context passed to hook handlers, containing all relevant data about
/// the event being intercepted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookContext {
    /// Which lifecycle event triggered this hook.
    pub hook: Hook,
    /// The entity type / collection (e.g. `"users"`, `"tasks"`).
    pub entity_type: String,
    /// The entity ID (if applicable — `None` for queries).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_id: Option<Uuid>,
    /// The authenticated user who initiated the operation (if known).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<Uuid>,
    /// The data payload — entity attributes for mutations, query params
    /// for queries, error details for `OnError`.
    #[serde(default)]
    pub data: Value,
    /// Arbitrary metadata for inter-plugin communication.
    #[serde(default)]
    pub metadata: HashMap<String, Value>,
}

impl HookContext {
    /// Create a new context for a mutation hook.
    pub fn mutation(
        hook: Hook,
        entity_type: impl Into<String>,
        entity_id: Uuid,
        user_id: Option<Uuid>,
        data: Value,
    ) -> Self {
        Self {
            hook,
            entity_type: entity_type.into(),
            entity_id: Some(entity_id),
            user_id,
            data,
            metadata: HashMap::new(),
        }
    }

    /// Create a new context for a query hook.
    pub fn query(
        hook: Hook,
        entity_type: impl Into<String>,
        user_id: Option<Uuid>,
        data: Value,
    ) -> Self {
        Self {
            hook,
            entity_type: entity_type.into(),
            entity_id: None,
            user_id,
            data,
            metadata: HashMap::new(),
        }
    }
}

/// The result returned by a hook handler, controlling whether the
/// pipeline continues, modifies data, or rejects the operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum HookResult {
    /// Allow the operation to proceed unchanged.
    Continue,
    /// Allow the operation to proceed with modified data.
    Modify(Value),
    /// Reject the operation with a reason string.
    Reject(String),
}

// ---------------------------------------------------------------------------
// Hook handler trait
// ---------------------------------------------------------------------------

/// A handler registered for a specific [`Hook`].
///
/// Implementations must be `Send + Sync` for concurrent execution
/// across tokio tasks.
pub trait HookHandler: Send + Sync {
    /// Human-readable name for logging and debugging.
    fn name(&self) -> &str;

    /// The plugin that registered this handler.
    fn plugin_id(&self) -> Uuid;

    /// Handle the hook event and return a result controlling the pipeline.
    fn handle(&self, ctx: &HookContext) -> Pin<Box<dyn Future<Output = HookResult> + Send + '_>>;
}

// ---------------------------------------------------------------------------
// Hook registry
// ---------------------------------------------------------------------------

/// A handler entry in the registry, wrapping the trait object with priority.
struct HandlerEntry {
    /// Lower priority executes first (default: 100).
    priority: u32,
    handler: Arc<dyn HookHandler>,
}

/// Thread-safe registry of hook handlers.
///
/// Handlers are stored per-hook in priority-sorted order. The registry
/// supports registration and unregistration (by plugin ID) and
/// executes handlers sequentially, short-circuiting on `Reject`.
pub struct HookRegistry {
    /// Map from hook variant to ordered list of handler entries.
    handlers: RwLock<HashMap<Hook, Vec<HandlerEntry>>>,
}

impl HookRegistry {
    /// Create an empty hook registry.
    pub fn new() -> Self {
        Self {
            handlers: RwLock::new(HashMap::new()),
        }
    }

    /// Register a handler for a specific hook with default priority (100).
    pub async fn register(&self, hook: Hook, handler: Arc<dyn HookHandler>) {
        self.register_with_priority(hook, handler, 100).await;
    }

    /// Register a handler for a specific hook with explicit priority.
    /// Lower values execute first.
    pub async fn register_with_priority(
        &self,
        hook: Hook,
        handler: Arc<dyn HookHandler>,
        priority: u32,
    ) {
        let mut map = self.handlers.write().await;
        let entries = map.entry(hook).or_default();
        entries.push(HandlerEntry { priority, handler });
        // Keep sorted by priority (stable sort preserves insertion order
        // for equal priorities).
        entries.sort_by_key(|e| e.priority);
        debug!(hook = %hook, "hook handler registered");
    }

    /// Remove all handlers registered by a given plugin.
    pub async fn unregister_plugin(&self, plugin_id: Uuid) {
        let mut map = self.handlers.write().await;
        for entries in map.values_mut() {
            entries.retain(|e| e.handler.plugin_id() != plugin_id);
        }
        debug!(plugin_id = %plugin_id, "unregistered all hooks for plugin");
    }

    /// Execute all handlers for a hook in priority order.
    ///
    /// If any handler returns `Reject`, execution stops immediately.
    /// If a handler returns `Modify`, the modified data is merged into
    /// the context for subsequent handlers.
    pub async fn execute(&self, ctx: &mut HookContext) -> HookResult {
        let map = self.handlers.read().await;
        let entries = match map.get(&ctx.hook) {
            Some(e) => e,
            None => return HookResult::Continue,
        };

        let mut last_result = HookResult::Continue;

        for entry in entries {
            let result = entry.handler.handle(ctx).await;

            match &result {
                HookResult::Continue => {}
                HookResult::Modify(new_data) => {
                    // Merge modified data into the context so the next
                    // handler sees the updated version.
                    ctx.data = new_data.clone();
                    last_result = result;
                }
                HookResult::Reject(reason) => {
                    warn!(
                        hook = %ctx.hook,
                        handler = entry.handler.name(),
                        reason = %reason,
                        "hook handler rejected operation"
                    );
                    return result;
                }
            }
        }

        last_result
    }

    /// Return the number of handlers registered for a specific hook.
    pub async fn handler_count(&self, hook: Hook) -> usize {
        let map = self.handlers.read().await;
        map.get(&hook).map_or(0, |v| v.len())
    }

    /// Return the total number of handlers across all hooks.
    pub async fn total_handler_count(&self) -> usize {
        let map = self.handlers.read().await;
        map.values().map(|v| v.len()).sum()
    }
}

impl Default for HookRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// A simple test handler that always continues.
    struct PassHandler {
        name: String,
        plugin_id: Uuid,
    }

    impl HookHandler for PassHandler {
        fn name(&self) -> &str {
            &self.name
        }
        fn plugin_id(&self) -> Uuid {
            self.plugin_id
        }
        fn handle(
            &self,
            _ctx: &HookContext,
        ) -> Pin<Box<dyn Future<Output = HookResult> + Send + '_>> {
            Box::pin(async { HookResult::Continue })
        }
    }

    /// A handler that rejects with a reason.
    struct RejectHandler {
        plugin_id: Uuid,
        reason: String,
    }

    impl HookHandler for RejectHandler {
        fn name(&self) -> &str {
            "reject"
        }
        fn plugin_id(&self) -> Uuid {
            self.plugin_id
        }
        fn handle(
            &self,
            _ctx: &HookContext,
        ) -> Pin<Box<dyn Future<Output = HookResult> + Send + '_>> {
            let reason = self.reason.clone();
            Box::pin(async move { HookResult::Reject(reason) })
        }
    }

    /// A handler that modifies data.
    struct ModifyHandler {
        plugin_id: Uuid,
        key: String,
        value: Value,
    }

    impl HookHandler for ModifyHandler {
        fn name(&self) -> &str {
            "modify"
        }
        fn plugin_id(&self) -> Uuid {
            self.plugin_id
        }
        fn handle(
            &self,
            ctx: &HookContext,
        ) -> Pin<Box<dyn Future<Output = HookResult> + Send + '_>> {
            let mut data = ctx.data.clone();
            let key = self.key.clone();
            let value = self.value.clone();
            Box::pin(async move {
                if let Some(obj) = data.as_object_mut() {
                    obj.insert(key, value);
                }
                HookResult::Modify(data)
            })
        }
    }

    fn make_ctx(hook: Hook) -> HookContext {
        HookContext::mutation(
            hook,
            "tasks",
            Uuid::new_v4(),
            Some(Uuid::new_v4()),
            serde_json::json!({ "title": "test" }),
        )
    }

    #[tokio::test]
    async fn empty_registry_continues() {
        let registry = HookRegistry::new();
        let mut ctx = make_ctx(Hook::BeforeCreate);
        let result = registry.execute(&mut ctx).await;
        assert!(matches!(result, HookResult::Continue));
    }

    #[tokio::test]
    async fn single_handler_continue() {
        let registry = HookRegistry::new();
        let pid = Uuid::new_v4();
        registry
            .register(
                Hook::BeforeCreate,
                Arc::new(PassHandler {
                    name: "pass".into(),
                    plugin_id: pid,
                }),
            )
            .await;

        let mut ctx = make_ctx(Hook::BeforeCreate);
        let result = registry.execute(&mut ctx).await;
        assert!(matches!(result, HookResult::Continue));
        assert_eq!(registry.handler_count(Hook::BeforeCreate).await, 1);
    }

    #[tokio::test]
    async fn reject_stops_pipeline() {
        let registry = HookRegistry::new();
        let pid1 = Uuid::new_v4();
        let pid2 = Uuid::new_v4();

        // Register a reject handler first (priority 50), then a pass (priority 100).
        registry
            .register_with_priority(
                Hook::BeforeCreate,
                Arc::new(RejectHandler {
                    plugin_id: pid1,
                    reason: "not allowed".into(),
                }),
                50,
            )
            .await;
        registry
            .register(
                Hook::BeforeCreate,
                Arc::new(PassHandler {
                    name: "pass".into(),
                    plugin_id: pid2,
                }),
            )
            .await;

        let mut ctx = make_ctx(Hook::BeforeCreate);
        let result = registry.execute(&mut ctx).await;
        match result {
            HookResult::Reject(reason) => assert_eq!(reason, "not allowed"),
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn modify_propagates_data() {
        let registry = HookRegistry::new();
        let pid = Uuid::new_v4();

        registry
            .register(
                Hook::BeforeCreate,
                Arc::new(ModifyHandler {
                    plugin_id: pid,
                    key: "injected".into(),
                    value: serde_json::json!(true),
                }),
            )
            .await;

        let mut ctx = make_ctx(Hook::BeforeCreate);
        let result = registry.execute(&mut ctx).await;

        // The context data should be updated.
        assert_eq!(ctx.data["injected"], true);
        assert!(matches!(result, HookResult::Modify(_)));
    }

    #[tokio::test]
    async fn priority_ordering() {
        let registry = HookRegistry::new();
        let pid = Uuid::new_v4();

        // Register handler with priority 200 that adds "second": true.
        registry
            .register_with_priority(
                Hook::BeforeUpdate,
                Arc::new(ModifyHandler {
                    plugin_id: pid,
                    key: "second".into(),
                    value: serde_json::json!(true),
                }),
                200,
            )
            .await;

        // Register handler with priority 10 that adds "first": true.
        registry
            .register_with_priority(
                Hook::BeforeUpdate,
                Arc::new(ModifyHandler {
                    plugin_id: pid,
                    key: "first".into(),
                    value: serde_json::json!(true),
                }),
                10,
            )
            .await;

        let mut ctx = make_ctx(Hook::BeforeUpdate);
        registry.execute(&mut ctx).await;

        // Both should be present because modify doesn't stop the pipeline.
        assert_eq!(ctx.data["first"], true);
        assert_eq!(ctx.data["second"], true);
    }

    #[tokio::test]
    async fn unregister_plugin_removes_handlers() {
        let registry = HookRegistry::new();
        let pid = Uuid::new_v4();
        let other_pid = Uuid::new_v4();

        registry
            .register(
                Hook::AfterCreate,
                Arc::new(PassHandler {
                    name: "a".into(),
                    plugin_id: pid,
                }),
            )
            .await;
        registry
            .register(
                Hook::AfterCreate,
                Arc::new(PassHandler {
                    name: "b".into(),
                    plugin_id: other_pid,
                }),
            )
            .await;

        assert_eq!(registry.handler_count(Hook::AfterCreate).await, 2);

        registry.unregister_plugin(pid).await;
        assert_eq!(registry.handler_count(Hook::AfterCreate).await, 1);
    }

    #[tokio::test]
    async fn total_handler_count() {
        let registry = HookRegistry::new();
        let pid = Uuid::new_v4();

        registry
            .register(
                Hook::BeforeCreate,
                Arc::new(PassHandler {
                    name: "a".into(),
                    plugin_id: pid,
                }),
            )
            .await;
        registry
            .register(
                Hook::AfterDelete,
                Arc::new(PassHandler {
                    name: "b".into(),
                    plugin_id: pid,
                }),
            )
            .await;

        assert_eq!(registry.total_handler_count().await, 2);
    }

    #[test]
    fn hook_display() {
        assert_eq!(Hook::BeforeCreate.to_string(), "before_create");
        assert_eq!(Hook::OnAuth.to_string(), "on_auth");
        assert_eq!(Hook::OnError.to_string(), "on_error");
    }

    #[test]
    fn hook_all_has_ten_variants() {
        assert_eq!(Hook::ALL.len(), 10);
    }

    #[test]
    fn hook_context_query_constructor() {
        let ctx = HookContext::query(
            Hook::BeforeQuery,
            "tasks",
            None,
            serde_json::json!({ "filter": "status = active" }),
        );
        assert!(ctx.entity_id.is_none());
        assert_eq!(ctx.entity_type, "tasks");
    }

    #[test]
    fn hook_result_serde_roundtrip() {
        let results = vec![
            HookResult::Continue,
            HookResult::Modify(serde_json::json!({ "x": 1 })),
            HookResult::Reject("nope".into()),
        ];

        for result in results {
            let json = serde_json::to_string(&result).unwrap();
            let back: HookResult = serde_json::from_str(&json).unwrap();
            // Verify the tag roundtrips.
            let orig_json = serde_json::to_value(&result).unwrap();
            let back_json = serde_json::to_value(&back).unwrap();
            assert_eq!(orig_json, back_json);
        }
    }
}
