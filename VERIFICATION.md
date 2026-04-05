# DarshJDB Verification Report

**Date:** 2026-04-05
**Auditor:** Automated deep-read verification
**Scope:** Full codebase -- Rust server, TypeScript SDKs, Python SDK, integration wiring

---

## 1. Test Results

### Rust (server library + doctests)

| Suite | Passed | Failed | Ignored | Status |
|-------|--------|--------|---------|--------|
| `ddb_server` (lib) | 446 | 0 | 0 | PASS |
| `ddb_server` (bin) | 0 | 0 | 0 | PASS (no binary tests) |
| Doc-tests | 2 | 0 | 2 | PASS |
| **Total** | **448** | **0** | **2** | **PASS** |

- `cargo fmt --all -- --check`: **PASS** (exit 0, no formatting issues)
- `cargo clippy --workspace -Dwarnings`: **FAIL** -- 5 errors (4 collapsible-if warnings promoted to errors, 1 unused import)

The clippy failures are minor style issues (nested `if let + if` blocks in rest.rs and ws.rs that Rust 1.94 wants collapsed using `let chains`, plus one unused import of `evaluate_permission` in default_permissions.rs tests). These are trivially fixable and do not indicate correctness problems.

### TypeScript (Vitest)

| Package | Files | Passed | Skipped | Status |
|---------|-------|--------|---------|--------|
| `@darshjdb/tests` (integration) | 2 | 61 | 15 | PASS |
| `@darshjdb/react` | 1 | 31 | 0 | PASS |
| `@darshjdb/angular` | 0 | -- | -- | No tests |
| `@darshjdb/nextjs` | 0 | -- | -- | No tests |
| **Total** | **3** | **92** | **15** | **PASS** |

### Python SDK (pytest)

| File | Passed | Failed | Status |
|------|--------|--------|--------|
| `test_client.py` | 94 | 0 | PASS |
| `test_exceptions.py` | 45 | 0 | PASS |
| `test_placeholder.py` | 2 | 0 | PASS |
| **Total** | **141** | **0** | **PASS** |

### Grand Total: 681 tests passed, 0 failed, 17 skipped/ignored

---

## 2. Integration Verification -- Is the Code Actually Wired?

### WIRED AND FUNCTIONAL (Real code, not stubs)

**Triple Store (PgTripleStore)**
- Creates real PostgreSQL table `triples` with 6 indexes on startup (`ensure_schema`)
- Uses `darshan_tx_seq` sequence for monotonic transaction IDs
- `set_triples` runs inside a real `pool.begin()` / `tx.commit()` transaction
- `retract` performs real `UPDATE ... SET retracted = true` SQL
- `get_entity`, `get_attribute`, `query_by_attribute` all execute real SQL with parameterized queries
- `get_entity_at` implements point-in-time reads via tx_id filtering
- `get_schema` infers schema by scanning `DISTINCT attribute` from the triples table
- **Verdict: FULLY REAL**

**Query Engine (DarshanQL)**
- `parse_darshan_ql` parses JSON into a typed AST with where-clauses, ordering, pagination, full-text search, nested queries
- `plan_query` generates real parameterized SQL with JOINs on the triples table, one per where-clause attribute
- `execute_query` runs the generated SQL via sqlx, groups raw triples into entity result rows, resolves nested references
- Full-text search uses ILIKE with proper wildcard-escape (`%`, `_`, `\` metacharacters escaped)
- LIKE injection is properly prevented
- **Verdict: FULLY REAL**

**Auth -- Signup + Signin Flow**
- `auth_signup`: Real SQL INSERT into `users` table, Argon2id password hashing, creates triple-store entity, issues JWT session
- `auth_signin`: Real SQL SELECT + Argon2id verify via `PasswordProvider::authenticate`, creates session with refresh token
- `auth_me`: Validates JWT, fetches user record from real SQL query
- Session management: JWT creation with RS256 or HS256, session stored in `sessions` table with refresh token hash
- `require_auth_middleware`: Real JWT validation, extracts user context, supports dev-mode bypass with `Bearer dev`
- **Verdict: FULLY REAL for password auth flow**

**Permission Engine**
- `PermissionEngine` with rule-based evaluation: role checks, WHERE-clause injection, composite AND/OR rules
- Default rules enforce row-level security via `owner_id = $user_id` WHERE clauses
- Admin role bypasses all restrictions
- `users` entity has stricter rules (can only see own record, admin-only create/delete)
- Permission WHERE clauses are injected into query ASTs before execution
- `$user_id` placeholder is substituted with parameterized `$N` bind values (not string interpolation -- SQL injection safe)
- **Verdict: FULLY REAL**

**WebSocket Sync Protocol**
- Real WebSocket upgrade at `/ws` via Axum
- Full auth flow with 5-second timeout
- Auto-detects JSON vs MessagePack codec from first message
- Subscription registration with query hash deduplication
- Initial query results executed via real query engine on subscribe
- Change event listener via `tokio::sync::broadcast` -- receives events from REST mutations
- On change: re-executes subscribed queries and sends diff to client
- Keepalive pings every 30 seconds
- Cleanup on disconnect: unregisters subscriptions, leaves presence rooms, removes session
- **Verdict: REAL, with caveats (see below)**

**Presence System**
- In-memory `PresenceManager` with room-based tracking
- Join, leave, state update, room snapshot all implemented
- Rate limiting per user per room (prevents spam)
- Room expiry after configurable timeout
- **Verdict: FULLY REAL (in-memory, no persistence)**

**REST Data Endpoints (CRUD)**
- `POST /api/data/:entity` creates real triples with `:db/type` attribute
- `GET /api/data/:entity` lists entities via query engine with pagination
- `GET /api/data/:entity/:id` fetches single entity from triple store
- `PATCH /api/data/:entity/:id` retracts old triples, inserts new ones
- `DELETE /api/data/:entity/:id` retracts all triples for entity
- `POST /api/mutate` handles batch insert/update/delete/upsert transactions
- All write operations emit `ChangeEvent` to broadcast channel for reactive subscriptions
- Permission checks on every operation
- **Verdict: FULLY REAL**

**Broadcast/SSE**
- SSE endpoint at `/api/subscribe?q=...` uses real `BroadcastStream`
- Receives events from `sse_tx` broadcast channel
- Sends proper SSE format with event type, data, and ID
- Keepalive heartbeats every 15 seconds
- **Verdict: REAL, but SSE events are not yet connected to triple-store changes (only WS is)**

---

### STUBBED / NOT WIRED (Exists as code but returns fake data)

| Feature | Location | Status |
|---------|----------|--------|
| Magic link auth | `rest.rs:600-617` | Returns "magic link sent" without sending anything |
| Token verification (magic link) | `rest.rs:628-645` | Returns fake tokens, no actual verification |
| OAuth (Google/GitHub/Apple) | `rest.rs:656-684` | Validates provider name but returns fake tokens |
| Refresh token rotation | `rest.rs:692-709` | Validates input but returns fake new tokens |
| Session revocation (signout) | `rest.rs:712-721` | Returns 204 without revoking |
| Server-side functions | `rest.rs:1433-1440` | Returns `null` result, no registry lookup |
| Storage upload | `rest.rs:1449-1486` | Validates size but does not persist file |
| Storage get | `rest.rs:1498-1525` | Always returns 404 |
| Storage delete | `rest.rs:1528-1544` | Returns 204 without deleting |
| Admin functions list | `rest.rs:1617-1631` | Returns empty array |
| Admin sessions list | `rest.rs:1634-1648` | Returns empty array |
| Rate limit middleware | `rest.rs:215-235` | Always returns hardcoded 1000/999/60 |
| WS mutations | `ws.rs:585-604` | Acknowledges but returns tx_id 0, no execution |
| WebAuthn (FIDO2) | `mfa.rs:288-398` | Interface stubs only |
| S3/R2/MinIO storage backend | `storage/mod.rs:572+` | Backend trait impl stubs |
| Semantic/vector search | `query/mod.rs:266-269` | Parsed but ignored with warning |
| Admin role check | `rest.rs:1695-1697` | Comment says "stub", checks dev mode only |

---

## 3. What Genuinely Works

1. **Triple store CRUD** -- The EAV storage layer is fully implemented against Postgres with proper transactions, indexes, and schema management.

2. **DarshanQL query engine** -- Parses declarative JSON queries, generates parameterized SQL, executes with proper binding. Supports where-clauses with 8 operators, ordering, pagination, full-text search, and nested entity resolution.

3. **Email/password authentication** -- Complete flow from signup (Argon2id hashing, user creation in both `users` table and triple store) through signin (credential verification, JWT issuance) to `/auth/me` (token validation, user profile fetch).

4. **JWT session management** -- RS256 (production) and HS256 (development) signing, with session table tracking, refresh tokens, device fingerprinting, IP/user-agent recording.

5. **Row-level security** -- Permission engine with composable rules, admin bypass, owner-based WHERE clause injection into queries. This is not cosmetic -- the WHERE clauses are actually injected into query ASTs before SQL generation.

6. **WebSocket real-time sync** -- Full protocol implementation with auth, subscription lifecycle, query execution on subscribe, change event propagation from REST writes to WS clients, presence rooms, and keepalive.

7. **Content negotiation** -- JSON and MessagePack responses based on `Accept` header, throughout both REST and WS.

8. **React hooks** -- 31 passing tests covering `useQuery`, `useMutation`, `useAuth`, `usePresence`, `useStorage`.

9. **Python SDK** -- 141 passing tests covering client initialization, auth, query, transact, admin operations, error handling.

10. **TypeScript client-core** -- 61 passing integration tests covering the core client library.

---

## 4. What Does Not Work

1. **OAuth authentication** -- Google, GitHub, Apple providers accept authorization codes but return fabricated tokens. No actual OAuth2 exchange occurs.

2. **Magic link authentication** -- Endpoint exists but sends nothing. Token verification returns fake credentials.

3. **Refresh token rotation** -- Endpoint accepts refresh tokens but generates new ones without validating/rotating the old one against the `sessions` table.

4. **Session revocation** -- Signout returns success without marking the session as revoked in the database.

5. **File storage** -- Upload accepts bytes but does not persist them. Download always returns 404. Delete is a no-op. The `StorageEngine` with local/S3/R2 backends exists as a module (~1647 lines) but is not wired to the REST handlers.

6. **Server-side functions** -- The `FunctionRegistry`, `Runtime`, `Scheduler`, and `Validator` modules exist (~3182 lines) but the `/api/fn/:name` endpoint does not call them.

7. **WS mutations** -- The WebSocket protocol handles `mut` messages but always returns `tx_id: 0` without executing against the triple store.

8. **Rate limiting** -- Headers are injected but always show 1000/999/60. The `RateLimiter` struct exists and is initialized in `main.rs` but the middleware does not read from it.

9. **SSE change propagation** -- The SSE endpoint streams from `sse_tx`, but REST mutations send events to `change_tx` (the WS channel). The SSE channel never receives mutation events.

10. **Clippy compliance** -- 5 warnings treated as errors under `-Dwarnings`. Trivially fixable.

---

## 5. Code Quality Assessment

**Strengths:**
- Clean Rust code with proper error types, structured logging, and `thiserror` derivations
- 18,676 lines of Rust server code -- substantial, not a toy project
- All SQL uses parameterized queries (no string interpolation for user input)
- Proper use of `sqlx` async database access with connection pooling
- Well-documented modules with doc comments and architecture diagrams
- TypeScript SDKs are properly typed with discriminated unions
- Test coverage is meaningful -- 446 Rust tests include auth, permissions, query engine, presence, sync, MFA, storage types, and session management
- Content negotiation (JSON + MessagePack) is implemented end-to-end, not just on one endpoint

**Weaknesses:**
- Several large handler functions in rest.rs could be factored into service layer methods
- The query planner generates SQL that may not perform well at scale (N+1-style joins for N where clauses)
- WS diff computation sends full result set as "added" rather than actual diffs -- needs result caching
- Admin role check is a stub (`require_admin_role` checks dev mode, not actual JWT roles)
- The `_args` field in `QueryRequest` is dead code
- Some struct fields are `#[allow(dead_code)]` and genuinely unused

---

## 6. Security Posture

**Strong:**
- Argon2id password hashing with unique salts (verified by tests)
- Parameterized SQL everywhere -- no SQL injection vectors found
- Path traversal prevention on storage endpoints (`..` check)
- Upload size limits (50 MB)
- LIKE wildcard injection prevention in full-text search
- JWT validation in auth middleware with proper 401 responses
- Row-level security via permission engine WHERE clause injection
- `$user_id` placeholder substitution uses parameterized binds, not string formatting
- Password length validation (8-128 chars)
- Email uniqueness enforced at database level
- Session table tracks IP, user-agent, device fingerprint

**Weak / Missing:**
- CORS is `allow_origin(Any)` -- fine for dev, dangerous for production
- No CSRF protection
- Rate limiting is not functional (hardcoded values)
- Admin role check in admin endpoints is a stub
- Signout does not actually revoke sessions
- Refresh token rotation is not implemented (replay attacks possible)
- No account lockout after failed login attempts
- No email verification on signup
- WebAuthn is interface-only stubs

---

## 7. Recommendations (Priority Order)

### Critical (Wire existing code)
1. **Wire storage handlers to StorageEngine** -- The module exists (1647 lines with local/S3 backends). Connect the REST endpoints.
2. **Wire function handlers to FunctionRegistry** -- The module exists (3182 lines). Connect `/api/fn/:name`.
3. **Implement refresh token rotation** -- The `sessions` table has `refresh_token_hash` and `refresh_expires_at`. Wire `SessionManager::rotate`.
4. **Implement signout** -- Set `revoked = true` in sessions table.
5. **Fix rate limit middleware** -- Read from the actual `RateLimiter` instead of hardcoded values.

### Important (Correctness)
6. **Fix clippy errors** -- Collapse nested if-lets, remove unused import. 5 minutes of work.
7. **Connect SSE to change_tx** -- Either forward `change_tx` events to `sse_tx`, or have SSE subscribe to `change_tx` directly.
8. **Wire WS mutations** -- Execute mutations through the triple store instead of returning tx_id 0.
9. **Implement WS diff caching** -- Currently sends full result set; needs previous-result cache per subscription for real diffs.
10. **Wire admin role check** -- Decode JWT and verify "admin" role instead of stub.

### Nice-to-Have (Completeness)
11. **Implement OAuth2 flows** -- Wire to actual provider libraries.
12. **Implement magic link** -- Wire email sending service.
13. **Add Angular and Next.js tests** -- Currently 0 tests for these packages.
14. **Production CORS configuration** -- Make origins configurable via environment.
15. **Account lockout / brute force protection** -- Leverage the existing RateLimiter.

---

## Summary

DarshJDB has a **solid, working core**. The triple store, query engine, password auth, JWT sessions, permission engine, WebSocket sync, and presence system are all genuinely implemented with real database operations. The 681 passing tests validate real behavior, not just type signatures.

The main gap is that several substantial modules (storage engine, function runtime, scheduler) exist as well-written implementations but are **not yet wired** to the REST API handlers. This is a classic "last mile" problem -- the hardest engineering is done, the plumbing just needs connecting.

The codebase is approximately **18,700 lines of Rust + 13,900 lines of TypeScript + 1,100 lines of Python** of real, tested code. This is not a prototype or a demo -- it is an early-stage database product with genuine functionality.
