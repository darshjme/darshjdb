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
