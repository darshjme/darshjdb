# API Keys

DarshJDB API keys provide programmatic access to the database without user-session tokens. Keys follow the format `ddb_key_<64-hex-chars>`, are generated with 256 bits of randomness, and only the SHA-256 hash is stored in the database. The raw key is returned exactly once at creation time.

## Creating an API Key

```bash
curl -X POST http://localhost:3000/api/keys \
  -H "Authorization: Bearer <jwt-token>" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "Production Backend",
    "scopes": ["Write"],
    "rate_limit": 1000,
    "expires_at": "2027-01-01T00:00:00Z"
  }'
```

Response (201 Created):

```json
{
  "id": "550e8400-e29b-41d4-a716-446655440000",
  "key": "ddb_key_a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2",
  "key_prefix": "ddb_key_a1b2c3d4",
  "name": "Production Backend",
  "scopes": ["Write"],
  "created_at": "2026-04-07T10:00:00Z"
}
```

Store the `key` value immediately. It cannot be retrieved again -- only the first 16 characters (`key_prefix`) are stored for display purposes.

## Scopes

Each API key carries one or more scopes that restrict its capabilities:

| Scope | Permissions |
|-------|-------------|
| `Read` | Read access to all tables (default if no scope specified) |
| `Write` | Read and write access to all tables |
| `Admin` | Full access to all operations including management endpoints |
| `Tables(["users", "orders"])` | Access restricted to the named tables only (any operation) |
| `Custom("webhook")` | Named custom scope for extensibility; requires explicit checks in plugin code |

Scope evaluation rules:
- `Admin` permits everything.
- `Write` permits `read` and `write` operations.
- `Read` permits only `read` operations.
- `Tables` restricts by table name regardless of operation type.
- `Custom` denies by default -- it exists as a marker for plugin-level authorization logic.

Multiple scopes can be assigned to a single key. The key is authorized if **any** scope permits the operation.

## Using API Keys

API keys are accepted via two mechanisms:

### Authorization Header

```bash
curl http://localhost:3000/api/tables/users \
  -H "Authorization: Bearer ddb_key_a1b2c3d4..."
```

### X-API-Key Header

```bash
curl http://localhost:3000/api/tables/users \
  -H "X-API-Key: ddb_key_a1b2c3d4..."
```

When a valid API key is presented, the auth middleware constructs an `ApiKeyAuth` context containing the key's ID, name, scopes, owner, and rate limit. This is used for authorization decisions throughout the request lifecycle.

## Key Rotation

Rotate a key to revoke the old one and issue a new key with the same name, scopes, rate limit, and expiry:

```bash
curl -X POST http://localhost:3000/api/keys/{id}/rotate \
  -H "Authorization: Bearer <jwt-token>"
```

Response:

```json
{
  "id": "new-key-uuid",
  "key": "ddb_key_new_random_hex...",
  "key_prefix": "ddb_key_f7e8d9c0"
}
```

The old key is immediately revoked. The new key inherits all attributes of the old key.

## Rate Limiting

Each key can optionally specify a `rate_limit` (requests per minute). When set, the server enforces this limit per key. Keys without a rate limit use the server's global rate limit.

```json
{
  "name": "Rate-limited key",
  "scopes": ["Read"],
  "rate_limit": 60
}
```

## Expiry

Keys can be created with an `expires_at` timestamp. Expired keys are rejected during validation with no grace period. Keys without an expiry never expire.

```json
{
  "name": "Temporary key",
  "scopes": ["Read"],
  "expires_at": "2026-05-01T00:00:00Z"
}
```

## Security Model

- **256-bit entropy**: Keys are generated using `OsRng` with 32 bytes of randomness.
- **Hash-only storage**: Only the SHA-256 hash is persisted. The raw key cannot be recovered from the database.
- **Constant-time validation**: Key lookup uses the SHA-256 hash, preventing timing attacks on key enumeration.
- **Prefix display**: The `key_prefix` (first 16 characters including `ddb_key_`) is stored for UI identification.
- **Last-used tracking**: The `last_used_at` timestamp is updated asynchronously (fire-and-forget) on each successful authentication.
- **Serialization safety**: The `key_hash` field is marked `#[serde(skip_serializing)]` and never appears in API responses.

## API Reference

### List API keys

```bash
curl http://localhost:3000/api/keys \
  -H "Authorization: Bearer <token>"
```

Response:

```json
{
  "keys": [
    {
      "id": "uuid",
      "name": "Production Backend",
      "key_prefix": "ddb_key_a1b2c3d4",
      "scopes": ["Write"],
      "rate_limit": 1000,
      "expires_at": "2027-01-01T00:00:00Z",
      "created_by": "user-uuid",
      "created_at": "2026-04-07T10:00:00Z",
      "last_used_at": "2026-04-07T12:30:00Z",
      "revoked": false
    }
  ]
}
```

Admins see all keys. Non-admin users see only their own. The raw key and key hash are never returned.

### Revoke an API key

```bash
curl -X DELETE http://localhost:3000/api/keys/{id} \
  -H "Authorization: Bearer <token>"
```

Response:

```json
{ "revoked": true }
```

Revoked keys are immediately rejected on subsequent authentication attempts. Revocation is permanent.

### Rotate an API key

```bash
curl -X POST http://localhost:3000/api/keys/{id}/rotate \
  -H "Authorization: Bearer <token>"
```

Atomically revokes the old key and issues a new one with identical attributes.
