# @darshandb/server

The DarshanDB server -- a single Rust binary that provides the complete backend: triple-store database engine, DarshanQL query compiler, reactive sync engine, authentication, permissions, file storage, and the V8 function runtime.

## Architecture

| Component | Description |
|-----------|-------------|
| **Triple Store** | EAV (Entity-Attribute-Value) data model over PostgreSQL 16+ |
| **Query Engine** | Compiles DarshanQL into optimized SQL with permission injection |
| **Sync Engine** | Tracks query dependencies and pushes delta diffs over WebSocket |
| **Auth Engine** | Email/password (Argon2id), OAuth, magic links, MFA, JWT RS256 |
| **Permission Engine** | Row-level and field-level security evaluated on every request |
| **Function Runtime** | V8 isolates (via Deno Core) for sandboxed TypeScript execution |
| **Storage Engine** | S3-compatible file storage with signed URLs and image transforms |
| **REST Handler** | Full CRUD API for clients that cannot use WebSocket |

## Building

```bash
# From the workspace root
cargo build --release -p darshandb-server
```

The binary is output to `target/release/darshandb-server`.

## Configuration

All configuration is via environment variables. See the [Self-Hosting Guide](../../docs/self-hosting.md) for the full list.

## Key Dependencies

- **axum** + **tokio** -- Async HTTP and WebSocket server
- **sqlx** -- Async PostgreSQL driver with compile-time query checking
- **jsonwebtoken** -- JWT signing and verification
- **argon2** -- Password hashing
- **rmp-serde** -- MessagePack serialization for the wire protocol
- **dashmap** -- Concurrent hash map for subscription tracking

## Documentation

- [Getting Started](../../docs/getting-started.md)
- [Self-Hosting](../../docs/self-hosting.md)
- [Security Architecture](../../docs/security.md)
- [Performance Tuning](../../docs/performance.md)
