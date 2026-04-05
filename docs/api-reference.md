# REST API Reference

All endpoints are available at `http://localhost:7700/api/`.

## Authentication

### Sign Up

```bash
curl -X POST http://localhost:7700/api/auth/signup \
  -H "Content-Type: application/json" \
  -d '{"email": "user@example.com", "password": "SecurePass123!"}'
```

**Response (201 Created):**
```json
{
  "user": {
    "id": "usr_abc123",
    "email": "user@example.com",
    "createdAt": "2026-04-05T12:00:00Z"
  },
  "accessToken": "eyJhbGciOiJSUzI1NiIs...",
  "refreshToken": "dGhpcyBpcyBhIHJlZnJl..."
}
```

### Sign In

```bash
curl -X POST http://localhost:7700/api/auth/signin \
  -H "Content-Type: application/json" \
  -d '{"email": "user@example.com", "password": "SecurePass123!"}'
```

**Response (200 OK):**
```json
{
  "accessToken": "eyJhbGciOiJSUzI1NiIs...",
  "refreshToken": "dGhpcyBpcyBhIHJlZnJl..."
}
```

**Response (200 OK, MFA required):**
```json
{
  "mfaRequired": true,
  "mfaToken": "mfa_temp_token_abc..."
}
```

**Response (401 Unauthorized):**
```json
{
  "error": {
    "code": "INVALID_CREDENTIALS",
    "message": "Invalid email or password",
    "status": 401
  }
}
```

**Response (429 Too Many Requests):**
```json
{
  "error": {
    "code": "ACCOUNT_LOCKED",
    "message": "Account locked due to too many failed attempts. Try again in 28 minutes.",
    "status": 429,
    "retryAfter": 1680
  }
}
```

### MFA Verification

```bash
curl -X POST http://localhost:7700/api/auth/mfa/verify \
  -H "Content-Type: application/json" \
  -d '{"mfaToken": "mfa_temp_token_abc...", "code": "123456"}'
```

**Response (200 OK):**
```json
{
  "accessToken": "eyJhbGciOiJSUzI1NiIs...",
  "refreshToken": "dGhpcyBpcyBhIHJlZnJl..."
}
```

### Get Current User

```bash
curl http://localhost:7700/api/auth/me \
  -H "Authorization: Bearer ACCESS_TOKEN"
```

**Response (200 OK):**
```json
{
  "id": "usr_abc123",
  "email": "user@example.com",
  "role": "editor",
  "claims": { "plan": "pro", "organizationId": "org_xyz" },
  "mfaEnabled": true,
  "createdAt": "2026-04-05T12:00:00Z"
}
```

### Refresh Token

```bash
curl -X POST http://localhost:7700/api/auth/refresh \
  -H "Content-Type: application/json" \
  -d '{"refreshToken": "dGhpcyBpcyBhIHJlZnJl..."}'
```

**Response (200 OK):**
```json
{
  "accessToken": "eyJhbGciOiJSUzI1NiIs...(new)",
  "refreshToken": "bmV3IHJlZnJlc2ggdG9r...(new, old one invalidated)"
}
```

### Sign Out

```bash
curl -X POST http://localhost:7700/api/auth/signout \
  -H "Authorization: Bearer ACCESS_TOKEN"
```

**Response (204 No Content)**

### List Sessions

```bash
curl http://localhost:7700/api/auth/sessions \
  -H "Authorization: Bearer ACCESS_TOKEN"
```

**Response (200 OK):**
```json
{
  "sessions": [
    {
      "id": "sess_abc",
      "device": "Chrome on macOS",
      "ip": "1.2.3.4",
      "lastUsedAt": "2026-04-05T12:00:00Z",
      "current": true
    }
  ]
}
```

## Data Queries

### DarshanQL Query

```bash
curl -X POST http://localhost:7700/api/query \
  -H "Authorization: Bearer TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"todos": {"$where": {"done": false}, "$order": {"createdAt": "desc"}, "$limit": 20}}'
```

**Response (200 OK):**
```json
{
  "todos": [
    {
      "id": "todo_abc",
      "title": "Buy groceries",
      "done": false,
      "priority": 3,
      "createdAt": 1712300000000,
      "userId": "usr_abc123"
    }
  ],
  "tx": 42
}
```

### REST-style CRUD

```bash
# List all todos
curl http://localhost:7700/api/data/todos \
  -H "Authorization: Bearer TOKEN"

# Get one todo
curl http://localhost:7700/api/data/todos/UUID \
  -H "Authorization: Bearer TOKEN"

# Create a todo
curl -X POST http://localhost:7700/api/data/todos \
  -H "Authorization: Bearer TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"title": "Buy groceries", "done": false}'

# Update a todo (partial merge)
curl -X PATCH http://localhost:7700/api/data/todos/UUID \
  -H "Authorization: Bearer TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"done": true}'

# Delete a todo
curl -X DELETE http://localhost:7700/api/data/todos/UUID \
  -H "Authorization: Bearer TOKEN"
```

**List Response (200 OK):**
```json
{
  "data": [
    { "id": "todo_abc", "title": "Buy groceries", "done": false }
  ],
  "total": 42,
  "hasMore": true
}
```

**Get Response (200 OK):**
```json
{
  "data": { "id": "todo_abc", "title": "Buy groceries", "done": false }
}
```

**Create Response (201 Created):**
```json
{
  "data": { "id": "todo_new", "title": "Buy groceries", "done": false },
  "tx": 43
}
```

**Update Response (200 OK):**
```json
{
  "data": { "id": "todo_abc", "done": true },
  "tx": 44
}
```

**Delete Response (204 No Content)**

## Mutations (Transactional)

```bash
curl -X POST http://localhost:7700/api/mutate \
  -H "Authorization: Bearer TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "ops": [
      {"entity": "todos", "id": "new-uuid", "op": "set", "data": {"title": "New", "done": false}},
      {"entity": "todos", "id": "existing-uuid", "op": "merge", "data": {"done": true}},
      {"entity": "todos", "id": "old-uuid", "op": "delete"}
    ]
  }'
```

**Response (200 OK):**
```json
{
  "tx": 45,
  "results": [
    { "op": "set", "id": "new-uuid", "status": "created" },
    { "op": "merge", "id": "existing-uuid", "status": "updated" },
    { "op": "delete", "id": "old-uuid", "status": "deleted" }
  ]
}
```

## Server Functions

```bash
curl -X POST http://localhost:7700/api/fn/createTodo \
  -H "Authorization: Bearer TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"title": "Buy milk", "listId": "list-1"}'
```

**Response (200 OK):**
```json
{
  "result": "todo_new_id",
  "tx": 46
}
```

**Response (400 Bad Request, validation error):**
```json
{
  "error": {
    "code": "INVALID_ARGUMENT",
    "message": "Validation failed",
    "status": 400,
    "details": [
      { "field": "title", "message": "String must be at least 1 character" }
    ]
  }
}
```

## Storage

```bash
# Upload
curl -X POST http://localhost:7700/api/storage/upload \
  -H "Authorization: Bearer TOKEN" \
  -F "file=@photo.jpg" \
  -F "path=images/photo.jpg"
```

**Upload Response (201 Created):**
```json
{
  "path": "images/photo.jpg",
  "url": "https://your-bucket.s3.amazonaws.com/images/photo.jpg?X-Amz-Signature=...",
  "size": 245760,
  "contentType": "image/jpeg"
}
```

```bash
# Get signed URL
curl http://localhost:7700/api/storage/images/photo.jpg \
  -H "Authorization: Bearer TOKEN"
```

**Response (200 OK):**
```json
{
  "url": "https://your-bucket.s3.amazonaws.com/images/photo.jpg?X-Amz-Signature=...",
  "expiresAt": "2026-04-05T13:00:00Z"
}
```

```bash
# List files
curl "http://localhost:7700/api/storage?prefix=images/&limit=50" \
  -H "Authorization: Bearer TOKEN"
```

**Response (200 OK):**
```json
{
  "files": [
    { "path": "images/photo.jpg", "size": 245760, "contentType": "image/jpeg", "updatedAt": "2026-04-05T12:00:00Z" }
  ],
  "hasMore": false
}
```

```bash
# Delete
curl -X DELETE http://localhost:7700/api/storage/images/photo.jpg \
  -H "Authorization: Bearer TOKEN"
```

**Response (204 No Content)**

## Presence

```bash
# Get all peers in a room
curl http://localhost:7700/api/presence/document-123 \
  -H "Authorization: Bearer TOKEN"
```

**Response (200 OK):**
```json
{
  "room": "document-123",
  "peers": [
    { "id": "conn-1", "data": { "name": "Darsh", "cursor": { "x": 120, "y": 340 } } },
    { "id": "conn-2", "data": { "name": "Alex", "cursor": { "x": 50, "y": 100 } } }
  ]
}
```

## Server-Sent Events (Real-Time over HTTP)

For clients that cannot use WebSocket, use SSE as a fallback:

```bash
curl -N "http://localhost:7700/api/subscribe?q=%7B%22todos%22%3A%7B%7D%7D" \
  -H "Authorization: Bearer TOKEN" \
  -H "Accept: text/event-stream"
```

**Event Stream:**
```
event: q-init
data: {"id":"sub-1","data":{"todos":[...]},"tx":42}

event: q-diff
data: {"id":"sub-1","added":[],"updated":[{"id":"todo-abc","done":true}],"removed":[],"tx":43}
```

## WebSocket API

Connect to `ws://localhost:7700/ws` for real-time subscriptions.

### Protocol

```javascript
const ws = new WebSocket('ws://localhost:7700/ws');

// 1. Authenticate (required before any other message)
ws.send(JSON.stringify({ type: 'auth', token: 'ACCESS_TOKEN' }));
// Response: { type: "auth-ok", userId: "usr_abc" }
// Or:       { type: "auth-error", message: "Invalid token" }

// 2. Subscribe to a query
ws.send(JSON.stringify({
  type: 'subscribe',
  id: 'sub-1',
  query: { todos: { $where: { done: false } } }
}));
// Response: { type: "q-init", id: "sub-1", data: { todos: [...] }, tx: 42 }

// 3. Receive live updates
// { type: "q-diff", id: "sub-1", added: [...], updated: [...], removed: [...], tx: 43 }

// 4. Send a mutation
ws.send(JSON.stringify({
  type: 'mutate',
  id: 'mut-1',
  ops: [{ entity: 'todos', id: 'new-uuid', op: 'set', data: { title: 'New', done: false } }]
}));
// Response: { type: "mutate-ok", id: "mut-1", tx: 44 }
// Or:       { type: "mutate-error", id: "mut-1", error: { code: "PERMISSION_DENIED", message: "..." } }

// 5. Unsubscribe
ws.send(JSON.stringify({ type: 'unsubscribe', id: 'sub-1' }));
// Response: { type: "unsubscribe-ok", id: "sub-1" }

// 6. Heartbeat (sent automatically by the server every 30s)
// { type: "ping" }
// Client should respond: { type: "pong" }
```

### WebSocket Message Types

| Client Message | Description |
|---------------|-------------|
| `auth` | Authenticate with access token |
| `subscribe` | Start a live query subscription |
| `unsubscribe` | Stop a subscription |
| `mutate` | Execute a transactional mutation |
| `pong` | Response to server ping |
| `presence-enter` | Join a presence room |
| `presence-update` | Update presence data |
| `presence-leave` | Leave a presence room |

| Server Message | Description |
|---------------|-------------|
| `auth-ok` | Authentication successful |
| `auth-error` | Authentication failed |
| `q-init` | Initial query result |
| `q-diff` | Incremental query update |
| `mutate-ok` | Mutation succeeded |
| `mutate-error` | Mutation failed |
| `unsubscribe-ok` | Subscription removed |
| `ping` | Heartbeat (client must respond with pong) |
| `presence-change` | Presence update in a room |

## Admin Endpoints

All admin endpoints require the admin token.

```bash
# Health check (no auth required)
curl http://localhost:7700/api/admin/health
```

**Response (200 OK):**
```json
{
  "status": "healthy",
  "version": "0.1.0",
  "uptime": 86400,
  "postgres": "connected",
  "connections": { "websocket": 42, "http": 3 },
  "protocolVersion": 1
}
```

```bash
# Server stats
curl http://localhost:7700/api/admin/stats \
  -H "Authorization: Bearer ADMIN_TOKEN"
```

**Response (200 OK):**
```json
{
  "queries": { "total": 150000, "perSecond": 1200, "avgLatencyMs": 1.4 },
  "mutations": { "total": 45000, "perSecond": 80, "avgLatencyMs": 3.2 },
  "subscriptions": { "active": 320 },
  "storage": { "totalFiles": 1500, "totalBytes": 2147483648 },
  "users": { "total": 500, "activeToday": 120 }
}
```

```bash
# Schema introspection
curl http://localhost:7700/api/admin/schema \
  -H "Authorization: Bearer ADMIN_TOKEN"
```

**Response (200 OK):**
```json
{
  "entities": [
    {
      "name": "todos",
      "count": 1500,
      "attributes": [
        { "name": "title", "type": "string", "count": 1500 },
        { "name": "done", "type": "boolean", "count": 1500 },
        { "name": "priority", "type": "number", "count": 800 }
      ],
      "relations": [
        { "name": "userId", "target": "users", "count": 1500 }
      ]
    }
  ]
}
```

## Error Format

All errors follow this format:

```json
{
  "error": {
    "code": "PERMISSION_DENIED",
    "message": "You do not have read access to users.email",
    "status": 403
  }
}
```

### Error Codes

| Code | HTTP Status | Description |
|------|-------------|-------------|
| `INVALID_ARGUMENT` | 400 | Validation error, malformed request |
| `INVALID_CREDENTIALS` | 401 | Wrong email or password |
| `UNAUTHENTICATED` | 401 | Missing or expired token |
| `PERMISSION_DENIED` | 403 | Insufficient permissions |
| `NOT_FOUND` | 404 | Entity or endpoint not found |
| `CONFLICT` | 409 | Duplicate key, concurrent edit |
| `QUERY_TOO_COMPLEX` | 400 | Query exceeds depth or result limits |
| `RESOURCE_EXCEEDED` | 400 | Server function exceeded CPU/memory |
| `ACCOUNT_LOCKED` | 429 | Too many failed login attempts |
| `RATE_LIMITED` | 429 | Rate limit exceeded |
| `INTERNAL` | 500 | Unexpected server error |

## Rate Limit Headers

Every response includes:
- `X-RateLimit-Limit`: requests allowed per window
- `X-RateLimit-Remaining`: requests remaining
- `X-RateLimit-Reset`: Unix timestamp when window resets

## OpenAPI Spec

```bash
# JSON spec
curl http://localhost:7700/api/openapi.json

# Swagger UI
open http://localhost:7700/api/docs
```

## HTTP Status Codes

| Code | Meaning |
|------|---------|
| `200` | Success |
| `201` | Created (new entity or file) |
| `204` | No Content (successful delete or signout) |
| `400` | Bad request (validation error, query too complex) |
| `401` | Unauthorized (missing or invalid token) |
| `403` | Forbidden (permission denied) |
| `404` | Not found |
| `409` | Conflict (duplicate key, concurrent edit) |
| `413` | Payload too large (file upload exceeds limit) |
| `415` | Unsupported media type (file type not allowed) |
| `429` | Rate limited or account locked |
| `500` | Internal server error |

---

[Previous: Permissions](permissions.md) | [Next: Security](security.md) | [All Docs](README.md)
