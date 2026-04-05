//! OpenAPI 3.1 specification auto-generation for DarshJDB.
//!
//! Builds the complete API spec at startup and serves it as JSON at
//! `GET /api/openapi.json`. The companion docs endpoint (`GET /api/docs`)
//! renders an interactive Scalar viewer.

use serde_json::{Value, json};

/// Generate the full OpenAPI 3.1 specification for DarshJDB.
///
/// This is computed once at server startup and cached in application state.
pub fn generate_openapi_spec() -> Value {
    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "DarshJDB API",
            "description": "Triple-store database with real-time subscriptions, auth, and server-side functions.",
            "version": "0.1.0",
            "license": {
                "name": "MIT",
                "url": "https://opensource.org/licenses/MIT"
            }
        },
        "servers": [
            {
                "url": "/api",
                "description": "Current server"
            }
        ],
        "components": {
            "securitySchemes": {
                "bearerAuth": {
                    "type": "http",
                    "scheme": "bearer",
                    "bearerFormat": "JWT"
                }
            },
            "schemas": {
                "ErrorResponse": error_schema(),
                "TokenPair": token_pair_schema(),
                "QueryRequest": query_request_schema(),
                "MutateRequest": mutate_request_schema(),
                "UserProfile": user_profile_schema(),
                "UploadResponse": upload_response_schema(),
            }
        },
        "paths": {
            "/auth/signup": auth_signup_path(),
            "/auth/signin": auth_signin_path(),
            "/auth/magic-link": auth_magic_link_path(),
            "/auth/verify": auth_verify_path(),
            "/auth/oauth/{provider}": auth_oauth_path(),
            "/auth/refresh": auth_refresh_path(),
            "/auth/signout": auth_signout_path(),
            "/auth/me": auth_me_path(),
            "/query": query_path(),
            "/mutate": mutate_path(),
            "/data/{entity}": data_collection_path(),
            "/data/{entity}/{id}": data_item_path(),
            "/fn/{name}": function_path(),
            "/storage/upload": storage_upload_path(),
            "/storage/{path}": storage_item_path(),
            "/subscribe": subscribe_path(),
            "/events": events_path(),
            "/events/publish": events_publish_path(),
            "/admin/schema": admin_schema_path(),
            "/admin/functions": admin_functions_path(),
            "/admin/sessions": admin_sessions_path(),
        }
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

// ---------------------------------------------------------------------------
// Schema helpers
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

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
                    "schema": { "type": "string", "enum": ["google", "github", "apple"] }
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
            "security": [{ "bearerAuth": [] }],
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
            "security": [{ "bearerAuth": [] }],
            "responses": {
                "200": json_response("Current user", "UserProfile"),
                "401": error_response("Not authenticated")
            }
        }
    })
}

fn query_path() -> Value {
    json!({
        "post": {
            "tags": ["Data"],
            "summary": "Execute a DarshJQL query",
            "operationId": "query",
            "security": [{ "bearerAuth": [] }],
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
            "security": [{ "bearerAuth": [] }],
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
            "security": [{ "bearerAuth": [] }],
            "parameters": [
                { "name": "entity", "in": "path", "required": true, "schema": { "type": "string" } },
                { "name": "limit", "in": "query", "schema": { "type": "integer", "default": 50 } },
                { "name": "cursor", "in": "query", "schema": { "type": "string" } }
            ],
            "responses": {
                "200": { "description": "Entity list" },
                "401": error_response("Not authenticated")
            }
        },
        "post": {
            "tags": ["Data"],
            "summary": "Create a new entity",
            "operationId": "dataCreate",
            "security": [{ "bearerAuth": [] }],
            "parameters": [
                { "name": "entity", "in": "path", "required": true, "schema": { "type": "string" } }
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
            "security": [{ "bearerAuth": [] }],
            "parameters": [
                { "name": "entity", "in": "path", "required": true, "schema": { "type": "string" } },
                { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }
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
            "security": [{ "bearerAuth": [] }],
            "parameters": [
                { "name": "entity", "in": "path", "required": true, "schema": { "type": "string" } },
                { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }
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
            "security": [{ "bearerAuth": [] }],
            "parameters": [
                { "name": "entity", "in": "path", "required": true, "schema": { "type": "string" } },
                { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } }
            ],
            "responses": {
                "204": { "description": "Deleted" },
                "404": error_response("Not found")
            }
        }
    })
}

fn function_path() -> Value {
    json!({
        "post": {
            "tags": ["Functions"],
            "summary": "Invoke a server-side function",
            "operationId": "functionInvoke",
            "security": [{ "bearerAuth": [] }],
            "parameters": [
                { "name": "name", "in": "path", "required": true, "schema": { "type": "string" } }
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

fn storage_upload_path() -> Value {
    json!({
        "post": {
            "tags": ["Storage"],
            "summary": "Upload a file",
            "operationId": "storageUpload",
            "security": [{ "bearerAuth": [] }],
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
                { "name": "path", "in": "path", "required": true, "schema": { "type": "string" } },
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
            "security": [{ "bearerAuth": [] }],
            "parameters": [
                { "name": "path", "in": "path", "required": true, "schema": { "type": "string" } }
            ],
            "responses": {
                "204": { "description": "Deleted" },
                "404": error_response("File not found")
            }
        }
    })
}

fn subscribe_path() -> Value {
    json!({
        "get": {
            "tags": ["Realtime"],
            "summary": "Subscribe to live query updates via Server-Sent Events",
            "operationId": "subscribe",
            "security": [{ "bearerAuth": [] }],
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
                    "content": {
                        "text/event-stream": {
                            "schema": { "type": "string" }
                        }
                    }
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
            "description": "Streams keyspace notification events matching the given channel pattern. Supports glob patterns like `entity:users:*`, `mutation:*`, `auth:*`.",
            "security": [{ "bearerAuth": [] }],
            "parameters": [
                {
                    "name": "channel",
                    "in": "query",
                    "required": true,
                    "schema": { "type": "string" },
                    "description": "Channel pattern to subscribe to (e.g., `entity:users:*`)"
                }
            ],
            "responses": {
                "200": {
                    "description": "SSE stream of matching pub/sub events",
                    "content": {
                        "text/event-stream": {
                            "schema": { "type": "string" }
                        }
                    }
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
            "description": "Broadcasts a custom event to all subscribers matching the given channel. Used for webhooks, notifications, or inter-service communication.",
            "security": [{ "bearerAuth": [] }],
            "requestBody": {
                "required": true,
                "content": {
                    "application/json": {
                        "schema": {
                            "type": "object",
                            "required": ["channel", "event"],
                            "properties": {
                                "channel": {
                                    "type": "string",
                                    "description": "Channel to publish to (e.g., `custom:notifications`)"
                                },
                                "event": {
                                    "type": "string",
                                    "description": "Event name (e.g., `new-message`)"
                                },
                                "payload": {
                                    "description": "Optional event payload",
                                    "nullable": true
                                }
                            }
                        }
                    }
                }
            },
            "responses": {
                "200": {
                    "description": "Event published successfully",
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

fn admin_schema_path() -> Value {
    json!({
        "get": {
            "tags": ["Admin"],
            "summary": "Get the current inferred schema",
            "operationId": "adminSchema",
            "security": [{ "bearerAuth": [] }],
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
            "security": [{ "bearerAuth": [] }],
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
            "security": [{ "bearerAuth": [] }],
            "responses": {
                "200": { "description": "Active sessions" },
                "403": error_response("Admin access required")
            }
        }
    })
}
