# Troubleshooting

Common errors and their solutions when working with DarshanDB.

## Installation Issues

### `darshan: command not found`

The binary was not added to your PATH.

```bash
# Check where it was installed
ls ~/.darshan/bin/darshan

# Add to PATH (add to ~/.bashrc or ~/.zshrc)
export PATH="$HOME/.darshan/bin:$PATH"

# Or move to a system path
sudo mv ~/.darshan/bin/darshan /usr/local/bin/
```

### Docker compose fails with "port already in use"

Another service is using port 7700 or 5432.

```bash
# Find what's using the port
lsof -i :7700
lsof -i :5432

# Use different ports
DARSHAN_PORT=7701 docker compose up -d
# Or edit docker-compose.yml ports mapping
```

### Binary won't run on Linux

If you get "permission denied" or "cannot execute binary file":

```bash
chmod +x ~/.darshan/bin/darshan

# Check architecture (must match your CPU)
file ~/.darshan/bin/darshan
# Should show: ELF 64-bit LSB, x86-64 (or aarch64 for ARM)

uname -m
# Should match: x86_64 or aarch64
```

## Development Server

### `darshan dev` hangs on "Waiting for PostgreSQL..."

PostgreSQL is not running or not reachable.

```bash
# If using Docker:
docker ps | grep postgres
# If not running:
docker compose up -d postgres

# If using external Postgres, test the connection:
psql "$DATABASE_URL" -c "SELECT 1"

# Check if pgvector extension is available:
psql "$DATABASE_URL" -c "CREATE EXTENSION IF NOT EXISTS vector"
```

### "relation triples does not exist"

The database was not initialized. Run:

```bash
darshan migrate up

# Or drop and recreate the database:
darshan reset --force
```

### Hot reload not working

The file watcher may not detect changes on certain systems.

```bash
# macOS: increase file descriptor limit
ulimit -n 10240

# Linux: increase inotify watchers
echo fs.inotify.max_user_watches=524288 | sudo tee -a /etc/sysctl.conf
sudo sysctl -p
```

## Client SDK Issues

### CORS errors

**Symptom:** Browser console shows "Access to XMLHttpRequest has been blocked by CORS policy"

```bash
# Development: darshan dev allows localhost by default
# If using a different port:
DARSHAN_CORS_ORIGINS=http://localhost:3000 darshan dev

# Production: explicitly set allowed origins
DARSHAN_CORS_ORIGINS=https://myapp.com,https://admin.myapp.com darshan start --prod
```

### WebSocket connection drops

**Symptom:** `WebSocket connection to 'ws://...' failed` or frequent reconnections.

1. **Reverse proxy timeout:** Your nginx/Caddy/Cloudflare may be closing idle WebSocket connections.

```nginx
# nginx: set long timeout for WebSocket
proxy_read_timeout 86400s;
proxy_send_timeout 86400s;
```

2. **Cloudflare:** WebSocket connections are limited to 100 seconds on the free plan. Upgrade to Pro or use a different proxy.

3. **Network firewall:** Some corporate firewalls block WebSocket. DarshanDB will fall back to SSE automatically if WebSocket fails.

### "PROTOCOL_VERSION_MISMATCH"

Your client SDK version is incompatible with the server.

```bash
# Check server version
curl http://localhost:7700/api/admin/health | jq .version

# Update client SDKs
npm update @darshan/react @darshan/nextjs @darshan/client
```

### Queries return empty data

1. **Check permissions:** Are your permission rules returning the right filter?

```bash
RUST_LOG=darshandb_server::permissions=debug darshan dev
# Watch the logs for which WHERE clause is being injected
```

2. **Check authentication:** Is your token valid?

```typescript
const user = db.auth.getUser();
console.log('Current user:', user); // null means not authenticated
```

3. **Check the entity name:** Entity names are case-sensitive.

```typescript
// Wrong
{ Todos: {} }

// Right
{ todos: {} }
```

### Optimistic update not rolling back

If a mutation fails server-side but the UI doesn't revert:

```typescript
// Make sure you're handling errors
try {
  await db.transact(db.tx.todos[id].merge({ done: true }));
} catch (error) {
  // Error handling triggers the rollback
  console.error('Mutation failed:', error);
}
```

## Authentication Issues

### "INVALID_CREDENTIALS" but password is correct

1. **Case sensitivity:** Email matching is case-insensitive, but make sure there are no trailing spaces.

2. **Account locked:** After 5 failed attempts, the account is locked for 30 minutes.

```bash
# Check via admin API
curl http://localhost:7700/api/admin/stats \
  -H "Authorization: Bearer ADMIN_TOKEN"
```

3. **Password hash migration:** If you imported users from another system, their passwords may use a different hashing algorithm.

### OAuth callback fails

**Symptom:** "OAuth callback error" or redirect loop.

1. Check that the callback URL matches exactly:
```
http://localhost:7700/api/auth/callback/google   (development)
https://api.example.com/api/auth/callback/google (production)
```

2. Verify environment variables are set:
```bash
echo $DARSHAN_OAUTH_GOOGLE_CLIENT_ID
echo $DARSHAN_OAUTH_GOOGLE_CLIENT_SECRET
```

3. Check the OAuth provider's console for error logs.

### JWT token expired but refresh fails

**Symptom:** 401 on every request, refresh endpoint also returns 401.

The refresh token may have expired (30 days) or been revoked. The user must sign in again.

```typescript
db.auth.onAuthStateChange((user) => {
  if (!user) {
    // Redirect to sign-in page
    window.location.href = '/sign-in';
  }
});
```

## Performance Issues

### High query latency

1. **Check query complexity:**

```bash
curl -X POST http://localhost:7700/api/query \
  -H "Authorization: Bearer TOKEN" \
  -H "X-Darshan-Debug: true" \
  -d '{"todos": {"$where": {"done": false}}}'
# Response includes X-Query-Time-Ms header
```

2. **Add indexes for filtered fields:**

```typescript
// darshan/schema.ts
export default defineSchema({
  todos: defineTable({
    done: v.boolean(),
    priority: v.number(),
  }).index('by_done', ['done'])
    .index('by_priority', ['priority']),
});
```

3. **Reduce query depth:**

```bash
DARSHAN_MAX_QUERY_DEPTH=6  # reduce from default 12
```

### Memory usage growing over time

1. **Check for subscription leaks:** Unsubscribe when components unmount.

```typescript
// React: useQuery handles this automatically
// Vanilla: clean up manually
const unsub = room.on('change', handler);
// Later:
unsub();
room.leave();
```

2. **Reduce query cache size:**

```bash
DARSHAN_QUERY_CACHE_SIZE=500
```

3. **Check PostgreSQL connection pool:**

```bash
# Monitor pool usage
curl http://localhost:7700/metrics | grep pg_pool
```

### WebSocket backpressure

**Symptom:** Clients receive updates with increasing delay.

```bash
# Increase send buffer
DARSHAN_WS_BUFFER_SIZE=4194304  # 4MB

# Or reduce subscription density by splitting queries
```

## Database Issues

### Migration fails with "relation already exists"

```bash
# Check migration state
darshan migrate status

# Mark as applied without running
darshan migrate resolve --applied 20260405_000001_add_priority_to_todos
```

### Schema drift in production

```bash
# Generate a reconciliation migration
darshan migrate generate --name reconcile-schema --from-database

# Review the generated SQL before applying
cat darshan/migrations/TIMESTAMP_reconcile_schema.sql

# Apply
darshan migrate up
```

### PostgreSQL connection pool exhausted

**Symptom:** "too many connections" or queries timing out.

```bash
# Check current connections
psql "$DATABASE_URL" -c "SELECT count(*) FROM pg_stat_activity"

# Increase pool size (carefully)
DARSHAN_PG_POOL_SIZE=30

# Better: use PgBouncer
```

## Storage Issues

### Upload fails with 413

File exceeds the configured maximum size.

```typescript
// darshan/permissions.ts
storage: {
  maxFileSize: 50 * 1024 * 1024, // increase to 50MB
}
```

Also check your reverse proxy limits:

```nginx
# nginx
client_max_body_size 50m;
```

### Signed URLs return 403

1. Check that the storage backend credentials are correct.
2. Check clock skew -- signed URLs are time-sensitive. Ensure your server clock is synced (use NTP).
3. Verify the URL hasn't expired (`DARSHAN_STORAGE_URL_EXPIRY`).

## Diagnostic Commands

```bash
# Server health
curl http://localhost:7700/api/admin/health

# Server version
darshan --version

# View logs (Docker)
docker compose logs darshandb -f

# View logs (systemd)
journalctl -u darshandb -f

# Check PostgreSQL connectivity
darshan db ping

# Run built-in benchmark
darshan bench --connections 10 --duration 10s

# Export diagnostic info
darshan debug-info > darshandb-debug.txt
```

---

[Previous: Admin Dashboard](admin-dashboard.md) | [All Docs](README.md)
