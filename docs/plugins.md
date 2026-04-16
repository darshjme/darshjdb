# Plugin System

DarshJDB's plugin system enables extending the database with custom field types, views, actions, API routes, webhooks, and middleware. Plugins follow a manifest-driven architecture with lifecycle management, a priority-based hook pipeline, and three built-in reference implementations.

## Plugin Manifest

Every plugin declares itself through a `PluginManifest`:

```json
{
  "id": "10000000-0000-0000-0000-000000000001",
  "name": "slack-notifications",
  "version": "1.0.0",
  "author": "DarshJDB",
  "description": "Send Slack notifications when records change",
  "homepage": "https://darshj.me/plugins/slack",
  "capabilities": [
    { "kind": "custom_action", "name": "slack_notify" },
    { "kind": "webhook", "name": "slack_incoming" }
  ],
  "config_schema": {
    "type": "object",
    "required": ["webhook_url"],
    "properties": {
      "webhook_url": { "type": "string" },
      "channel": { "type": "string" }
    }
  },
  "entry_point": "main.ts"
}
```

The `config_schema` is a JSON Schema object that clients use to render a settings form. The `entry_point` field indicates how the plugin is loaded: a Rust struct name (native), a `.ts`/`.js` file path (script), or a `.wasm` file path.

## Capabilities

Each capability declares a specific extension point the plugin provides:

| Capability | Description | Name Example |
|------------|-------------|--------------|
| `custom_field` | Registers a new field type | `"color_picker"`, `"rating"` |
| `custom_view` | Registers a custom view | `"kanban"`, `"timeline"` |
| `custom_action` | Registers an automation action | `"slack_notify"`, `"validate"` |
| `api_extension` | Mounts routes under `/api/ext/{name}` | `"analytics"` |
| `webhook` | Registers an incoming webhook endpoint | `"stripe"`, `"slack_incoming"` |
| `middleware` | Injects middleware into the request pipeline | `"audit_middleware"` |

Capabilities are queryable: you can list all plugins providing a given capability kind via the registry.

## Hook System

Plugins intercept the request lifecycle at well-defined points through **hooks**. Each hook has an ordered list of handlers executed in priority order (lower values execute first, default priority: 100).

### Hook Points

| Hook | When It Fires |
|------|--------------|
| `before_create` | Before a new entity is created |
| `after_create` | After a new entity has been created |
| `before_update` | Before an entity is updated |
| `after_update` | After an entity has been updated |
| `before_delete` | Before an entity is deleted |
| `after_delete` | After an entity has been deleted |
| `before_query` | Before a query is executed |
| `after_query` | After a query has returned results |
| `on_auth` | On authentication events (login, token refresh) |
| `on_error` | When an unhandled error occurs |

### Hook Context

Every handler receives a `HookContext` containing:

```json
{
  "hook": "before_create",
  "entity_type": "users",
  "entity_id": "uuid-or-null",
  "user_id": "uuid-or-null",
  "data": { "name": "Alice", "email": "alice@example.com" },
  "metadata": {}
}
```

The `metadata` field enables inter-plugin communication: one handler can write data that a later handler reads.

### Hook Results

Each handler returns one of three results:

| Result | Effect |
|--------|--------|
| `Continue` | Allow the operation to proceed unchanged |
| `Modify(data)` | Allow the operation but replace the data payload. The modified data is merged into the context for subsequent handlers. |
| `Reject(reason)` | Immediately halt the hook pipeline and reject the operation with the given reason string. |

If any handler returns `Reject`, no further handlers execute and the operation is aborted. `Modify` results propagate through the pipeline -- each subsequent handler sees the modified data.

## Built-in Plugins

### SlackNotification

Sends Slack messages via incoming webhook when records are created or updated.

**Plugin ID**: `10000000-0000-0000-0000-000000000001`

**Configuration**:

```json
{
  "webhook_url": "https://hooks.slack.com/services/T.../B.../...",
  "channel": "#notifications",
  "entity_types": ["tasks", "users"]
}
```

Hooks into `after_create` and `after_update`. If `entity_types` is empty, notifications fire for all entity types.

### DataValidation

Custom validation rules that go beyond field-level constraints. Rejects mutations that fail validation.

**Plugin ID**: `10000000-0000-0000-0000-000000000002`

**Configuration**:

```json
{
  "rules": [
    {
      "entity_type": "users",
      "field": "email",
      "rule": "regex",
      "pattern": "^[^@]+@[^@]+\\.[^@]+$",
      "message": "Invalid email format"
    },
    {
      "entity_type": "products",
      "field": "price",
      "rule": "range",
      "min": 0,
      "max": 999999,
      "message": "Price must be between 0 and 999999"
    },
    {
      "entity_type": "users",
      "field": "name",
      "rule": "required",
      "message": "Name is required"
    }
  ]
}
```

Supported rule types: `regex`, `range` (min/max), `required`. Hooks into `before_create` and `before_update`. Rules are scoped by `entity_type` -- rules for a different entity type are ignored.

### AuditLog

Enhanced audit logging with user action tracking and before/after diffs.

**Plugin ID**: `10000000-0000-0000-0000-000000000003`

**Configuration**:

```json
{
  "log_reads": false,
  "include_diff": true,
  "entity_types": []
}
```

When `log_reads` is `true`, query operations (`before_query`/`after_query`) are also logged. When `entity_types` is empty, all entity types are audited.

## Plugin Lifecycle

```
install --> Installed --> activate --> Active --> deactivate --> Disabled
                                        |                         |
                                        +--- error --> Error      |
                                                                  |
                                    configure (re-init) <---------+
                                        |
                            uninstall (shutdown + remove)
```

1. **Install**: Register a plugin by its manifest. State: `Installed`.
2. **Activate**: Call the plugin's `initialize(config)` method. State: `Active` on success, `Error(msg)` on failure.
3. **Configure**: Update the stored config. If the plugin is active, it is deactivated then re-activated with the new config.
4. **Deactivate**: Call the plugin's `shutdown()` method and unregister all its hooks. State: `Disabled`.
5. **Uninstall**: Shut down if active, remove all hooks, and delete the plugin entry.

Manifest-only plugins (script/WASM without a Rust trait object) transition to `Active` without calling `initialize`.

## Plugin Registry

The `PluginRegistry` is a thread-safe `DashMap` providing lock-free concurrent reads. It supports:

- Register by manifest or by trait object instance
- Lookup by ID, by capability kind, or list all
- Activation/deactivation with automatic hook cleanup
- Configuration updates with live re-initialization

## API Endpoints

### List all plugins

```bash
curl http://localhost:3000/api/plugins \
  -H "Authorization: Bearer <token>"
```

### Get plugin details

```bash
curl http://localhost:3000/api/plugins/{plugin_id} \
  -H "Authorization: Bearer <token>"
```

### Install a plugin

```bash
curl -X POST http://localhost:3000/api/plugins \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{ "manifest": { ... } }'
```

### Activate a plugin

```bash
curl -X POST http://localhost:3000/api/plugins/{plugin_id}/activate \
  -H "Authorization: Bearer <token>"
```

### Deactivate a plugin

```bash
curl -X POST http://localhost:3000/api/plugins/{plugin_id}/deactivate \
  -H "Authorization: Bearer <token>"
```

### Configure a plugin

```bash
curl -X PUT http://localhost:3000/api/plugins/{plugin_id}/configure \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{ "webhook_url": "https://hooks.slack.com/new-url" }'
```

If the plugin is active, it is re-initialized with the new configuration.

### Uninstall a plugin

```bash
curl -X DELETE http://localhost:3000/api/plugins/{plugin_id} \
  -H "Authorization: Bearer <token>"
```

### List plugins by capability

```bash
curl "http://localhost:3000/api/plugins?capability=custom_field" \
  -H "Authorization: Bearer <token>"
```
