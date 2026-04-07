//! OpenAPI 3.1 specification auto-generation for DarshJDB.
//!
//! Builds the complete API spec at startup and serves it as JSON at
//! `GET /api/openapi.json`. The companion docs endpoint (`GET /api/docs`)
//! renders an interactive Scalar viewer.
//!
//! Covers all endpoint groups: Auth, Data, Query, Views, Fields, Tables,
//! Relations, Aggregation, Import/Export, Collaboration, Plugins,
//! Activity, History, Webhooks, Admin, and API Keys.

use serde_json::{Value, json};

/// Generate the full OpenAPI 3.1 specification for DarshJDB.
///
/// This is computed once at server startup and cached in application state.
pub fn generate_openapi_spec() -> Value {
    let mut paths = serde_json::Map::new();

    // Auth
    paths.insert("/auth/signup".into(), auth_signup_path());
    paths.insert("/auth/signin".into(), auth_signin_path());
    paths.insert("/auth/magic-link".into(), auth_magic_link_path());
    paths.insert("/auth/verify".into(), auth_verify_path());
    paths.insert("/auth/oauth/{provider}".into(), auth_oauth_path());
    paths.insert("/auth/refresh".into(), auth_refresh_path());
    paths.insert("/auth/signout".into(), auth_signout_path());
    paths.insert("/auth/me".into(), auth_me_path());

    // Data / Query
    paths.insert("/query".into(), query_path());
    paths.insert("/mutate".into(), mutate_path());
    paths.insert("/data/{entity}".into(), data_collection_path());
    paths.insert("/data/{entity}/{id}".into(), data_item_path());

    // Functions
    paths.insert("/fn/{name}".into(), function_path());

    // Storage
    paths.insert("/storage/upload".into(), storage_upload_path());
    paths.insert("/storage/{path}".into(), storage_item_path());

    // Realtime
    paths.insert("/subscribe".into(), subscribe_path());

    // Pub/Sub
    paths.insert("/events".into(), events_path());
    paths.insert("/events/publish".into(), events_publish_path());

    // Views
    paths.insert("/views".into(), views_collection_path());
    paths.insert("/views/{id}".into(), views_item_path());
    paths.insert("/views/{id}/query".into(), views_query_path());

    // Fields
    paths.insert("/tables/{table}/fields".into(), fields_collection_path());
    paths.insert("/tables/{table}/fields/{field_id}".into(), fields_item_path());
    paths.insert("/tables/{table}/fields/{field_id}/convert".into(), fields_convert_path());

    // Tables
    paths.insert("/tables".into(), tables_collection_path());
    paths.insert("/tables/{table}".into(), tables_item_path());
    paths.insert("/tables/templates".into(), tables_templates_path());
    paths.insert("/tables/{table}/stats".into(), tables_stats_path());

    // Automations
    paths.insert("/automations".into(), automations_collection_path());
    paths.insert("/automations/{id}".into(), automations_item_path());
    paths.insert("/automations/{id}/trigger".into(), automations_trigger_path());
    paths.insert("/automations/{id}/runs".into(), automations_runs_path());

    // Relations
    paths.insert("/relations/link".into(), relations_link_path());
    paths.insert("/relations/unlink".into(), relations_unlink_path());
    paths.insert("/relations/lookup".into(), relations_lookup_path());
    paths.insert("/relations/rollup".into(), relations_rollup_path());

    // Aggregation
    paths.insert("/aggregate".into(), aggregation_path());
    paths.insert("/aggregate/summary".into(), aggregation_summary_path());
    paths.insert("/aggregate/chart".into(), aggregation_chart_path());

    // Import / Export
    paths.insert("/import".into(), import_path());
    paths.insert("/export".into(), export_path());

    // Collaboration
    paths.insert("/collaboration/share".into(), collab_share_path());
    paths.insert("/collaboration/share/{id}".into(), collab_share_item_path());
    paths.insert("/collaboration/collaborators".into(), collab_collaborators_path());
    paths.insert("/collaboration/collaborators/{id}".into(), collab_collaborator_item_path());
    paths.insert("/collaboration/workspaces".into(), collab_workspaces_path());
    paths.insert("/collaboration/workspaces/{id}".into(), collab_workspace_item_path());

    // Plugins
    paths.insert("/plugins".into(), plugins_collection_path());
    paths.insert("/plugins/{id}".into(), plugins_item_path());
    paths.insert("/plugins/{id}/configure".into(), plugins_configure_path());

    // Comments
    paths.insert("/comments".into(), comments_collection_path());
    paths.insert("/comments/{id}".into(), comments_item_path());

    // Activity
    paths.insert("/activity".into(), activity_log_path());
    paths.insert("/activity/notifications".into(), activity_notifications_path());

    // History
    paths.insert("/history/{entity}/{id}".into(), history_versions_path());
    paths.insert("/history/{entity}/{id}/restore".into(), history_restore_path());
    paths.insert("/history/snapshots".into(), history_snapshots_path());

    // Webhooks
    paths.insert("/webhooks".into(), webhooks_collection_path());
    paths.insert("/webhooks/{id}".into(), webhooks_item_path());
    paths.insert("/webhooks/{id}/deliveries".into(), webhooks_deliveries_path());
    paths.insert("/webhooks/{id}/test".into(), webhooks_test_path());

    // API Keys
    paths.insert("/api-keys".into(), api_keys_collection_path());
    paths.insert("/api-keys/{id}".into(), api_keys_item_path());
    paths.insert("/api-keys/{id}/rotate".into(), api_keys_rotate_path());

    // Admin
    paths.insert("/admin/schema".into(), admin_schema_path());
    paths.insert("/admin/functions".into(), admin_functions_path());
    paths.insert("/admin/sessions".into(), admin_sessions_path());
    paths.insert("/admin/bulk-load".into(), admin_bulk_load_path());
    paths.insert("/admin/cache".into(), admin_cache_path());

    // Batch
    paths.insert("/batch".into(), batch_path());
    paths.insert("/batch/parallel".into(), batch_parallel_path());
    paths.insert("/batch/metrics".into(), batch_metrics_path());

    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "DarshJDB API",
            "description": "Triple-store Backend-as-a-Service with EAV over PostgreSQL. Real-time subscriptions, row-level security, server-side functions, views, automations, and more.",
            "version": "0.2.0",
            "license": {
                "name": "MIT",
                "url": "https://opensource.org/licenses/MIT"
            },
            "contact": {
                "name": "DarshJDB",
                "url": "https://db.darshj.me"
            }
        },
        "servers": [
            {
                "url": "/api",
                "description": "Current server"
            }
        ],
        "tags": [
            { "name": "Auth", "description": "Authentication, sessions, OAuth2, and magic links" },
            { "name": "Data", "description": "CRUD operations on entities via the triple store" },
            { "name": "Query", "description": "DarshJQL query execution and subscriptions" },
            { "name": "Views", "description": "Named reusable query lenses (Grid, Kanban, Form, Gallery, Calendar)" },
            { "name": "Fields", "description": "Field schema management and type conversion" },
            { "name": "Tables", "description": "Table-level operations, templates, and statistics" },
            { "name": "Relations", "description": "Entity linking, lookups, and rollup computations" },
            { "name": "Aggregation", "description": "Aggregate queries, summaries, and chart data" },
            { "name": "Import/Export", "description": "CSV and JSON import/export for bulk data operations" },
            { "name": "Collaboration", "description": "Share links, collaborators, and workspace management" },
            { "name": "Plugins", "description": "Plugin lifecycle and configuration" },
            { "name": "Activity", "description": "Activity logs and notification management" },
            { "name": "History", "description": "Version history, point-in-time restore, and snapshots" },
            { "name": "Webhooks", "description": "Webhook registration, delivery logs, and testing" },
            { "name": "Admin", "description": "Schema introspection, function registry, sessions, cache, and audit" },
            { "name": "Functions", "description": "Server-side function invocation" },
            { "name": "Storage", "description": "File upload, download, and management" },
            { "name": "Realtime", "description": "Server-Sent Events for live query subscriptions" },
            { "name": "Pub/Sub", "description": "Publish/subscribe event channels" },
            { "name": "Batch", "description": "Batch and parallel operation execution" },
            { "name": "API Keys", "description": "API key management and rotation" }
        ],
        "components": {
            "securitySchemes": {
                "bearerAuth": {
                    "type": "http",
                    "scheme": "bearer",
                    "bearerFormat": "JWT",
                    "description": "JWT access token obtained from /auth/signin, /auth/signup, or /auth/oauth/{provider}. In dev mode, use `Bearer dev` to bypass authentication."
                },
                "apiKeyAuth": {
                    "type": "apiKey",
                    "in": "header",
                    "name": "X-API-Key",
                    "description": "API key for server-to-server access"
                }
            },
            "schemas": component_schemas()
        },
        "paths": Value::Object(paths)
    })
}

/// HTML page that loads the Scalar API reference viewer.
pub fn docs_html(spec_url: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html>
<head>
  <title>DarshJDB API Docs</title>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
</head>
<body>
  <script id="api-reference" data-url="{spec_url}"></script>
  <script src="https://cdn.jsdelivr.net/npm/@scalar/api-reference"></script>
</body>
</html>"#
    )
}

// ===========================================================================
// Component schemas
// ===========================================================================

fn component_schemas() -> Value {
    json!({
        "ErrorResponse": error_schema(),
        "TokenPair": token_pair_schema(),
        "QueryRequest": query_request_schema(),
        "MutateRequest": mutate_request_schema(),
        "UserProfile": user_profile_schema(),
        "UploadResponse": upload_response_schema(),
        "ViewConfig": view_config_schema(),
        "CreateViewRequest": create_view_request_schema(),
        "ViewUpdate": view_update_schema(),
        "ViewQueryRequest": view_query_request_schema(),
        "FilterClause": filter_clause_schema(),
        "SortClause": sort_clause_schema(),
        "FieldDefinition": field_definition_schema(),
        "CreateFieldRequest": create_field_request_schema(),
        "UpdateFieldRequest": update_field_request_schema(),
        "ConvertFieldRequest": convert_field_request_schema(),
        "TableConfig": table_config_schema(),
        "CreateTableRequest": create_table_request_schema(),
        "UpdateTableRequest": update_table_request_schema(),
        "TableStats": table_stats_schema(),
        "TableTemplate": table_template_schema(),
        "AutomationConfig": automation_config_schema(),
        "CreateAutomationRequest": create_automation_request_schema(),
        "AutomationRun": automation_run_schema(),
        "LinkRequest": link_request_schema(),
        "UnlinkRequest": unlink_request_schema(),
        "LookupRequest": lookup_request_schema(),
        "RollupRequest": rollup_request_schema(),
        "AggregateRequest": aggregate_request_schema(),
        "AggregateResponse": aggregate_response_schema(),
        "SummaryRequest": summary_request_schema(),
        "ChartRequest": chart_request_schema(),
        "ImportRequest": import_request_schema(),
        "ImportResult": import_result_schema(),
        "ExportRequest": export_request_schema(),
        "ShareLink": share_link_schema(),
        "CreateShareRequest": create_share_request_schema(),
        "Collaborator": collaborator_schema(),
        "AddCollaboratorRequest": add_collaborator_request_schema(),
        "Workspace": workspace_schema(),
        "CreateWorkspaceRequest": create_workspace_request_schema(),
        "PluginConfig": plugin_config_schema(),
        "PluginConfigureRequest": plugin_configure_request_schema(),
        "Comment": comment_schema(),
        "CreateCommentRequest": create_comment_request_schema(),
        "ActivityEntry": activity_entry_schema(),
        "Notification": notification_schema(),
        "VersionEntry": version_entry_schema(),
        "Snapshot": snapshot_schema(),
        "WebhookConfig": webhook_config_schema(),
        "CreateWebhookRequest": create_webhook_request_schema(),
        "WebhookDelivery": webhook_delivery_schema(),
        "ApiKeyConfig": api_key_config_schema(),
        "CreateApiKeyRequest": create_api_key_request_schema(),
        "ApiKeyCreated": api_key_created_schema(),
        "PaginationMeta": pagination_meta_schema()
    })
}

// ===========================================================================
// Schema definitions
// ===========================================================================

fn error_schema() -> Value {
    json!({
        "type": "object",
        "required": ["error"],
        "properties": {
            "error": {
                "type": "object",
                "required": ["code", "message", "status"],
                "properties": {
                    "code": { "type": "string", "example": "PERMISSION_DENIED" },
                    "message": { "type": "string", "example": "You do not have access to this resource." },
                    "status": { "type": "integer", "example": 403 },
                    "retry_after_secs": { "type": "integer", "nullable": true }
                }
            }
        }
    })
}

fn token_pair_schema() -> Value {
    json!({
        "type": "object",
        "required": ["access_token", "refresh_token", "expires_in"],
        "properties": {
            "access_token": { "type": "string" },
            "refresh_token": { "type": "string" },
            "expires_in": { "type": "integer", "description": "Seconds until access token expires" }
        }
    })
}

fn query_request_schema() -> Value {
    json!({
        "type": "object",
        "required": ["query"],
        "properties": {
            "query": { "type": "string", "description": "DarshJQL query expression" },
            "args": { "type": "object", "additionalProperties": true, "description": "Named query arguments" }
        }
    })
}

fn mutate_request_schema() -> Value {
    json!({
        "type": "object",
        "required": ["mutations"],
        "properties": {
            "mutations": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "op": { "type": "string", "enum": ["insert", "update", "delete", "upsert"] },
                        "entity": { "type": "string" },
                        "id": { "type": "string", "format": "uuid", "nullable": true },
                        "data": { "type": "object", "additionalProperties": true }
                    }
                }
            }
        }
    })
}

fn user_profile_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "user_id": { "type": "string", "format": "uuid" },
            "email": { "type": "string", "format": "email" },
            "roles": { "type": "array", "items": { "type": "string" } },
            "created_at": { "type": "string", "format": "date-time" }
        }
    })
}

fn upload_response_schema() -> Value {
    json!({
        "type": "object",
        "required": ["path", "size", "content_type"],
        "properties": {
            "path": { "type": "string" },
            "size": { "type": "integer" },
            "content_type": { "type": "string" },
            "signed_url": { "type": "string", "nullable": true }
        }
    })
}

fn view_config_schema() -> Value {
    json!({
        "type": "object",
        "required": ["id", "name", "kind", "table_entity_type", "created_by", "created_at", "updated_at"],
        "properties": {
            "id": { "type": "string", "format": "uuid" },
            "name": { "type": "string", "example": "Active Tasks" },
            "kind": { "type": "string", "enum": ["grid", "form", "kanban", "gallery", "calendar"] },
            "table_entity_type": { "type": "string", "example": "Task" },
            "filters": { "type": "array", "items": { "$ref": "#/components/schemas/FilterClause" } },
            "sorts": { "type": "array", "items": { "$ref": "#/components/schemas/SortClause" } },
            "field_order": { "type": "array", "items": { "type": "string" } },
            "hidden_fields": { "type": "array", "items": { "type": "string" } },
            "group_by": { "type": "string", "nullable": true },
            "kanban_field": { "type": "string", "nullable": true },
            "calendar_field": { "type": "string", "nullable": true },
            "color_field": { "type": "string", "nullable": true },
            "row_height": { "type": "integer", "nullable": true },
            "created_by": { "type": "string", "format": "uuid" },
            "created_at": { "type": "string", "format": "date-time" },
            "updated_at": { "type": "string", "format": "date-time" }
        }
    })
}

fn create_view_request_schema() -> Value {
    json!({
        "type": "object",
        "required": ["name", "kind", "table_entity_type"],
        "properties": {
            "name": { "type": "string", "example": "Active Tasks" },
            "kind": { "type": "string", "enum": ["grid", "form", "kanban", "gallery", "calendar"] },
            "table_entity_type": { "type": "string", "example": "Task" },
            "filters": { "type": "array", "items": { "$ref": "#/components/schemas/FilterClause" }, "default": [] },
            "sorts": { "type": "array", "items": { "$ref": "#/components/schemas/SortClause" }, "default": [] },
            "field_order": { "type": "array", "items": { "type": "string" }, "default": [] },
            "hidden_fields": { "type": "array", "items": { "type": "string" }, "default": [] },
            "group_by": { "type": "string", "nullable": true },
            "kanban_field": { "type": "string", "nullable": true },
            "calendar_field": { "type": "string", "nullable": true },
            "color_field": { "type": "string", "nullable": true },
            "row_height": { "type": "integer", "nullable": true }
        }
    })
}

fn view_update_schema() -> Value {
    json!({
        "type": "object",
        "description": "Partial update payload for a view. Only provided fields are updated.",
        "properties": {
            "name": { "type": "string" },
            "kind": { "type": "string", "enum": ["grid", "form", "kanban", "gallery", "calendar"] },
            "filters": { "type": "array", "items": { "$ref": "#/components/schemas/FilterClause" } },
            "sorts": { "type": "array", "items": { "$ref": "#/components/schemas/SortClause" } },
            "field_order": { "type": "array", "items": { "type": "string" } },
            "hidden_fields": { "type": "array", "items": { "type": "string" } },
            "group_by": { "type": "string", "nullable": true },
            "kanban_field": { "type": "string", "nullable": true },
            "calendar_field": { "type": "string", "nullable": true },
            "color_field": { "type": "string", "nullable": true },
            "row_height": { "type": "integer", "nullable": true }
        }
    })
}

fn view_query_request_schema() -> Value {
    json!({
        "type": "object",
        "description": "Optional additional query layered on top of the view's built-in filters and sorts.",
        "properties": {
            "query": {
                "description": "DarshJQL query object to merge with the view's configuration",
                "nullable": true
            }
        }
    })
}

fn filter_clause_schema() -> Value {
    json!({
        "type": "object",
        "required": ["field", "op"],
        "properties": {
            "field": { "type": "string", "description": "Attribute name to filter on" },
            "op": { "type": "string", "enum": ["eq", "neq", "gt", "gte", "lt", "lte", "contains", "is_empty", "is_not_empty"] },
            "value": { "description": "Value to compare against. Null is valid for is_empty/is_not_empty." }
        }
    })
}

fn sort_clause_schema() -> Value {
    json!({
        "type": "object",
        "required": ["field", "direction"],
        "properties": {
            "field": { "type": "string" },
            "direction": { "type": "string", "enum": ["asc", "desc"] }
        }
    })
}

fn field_definition_schema() -> Value {
    json!({
        "type": "object",
        "required": ["id", "name", "field_type", "table"],
        "properties": {
            "id": { "type": "string", "format": "uuid" },
            "name": { "type": "string", "example": "email" },
            "field_type": {
                "type": "string",
                "enum": ["text", "number", "boolean", "date", "datetime", "email", "url", "select", "multi_select", "attachment", "relation", "formula", "rollup", "lookup", "json", "rich_text", "phone", "rating", "checkbox", "currency", "percent", "duration", "auto_number", "created_at", "updated_at", "created_by", "updated_by"]
            },
            "table": { "type": "string" },
            "required": { "type": "boolean", "default": false },
            "unique": { "type": "boolean", "default": false },
            "default_value": { "nullable": true },
            "options": {
                "type": "object",
                "description": "Type-specific configuration (e.g. select choices, formula expression, relation config)",
                "additionalProperties": true,
                "nullable": true
            },
            "description": { "type": "string", "nullable": true },
            "order": { "type": "integer", "description": "Display order within the table" },
            "created_at": { "type": "string", "format": "date-time" },
            "updated_at": { "type": "string", "format": "date-time" }
        }
    })
}

fn create_field_request_schema() -> Value {
    json!({
        "type": "object",
        "required": ["name", "field_type"],
        "properties": {
            "name": { "type": "string" },
            "field_type": { "type": "string", "enum": ["text", "number", "boolean", "date", "datetime", "email", "url", "select", "multi_select", "attachment", "relation", "formula", "rollup", "lookup", "json", "rich_text", "phone", "rating", "checkbox", "currency", "percent", "duration"] },
            "required": { "type": "boolean", "default": false },
            "unique": { "type": "boolean", "default": false },
            "default_value": { "nullable": true },
            "options": { "type": "object", "additionalProperties": true, "nullable": true },
            "description": { "type": "string", "nullable": true }
        }
    })
}

fn update_field_request_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "required": { "type": "boolean" },
            "unique": { "type": "boolean" },
            "default_value": { "nullable": true },
            "options": { "type": "object", "additionalProperties": true },
            "description": { "type": "string" }
        }
    })
}

fn convert_field_request_schema() -> Value {
    json!({
        "type": "object",
        "required": ["target_type"],
        "properties": {
            "target_type": {
                "type": "string",
                "description": "The field type to convert to",
                "enum": ["text", "number", "boolean", "date", "datetime", "select", "multi_select", "json"]
            },
            "options": {
                "type": "object",
                "description": "Type-specific options for the conversion",
                "additionalProperties": true,
                "nullable": true
            }
        }
    })
}

fn table_config_schema() -> Value {
    json!({
        "type": "object",
        "required": ["name", "entity_type"],
        "properties": {
            "name": { "type": "string", "example": "Tasks" },
            "entity_type": { "type": "string", "example": "Task" },
            "description": { "type": "string", "nullable": true },
            "icon": { "type": "string", "nullable": true },
            "color": { "type": "string", "nullable": true },
            "fields": { "type": "array", "items": { "$ref": "#/components/schemas/FieldDefinition" } },
            "primary_field": { "type": "string", "description": "Name of the primary display field" },
            "created_at": { "type": "string", "format": "date-time" },
            "updated_at": { "type": "string", "format": "date-time" }
        }
    })
}

fn create_table_request_schema() -> Value {
    json!({
        "type": "object",
        "required": ["name"],
        "properties": {
            "name": { "type": "string" },
            "description": { "type": "string", "nullable": true },
            "icon": { "type": "string", "nullable": true },
            "color": { "type": "string", "nullable": true },
            "fields": { "type": "array", "items": { "$ref": "#/components/schemas/CreateFieldRequest" }, "description": "Initial fields to create with the table" },
            "template": { "type": "string", "description": "Template name to use (e.g. 'project_tracker', 'crm')", "nullable": true }
        }
    })
}

fn update_table_request_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "description": { "type": "string" },
            "icon": { "type": "string" },
            "color": { "type": "string" }
        }
    })
}

fn table_stats_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "entity_type": { "type": "string" },
            "row_count": { "type": "integer" },
            "field_count": { "type": "integer" },
            "view_count": { "type": "integer" },
            "triple_count": { "type": "integer" },
            "storage_bytes": { "type": "integer" },
            "last_modified": { "type": "string", "format": "date-time" }
        }
    })
}

fn table_template_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "name": { "type": "string", "example": "project_tracker" },
            "display_name": { "type": "string", "example": "Project Tracker" },
            "description": { "type": "string" },
            "category": { "type": "string" },
            "fields": { "type": "array", "items": { "$ref": "#/components/schemas/CreateFieldRequest" } },
            "sample_views": { "type": "array", "items": { "$ref": "#/components/schemas/CreateViewRequest" } }
        }
    })
}

fn automation_config_schema() -> Value {
    json!({
        "type": "object",
        "required": ["id", "name", "trigger", "actions", "enabled"],
        "properties": {
            "id": { "type": "string", "format": "uuid" },
            "name": { "type": "string" },
            "description": { "type": "string", "nullable": true },
            "trigger": {
                "type": "object",
                "required": ["type"],
                "properties": {
                    "type": { "type": "string", "enum": ["on_create", "on_update", "on_delete", "schedule", "webhook", "manual"] },
                    "entity_type": { "type": "string" },
                    "filter": { "$ref": "#/components/schemas/FilterClause" },
                    "cron": { "type": "string", "description": "Cron expression for schedule triggers" }
                }
            },
            "actions": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["type"],
                    "properties": {
                        "type": { "type": "string", "enum": ["set_field", "create_entity", "delete_entity", "send_webhook", "send_email", "invoke_function", "notify"] },
                        "config": { "type": "object", "additionalProperties": true }
                    }
                }
            },
            "enabled": { "type": "boolean" },
            "last_run_at": { "type": "string", "format": "date-time", "nullable": true },
            "run_count": { "type": "integer" },
            "created_at": { "type": "string", "format": "date-time" },
            "updated_at": { "type": "string", "format": "date-time" }
        }
    })
}

fn create_automation_request_schema() -> Value {
    json!({
        "type": "object",
        "required": ["name", "trigger", "actions"],
        "properties": {
            "name": { "type": "string" },
            "description": { "type": "string", "nullable": true },
            "trigger": {
                "type": "object",
                "required": ["type"],
                "properties": {
                    "type": { "type": "string", "enum": ["on_create", "on_update", "on_delete", "schedule", "webhook", "manual"] },
                    "entity_type": { "type": "string" },
                    "filter": { "$ref": "#/components/schemas/FilterClause" },
                    "cron": { "type": "string" }
                }
            },
            "actions": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["type"],
                    "properties": {
                        "type": { "type": "string" },
                        "config": { "type": "object", "additionalProperties": true }
                    }
                }
            },
            "enabled": { "type": "boolean", "default": true }
        }
    })
}

fn automation_run_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": { "type": "string", "format": "uuid" },
            "automation_id": { "type": "string", "format": "uuid" },
            "status": { "type": "string", "enum": ["pending", "running", "completed", "failed"] },
            "trigger_data": { "type": "object", "additionalProperties": true },
            "result": { "nullable": true },
            "error": { "type": "string", "nullable": true },
            "duration_ms": { "type": "number" },
            "started_at": { "type": "string", "format": "date-time" },
            "completed_at": { "type": "string", "format": "date-time", "nullable": true }
        }
    })
}

fn link_request_schema() -> Value {
    json!({
        "type": "object",
        "required": ["source_entity", "source_id", "target_entity", "target_id", "relation"],
        "properties": {
            "source_entity": { "type": "string", "example": "Task" },
            "source_id": { "type": "string", "format": "uuid" },
            "target_entity": { "type": "string", "example": "Project" },
            "target_id": { "type": "string", "format": "uuid" },
            "relation": { "type": "string", "example": "belongs_to", "description": "Named relation type" }
        }
    })
}

fn unlink_request_schema() -> Value {
    json!({
        "type": "object",
        "required": ["source_entity", "source_id", "target_entity", "target_id", "relation"],
        "properties": {
            "source_entity": { "type": "string" },
            "source_id": { "type": "string", "format": "uuid" },
            "target_entity": { "type": "string" },
            "target_id": { "type": "string", "format": "uuid" },
            "relation": { "type": "string" }
        }
    })
}

fn lookup_request_schema() -> Value {
    json!({
        "type": "object",
        "required": ["source_entity", "source_id", "relation", "target_field"],
        "properties": {
            "source_entity": { "type": "string" },
            "source_id": { "type": "string", "format": "uuid" },
            "relation": { "type": "string" },
            "target_field": { "type": "string", "description": "Field to read from the related entity" }
        }
    })
}

fn rollup_request_schema() -> Value {
    json!({
        "type": "object",
        "required": ["source_entity", "source_id", "relation", "target_field", "function"],
        "properties": {
            "source_entity": { "type": "string" },
            "source_id": { "type": "string", "format": "uuid" },
            "relation": { "type": "string" },
            "target_field": { "type": "string" },
            "function": { "type": "string", "enum": ["count", "sum", "avg", "min", "max", "concat"], "description": "Aggregation function to apply across related entities" }
        }
    })
}

fn aggregate_request_schema() -> Value {
    json!({
        "type": "object",
        "required": ["entity_type", "measures"],
        "properties": {
            "entity_type": { "type": "string" },
            "measures": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["field", "function"],
                    "properties": {
                        "field": { "type": "string" },
                        "function": { "type": "string", "enum": ["count", "sum", "avg", "min", "max", "count_distinct"] }
                    }
                }
            },
            "group_by": { "type": "array", "items": { "type": "string" } },
            "filters": { "type": "array", "items": { "$ref": "#/components/schemas/FilterClause" } },
            "having": { "type": "array", "items": { "$ref": "#/components/schemas/FilterClause" } },
            "order_by": { "type": "array", "items": { "$ref": "#/components/schemas/SortClause" } },
            "limit": { "type": "integer" }
        }
    })
}

fn aggregate_response_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "data": { "type": "array", "items": { "type": "object", "additionalProperties": true } },
            "meta": {
                "type": "object",
                "properties": {
                    "group_count": { "type": "integer" },
                    "total_rows": { "type": "integer" },
                    "duration_ms": { "type": "number" }
                }
            }
        }
    })
}

fn summary_request_schema() -> Value {
    json!({
        "type": "object",
        "required": ["entity_type"],
        "properties": {
            "entity_type": { "type": "string" },
            "fields": { "type": "array", "items": { "type": "string" }, "description": "Fields to include in the summary. If omitted, all numeric fields are summarized." },
            "filters": { "type": "array", "items": { "$ref": "#/components/schemas/FilterClause" } }
        }
    })
}

fn chart_request_schema() -> Value {
    json!({
        "type": "object",
        "required": ["entity_type", "chart_type"],
        "properties": {
            "entity_type": { "type": "string" },
            "chart_type": { "type": "string", "enum": ["bar", "line", "pie", "scatter", "area", "histogram"] },
            "x_field": { "type": "string" },
            "y_field": { "type": "string" },
            "measure": { "type": "string", "enum": ["count", "sum", "avg", "min", "max"] },
            "group_by": { "type": "string" },
            "filters": { "type": "array", "items": { "$ref": "#/components/schemas/FilterClause" } },
            "limit": { "type": "integer", "default": 100 }
        }
    })
}

fn import_request_schema() -> Value {
    json!({
        "type": "object",
        "required": ["entity_type", "format"],
        "properties": {
            "entity_type": { "type": "string" },
            "format": { "type": "string", "enum": ["csv", "json", "jsonl"] },
            "data": { "type": "string", "description": "Inline data content (for small imports). Use multipart/form-data for file uploads." },
            "on_conflict": { "type": "string", "enum": ["skip", "update", "error"], "default": "error" },
            "field_mapping": { "type": "object", "additionalProperties": { "type": "string" }, "description": "Map source column names to entity attributes" },
            "dry_run": { "type": "boolean", "default": false }
        }
    })
}

fn import_result_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "imported": { "type": "integer" },
            "skipped": { "type": "integer" },
            "errors": { "type": "integer" },
            "error_details": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "row": { "type": "integer" },
                        "field": { "type": "string" },
                        "message": { "type": "string" }
                    }
                }
            },
            "duration_ms": { "type": "number" },
            "dry_run": { "type": "boolean" }
        }
    })
}

fn export_request_schema() -> Value {
    json!({
        "type": "object",
        "required": ["entity_type", "format"],
        "properties": {
            "entity_type": { "type": "string" },
            "format": { "type": "string", "enum": ["csv", "json", "jsonl"] },
            "fields": { "type": "array", "items": { "type": "string" }, "description": "Fields to include. If omitted, all fields are exported." },
            "filters": { "type": "array", "items": { "$ref": "#/components/schemas/FilterClause" } },
            "view_id": { "type": "string", "format": "uuid", "description": "Export through a view lens", "nullable": true }
        }
    })
}

fn share_link_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": { "type": "string", "format": "uuid" },
            "token": { "type": "string" },
            "resource_type": { "type": "string", "enum": ["view", "table", "record"] },
            "resource_id": { "type": "string", "format": "uuid" },
            "permission": { "type": "string", "enum": ["read", "comment", "edit"] },
            "expires_at": { "type": "string", "format": "date-time", "nullable": true },
            "password_protected": { "type": "boolean" },
            "created_by": { "type": "string", "format": "uuid" },
            "created_at": { "type": "string", "format": "date-time" }
        }
    })
}

fn create_share_request_schema() -> Value {
    json!({
        "type": "object",
        "required": ["resource_type", "resource_id"],
        "properties": {
            "resource_type": { "type": "string", "enum": ["view", "table", "record"] },
            "resource_id": { "type": "string", "format": "uuid" },
            "permission": { "type": "string", "enum": ["read", "comment", "edit"], "default": "read" },
            "expires_in_hours": { "type": "integer", "nullable": true },
            "password": { "type": "string", "nullable": true }
        }
    })
}

fn collaborator_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": { "type": "string", "format": "uuid" },
            "user_id": { "type": "string", "format": "uuid" },
            "email": { "type": "string", "format": "email" },
            "role": { "type": "string", "enum": ["owner", "admin", "editor", "commenter", "viewer"] },
            "workspace_id": { "type": "string", "format": "uuid" },
            "added_at": { "type": "string", "format": "date-time" }
        }
    })
}

fn add_collaborator_request_schema() -> Value {
    json!({
        "type": "object",
        "required": ["email", "role"],
        "properties": {
            "email": { "type": "string", "format": "email" },
            "role": { "type": "string", "enum": ["admin", "editor", "commenter", "viewer"], "default": "viewer" },
            "workspace_id": { "type": "string", "format": "uuid", "nullable": true }
        }
    })
}

fn workspace_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": { "type": "string", "format": "uuid" },
            "name": { "type": "string" },
            "description": { "type": "string", "nullable": true },
            "owner_id": { "type": "string", "format": "uuid" },
            "member_count": { "type": "integer" },
            "table_count": { "type": "integer" },
            "created_at": { "type": "string", "format": "date-time" },
            "updated_at": { "type": "string", "format": "date-time" }
        }
    })
}

fn create_workspace_request_schema() -> Value {
    json!({
        "type": "object",
        "required": ["name"],
        "properties": {
            "name": { "type": "string" },
            "description": { "type": "string", "nullable": true }
        }
    })
}

fn plugin_config_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": { "type": "string", "format": "uuid" },
            "name": { "type": "string" },
            "version": { "type": "string" },
            "description": { "type": "string", "nullable": true },
            "enabled": { "type": "boolean" },
            "config": { "type": "object", "additionalProperties": true },
            "permissions": { "type": "array", "items": { "type": "string" } },
            "installed_at": { "type": "string", "format": "date-time" },
            "updated_at": { "type": "string", "format": "date-time" }
        }
    })
}

fn plugin_configure_request_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "enabled": { "type": "boolean" },
            "config": { "type": "object", "additionalProperties": true }
        }
    })
}

fn comment_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": { "type": "string", "format": "uuid" },
            "entity_type": { "type": "string" },
            "entity_id": { "type": "string", "format": "uuid" },
            "parent_id": { "type": "string", "format": "uuid", "nullable": true, "description": "Parent comment ID for threaded replies" },
            "body": { "type": "string" },
            "author_id": { "type": "string", "format": "uuid" },
            "author_email": { "type": "string" },
            "mentions": { "type": "array", "items": { "type": "string", "format": "uuid" } },
            "created_at": { "type": "string", "format": "date-time" },
            "updated_at": { "type": "string", "format": "date-time" }
        }
    })
}

fn create_comment_request_schema() -> Value {
    json!({
        "type": "object",
        "required": ["entity_type", "entity_id", "body"],
        "properties": {
            "entity_type": { "type": "string" },
            "entity_id": { "type": "string", "format": "uuid" },
            "parent_id": { "type": "string", "format": "uuid", "nullable": true },
            "body": { "type": "string" },
            "mentions": { "type": "array", "items": { "type": "string", "format": "uuid" } }
        }
    })
}

fn activity_entry_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": { "type": "string", "format": "uuid" },
            "action": { "type": "string", "enum": ["create", "update", "delete", "link", "unlink", "comment", "share", "import", "export", "automation_run", "login", "logout"] },
            "entity_type": { "type": "string" },
            "entity_id": { "type": "string", "format": "uuid" },
            "user_id": { "type": "string", "format": "uuid" },
            "changes": { "type": "object", "additionalProperties": true, "description": "Before/after values for update actions" },
            "metadata": { "type": "object", "additionalProperties": true },
            "timestamp": { "type": "string", "format": "date-time" }
        }
    })
}

fn notification_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": { "type": "string", "format": "uuid" },
            "type": { "type": "string", "enum": ["mention", "comment", "share", "automation", "system"] },
            "title": { "type": "string" },
            "body": { "type": "string" },
            "resource_type": { "type": "string", "nullable": true },
            "resource_id": { "type": "string", "format": "uuid", "nullable": true },
            "read": { "type": "boolean" },
            "created_at": { "type": "string", "format": "date-time" }
        }
    })
}

fn version_entry_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "tx_id": { "type": "integer" },
            "entity_id": { "type": "string", "format": "uuid" },
            "attribute": { "type": "string" },
            "old_value": { "nullable": true },
            "new_value": { "nullable": true },
            "operation": { "type": "string", "enum": ["insert", "update", "retract"] },
            "user_id": { "type": "string", "format": "uuid" },
            "timestamp": { "type": "string", "format": "date-time" }
        }
    })
}

fn snapshot_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": { "type": "string", "format": "uuid" },
            "name": { "type": "string" },
            "description": { "type": "string", "nullable": true },
            "tx_id": { "type": "integer", "description": "Transaction ID this snapshot captures" },
            "entity_count": { "type": "integer" },
            "created_by": { "type": "string", "format": "uuid" },
            "created_at": { "type": "string", "format": "date-time" }
        }
    })
}

fn webhook_config_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": { "type": "string", "format": "uuid" },
            "name": { "type": "string" },
            "url": { "type": "string", "format": "uri" },
            "events": { "type": "array", "items": { "type": "string" }, "description": "Event patterns to subscribe to (e.g. entity:Task:create)" },
            "secret": { "type": "string", "description": "HMAC secret for signature verification" },
            "headers": { "type": "object", "additionalProperties": { "type": "string" }, "description": "Custom headers sent with each delivery" },
            "enabled": { "type": "boolean" },
            "retry_count": { "type": "integer", "default": 3 },
            "created_at": { "type": "string", "format": "date-time" },
            "updated_at": { "type": "string", "format": "date-time" }
        }
    })
}

fn create_webhook_request_schema() -> Value {
    json!({
        "type": "object",
        "required": ["name", "url", "events"],
        "properties": {
            "name": { "type": "string" },
            "url": { "type": "string", "format": "uri" },
            "events": { "type": "array", "items": { "type": "string" } },
            "secret": { "type": "string", "nullable": true },
            "headers": { "type": "object", "additionalProperties": { "type": "string" } },
            "enabled": { "type": "boolean", "default": true },
            "retry_count": { "type": "integer", "default": 3 }
        }
    })
}

fn webhook_delivery_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": { "type": "string", "format": "uuid" },
            "webhook_id": { "type": "string", "format": "uuid" },
            "event": { "type": "string" },
            "request_body": { "type": "object" },
            "response_status": { "type": "integer", "nullable": true },
            "response_body": { "type": "string", "nullable": true },
            "status": { "type": "string", "enum": ["pending", "delivered", "failed"] },
            "attempt": { "type": "integer" },
            "duration_ms": { "type": "number", "nullable": true },
            "delivered_at": { "type": "string", "format": "date-time", "nullable": true },
            "next_retry_at": { "type": "string", "format": "date-time", "nullable": true }
        }
    })
}

fn api_key_config_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": { "type": "string", "format": "uuid" },
            "name": { "type": "string" },
            "prefix": { "type": "string", "description": "First 8 characters of the key for identification" },
            "scopes": { "type": "array", "items": { "type": "string" } },
            "expires_at": { "type": "string", "format": "date-time", "nullable": true },
            "last_used_at": { "type": "string", "format": "date-time", "nullable": true },
            "created_at": { "type": "string", "format": "date-time" }
        }
    })
}

fn create_api_key_request_schema() -> Value {
    json!({
        "type": "object",
        "required": ["name"],
        "properties": {
            "name": { "type": "string" },
            "scopes": { "type": "array", "items": { "type": "string" }, "description": "Permission scopes (e.g. data:read, data:write, admin)" },
            "expires_in_days": { "type": "integer", "nullable": true }
        }
    })
}

fn api_key_created_schema() -> Value {
    json!({
        "type": "object",
        "required": ["id", "key"],
        "properties": {
            "id": { "type": "string", "format": "uuid" },
            "key": { "type": "string", "description": "The full API key. Only shown once at creation time." },
            "name": { "type": "string" },
            "scopes": { "type": "array", "items": { "type": "string" } },
            "expires_at": { "type": "string", "format": "date-time", "nullable": true }
        }
    })
}

fn pagination_meta_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "count": { "type": "integer" },
            "total": { "type": "integer", "nullable": true },
            "cursor": { "type": "string", "nullable": true },
            "has_more": { "type": "boolean" },
            "duration_ms": { "type": "number" }
        }
    })
}

// ===========================================================================
// Shared response/request builders
// ===========================================================================

fn json_body(schema_ref: &str) -> Value {
    json!({
        "required": true,
        "content": {
            "application/json": {
                "schema": { "$ref": format!("#/components/schemas/{schema_ref}") }
            }
        }
    })
}

fn json_response(description: &str, schema_ref: &str) -> Value {
    json!({
        "description": description,
        "content": {
            "application/json": {
                "schema": { "$ref": format!("#/components/schemas/{schema_ref}") }
            }
        }
    })
}

fn json_list_response(description: &str, schema_ref: &str) -> Value {
    json!({
        "description": description,
        "content": {
            "application/json": {
                "schema": {
                    "type": "object",
                    "properties": {
                        "data": {
                            "type": "array",
                            "items": { "$ref": format!("#/components/schemas/{schema_ref}") }
                        },
                        "meta": { "$ref": "#/components/schemas/PaginationMeta" }
                    }
                }
            }
        }
    })
}

fn data_response(description: &str, schema_ref: &str) -> Value {
    json!({
        "description": description,
        "content": {
            "application/json": {
                "schema": {
                    "type": "object",
                    "properties": {
                        "data": { "$ref": format!("#/components/schemas/{schema_ref}") },
                        "meta": { "type": "object", "additionalProperties": true }
                    }
                }
            }
        }
    })
}

fn error_response(description: &str) -> Value {
    json!({
        "description": description,
        "content": {
            "application/json": {
                "schema": { "$ref": "#/components/schemas/ErrorResponse" }
            }
        }
    })
}

fn uuid_path_param(name: &str, description: &str) -> Value {
    json!({
        "name": name,
        "in": "path",
        "required": true,
        "schema": { "type": "string", "format": "uuid" },
        "description": description
    })
}

fn string_path_param(name: &str, description: &str) -> Value {
    json!({
        "name": name,
        "in": "path",
        "required": true,
        "schema": { "type": "string" },
        "description": description
    })
}

fn secured() -> Value {
    json!([{ "bearerAuth": [] }])
}

// ===========================================================================
// Auth paths
// ===========================================================================

fn auth_signup_path() -> Value {
    json!({
        "post": {
            "tags": ["Auth"],
            "summary": "Create a new account",
            "operationId": "authSignup",
            "requestBody": {
                "required": true,
                "content": {
                    "application/json": {
                        "schema": {
                            "type": "object",
                            "required": ["email", "password"],
                            "properties": {
                                "email": { "type": "string", "format": "email" },
                                "password": { "type": "string", "minLength": 8 }
                            }
                        }
                    }
                }
            },
            "responses": {
                "201": json_response("Account created", "TokenPair"),
                "400": error_response("Invalid input"),
                "409": error_response("Email already registered")
            }
        }
    })
}

fn auth_signin_path() -> Value {
    json!({
        "post": {
            "tags": ["Auth"],
            "summary": "Sign in with email and password",
            "operationId": "authSignin",
            "requestBody": {
                "required": true,
                "content": {
                    "application/json": {
                        "schema": {
                            "type": "object",
                            "required": ["email", "password"],
                            "properties": {
                                "email": { "type": "string", "format": "email" },
                                "password": { "type": "string" }
                            }
                        }
                    }
                }
            },
            "responses": {
                "200": json_response("Signed in", "TokenPair"),
                "401": error_response("Invalid credentials"),
                "429": error_response("Rate limited")
            }
        }
    })
}

fn auth_magic_link_path() -> Value {
    json!({
        "post": {
            "tags": ["Auth"],
            "summary": "Request a magic link sent to email",
            "operationId": "authMagicLink",
            "requestBody": {
                "required": true,
                "content": {
                    "application/json": {
                        "schema": {
                            "type": "object",
                            "required": ["email"],
                            "properties": {
                                "email": { "type": "string", "format": "email" }
                            }
                        }
                    }
                }
            },
            "responses": {
                "200": { "description": "Magic link sent (always returns 200 to prevent enumeration)" },
                "429": error_response("Rate limited")
            }
        }
    })
}

fn auth_verify_path() -> Value {
    json!({
        "post": {
            "tags": ["Auth"],
            "summary": "Verify a magic-link or MFA token",
            "operationId": "authVerify",
            "requestBody": {
                "required": true,
                "content": {
                    "application/json": {
                        "schema": {
                            "type": "object",
                            "required": ["token"],
                            "properties": {
                                "token": { "type": "string" },
                                "mfa_code": { "type": "string", "nullable": true }
                            }
                        }
                    }
                }
            },
            "responses": {
                "200": json_response("Verified", "TokenPair"),
                "401": error_response("Invalid or expired token")
            }
        }
    })
}

fn auth_oauth_path() -> Value {
    json!({
        "post": {
            "tags": ["Auth"],
            "summary": "Initiate or complete OAuth2 flow",
            "operationId": "authOAuth",
            "parameters": [
                {
                    "name": "provider",
                    "in": "path",
                    "required": true,
                    "schema": { "type": "string", "enum": ["google", "github", "apple", "discord", "microsoft", "twitter", "linkedin", "slack", "gitlab", "bitbucket", "facebook", "spotify"] }
                }
            ],
            "requestBody": {
                "required": true,
                "content": {
                    "application/json": {
                        "schema": {
                            "type": "object",
                            "properties": {
                                "code": { "type": "string", "description": "OAuth authorization code" },
                                "redirect_uri": { "type": "string" }
                            }
                        }
                    }
                }
            },
            "responses": {
                "200": json_response("OAuth complete", "TokenPair"),
                "400": error_response("OAuth error")
            }
        }
    })
}

fn auth_refresh_path() -> Value {
    json!({
        "post": {
            "tags": ["Auth"],
            "summary": "Refresh an access token",
            "operationId": "authRefresh",
            "requestBody": {
                "required": true,
                "content": {
                    "application/json": {
                        "schema": {
                            "type": "object",
                            "required": ["refresh_token"],
                            "properties": {
                                "refresh_token": { "type": "string" }
                            }
                        }
                    }
                }
            },
            "responses": {
                "200": json_response("Token refreshed", "TokenPair"),
                "401": error_response("Invalid refresh token")
            }
        }
    })
}

fn auth_signout_path() -> Value {
    json!({
        "post": {
            "tags": ["Auth"],
            "summary": "Sign out and revoke the current session",
            "operationId": "authSignout",
            "security": secured(),
            "responses": {
                "204": { "description": "Signed out" },
                "401": error_response("Not authenticated")
            }
        }
    })
}

fn auth_me_path() -> Value {
    json!({
        "get": {
            "tags": ["Auth"],
            "summary": "Get the current user profile",
            "operationId": "authMe",
            "security": secured(),
            "responses": {
                "200": json_response("Current user", "UserProfile"),
                "401": error_response("Not authenticated")
            }
        }
    })
}

// ===========================================================================
// Data / Query paths
// ===========================================================================

fn query_path() -> Value {
    json!({
        "post": {
            "tags": ["Query"],
            "summary": "Execute a DarshJQL query",
            "operationId": "query",
            "security": secured(),
            "requestBody": json_body("QueryRequest"),
            "responses": {
                "200": {
                    "description": "Query results",
                    "content": {
                        "application/json": {
                            "schema": {
                                "type": "object",
                                "properties": {
                                    "data": { "type": "array", "items": { "type": "object" } },
                                    "meta": {
                                        "type": "object",
                                        "properties": {
                                            "count": { "type": "integer" },
                                            "duration_ms": { "type": "number" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                },
                "400": error_response("Invalid query"),
                "401": error_response("Not authenticated")
            }
        }
    })
}

fn mutate_path() -> Value {
    json!({
        "post": {
            "tags": ["Data"],
            "summary": "Submit a batch of mutations as a transaction",
            "operationId": "mutate",
            "security": secured(),
            "requestBody": json_body("MutateRequest"),
            "responses": {
                "200": {
                    "description": "Transaction result",
                    "content": {
                        "application/json": {
                            "schema": {
                                "type": "object",
                                "properties": {
                                    "tx_id": { "type": "integer" },
                                    "affected": { "type": "integer" }
                                }
                            }
                        }
                    }
                },
                "400": error_response("Invalid mutation"),
                "401": error_response("Not authenticated"),
                "403": error_response("Permission denied")
            }
        }
    })
}

fn data_collection_path() -> Value {
    json!({
        "get": {
            "tags": ["Data"],
            "summary": "List entities of a type",
            "operationId": "dataList",
            "security": secured(),
            "parameters": [
                string_path_param("entity", "Entity type name"),
                { "name": "limit", "in": "query", "schema": { "type": "integer", "default": 50 } },
                { "name": "cursor", "in": "query", "schema": { "type": "string" } }
            ],
            "responses": {
                "200": { "description": "Entity list with pagination" },
                "401": error_response("Not authenticated")
            }
        },
        "post": {
            "tags": ["Data"],
            "summary": "Create a new entity",
            "operationId": "dataCreate",
            "security": secured(),
            "parameters": [
                string_path_param("entity", "Entity type name")
            ],
            "requestBody": {
                "required": true,
                "content": {
                    "application/json": {
                        "schema": { "type": "object", "additionalProperties": true }
                    }
                }
            },
            "responses": {
                "201": { "description": "Entity created" },
                "400": error_response("Invalid data"),
                "401": error_response("Not authenticated")
            }
        }
    })
}

fn data_item_path() -> Value {
    json!({
        "get": {
            "tags": ["Data"],
            "summary": "Get an entity by ID",
            "operationId": "dataGet",
            "security": secured(),
            "parameters": [
                string_path_param("entity", "Entity type name"),
                uuid_path_param("id", "Entity ID")
            ],
            "responses": {
                "200": { "description": "Entity data" },
                "404": error_response("Not found")
            }
        },
        "patch": {
            "tags": ["Data"],
            "summary": "Partially update an entity",
            "operationId": "dataPatch",
            "security": secured(),
            "parameters": [
                string_path_param("entity", "Entity type name"),
                uuid_path_param("id", "Entity ID")
            ],
            "requestBody": {
                "required": true,
                "content": {
                    "application/json": {
                        "schema": { "type": "object", "additionalProperties": true }
                    }
                }
            },
            "responses": {
                "200": { "description": "Entity updated" },
                "404": error_response("Not found")
            }
        },
        "delete": {
            "tags": ["Data"],
            "summary": "Delete an entity",
            "operationId": "dataDelete",
            "security": secured(),
            "parameters": [
                string_path_param("entity", "Entity type name"),
                uuid_path_param("id", "Entity ID")
            ],
            "responses": {
                "204": { "description": "Deleted" },
                "404": error_response("Not found")
            }
        }
    })
}

// ===========================================================================
// Functions
// ===========================================================================

fn function_path() -> Value {
    json!({
        "post": {
            "tags": ["Functions"],
            "summary": "Invoke a server-side function",
            "operationId": "functionInvoke",
            "security": secured(),
            "parameters": [
                string_path_param("name", "Function name")
            ],
            "requestBody": {
                "required": false,
                "content": {
                    "application/json": {
                        "schema": { "type": "object", "additionalProperties": true }
                    }
                }
            },
            "responses": {
                "200": { "description": "Function result" },
                "400": error_response("Validation error"),
                "404": error_response("Function not found")
            }
        }
    })
}

// ===========================================================================
// Storage
// ===========================================================================

fn storage_upload_path() -> Value {
    json!({
        "post": {
            "tags": ["Storage"],
            "summary": "Upload a file",
            "operationId": "storageUpload",
            "security": secured(),
            "requestBody": {
                "required": true,
                "content": {
                    "multipart/form-data": {
                        "schema": {
                            "type": "object",
                            "properties": {
                                "file": { "type": "string", "format": "binary" },
                                "path": { "type": "string", "description": "Destination path" }
                            }
                        }
                    }
                }
            },
            "responses": {
                "201": json_response("File uploaded", "UploadResponse"),
                "413": error_response("File too large")
            }
        }
    })
}

fn storage_item_path() -> Value {
    json!({
        "get": {
            "tags": ["Storage"],
            "summary": "Download a file or get a signed URL",
            "operationId": "storageGet",
            "parameters": [
                string_path_param("path", "File path"),
                { "name": "signed", "in": "query", "schema": { "type": "boolean", "default": false } },
                { "name": "transform", "in": "query", "schema": { "type": "string", "description": "Image transform params (e.g. w=200,h=200,fit=cover)" } }
            ],
            "responses": {
                "200": { "description": "File content or signed URL" },
                "404": error_response("File not found")
            }
        },
        "delete": {
            "tags": ["Storage"],
            "summary": "Delete a file",
            "operationId": "storageDelete",
            "security": secured(),
            "parameters": [
                string_path_param("path", "File path")
            ],
            "responses": {
                "204": { "description": "Deleted" },
                "404": error_response("File not found")
            }
        }
    })
}

// ===========================================================================
// Realtime / Pub/Sub
// ===========================================================================

fn subscribe_path() -> Value {
    json!({
        "get": {
            "tags": ["Realtime"],
            "summary": "Subscribe to live query updates via Server-Sent Events",
            "operationId": "subscribe",
            "security": secured(),
            "parameters": [
                {
                    "name": "q",
                    "in": "query",
                    "required": true,
                    "schema": { "type": "string" },
                    "description": "DarshJQL query to subscribe to"
                }
            ],
            "responses": {
                "200": {
                    "description": "SSE stream",
                    "content": { "text/event-stream": { "schema": { "type": "string" } } }
                },
                "401": error_response("Not authenticated")
            }
        }
    })
}

fn events_path() -> Value {
    json!({
        "get": {
            "tags": ["Pub/Sub"],
            "summary": "Subscribe to pub/sub events via Server-Sent Events",
            "operationId": "eventsSse",
            "description": "Streams keyspace notification events matching the given channel pattern.",
            "security": secured(),
            "parameters": [
                {
                    "name": "channel",
                    "in": "query",
                    "required": true,
                    "schema": { "type": "string" },
                    "description": "Channel pattern to subscribe to (e.g. entity:users:*)"
                }
            ],
            "responses": {
                "200": {
                    "description": "SSE stream of matching pub/sub events",
                    "content": { "text/event-stream": { "schema": { "type": "string" } } }
                },
                "401": error_response("Not authenticated")
            }
        }
    })
}

fn events_publish_path() -> Value {
    json!({
        "post": {
            "tags": ["Pub/Sub"],
            "summary": "Publish a custom event to a channel",
            "operationId": "eventsPublish",
            "security": secured(),
            "requestBody": {
                "required": true,
                "content": {
                    "application/json": {
                        "schema": {
                            "type": "object",
                            "required": ["channel", "event"],
                            "properties": {
                                "channel": { "type": "string" },
                                "event": { "type": "string" },
                                "payload": { "nullable": true }
                            }
                        }
                    }
                }
            },
            "responses": {
                "200": {
                    "description": "Event published",
                    "content": {
                        "application/json": {
                            "schema": {
                                "type": "object",
                                "properties": {
                                    "ok": { "type": "boolean" },
                                    "channel": { "type": "string" },
                                    "event": { "type": "string" },
                                    "receivers": { "type": "integer" }
                                }
                            }
                        }
                    }
                },
                "401": error_response("Not authenticated")
            }
        }
    })
}

// ===========================================================================
// Views
// ===========================================================================

fn views_collection_path() -> Value {
    json!({
        "get": {
            "tags": ["Views"],
            "summary": "List views for an entity type",
            "operationId": "viewsList",
            "security": secured(),
            "parameters": [
                { "name": "type", "in": "query", "required": true, "schema": { "type": "string" }, "description": "Entity type to list views for" }
            ],
            "responses": {
                "200": json_list_response("Views list", "ViewConfig"),
                "401": error_response("Not authenticated")
            }
        },
        "post": {
            "tags": ["Views"],
            "summary": "Create a new view",
            "operationId": "viewsCreate",
            "security": secured(),
            "requestBody": json_body("CreateViewRequest"),
            "responses": {
                "201": data_response("View created", "ViewConfig"),
                "400": error_response("Invalid input"),
                "401": error_response("Not authenticated")
            }
        }
    })
}

fn views_item_path() -> Value {
    json!({
        "get": {
            "tags": ["Views"],
            "summary": "Get a view by ID",
            "operationId": "viewsGet",
            "security": secured(),
            "parameters": [ uuid_path_param("id", "View ID") ],
            "responses": {
                "200": data_response("View configuration", "ViewConfig"),
                "404": error_response("View not found")
            }
        },
        "patch": {
            "tags": ["Views"],
            "summary": "Partially update a view",
            "operationId": "viewsUpdate",
            "security": secured(),
            "parameters": [ uuid_path_param("id", "View ID") ],
            "requestBody": json_body("ViewUpdate"),
            "responses": {
                "200": data_response("View updated", "ViewConfig"),
                "404": error_response("View not found")
            }
        },
        "delete": {
            "tags": ["Views"],
            "summary": "Delete a view",
            "operationId": "viewsDelete",
            "security": secured(),
            "parameters": [ uuid_path_param("id", "View ID") ],
            "responses": {
                "204": { "description": "Deleted" },
                "404": error_response("View not found")
            }
        }
    })
}

fn views_query_path() -> Value {
    json!({
        "post": {
            "tags": ["Views"],
            "summary": "Execute a query through a view lens",
            "operationId": "viewsQuery",
            "description": "The view's filters and sorts are merged with any user-supplied query. Hidden fields are stripped. Field ordering is applied.",
            "security": secured(),
            "parameters": [ uuid_path_param("id", "View ID") ],
            "requestBody": json_body("ViewQueryRequest"),
            "responses": {
                "200": {
                    "description": "Query results filtered through the view",
                    "content": {
                        "application/json": {
                            "schema": {
                                "type": "object",
                                "properties": {
                                    "data": { "type": "array", "items": { "type": "object" } },
                                    "meta": {
                                        "type": "object",
                                        "properties": {
                                            "view_id": { "type": "string", "format": "uuid" },
                                            "view_name": { "type": "string" },
                                            "view_kind": { "type": "string" },
                                            "count": { "type": "integer" },
                                            "duration_ms": { "type": "number" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                },
                "400": error_response("Invalid query"),
                "404": error_response("View not found")
            }
        }
    })
}

// ===========================================================================
// Fields
// ===========================================================================

fn fields_collection_path() -> Value {
    json!({
        "get": {
            "tags": ["Fields"],
            "summary": "List all fields for a table",
            "operationId": "fieldsList",
            "security": secured(),
            "parameters": [ string_path_param("table", "Table entity type") ],
            "responses": {
                "200": json_list_response("Fields list", "FieldDefinition"),
                "401": error_response("Not authenticated")
            }
        },
        "post": {
            "tags": ["Fields"],
            "summary": "Add a field to a table",
            "operationId": "fieldsCreate",
            "security": secured(),
            "parameters": [ string_path_param("table", "Table entity type") ],
            "requestBody": json_body("CreateFieldRequest"),
            "responses": {
                "201": data_response("Field created", "FieldDefinition"),
                "400": error_response("Invalid field definition"),
                "409": error_response("Field name already exists")
            }
        }
    })
}

fn fields_item_path() -> Value {
    json!({
        "get": {
            "tags": ["Fields"],
            "summary": "Get a field by ID",
            "operationId": "fieldsGet",
            "security": secured(),
            "parameters": [
                string_path_param("table", "Table entity type"),
                uuid_path_param("field_id", "Field ID")
            ],
            "responses": {
                "200": data_response("Field definition", "FieldDefinition"),
                "404": error_response("Field not found")
            }
        },
        "patch": {
            "tags": ["Fields"],
            "summary": "Update a field",
            "operationId": "fieldsUpdate",
            "security": secured(),
            "parameters": [
                string_path_param("table", "Table entity type"),
                uuid_path_param("field_id", "Field ID")
            ],
            "requestBody": json_body("UpdateFieldRequest"),
            "responses": {
                "200": data_response("Field updated", "FieldDefinition"),
                "404": error_response("Field not found")
            }
        },
        "delete": {
            "tags": ["Fields"],
            "summary": "Delete a field from a table",
            "operationId": "fieldsDelete",
            "security": secured(),
            "parameters": [
                string_path_param("table", "Table entity type"),
                uuid_path_param("field_id", "Field ID")
            ],
            "responses": {
                "204": { "description": "Field deleted" },
                "404": error_response("Field not found")
            }
        }
    })
}

fn fields_convert_path() -> Value {
    json!({
        "post": {
            "tags": ["Fields"],
            "summary": "Convert a field to a different type",
            "operationId": "fieldsConvert",
            "description": "Converts all existing values to the target type. Irreversible for lossy conversions.",
            "security": secured(),
            "parameters": [
                string_path_param("table", "Table entity type"),
                uuid_path_param("field_id", "Field ID")
            ],
            "requestBody": json_body("ConvertFieldRequest"),
            "responses": {
                "200": data_response("Field converted", "FieldDefinition"),
                "400": error_response("Conversion not supported or would cause data loss"),
                "404": error_response("Field not found")
            }
        }
    })
}

// ===========================================================================
// Tables
// ===========================================================================

fn tables_collection_path() -> Value {
    json!({
        "get": {
            "tags": ["Tables"],
            "summary": "List all tables",
            "operationId": "tablesList",
            "security": secured(),
            "responses": {
                "200": json_list_response("Tables list", "TableConfig"),
                "401": error_response("Not authenticated")
            }
        },
        "post": {
            "tags": ["Tables"],
            "summary": "Create a new table",
            "operationId": "tablesCreate",
            "security": secured(),
            "requestBody": json_body("CreateTableRequest"),
            "responses": {
                "201": data_response("Table created", "TableConfig"),
                "400": error_response("Invalid table definition"),
                "409": error_response("Table name already exists")
            }
        }
    })
}

fn tables_item_path() -> Value {
    json!({
        "get": {
            "tags": ["Tables"],
            "summary": "Get a table by name",
            "operationId": "tablesGet",
            "security": secured(),
            "parameters": [ string_path_param("table", "Table entity type") ],
            "responses": {
                "200": data_response("Table configuration", "TableConfig"),
                "404": error_response("Table not found")
            }
        },
        "patch": {
            "tags": ["Tables"],
            "summary": "Update table metadata",
            "operationId": "tablesUpdate",
            "security": secured(),
            "parameters": [ string_path_param("table", "Table entity type") ],
            "requestBody": json_body("UpdateTableRequest"),
            "responses": {
                "200": data_response("Table updated", "TableConfig"),
                "404": error_response("Table not found")
            }
        },
        "delete": {
            "tags": ["Tables"],
            "summary": "Delete a table and all its data",
            "operationId": "tablesDelete",
            "security": secured(),
            "parameters": [ string_path_param("table", "Table entity type") ],
            "responses": {
                "204": { "description": "Table deleted" },
                "404": error_response("Table not found")
            }
        }
    })
}

fn tables_templates_path() -> Value {
    json!({
        "get": {
            "tags": ["Tables"],
            "summary": "List available table templates",
            "operationId": "tablesTemplates",
            "security": secured(),
            "responses": {
                "200": json_list_response("Available templates", "TableTemplate")
            }
        }
    })
}

fn tables_stats_path() -> Value {
    json!({
        "get": {
            "tags": ["Tables"],
            "summary": "Get statistics for a table",
            "operationId": "tablesStats",
            "security": secured(),
            "parameters": [ string_path_param("table", "Table entity type") ],
            "responses": {
                "200": data_response("Table statistics", "TableStats"),
                "404": error_response("Table not found")
            }
        }
    })
}

// ===========================================================================
// Automations
// ===========================================================================

fn automations_collection_path() -> Value {
    json!({
        "get": {
            "tags": ["Admin"],
            "summary": "List all automations",
            "operationId": "automationsList",
            "security": secured(),
            "parameters": [
                { "name": "entity_type", "in": "query", "schema": { "type": "string" }, "description": "Filter by entity type" },
                { "name": "enabled", "in": "query", "schema": { "type": "boolean" }, "description": "Filter by enabled status" }
            ],
            "responses": {
                "200": json_list_response("Automations list", "AutomationConfig"),
                "401": error_response("Not authenticated")
            }
        },
        "post": {
            "tags": ["Admin"],
            "summary": "Create a new automation",
            "operationId": "automationsCreate",
            "security": secured(),
            "requestBody": json_body("CreateAutomationRequest"),
            "responses": {
                "201": data_response("Automation created", "AutomationConfig"),
                "400": error_response("Invalid automation definition")
            }
        }
    })
}

fn automations_item_path() -> Value {
    json!({
        "get": {
            "tags": ["Admin"],
            "summary": "Get an automation by ID",
            "operationId": "automationsGet",
            "security": secured(),
            "parameters": [ uuid_path_param("id", "Automation ID") ],
            "responses": {
                "200": data_response("Automation configuration", "AutomationConfig"),
                "404": error_response("Automation not found")
            }
        },
        "patch": {
            "tags": ["Admin"],
            "summary": "Update an automation",
            "operationId": "automationsUpdate",
            "security": secured(),
            "parameters": [ uuid_path_param("id", "Automation ID") ],
            "requestBody": json_body("CreateAutomationRequest"),
            "responses": {
                "200": data_response("Automation updated", "AutomationConfig"),
                "404": error_response("Automation not found")
            }
        },
        "delete": {
            "tags": ["Admin"],
            "summary": "Delete an automation",
            "operationId": "automationsDelete",
            "security": secured(),
            "parameters": [ uuid_path_param("id", "Automation ID") ],
            "responses": {
                "204": { "description": "Automation deleted" },
                "404": error_response("Automation not found")
            }
        }
    })
}

fn automations_trigger_path() -> Value {
    json!({
        "post": {
            "tags": ["Admin"],
            "summary": "Manually trigger an automation",
            "operationId": "automationsTrigger",
            "security": secured(),
            "parameters": [ uuid_path_param("id", "Automation ID") ],
            "requestBody": {
                "required": false,
                "content": {
                    "application/json": {
                        "schema": {
                            "type": "object",
                            "description": "Custom trigger data to pass to the automation",
                            "additionalProperties": true
                        }
                    }
                }
            },
            "responses": {
                "200": data_response("Automation run result", "AutomationRun"),
                "404": error_response("Automation not found")
            }
        }
    })
}

fn automations_runs_path() -> Value {
    json!({
        "get": {
            "tags": ["Admin"],
            "summary": "List run history for an automation",
            "operationId": "automationsRuns",
            "security": secured(),
            "parameters": [
                uuid_path_param("id", "Automation ID"),
                { "name": "limit", "in": "query", "schema": { "type": "integer", "default": 50 } },
                { "name": "status", "in": "query", "schema": { "type": "string", "enum": ["pending", "running", "completed", "failed"] } }
            ],
            "responses": {
                "200": json_list_response("Automation run history", "AutomationRun"),
                "404": error_response("Automation not found")
            }
        }
    })
}

// ===========================================================================
// Relations
// ===========================================================================

fn relations_link_path() -> Value {
    json!({
        "post": {
            "tags": ["Relations"],
            "summary": "Link two entities",
            "operationId": "relationsLink",
            "security": secured(),
            "requestBody": json_body("LinkRequest"),
            "responses": {
                "201": { "description": "Link created", "content": { "application/json": { "schema": { "type": "object", "properties": { "ok": { "type": "boolean" }, "tx_id": { "type": "integer" } } } } } },
                "400": error_response("Invalid link request"),
                "404": error_response("Entity not found"),
                "409": error_response("Link already exists")
            }
        }
    })
}

fn relations_unlink_path() -> Value {
    json!({
        "post": {
            "tags": ["Relations"],
            "summary": "Remove a link between entities",
            "operationId": "relationsUnlink",
            "security": secured(),
            "requestBody": json_body("UnlinkRequest"),
            "responses": {
                "200": { "description": "Link removed" },
                "404": error_response("Link not found")
            }
        }
    })
}

fn relations_lookup_path() -> Value {
    json!({
        "post": {
            "tags": ["Relations"],
            "summary": "Lookup a field from a related entity",
            "operationId": "relationsLookup",
            "security": secured(),
            "requestBody": json_body("LookupRequest"),
            "responses": {
                "200": {
                    "description": "Lookup result",
                    "content": {
                        "application/json": {
                            "schema": {
                                "type": "object",
                                "properties": {
                                    "values": { "type": "array", "items": {} },
                                    "count": { "type": "integer" }
                                }
                            }
                        }
                    }
                },
                "400": error_response("Invalid lookup request"),
                "404": error_response("Entity not found")
            }
        }
    })
}

fn relations_rollup_path() -> Value {
    json!({
        "post": {
            "tags": ["Relations"],
            "summary": "Compute a rollup over related entities",
            "operationId": "relationsRollup",
            "security": secured(),
            "requestBody": json_body("RollupRequest"),
            "responses": {
                "200": {
                    "description": "Rollup result",
                    "content": {
                        "application/json": {
                            "schema": {
                                "type": "object",
                                "properties": {
                                    "result": {},
                                    "count": { "type": "integer" },
                                    "function": { "type": "string" }
                                }
                            }
                        }
                    }
                },
                "400": error_response("Invalid rollup request"),
                "404": error_response("Entity not found")
            }
        }
    })
}

// ===========================================================================
// Aggregation
// ===========================================================================

fn aggregation_path() -> Value {
    json!({
        "post": {
            "tags": ["Aggregation"],
            "summary": "Execute an aggregate query",
            "operationId": "aggregate",
            "security": secured(),
            "requestBody": json_body("AggregateRequest"),
            "responses": {
                "200": json_response("Aggregation results", "AggregateResponse"),
                "400": error_response("Invalid aggregation request"),
                "401": error_response("Not authenticated")
            }
        }
    })
}

fn aggregation_summary_path() -> Value {
    json!({
        "post": {
            "tags": ["Aggregation"],
            "summary": "Get a statistical summary for an entity type",
            "operationId": "aggregateSummary",
            "security": secured(),
            "requestBody": json_body("SummaryRequest"),
            "responses": {
                "200": {
                    "description": "Summary statistics",
                    "content": {
                        "application/json": {
                            "schema": {
                                "type": "object",
                                "properties": {
                                    "data": { "type": "object", "additionalProperties": { "type": "object", "properties": { "count": { "type": "integer" }, "sum": { "type": "number" }, "avg": { "type": "number" }, "min": { "type": "number" }, "max": { "type": "number" } } } },
                                    "total_rows": { "type": "integer" }
                                }
                            }
                        }
                    }
                },
                "400": error_response("Invalid request")
            }
        }
    })
}

fn aggregation_chart_path() -> Value {
    json!({
        "post": {
            "tags": ["Aggregation"],
            "summary": "Generate chart-ready data",
            "operationId": "aggregateChart",
            "security": secured(),
            "requestBody": json_body("ChartRequest"),
            "responses": {
                "200": {
                    "description": "Chart data points",
                    "content": {
                        "application/json": {
                            "schema": {
                                "type": "object",
                                "properties": {
                                    "labels": { "type": "array", "items": { "type": "string" } },
                                    "datasets": { "type": "array", "items": { "type": "object", "properties": { "label": { "type": "string" }, "data": { "type": "array", "items": { "type": "number" } } } } },
                                    "chart_type": { "type": "string" }
                                }
                            }
                        }
                    }
                },
                "400": error_response("Invalid chart request")
            }
        }
    })
}

// ===========================================================================
// Import / Export
// ===========================================================================

fn import_path() -> Value {
    json!({
        "post": {
            "tags": ["Import/Export"],
            "summary": "Import data from CSV or JSON",
            "operationId": "importData",
            "security": secured(),
            "requestBody": {
                "required": true,
                "content": {
                    "application/json": {
                        "schema": { "$ref": "#/components/schemas/ImportRequest" }
                    },
                    "multipart/form-data": {
                        "schema": {
                            "type": "object",
                            "properties": {
                                "file": { "type": "string", "format": "binary" },
                                "entity_type": { "type": "string" },
                                "format": { "type": "string", "enum": ["csv", "json", "jsonl"] },
                                "on_conflict": { "type": "string", "enum": ["skip", "update", "error"] },
                                "field_mapping": { "type": "string", "description": "JSON-encoded field mapping" },
                                "dry_run": { "type": "boolean" }
                            }
                        }
                    }
                }
            },
            "responses": {
                "200": json_response("Import result", "ImportResult"),
                "400": error_response("Invalid import request"),
                "413": error_response("File too large")
            }
        }
    })
}

fn export_path() -> Value {
    json!({
        "post": {
            "tags": ["Import/Export"],
            "summary": "Export data to CSV or JSON",
            "operationId": "exportData",
            "security": secured(),
            "requestBody": json_body("ExportRequest"),
            "responses": {
                "200": {
                    "description": "Exported data",
                    "content": {
                        "text/csv": { "schema": { "type": "string" } },
                        "application/json": { "schema": { "type": "array", "items": { "type": "object" } } },
                        "application/x-ndjson": { "schema": { "type": "string" } }
                    }
                },
                "400": error_response("Invalid export request")
            }
        }
    })
}

// ===========================================================================
// Collaboration
// ===========================================================================

fn collab_share_path() -> Value {
    json!({
        "get": {
            "tags": ["Collaboration"],
            "summary": "List active share links",
            "operationId": "shareList",
            "security": secured(),
            "parameters": [
                { "name": "resource_type", "in": "query", "schema": { "type": "string" } },
                { "name": "resource_id", "in": "query", "schema": { "type": "string", "format": "uuid" } }
            ],
            "responses": {
                "200": json_list_response("Share links", "ShareLink"),
                "401": error_response("Not authenticated")
            }
        },
        "post": {
            "tags": ["Collaboration"],
            "summary": "Create a share link",
            "operationId": "shareCreate",
            "security": secured(),
            "requestBody": json_body("CreateShareRequest"),
            "responses": {
                "201": data_response("Share link created", "ShareLink"),
                "400": error_response("Invalid share request")
            }
        }
    })
}

fn collab_share_item_path() -> Value {
    json!({
        "delete": {
            "tags": ["Collaboration"],
            "summary": "Revoke a share link",
            "operationId": "shareRevoke",
            "security": secured(),
            "parameters": [ uuid_path_param("id", "Share link ID") ],
            "responses": {
                "204": { "description": "Share link revoked" },
                "404": error_response("Share link not found")
            }
        }
    })
}

fn collab_collaborators_path() -> Value {
    json!({
        "get": {
            "tags": ["Collaboration"],
            "summary": "List collaborators",
            "operationId": "collaboratorsList",
            "security": secured(),
            "parameters": [
                { "name": "workspace_id", "in": "query", "schema": { "type": "string", "format": "uuid" } }
            ],
            "responses": {
                "200": json_list_response("Collaborators", "Collaborator"),
                "401": error_response("Not authenticated")
            }
        },
        "post": {
            "tags": ["Collaboration"],
            "summary": "Add a collaborator",
            "operationId": "collaboratorsAdd",
            "security": secured(),
            "requestBody": json_body("AddCollaboratorRequest"),
            "responses": {
                "201": data_response("Collaborator added", "Collaborator"),
                "400": error_response("Invalid request"),
                "409": error_response("Collaborator already exists")
            }
        }
    })
}

fn collab_collaborator_item_path() -> Value {
    json!({
        "patch": {
            "tags": ["Collaboration"],
            "summary": "Update a collaborator role",
            "operationId": "collaboratorsUpdate",
            "security": secured(),
            "parameters": [ uuid_path_param("id", "Collaborator ID") ],
            "requestBody": {
                "required": true,
                "content": {
                    "application/json": {
                        "schema": {
                            "type": "object",
                            "properties": {
                                "role": { "type": "string", "enum": ["admin", "editor", "commenter", "viewer"] }
                            }
                        }
                    }
                }
            },
            "responses": {
                "200": data_response("Collaborator updated", "Collaborator"),
                "404": error_response("Collaborator not found")
            }
        },
        "delete": {
            "tags": ["Collaboration"],
            "summary": "Remove a collaborator",
            "operationId": "collaboratorsRemove",
            "security": secured(),
            "parameters": [ uuid_path_param("id", "Collaborator ID") ],
            "responses": {
                "204": { "description": "Collaborator removed" },
                "404": error_response("Collaborator not found")
            }
        }
    })
}

fn collab_workspaces_path() -> Value {
    json!({
        "get": {
            "tags": ["Collaboration"],
            "summary": "List workspaces",
            "operationId": "workspacesList",
            "security": secured(),
            "responses": {
                "200": json_list_response("Workspaces", "Workspace"),
                "401": error_response("Not authenticated")
            }
        },
        "post": {
            "tags": ["Collaboration"],
            "summary": "Create a workspace",
            "operationId": "workspacesCreate",
            "security": secured(),
            "requestBody": json_body("CreateWorkspaceRequest"),
            "responses": {
                "201": data_response("Workspace created", "Workspace"),
                "400": error_response("Invalid request")
            }
        }
    })
}

fn collab_workspace_item_path() -> Value {
    json!({
        "get": {
            "tags": ["Collaboration"],
            "summary": "Get a workspace",
            "operationId": "workspacesGet",
            "security": secured(),
            "parameters": [ uuid_path_param("id", "Workspace ID") ],
            "responses": {
                "200": data_response("Workspace", "Workspace"),
                "404": error_response("Workspace not found")
            }
        },
        "patch": {
            "tags": ["Collaboration"],
            "summary": "Update workspace metadata",
            "operationId": "workspacesUpdate",
            "security": secured(),
            "parameters": [ uuid_path_param("id", "Workspace ID") ],
            "requestBody": json_body("CreateWorkspaceRequest"),
            "responses": {
                "200": data_response("Workspace updated", "Workspace"),
                "404": error_response("Workspace not found")
            }
        },
        "delete": {
            "tags": ["Collaboration"],
            "summary": "Delete a workspace",
            "operationId": "workspacesDelete",
            "security": secured(),
            "parameters": [ uuid_path_param("id", "Workspace ID") ],
            "responses": {
                "204": { "description": "Workspace deleted" },
                "404": error_response("Workspace not found")
            }
        }
    })
}

// ===========================================================================
// Plugins
// ===========================================================================

fn plugins_collection_path() -> Value {
    json!({
        "get": {
            "tags": ["Plugins"],
            "summary": "List installed plugins",
            "operationId": "pluginsList",
            "security": secured(),
            "responses": {
                "200": json_list_response("Installed plugins", "PluginConfig"),
                "401": error_response("Not authenticated")
            }
        },
        "post": {
            "tags": ["Plugins"],
            "summary": "Install a plugin",
            "operationId": "pluginsInstall",
            "security": secured(),
            "requestBody": {
                "required": true,
                "content": {
                    "application/json": {
                        "schema": {
                            "type": "object",
                            "required": ["name", "version"],
                            "properties": {
                                "name": { "type": "string" },
                                "version": { "type": "string" },
                                "config": { "type": "object", "additionalProperties": true }
                            }
                        }
                    }
                }
            },
            "responses": {
                "201": data_response("Plugin installed", "PluginConfig"),
                "400": error_response("Invalid plugin"),
                "409": error_response("Plugin already installed")
            }
        }
    })
}

fn plugins_item_path() -> Value {
    json!({
        "get": {
            "tags": ["Plugins"],
            "summary": "Get a plugin by ID",
            "operationId": "pluginsGet",
            "security": secured(),
            "parameters": [ uuid_path_param("id", "Plugin ID") ],
            "responses": {
                "200": data_response("Plugin", "PluginConfig"),
                "404": error_response("Plugin not found")
            }
        },
        "delete": {
            "tags": ["Plugins"],
            "summary": "Uninstall a plugin",
            "operationId": "pluginsUninstall",
            "security": secured(),
            "parameters": [ uuid_path_param("id", "Plugin ID") ],
            "responses": {
                "204": { "description": "Plugin uninstalled" },
                "404": error_response("Plugin not found")
            }
        }
    })
}

fn plugins_configure_path() -> Value {
    json!({
        "put": {
            "tags": ["Plugins"],
            "summary": "Configure a plugin",
            "operationId": "pluginsConfigure",
            "security": secured(),
            "parameters": [ uuid_path_param("id", "Plugin ID") ],
            "requestBody": json_body("PluginConfigureRequest"),
            "responses": {
                "200": data_response("Plugin configured", "PluginConfig"),
                "404": error_response("Plugin not found")
            }
        }
    })
}

// ===========================================================================
// Comments
// ===========================================================================

fn comments_collection_path() -> Value {
    json!({
        "get": {
            "tags": ["Activity"],
            "summary": "List comments on an entity",
            "operationId": "commentsList",
            "security": secured(),
            "parameters": [
                { "name": "entity_type", "in": "query", "required": true, "schema": { "type": "string" } },
                { "name": "entity_id", "in": "query", "required": true, "schema": { "type": "string", "format": "uuid" } },
                { "name": "limit", "in": "query", "schema": { "type": "integer", "default": 50 } }
            ],
            "responses": {
                "200": json_list_response("Comments", "Comment"),
                "401": error_response("Not authenticated")
            }
        },
        "post": {
            "tags": ["Activity"],
            "summary": "Add a comment",
            "operationId": "commentsCreate",
            "security": secured(),
            "requestBody": json_body("CreateCommentRequest"),
            "responses": {
                "201": data_response("Comment created", "Comment"),
                "400": error_response("Invalid comment")
            }
        }
    })
}

fn comments_item_path() -> Value {
    json!({
        "patch": {
            "tags": ["Activity"],
            "summary": "Edit a comment",
            "operationId": "commentsUpdate",
            "security": secured(),
            "parameters": [ uuid_path_param("id", "Comment ID") ],
            "requestBody": {
                "required": true,
                "content": {
                    "application/json": {
                        "schema": {
                            "type": "object",
                            "required": ["body"],
                            "properties": {
                                "body": { "type": "string" }
                            }
                        }
                    }
                }
            },
            "responses": {
                "200": data_response("Comment updated", "Comment"),
                "404": error_response("Comment not found")
            }
        },
        "delete": {
            "tags": ["Activity"],
            "summary": "Delete a comment",
            "operationId": "commentsDelete",
            "security": secured(),
            "parameters": [ uuid_path_param("id", "Comment ID") ],
            "responses": {
                "204": { "description": "Comment deleted" },
                "404": error_response("Comment not found")
            }
        }
    })
}

// ===========================================================================
// Activity / Notifications
// ===========================================================================

fn activity_log_path() -> Value {
    json!({
        "get": {
            "tags": ["Activity"],
            "summary": "Get the activity log",
            "operationId": "activityLog",
            "security": secured(),
            "parameters": [
                { "name": "entity_type", "in": "query", "schema": { "type": "string" } },
                { "name": "entity_id", "in": "query", "schema": { "type": "string", "format": "uuid" } },
                { "name": "user_id", "in": "query", "schema": { "type": "string", "format": "uuid" } },
                { "name": "action", "in": "query", "schema": { "type": "string" } },
                { "name": "since", "in": "query", "schema": { "type": "string", "format": "date-time" } },
                { "name": "limit", "in": "query", "schema": { "type": "integer", "default": 50 } },
                { "name": "cursor", "in": "query", "schema": { "type": "string" } }
            ],
            "responses": {
                "200": json_list_response("Activity entries", "ActivityEntry"),
                "401": error_response("Not authenticated")
            }
        }
    })
}

fn activity_notifications_path() -> Value {
    json!({
        "get": {
            "tags": ["Activity"],
            "summary": "Get notifications for the current user",
            "operationId": "notificationsList",
            "security": secured(),
            "parameters": [
                { "name": "unread_only", "in": "query", "schema": { "type": "boolean", "default": false } },
                { "name": "limit", "in": "query", "schema": { "type": "integer", "default": 50 } }
            ],
            "responses": {
                "200": json_list_response("Notifications", "Notification"),
                "401": error_response("Not authenticated")
            }
        },
        "patch": {
            "tags": ["Activity"],
            "summary": "Mark notifications as read",
            "operationId": "notificationsMarkRead",
            "security": secured(),
            "requestBody": {
                "required": true,
                "content": {
                    "application/json": {
                        "schema": {
                            "type": "object",
                            "properties": {
                                "ids": { "type": "array", "items": { "type": "string", "format": "uuid" }, "description": "Notification IDs to mark as read. If omitted, marks all as read." }
                            }
                        }
                    }
                }
            },
            "responses": {
                "200": { "description": "Notifications marked as read", "content": { "application/json": { "schema": { "type": "object", "properties": { "marked": { "type": "integer" } } } } } }
            }
        }
    })
}

// ===========================================================================
// History
// ===========================================================================

fn history_versions_path() -> Value {
    json!({
        "get": {
            "tags": ["History"],
            "summary": "Get version history for an entity",
            "operationId": "historyVersions",
            "security": secured(),
            "parameters": [
                string_path_param("entity", "Entity type"),
                uuid_path_param("id", "Entity ID"),
                { "name": "limit", "in": "query", "schema": { "type": "integer", "default": 50 } },
                { "name": "since_tx", "in": "query", "schema": { "type": "integer" }, "description": "Only return changes after this transaction ID" }
            ],
            "responses": {
                "200": json_list_response("Version entries", "VersionEntry"),
                "404": error_response("Entity not found")
            }
        }
    })
}

fn history_restore_path() -> Value {
    json!({
        "post": {
            "tags": ["History"],
            "summary": "Restore an entity to a previous version",
            "operationId": "historyRestore",
            "security": secured(),
            "parameters": [
                string_path_param("entity", "Entity type"),
                uuid_path_param("id", "Entity ID")
            ],
            "requestBody": {
                "required": true,
                "content": {
                    "application/json": {
                        "schema": {
                            "type": "object",
                            "required": ["tx_id"],
                            "properties": {
                                "tx_id": { "type": "integer", "description": "Transaction ID to restore to" }
                            }
                        }
                    }
                }
            },
            "responses": {
                "200": { "description": "Entity restored", "content": { "application/json": { "schema": { "type": "object", "properties": { "restored_to_tx": { "type": "integer" }, "new_tx_id": { "type": "integer" } } } } } },
                "400": error_response("Invalid restore request"),
                "404": error_response("Entity or transaction not found")
            }
        }
    })
}

fn history_snapshots_path() -> Value {
    json!({
        "get": {
            "tags": ["History"],
            "summary": "List snapshots",
            "operationId": "snapshotsList",
            "security": secured(),
            "parameters": [
                { "name": "limit", "in": "query", "schema": { "type": "integer", "default": 50 } }
            ],
            "responses": {
                "200": json_list_response("Snapshots", "Snapshot"),
                "401": error_response("Not authenticated")
            }
        },
        "post": {
            "tags": ["History"],
            "summary": "Create a snapshot",
            "operationId": "snapshotsCreate",
            "security": secured(),
            "requestBody": {
                "required": true,
                "content": {
                    "application/json": {
                        "schema": {
                            "type": "object",
                            "required": ["name"],
                            "properties": {
                                "name": { "type": "string" },
                                "description": { "type": "string", "nullable": true }
                            }
                        }
                    }
                }
            },
            "responses": {
                "201": data_response("Snapshot created", "Snapshot"),
                "401": error_response("Not authenticated")
            }
        }
    })
}

// ===========================================================================
// Webhooks
// ===========================================================================

fn webhooks_collection_path() -> Value {
    json!({
        "get": {
            "tags": ["Webhooks"],
            "summary": "List registered webhooks",
            "operationId": "webhooksList",
            "security": secured(),
            "responses": {
                "200": json_list_response("Webhooks", "WebhookConfig"),
                "401": error_response("Not authenticated")
            }
        },
        "post": {
            "tags": ["Webhooks"],
            "summary": "Register a webhook",
            "operationId": "webhooksCreate",
            "security": secured(),
            "requestBody": json_body("CreateWebhookRequest"),
            "responses": {
                "201": data_response("Webhook created", "WebhookConfig"),
                "400": error_response("Invalid webhook configuration")
            }
        }
    })
}

fn webhooks_item_path() -> Value {
    json!({
        "get": {
            "tags": ["Webhooks"],
            "summary": "Get a webhook by ID",
            "operationId": "webhooksGet",
            "security": secured(),
            "parameters": [ uuid_path_param("id", "Webhook ID") ],
            "responses": {
                "200": data_response("Webhook", "WebhookConfig"),
                "404": error_response("Webhook not found")
            }
        },
        "patch": {
            "tags": ["Webhooks"],
            "summary": "Update a webhook",
            "operationId": "webhooksUpdate",
            "security": secured(),
            "parameters": [ uuid_path_param("id", "Webhook ID") ],
            "requestBody": json_body("CreateWebhookRequest"),
            "responses": {
                "200": data_response("Webhook updated", "WebhookConfig"),
                "404": error_response("Webhook not found")
            }
        },
        "delete": {
            "tags": ["Webhooks"],
            "summary": "Delete a webhook",
            "operationId": "webhooksDelete",
            "security": secured(),
            "parameters": [ uuid_path_param("id", "Webhook ID") ],
            "responses": {
                "204": { "description": "Webhook deleted" },
                "404": error_response("Webhook not found")
            }
        }
    })
}

fn webhooks_deliveries_path() -> Value {
    json!({
        "get": {
            "tags": ["Webhooks"],
            "summary": "List delivery history for a webhook",
            "operationId": "webhooksDeliveries",
            "security": secured(),
            "parameters": [
                uuid_path_param("id", "Webhook ID"),
                { "name": "limit", "in": "query", "schema": { "type": "integer", "default": 50 } },
                { "name": "status", "in": "query", "schema": { "type": "string", "enum": ["pending", "delivered", "failed"] } }
            ],
            "responses": {
                "200": json_list_response("Webhook deliveries", "WebhookDelivery"),
                "404": error_response("Webhook not found")
            }
        }
    })
}

fn webhooks_test_path() -> Value {
    json!({
        "post": {
            "tags": ["Webhooks"],
            "summary": "Send a test delivery to a webhook",
            "operationId": "webhooksTest",
            "security": secured(),
            "parameters": [ uuid_path_param("id", "Webhook ID") ],
            "requestBody": {
                "required": false,
                "content": {
                    "application/json": {
                        "schema": {
                            "type": "object",
                            "description": "Custom test payload",
                            "additionalProperties": true
                        }
                    }
                }
            },
            "responses": {
                "200": data_response("Test delivery result", "WebhookDelivery"),
                "404": error_response("Webhook not found")
            }
        }
    })
}

// ===========================================================================
// API Keys
// ===========================================================================

fn api_keys_collection_path() -> Value {
    json!({
        "get": {
            "tags": ["API Keys"],
            "summary": "List API keys for the current user",
            "operationId": "apiKeysList",
            "security": secured(),
            "responses": {
                "200": json_list_response("API keys", "ApiKeyConfig"),
                "401": error_response("Not authenticated")
            }
        },
        "post": {
            "tags": ["API Keys"],
            "summary": "Create a new API key",
            "operationId": "apiKeysCreate",
            "security": secured(),
            "requestBody": json_body("CreateApiKeyRequest"),
            "responses": {
                "201": json_response("API key created (key shown only once)", "ApiKeyCreated"),
                "400": error_response("Invalid request")
            }
        }
    })
}

fn api_keys_item_path() -> Value {
    json!({
        "get": {
            "tags": ["API Keys"],
            "summary": "Get an API key by ID",
            "operationId": "apiKeysGet",
            "security": secured(),
            "parameters": [ uuid_path_param("id", "API key ID") ],
            "responses": {
                "200": data_response("API key (without secret)", "ApiKeyConfig"),
                "404": error_response("API key not found")
            }
        },
        "delete": {
            "tags": ["API Keys"],
            "summary": "Revoke an API key",
            "operationId": "apiKeysRevoke",
            "security": secured(),
            "parameters": [ uuid_path_param("id", "API key ID") ],
            "responses": {
                "204": { "description": "API key revoked" },
                "404": error_response("API key not found")
            }
        }
    })
}

fn api_keys_rotate_path() -> Value {
    json!({
        "post": {
            "tags": ["API Keys"],
            "summary": "Rotate an API key (revoke old, issue new)",
            "operationId": "apiKeysRotate",
            "security": secured(),
            "parameters": [ uuid_path_param("id", "API key ID") ],
            "responses": {
                "200": json_response("New API key (shown only once)", "ApiKeyCreated"),
                "404": error_response("API key not found")
            }
        }
    })
}

// ===========================================================================
// Admin
// ===========================================================================

fn admin_schema_path() -> Value {
    json!({
        "get": {
            "tags": ["Admin"],
            "summary": "Get the current inferred schema",
            "operationId": "adminSchema",
            "security": secured(),
            "responses": {
                "200": { "description": "Schema snapshot" },
                "403": error_response("Admin access required")
            }
        }
    })
}

fn admin_functions_path() -> Value {
    json!({
        "get": {
            "tags": ["Admin"],
            "summary": "List registered server-side functions",
            "operationId": "adminFunctions",
            "security": secured(),
            "responses": {
                "200": { "description": "Function registry" },
                "403": error_response("Admin access required")
            }
        }
    })
}

fn admin_sessions_path() -> Value {
    json!({
        "get": {
            "tags": ["Admin"],
            "summary": "List active sync sessions",
            "operationId": "adminSessions",
            "security": secured(),
            "responses": {
                "200": { "description": "Active sessions" },
                "403": error_response("Admin access required")
            }
        }
    })
}

fn admin_bulk_load_path() -> Value {
    json!({
        "post": {
            "tags": ["Admin"],
            "summary": "Bulk load triples into the store",
            "operationId": "adminBulkLoad",
            "security": secured(),
            "requestBody": {
                "required": true,
                "content": {
                    "application/json": {
                        "schema": {
                            "type": "object",
                            "required": ["triples"],
                            "properties": {
                                "triples": {
                                    "type": "array",
                                    "items": {
                                        "type": "object",
                                        "required": ["entity_id", "attribute", "value"],
                                        "properties": {
                                            "entity_id": { "type": "string", "format": "uuid" },
                                            "attribute": { "type": "string" },
                                            "value": {},
                                            "value_type": { "type": "integer" },
                                            "ttl_seconds": { "type": "integer", "nullable": true }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            },
            "responses": {
                "200": { "description": "Bulk load result", "content": { "application/json": { "schema": { "type": "object", "properties": { "loaded": { "type": "integer" }, "tx_id": { "type": "integer" } } } } } },
                "400": error_response("Invalid triple data"),
                "403": error_response("Admin access required")
            }
        }
    })
}

fn admin_cache_path() -> Value {
    json!({
        "get": {
            "tags": ["Admin"],
            "summary": "Get cache statistics",
            "operationId": "adminCache",
            "security": secured(),
            "responses": {
                "200": { "description": "Cache statistics" },
                "403": error_response("Admin access required")
            }
        }
    })
}

// ===========================================================================
// Batch
// ===========================================================================

fn batch_path() -> Value {
    json!({
        "post": {
            "tags": ["Batch"],
            "summary": "Execute a batch of operations sequentially",
            "operationId": "batch",
            "security": secured(),
            "requestBody": {
                "required": true,
                "content": {
                    "application/json": {
                        "schema": {
                            "type": "object",
                            "required": ["operations"],
                            "properties": {
                                "operations": {
                                    "type": "array",
                                    "items": {
                                        "type": "object",
                                        "required": ["method", "path"],
                                        "properties": {
                                            "method": { "type": "string", "enum": ["GET", "POST", "PATCH", "DELETE"] },
                                            "path": { "type": "string" },
                                            "body": { "type": "object", "additionalProperties": true }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            },
            "responses": {
                "200": {
                    "description": "Batch results",
                    "content": {
                        "application/json": {
                            "schema": {
                                "type": "object",
                                "properties": {
                                    "results": {
                                        "type": "array",
                                        "items": {
                                            "type": "object",
                                            "properties": {
                                                "status": { "type": "integer" },
                                                "body": {}
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                },
                "401": error_response("Not authenticated")
            }
        }
    })
}

fn batch_parallel_path() -> Value {
    json!({
        "post": {
            "tags": ["Batch"],
            "summary": "Execute a batch of operations in parallel",
            "operationId": "batchParallel",
            "description": "Uses Solana-inspired parallel execution for non-conflicting operations.",
            "security": secured(),
            "requestBody": {
                "required": true,
                "content": {
                    "application/json": {
                        "schema": {
                            "type": "object",
                            "required": ["operations"],
                            "properties": {
                                "operations": {
                                    "type": "array",
                                    "items": {
                                        "type": "object",
                                        "required": ["method", "path"],
                                        "properties": {
                                            "method": { "type": "string" },
                                            "path": { "type": "string" },
                                            "body": { "type": "object", "additionalProperties": true }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            },
            "responses": {
                "200": { "description": "Parallel batch results" },
                "401": error_response("Not authenticated")
            }
        }
    })
}

fn batch_metrics_path() -> Value {
    json!({
        "get": {
            "tags": ["Batch"],
            "summary": "Get parallel batch execution metrics",
            "operationId": "batchMetrics",
            "security": secured(),
            "responses": {
                "200": { "description": "Parallel execution metrics" },
                "401": error_response("Not authenticated")
            }
        }
    })
}
