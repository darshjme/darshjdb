# Real-Time Subscriptions

DarshJDB provides three real-time primitives over a single WebSocket connection: **query subscriptions** (reactive queries with incremental diffs), **presence** (ephemeral per-room user state), and **pub/sub** (channel-based event streams). This document covers the full protocol, internal architecture, and client SDK usage.

---

## Architecture Overview

```
Client --WebSocket--> Session --subscribe--> Registry
                         |                       |
                         +-- LIVE SELECT --> LiveQueryManager
                         |                       |
                         v                       v
                     Presence            Broadcaster
                                             |
                                        DiffEngine
                                             |
                                        ChangeFeed --> PgNotifyBridge
```

| Component | Responsibility |
|-----------|---------------|
| **Session** | Per-connection state: active subscriptions, tx cursor, authenticated user ID |
| **Registry** | Global query-hash to session-set mapping for fan-out deduplication |
| **Broadcaster** | Listens for triple-store mutations, identifies affected queries, re-executes with permission context, pushes diffs |
| **DiffEngine** | Computes minimal delta patches between query result snapshots |
| **Presence** | Ephemeral per-room user state with auto-expiry and rate limiting |
| **LiveQueryManager** | SurrealDB-style LIVE SELECT with filter evaluation and push |
| **ChangeFeed** | Append-only mutation log with cursor-based replay and TTL retention |
| **PgNotifyBridge** | PostgreSQL LISTEN/NOTIFY for cluster-wide change propagation |

---

## WebSocket Connection Lifecycle

### 1. Connection Upgrade

Clients connect to the WebSocket endpoint at `/ws`. The server accepts the upgrade with a maximum inbound message size of 1 MiB.

### 2. Authentication (5-second timeout)

The first message **must** be an auth message. If no auth message arrives within 5 seconds, the server sends an `auth-err` and closes the connection.

```json
// Client sends:
{ "type": "auth", "token": "<jwt>" }

// Server responds on success:
{ "type": "auth-ok", "session_id": "<uuid>" }

// Server responds on failure:
{ "type": "auth-err", "error": "<reason>" }
```

### 3. Codec Detection

The server auto-detects the wire format from the first message:

- **Text frame** (UTF-8): JSON codec is used for the entire session
- **Binary frame**: MessagePack codec is used for the entire session

Once detected, the codec is fixed for the connection lifetime. MessagePack reduces bandwidth by 30-50% compared to JSON for typical workloads.

### 4. Message Loop

After authentication, the connection enters the main message loop. The server uses `tokio::select!` with biased polling to handle three event sources concurrently:

1. **Inbound messages** from the client (subscriptions, mutations, presence, pings)
2. **Change events** from the triple-store broadcaster (diffs pushed to subscribed clients)
3. **Keepalive timer** (30-second interval, sends WebSocket-level ping)

### 5. Cleanup on Disconnect

When the connection closes (client disconnect, error, or keepalive failure):

- All query subscriptions are unregistered from the `SubscriptionRegistry`
- The session is removed from the `SessionManager`
- The user is removed from all presence rooms via `PresenceManager::leave_all()`
- All pub/sub subscriptions are cleaned up via `PubSubEngine::unsubscribe_all()`
- All live queries are killed via `LiveQueryManager::kill_session()`

---

## Subscribe / Unsubscribe Protocol

### Subscribing to a Query

```json
// Client sends:
{
  "type": "sub",
  "id": "req-1",
  "query": { "from": "tasks", "where": { "status": "active" } }
}

// Server responds with initial results:
{
  "type": "sub-ok",
  "id": "req-1",
  "sub_id": "sub-uuid-abc",
  "initial": [
    { "_id": "task-1", "title": "Ship v1", "status": "active" },
    { "_id": "task-2", "title": "Write docs", "status": "active" }
  ]
}
```

The `id` field is a client-chosen request identifier for correlating responses. The `sub_id` is a server-assigned subscription identifier used for subsequent diffs and unsubscribe.

### How Subscriptions Work Internally

1. The query AST is normalized and hashed to produce a `query_hash`.
2. The `SubscriptionRegistry` maps `query_hash` to the set of `(session_id, sub_id)` handles.
3. The query is executed immediately and the full result set is sent as `initial`.
4. The result set hash is stored in the `ActiveSubscription` for diff detection.
5. When a mutation arrives, the broadcaster checks which query hashes are affected, re-executes each query **once** (deduplication), and pushes diffs to all subscribed sessions.

### Receiving Diffs

After the initial snapshot, the server pushes incremental diffs whenever the query results change:

```json
{
  "type": "diff",
  "sub_id": "sub-uuid-abc",
  "tx": 42,
  "changes": {
    "added": [
      { "_id": "task-3", "title": "New task", "status": "active" }
    ],
    "removed": ["task-1"],
    "updated": [
      {
        "entity_id": "task-2",
        "changed_fields": { "title": "Write better docs" },
        "removed_fields": [],
        "updated_triples": [
          { "attribute": "title", "value": "Write better docs" }
        ]
      }
    ]
  }
}
```

### Unsubscribing

```json
// Client sends:
{ "type": "unsub", "id": "req-2", "sub_id": "sub-uuid-abc" }

// Server responds:
{ "type": "unsub-ok", "id": "req-2" }
```

---

## How Diffs Are Computed

The diff engine (`sync::diff`) computes minimal deltas between two snapshots of a query's result set.

### Fast Path: Hash Comparison

Before doing any field-level comparison, the engine computes an **order-independent hash** of the entire result set. If the hash matches the previous snapshot, no diff is emitted. The hash uses XOR-combination of individual entity hashes, so result sets with the same entities in different row order produce identical hashes.

### Entity Matching

Entities are matched by their ID field, checked in order of priority:
1. `_id`
2. `id`
3. `entity_id`

Entities without any of these fields are treated as "unkeyed" and handled with hash-based comparison.

### Diff Categories

| Category | Description | Payload |
|----------|-------------|---------|
| `added` | Entities present in new results but not in old | Full entity object |
| `removed` | Entity IDs present in old but not in new | Entity ID string |
| `updated` | Entities present in both but with changed fields | `EntityPatch` with `changed_fields`, `removed_fields`, and triple-level changes |

### EntityPatch Structure

For updated entities, the diff includes both field-level and triple-level changes:

```json
{
  "entity_id": "task-2",
  "changed_fields": { "title": "New title", "priority": "high" },
  "removed_fields": ["temp_flag"],
  "added_triples": [
    { "attribute": "priority", "value": "high" }
  ],
  "removed_triples": [
    { "attribute": "temp_flag", "value": true }
  ],
  "updated_triples": [
    { "attribute": "title", "value": "New title" }
  ]
}
```

### Debounce Window (50ms)

Rapid mutations are batched within a 50ms debounce window. If multiple mutations arrive within 50ms, the broadcaster collapses them into a single re-query pass. This prevents flooding clients with per-keystroke diffs during fast typing scenarios.

### Canonical Hashing

JSON object keys are sorted before hashing to ensure deterministic results regardless of insertion order. This prevents spurious diffs when PostgreSQL returns JSONB with different key ordering.

---

## LIVE SELECT Queries

LIVE SELECT provides SurrealDB-style push notifications for matching mutations. Unlike query subscriptions (which re-execute the full query and diff), LIVE SELECT evaluates a filter predicate against each individual mutation and pushes matching changes immediately.

### Registering a Live Query

```json
// Client sends:
{
  "type": "live-select",
  "id": "req-3",
  "query": "LIVE SELECT * FROM users WHERE age > 18"
}

// Server responds:
{
  "type": "live-select-ok",
  "id": "req-3",
  "live_id": "live-uuid-xyz"
}
```

### Receiving Live Events

When a mutation matches the live query's filter:

```json
{
  "type": "live-event",
  "live_id": "live-uuid-xyz",
  "action": "CREATE",
  "result": { "_id": "user-99", "name": "Bob", "age": 25 },
  "tx_id": 55
}
```

The `action` field is one of: `CREATE`, `UPDATE`, `DELETE`.

### Filter Predicates

LIVE SELECT supports these comparison operators in WHERE clauses:

| Operator | Example | Description |
|----------|---------|-------------|
| `=` | `age = 18` | Equality |
| `!=` | `status != "deleted"` | Inequality |
| `>` | `age > 18` | Greater than |
| `>=` | `age >= 18` | Greater than or equal |
| `<` | `price < 100` | Less than |
| `<=` | `price <= 100` | Less than or equal |
| `CONTAINS` | `tags CONTAINS "rust"` | Array/string containment |
| `IN` | `status IN ["active", "pending"]` | Value in set |
| `AND` | `age > 18 AND status = "active"` | Logical conjunction |
| `OR` | `role = "admin" OR role = "moderator"` | Logical disjunction |

### Killing a Live Query

```json
// Client sends:
{ "type": "kill", "id": "req-4", "live_id": "live-uuid-xyz" }

// Server responds:
{ "type": "kill-ok", "id": "req-4", "live_id": "live-uuid-xyz" }
```

---

## Presence Rooms

Presence tracks which users are in a "room" (a document, a channel, a page) along with arbitrary ephemeral state like cursor positions and typing indicators.

### Joining a Room

```json
// Client sends:
{
  "type": "pres-join",
  "room": "doc-123",
  "state": { "cursor": { "line": 10, "col": 5 }, "status": "editing" }
}

// Server responds with current room snapshot:
{
  "type": "pres-snap",
  "room": "doc-123",
  "members": [
    { "user_id": "alice", "state": { "cursor": { "line": 3, "col": 12 }, "status": "viewing" } },
    { "user_id": "bob", "state": { "cursor": { "line": 10, "col": 5 }, "status": "editing" } }
  ]
}
```

### Updating State

```json
// Client sends:
{
  "type": "pres-state",
  "room": "doc-123",
  "state": { "cursor": { "line": 15, "col": 0 }, "status": "editing" }
}
```

State updates overwrite the previous state entirely. There is no merge -- send the full state object each time.

### Leaving a Room

```json
// Client sends:
{ "type": "pres-leave", "room": "doc-123" }
```

### Presence Internals

| Parameter | Value | Description |
|-----------|-------|-------------|
| Default TTL | 60 seconds | Entries expire if not refreshed within this window |
| Rate limit | 20 updates/sec per room | Sliding-window rate limiter prevents flooding |
| Auto-expiry | Background task every 10 seconds | Stale entries are evicted and empty rooms cleaned up |
| Auto-join | Yes | Updating state in a non-existent room auto-creates it |
| Disconnect cleanup | Automatic | `leave_all()` removes the user from every room on disconnect |

Rooms are created lazily on first join and removed automatically when the last user leaves or expires. The `PresenceManager` uses `DashMap` for lock-free concurrent access across WebSocket handler tasks.

---

## Pub/Sub Channels

Pub/Sub provides Redis-style channel subscriptions with glob pattern matching. Clients subscribe to patterns and receive events when matching changes occur in the triple store.

### Channel Naming Convention

```
entity:*              -- all entity changes
entity:users:*        -- all user entity changes
entity:users:<uuid>   -- specific entity changes
mutation:*            -- all mutations (created, updated, deleted)
auth:*                -- auth events (signup, signin, signout)
custom:<topic>        -- user-defined channels
```

### Subscribing to a Channel

```json
// Client sends:
{ "type": "pub-sub", "id": "ps-1", "channel": "entity:users:*" }

// Server responds:
{ "type": "pub-sub-ok", "id": "ps-1", "channel": "entity:users:*" }
```

### Receiving Events

```json
{
  "type": "pub-event",
  "id": "ps-1",
  "event": "updated",
  "entity_type": "users",
  "entity_id": "uuid-123",
  "changed": ["name", "email"],
  "tx_id": 42
}
```

### Pattern Matching Rules

| Pattern | Matches | Does not match |
|---------|---------|---------------|
| `entity:users:*` | `entity:users:abc-123` | `entity:posts:abc-123` |
| `entity:*` | `entity:users`, `entity:users:abc`, `entity:posts:def:nested` | `mutation:insert` |
| `entity:*:abc-123` | `entity:users:abc-123`, `entity:posts:abc-123` | `entity:users:def-456` |
| `mutation:*` | `mutation:updated`, `mutation:deleted` | `entity:users:abc` |
| `*` | Everything | Nothing |

A trailing `*` matches one or more remaining segments. A middle `*` matches exactly one segment.

### Unsubscribing

```json
// Client sends:
{ "type": "pub-unsub", "id": "ps-1" }

// Server responds:
{ "type": "pub-unsub-ok", "id": "ps-1" }
```

### Custom Event Publishing

Applications can publish custom events via the REST API:

```bash
curl -X POST http://localhost:7700/api/events/publish \
  -H "Authorization: Bearer TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "channel": "custom:notifications",
    "event": "new-message",
    "payload": { "message": "Hello world", "from": "system" }
  }'
```

These events are delivered to all WebSocket clients subscribed to matching patterns and are also available via the SSE endpoint at `GET /api/events`.

---

## SSE (Server-Sent Events) Alternative

For clients that cannot use WebSockets (browser restrictions, proxies, serverless functions), DarshJDB provides an SSE endpoint:

```bash
curl -N http://localhost:7700/api/events \
  -H "Authorization: Bearer TOKEN"
```

The SSE stream receives entity and mutation events from the pub/sub engine. It does not support query subscriptions or presence -- those require the full WebSocket connection.

---

## Client SDK Examples

### TypeScript (Core Client)

```typescript
import { DarshJDB } from "@darshjdb/client";

const db = new DarshJDB({ url: "http://localhost:7700" });
await db.auth.signIn({ email: "alice@example.com", password: "..." });

// Subscribe to a query
const unsub = db.subscribe(
  { from: "tasks", where: { status: "active" } },
  {
    onInitial(results) {
      console.log("Initial tasks:", results);
    },
    onDiff(diff) {
      console.log("Added:", diff.added);
      console.log("Removed:", diff.removed);
      console.log("Updated:", diff.updated);
    },
    onError(error) {
      console.error("Subscription error:", error);
    },
  }
);

// Join a presence room
db.presence.join("doc-123", { cursor: { line: 1, col: 0 } });
db.presence.onSnapshot("doc-123", (members) => {
  console.log("Room members:", members);
});

// Subscribe to pub/sub channel
db.pubsub.subscribe("entity:tasks:*", (event) => {
  console.log(`Task ${event.entity_id} was ${event.event}`);
});

// Clean up
unsub();
db.presence.leave("doc-123");
```

### React

```tsx
import { useQuery, usePresence } from "@darshjdb/react";

function TaskList() {
  // Reactive query -- re-renders automatically on changes
  const { data: tasks, isLoading } = useQuery({
    from: "tasks",
    where: { status: "active" },
    orderBy: { created_at: "desc" },
  });

  if (isLoading) return <div>Loading...</div>;

  return (
    <ul>
      {tasks.map((task) => (
        <li key={task._id}>{task.title}</li>
      ))}
    </ul>
  );
}

function CollaborativeEditor({ docId }: { docId: string }) {
  const { members, updateState } = usePresence(`doc-${docId}`, {
    cursor: { line: 0, col: 0 },
  });

  return (
    <div>
      <div className="avatars">
        {members.map((m) => (
          <Avatar key={m.user_id} user={m.user_id} />
        ))}
      </div>
      <Editor
        onCursorChange={(pos) => updateState({ cursor: pos })}
      />
    </div>
  );
}
```

---

## Mutations via WebSocket

Mutations can be sent over the WebSocket connection for lower latency compared to REST:

```json
// Client sends:
{
  "type": "mut",
  "id": "req-5",
  "ops": [
    { "action": "insert", "entity": "tasks", "data": { "title": "New task", "status": "active" } },
    { "action": "update", "entity": "tasks", "id": "task-1", "data": { "status": "done" } },
    { "action": "retract", "entity": "tasks", "id": "task-2" }
  ]
}

// Server responds:
{ "type": "mut-ok", "id": "req-5", "tx": 43 }

// On error:
{ "type": "mut-err", "id": "req-5", "error": "permission denied" }
```

The mutation is written atomically under a single `tx_id`. If any operation fails, the entire batch is rolled back.

---

## Change Feed

The change feed provides a durable, append-only log of all mutations with cursor-based replay support.

### Configuration

| Parameter | Default | Description |
|-----------|---------|-------------|
| `retention_ttl` | 24 hours | How long entries are kept before pruning |
| `buffer_capacity` | 10,000 entries | In-memory ring buffer size for fast access |
| `prune_interval` | 60 seconds | How often the background prune task runs |
| `pg_channel` | `darshjdb_changes` | PostgreSQL NOTIFY channel name |

### Cluster-Wide Propagation

In multi-node deployments, changes are propagated via PostgreSQL LISTEN/NOTIFY. Each mutation sends a `NOTIFY ddb_changes` with the `tx_id:entity_type` payload inside the same database transaction, ensuring atomic visibility.

---

## Troubleshooting Real-Time Issues

### Connection drops immediately after upgrade

- Verify the JWT token is valid and not expired. The server requires auth within 5 seconds.
- Check that the `Authorization` header is not being stripped by a reverse proxy.

### Subscriptions stop receiving diffs

- Check the server logs for `change receiver lagged` messages. This means the broadcast channel overflowed and some events were dropped. Increase the channel capacity.
- Verify the query still returns results -- if the data was deleted, the subscription correctly produces no diffs.
- Check that the mutation is going through the DarshJDB API (direct SQL writes to the `triples` table bypass the broadcaster).

### Presence updates are being dropped

- The rate limiter caps presence updates at 20 per second per room. If you are sending cursor positions on every mouse move, throttle to 10-15 updates per second on the client side.
- Check for `presence update rate-limited` in the server logs.

### High memory usage with many subscriptions

- Each subscription stores the last result set hash (8 bytes) and the query AST. The actual result data is not cached per-subscription.
- The `SubscriptionRegistry` deduplicates by query hash -- 1000 clients subscribing to the same query consume the same resources as one.
- Use `GET /api/admin/sessions` to see active subscription counts.

### Pub/sub events not arriving

- Verify the channel pattern matches the event channel. Use `entity:users:*` not `entity:users` (the latter is an exact match, not a wildcard).
- Custom events published via REST are delivered to WebSocket clients with matching patterns. SSE clients also receive entity and mutation events.

### MessagePack codec issues

- The codec is detected from the first message (auth). If you send auth as JSON text, all subsequent messages must be JSON. If you send auth as binary MessagePack, all subsequent messages must be MessagePack.
- Mixed codecs within a session are not supported and will produce parse errors.

---

## Related Documentation

- [Architecture](architecture.md) -- How the sync engine fits into the system
- [Presence](presence.md) -- Detailed presence API reference
- [API Reference](api-reference.md) -- REST endpoints for SSE and events
- [Security](security.md) -- Permission-scoped subscription execution
- [Performance](performance.md) -- WebSocket tuning and capacity planning
