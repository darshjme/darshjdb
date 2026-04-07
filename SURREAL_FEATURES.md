# SurrealDB Features to Add to DarshjDB

## Features from SurrealDB to integrate:

### 1. SurrealQL-like Query Language (DQL - DarshQL)
- SQL-like but with graph traversal operators (-> and <-)
- Record links: `user->write->article`
- Nested array filtering
- Type casting: `<int>`, `<float>`, `<decimal>`
- Computed fields evaluated at retrieval
- RELATE statement for graph edges

### 2. Multi-Model Support
- Document store (JSON documents)
- Graph database (directed typed edges)
- Relational (tables with schemas)
- Time-series (timestamp-indexed data)
- Key-value (simple get/set)
- Vector/embedding (already have this)

### 3. Storage Backends
- In-memory (for testing/development)
- File-based (RocksDB or similar)
- PostgreSQL (already have this)
- Distributed (TiKV or custom)

### 4. Live Queries
- Real-time subscriptions via WebSocket (already have WS)
- LIVE SELECT statement
- Change feeds
- Incremental view updates

### 5. Schema Modes
- Schema-full: strict types, validation
- Schema-less: flexible documents
- Mixed: some fields enforced, rest flexible

### 6. Container Support
- Docker image: darshjme/darshjdb
- Docker Compose for dev
- LXC container support
- Single binary deployment

### 7. Edge Computing
- Embedded mode (library, not server)
- WASM compilation for browser
- Edge deployment

### 8. Advanced Auth
- Row-level permissions (already have)
- PERMISSIONS FOR select WHERE conditions
- Scope-based auth
- Token auth with custom claims

### 9. Functions
- Embedded JavaScript/TypeScript functions (already have Deno runtime)
- Custom aggregations
- Triggers on data changes

### 10. SDKs
- Rust SDK
- JavaScript/TypeScript SDK (already have)
- Python SDK
- Go SDK
