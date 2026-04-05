# REST API Reference

All endpoints are available at `http://localhost:7700/api/`.

## Authentication

### Sign Up
```bash
curl -X POST http://localhost:7700/api/auth/signup \
  -H "Content-Type: application/json" \
  -d '{"email": "user@example.com", "password": "SecurePass123!"}'
```

### Sign In
```bash
curl -X POST http://localhost:7700/api/auth/signin \
  -H "Content-Type: application/json" \
  -d '{"email": "user@example.com", "password": "SecurePass123!"}'
# Returns: { "accessToken": "...", "refreshToken": "..." }
```

### Get Current User
```bash
curl http://localhost:7700/api/auth/me \
  -H "Authorization: Bearer ACCESS_TOKEN"
```

### Refresh Token
```bash
curl -X POST http://localhost:7700/api/auth/refresh \
  -H "Content-Type: application/json" \
  -d '{"refreshToken": "..."}'
```

## Data Queries

### DarshanQL Query
```bash
curl -X POST http://localhost:7700/api/query \
  -H "Authorization: Bearer TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"todos": {"$where": {"done": false}, "$order": {"createdAt": "desc"}, "$limit": 20}}'
```

### REST-style CRUD

```bash
# List all todos
curl http://localhost:7700/api/data/todos -H "Authorization: Bearer TOKEN"

# Get one todo
curl http://localhost:7700/api/data/todos/UUID -H "Authorization: Bearer TOKEN"

# Create a todo
curl -X POST http://localhost:7700/api/data/todos \
  -H "Authorization: Bearer TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"title": "Buy groceries", "done": false}'

# Update a todo
curl -X PATCH http://localhost:7700/api/data/todos/UUID \
  -H "Authorization: Bearer TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"done": true}'

# Delete a todo
curl -X DELETE http://localhost:7700/api/data/todos/UUID \
  -H "Authorization: Bearer TOKEN"
```

## Mutations (Transactional)

```bash
curl -X POST http://localhost:7700/api/mutate \
  -H "Authorization: Bearer TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"ops": [{"entity": "todos", "id": "new-uuid", "op": "set", "data": {"title": "New", "done": false}}]}'
```

## Server Functions

```bash
curl -X POST http://localhost:7700/api/fn/createTodo \
  -H "Authorization: Bearer TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"title": "Buy milk", "listId": "list-1"}'
```

## Storage

```bash
# Upload
curl -X POST http://localhost:7700/api/storage/upload \
  -H "Authorization: Bearer TOKEN" \
  -F "file=@photo.jpg" \
  -F "path=images/photo.jpg"

# Get signed URL
curl http://localhost:7700/api/storage/images/photo.jpg \
  -H "Authorization: Bearer TOKEN"

# Delete
curl -X DELETE http://localhost:7700/api/storage/images/photo.jpg \
  -H "Authorization: Bearer TOKEN"
```

## Server-Sent Events (Real-Time over HTTP)

```bash
curl -N http://localhost:7700/api/subscribe?q=%7B%22todos%22%3A%7B%7D%7D \
  -H "Authorization: Bearer TOKEN" \
  -H "Accept: text/event-stream"
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

## WebSocket API

Connect to `ws://localhost:7700/ws` for real-time subscriptions:

```javascript
const ws = new WebSocket('ws://localhost:7700/ws');

// Authenticate
ws.send(JSON.stringify({ type: 'auth', token: 'ACCESS_TOKEN' }));

// Subscribe to a query
ws.send(JSON.stringify({
  type: 'subscribe',
  id: 'sub-1',
  query: { todos: { $where: { done: false } } }
}));

// Receive initial data
// { type: "q-init", id: "sub-1", data: { todos: [...] }, tx: 42 }

// Receive live updates
// { type: "q-diff", id: "sub-1", added: [...], updated: [...], removed: [...], tx: 43 }

// Unsubscribe
ws.send(JSON.stringify({ type: 'unsubscribe', id: 'sub-1' }));
```

## Admin Endpoints

All admin endpoints require the admin token.

```bash
# Health check
curl http://localhost:7700/api/admin/health

# Server stats
curl http://localhost:7700/api/admin/stats \
  -H "Authorization: Bearer ADMIN_TOKEN"

# List all entities (schema introspection)
curl http://localhost:7700/api/admin/schema \
  -H "Authorization: Bearer ADMIN_TOKEN"
```

## HTTP Status Codes

| Code | Meaning |
|------|---------|
| `200` | Success |
| `201` | Created (new entity) |
| `400` | Bad request (validation error) |
| `401` | Unauthorized (missing or invalid token) |
| `403` | Forbidden (permission denied) |
| `404` | Not found |
| `409` | Conflict (duplicate key, concurrent edit) |
| `429` | Rate limited |
| `500` | Internal server error |

---

[Previous: Permissions](permissions.md) | [Next: Security](security.md) | [All Docs](README.md)
