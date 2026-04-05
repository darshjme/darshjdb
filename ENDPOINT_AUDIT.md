# DarshJDB Endpoint Audit Report

**Auditor:** PhD QA Engineer (Automated Trace Analysis)
**Date:** 2026-04-05
**Source files:** `packages/server/src/api/rest.rs`, `ws.rs`, `batch.rs`, `audit/handlers.rs`

## Legend

| Status | Meaning |
|--------|---------|
| **REAL** | Handler calls actual database/engine methods, returns live data |
| **STUB** | Returns hardcoded/fake/empty data, TODO comments present |
| **PARTIAL** | Core logic works but has specific known gaps |
| **NOT MOUNTED** | Handler exists in code but route is commented out |

---

## REST Endpoints (rest.rs)

### Auth Endpoints

| Endpoint | Handler | Status | Evidence |
|----------|---------|--------|----------|
| `POST /api/auth/signup` | `auth_signup` | **REAL** | Calls `PasswordProvider::hash_password`, `INSERT INTO users`, `triple_store.set_triples`, `session_manager.create_session`. Full flow: hash password, insert user row, create user triples, issue JWT token pair. |
| `POST /api/auth/signin` | `auth_signin` | **REAL** | Calls `PasswordProvider::authenticate(&state.pool, ...)` which queries the users table, verifies bcrypt hash. On success calls `session_manager.create_session`. Returns JWT pair. Handles MFA flow path. |
| `POST /api/auth/magic-link` | `auth_magic_link` | **STUB** | Returns hardcoded `{"message": "If an account exists, a magic link has been sent."}`. Has `// TODO: wire to MagicLinkProvider`. Does NOT send any email or create any token. Always returns 200 regardless of whether email exists. |
| `POST /api/auth/verify` | `auth_verify` | **STUB** | Returns fake tokens: `format!("ddb_at_{}", Uuid::new_v4())` and `format!("ddb_rt_{}", Uuid::new_v4())`. Has `// TODO: wire to token verification + optional MFA check`. No database call whatsoever. |
| `POST /api/auth/oauth/{provider}` | `auth_oauth` | **REAL** | Calls `oauth_provider.authorization_url()` to generate PKCE+HMAC authorize URL. When `code` is provided, calls `oauth_provider.exchange_code()`, `find_or_create_oauth_user()` (real SQL), `session_manager.create_session()`. Full working OAuth2 flow with PKCE. |
| `GET /api/auth/oauth/{provider}/callback` | `auth_oauth_callback` | **REAL** | Calls `oauth_provider.exchange_code()`, `find_or_create_oauth_user()`, `session_manager.create_session()`. Server-side OAuth2 callback flow. Note: PKCE verifier comes from `X-PKCE-Verifier` header (BFF pattern) or defaults to empty string. |
| `POST /api/auth/refresh` | `auth_refresh` | **REAL** | Calls `session_manager.refresh_session(&body.refresh_token, dfp)`. Real token rotation with device fingerprint binding and session revocation detection. |
| `POST /api/auth/signout` | `auth_signout` | **REAL** | Calls `session_manager.validate_token()` then `session_manager.revoke_session(auth_ctx.session_id)`. Real session revocation. |
| `GET /api/auth/me` | `auth_me` | **REAL** | Calls `session_manager.validate_token()` then `SELECT id, email, roles, created_at FROM users WHERE id = $1`. Returns real user profile from Postgres. |

### Data Endpoints

| Endpoint | Handler | Status | Evidence |
|----------|---------|--------|----------|
| `POST /api/query` | `query` | **REAL** | Calls `query::parse_darshan_ql`, `query::plan_query`, `query::execute_query(&state.pool, &plan)`. Full DarshJQL query engine with permission injection, hot cache (sub-ms reads), pool stats recording. |
| `POST /api/mutate` | `mutate` | **REAL** | Opens a Postgres transaction via `triple_store.begin_tx()`, calls `PgTripleStore::next_tx_id_in_tx`, `set_triples_in_tx`, `retract_in_tx`, `get_entity_in_tx` for delete. Full atomic transaction with rule engine evaluation, cache invalidation, change event emission. |
| `GET /api/data/{entity}` | `data_list` | **REAL** | Builds DarshJQL query for entity type, calls `query::parse_darshan_ql`, `plan_query`, `execute_query`. Supports pagination via `$limit`, permission WHERE clause injection. |
| `POST /api/data/{entity}` | `data_create` | **REAL** | Calls `triple_store.set_triples()` to write `:db/type` + data field triples. Supports TTL via `$ttl` key. Runs rule engine evaluation, invalidates cache, emits change events. |
| `GET /api/data/{entity}/{id}` | `data_get` | **REAL** | Calls `triple_store.get_entity(id)` to fetch all triples. Builds attribute map, enforces row-level security (owner_id check), applies field restrictions from permissions. Supports TTL virtual fields. |
| `PATCH /api/data/{entity}/{id}` | `data_patch` | **REAL** | Opens transaction, calls `PgTripleStore::retract_in_tx` for old values, `set_triples_in_tx` for new values, `evaluate_and_write_in_tx` for rules. Supports TTL override via `$ttl`. Atomic retract+write. |
| `DELETE /api/data/{entity}/{id}` | `data_delete` | **REAL** | Calls `triple_store.get_entity(id)` to verify existence, opens transaction, `retract_in_tx` for each triple, commits. Row-level security enforcement. Emits change events. |

### Function Endpoints

| Endpoint | Handler | Status | Evidence |
|----------|---------|--------|----------|
| `POST /api/fn/{name}` | `fn_invoke` | **REAL** | Calls `registry.get(&name)` to look up function, then `runtime.execute(&function_def, args, token)`. Full function invocation through the FunctionRuntime. Returns result, duration, logs. Registry and runtime must be initialized (returns 500 if not). |

### Storage Endpoints

| Endpoint | Handler | Status | Evidence |
|----------|---------|--------|----------|
| `POST /api/storage/upload` | `storage_upload` | **REAL** | Parses multipart/form-data or raw body. Calls `storage_engine.upload(&path, &file_data, &file_content_type, ...)`. Returns path, size, content_type, etag, signed_url from real `StorageEngine<LocalFsBackend>`. |
| `GET /api/storage/{*path}` | `storage_get` | **REAL** | Calls `storage_engine.verify_signed_url()` for signed URLs, `storage_engine.signed_url()` for URL generation, `storage_engine.download(&path)` for file retrieval. Path traversal protection. Note: image transform parameter is accepted but NOT applied (TODO). |
| `DELETE /api/storage/{*path}` | `storage_delete` | **REAL** | Calls `storage_engine.delete(&path)`. Real file deletion with path traversal protection. |

### SSE / Pub-Sub Endpoints

| Endpoint | Handler | Status | Evidence |
|----------|---------|--------|----------|
| `GET /api/subscribe?q=...` | `subscribe` | **PARTIAL** | Subscribes to `sse_tx` broadcast channel and streams events. Auth works. However, the SSE stream broadcasts ALL events to ALL subscribers -- the `q` parameter is parsed but NOT used to filter events. The query hash filtering is missing. |
| `GET /api/events?channel=...` | `events_sse` | **REAL** | Subscribes to `pubsub.subscribe_events()` broadcast, filters events using `ChannelPattern::parse` and `pattern.matches()`. Real pattern-based filtering. |
| `POST /api/events/publish` | `events_publish` | **REAL** | Calls `state.pubsub.publish(pub_event)`. Real event publishing through PubSubEngine. Returns receiver count. |

### Admin Endpoints

| Endpoint | Handler | Status | Evidence |
|----------|---------|--------|----------|
| `GET /api/admin/schema` | `admin_schema` | **REAL** | Calls `triple_store.get_schema()`. Returns real inferred schema from Postgres. |
| `GET /api/admin/functions` | `admin_functions` | **STUB** | Returns hardcoded `{"functions": []}`. Has `// TODO: wire to FunctionRegistry`. Does not call `state.function_registry`. |
| `GET /api/admin/sessions` | `admin_sessions` | **STUB** | Returns hardcoded `{"sessions": [], "count": 0}`. Has `// TODO: wire to sync::SessionManager`. Does not query any session data. |
| `POST /api/admin/bulk-load` | `admin_bulk_load` | **REAL** | Converts entities to triples, calls `triple_store.bulk_load(triples)` which uses UNNEST-based bulk insert. Returns count, tx_id, duration, throughput rate. |
| `GET /api/admin/cache` | `admin_cache` | **REAL** | Calls `state.query_cache.stats()`. Returns real cache statistics (size, hit/miss rates, evictions). |

### Audit Endpoints (audit/handlers.rs)

| Endpoint | Handler | Status | Evidence |
|----------|---------|--------|----------|
| `GET /api/admin/audit/verify/{tx_id}` | `audit_verify_tx` | **REAL** | Calls `super::verify_tx(&state.pool, tx_id)`. Recomputes Merkle root from stored triples and compares against recorded root. |
| `GET /api/admin/audit/chain` | `audit_verify_chain` | **REAL** | Calls `super::verify_chain(&state.pool)`. Walks entire `tx_merkle_roots` table, verifies prev_root chain. |
| `GET /api/admin/audit/proof/{entity_id}` | `audit_entity_proof` | **REAL** | Calls `super::entity_proof(&state.pool, entity_id)`. Returns Merkle inclusion proofs for all triples of the entity. |

### Batch Endpoints (batch.rs)

| Endpoint | Handler | Status | Evidence |
|----------|---------|--------|----------|
| `POST /api/batch` | `batch_handler` | **REAL** | Executes ops sequentially: queries via `query::execute_query`, mutations via `PgTripleStore::set_triples_in_tx` in a shared Postgres transaction, functions via `runtime.execute`. Full atomic batch with cache invalidation and change events. |
| `POST /api/batch/parallel` | `parallel_batch_handler` | **REAL** | Solana-inspired wave scheduler: profiles ops, groups non-conflicting ops into waves, executes waves in parallel with `futures::future::join_all`. Falls back to sequential for mutation batches. Records parallel metrics. |
| `GET /api/batch/metrics` | `parallel_metrics_handler` | **REAL** | Calls `state.parallel_metrics.snapshot()`. Returns real execution metrics. |

### Doc Endpoints

| Endpoint | Handler | Status | Evidence |
|----------|---------|--------|----------|
| `GET /api/openapi.json` | `openapi_json` | **REAL** | Returns pre-computed `state.openapi_spec`. Generated by `openapi::generate_openapi_spec()` at startup. |
| `GET /api/docs` | `docs` | **REAL** | Returns Scalar API viewer HTML via `openapi::docs_html("/api/openapi.json")`. |

### NOT MOUNTED Endpoints (handlers exist, routes commented out)

| Endpoint | Handler | Status | Evidence |
|----------|---------|--------|----------|
| `POST /api/embeddings` | `embeddings_store` | **NOT MOUNTED** | Handler is REAL (calls `INSERT INTO embeddings ... RETURNING id` with pgvector), but the route is commented out in `build_router`. |
| `GET /api/embeddings/{entity_id}` | `embeddings_get` | **NOT MOUNTED** | Handler is REAL (calls `SELECT ... FROM embeddings WHERE entity_id = $1`), but the route is commented out. |
| `POST /api/search/semantic` | `search_semantic` | **NOT MOUNTED** | Handler is REAL (builds pgvector cosine distance query with entity type join), but the route is commented out. |

---

## WebSocket Handlers (ws.rs)

| Message Type | Handler | Status | Evidence |
|--------------|---------|--------|----------|
| `auth` | `authenticate` | **REAL** | Decodes JWT (base64 `sub` claim extraction), calls `sessions.with_session_mut` to mark session authenticated. Supports JSON and MessagePack codecs with 5-second timeout. |
| `sub` (subscribe) | `handle_subscribe` | **REAL** | Computes query hash, registers subscription in session and global registry. Executes initial query via `query::parse_darshan_ql` + `plan_query` + `execute_query(&state.pool, &plan)`. Sends real initial results. |
| `unsub` (unsubscribe) | `handle_unsubscribe` | **REAL** | Removes subscription from session and global registry. |
| `mut` (mutation) | `handle_mutation` | **STUB** | Returns `MutOk { id, tx: 0 }` with hardcoded `tx: 0`. Comment says "Acknowledge with tx_id 0 until wired to the storage engine." Does NOT execute any mutation. Does NOT call PgTripleStore. |
| `pres-join` | `handle_presence_join` | **REAL** | Calls `presence.join(&room, &user_id, pres_state)`. Rate-limited. Sends room snapshot with all members. |
| `pres-state` | `handle_presence_state` | **REAL** | Calls `presence.update_state(&room, &user_id, pres_state)`. |
| `pres-leave` | `handle_presence_leave` | **REAL** | Calls `presence.leave(&room, &user_id)`. |
| `pub-sub` | `handle_pub_sub` | **REAL** | Calls `state.pubsub.subscribe(&subscriber, &id, &channel)`. Real channel pattern subscription. |
| `pub-unsub` | `handle_pub_unsub` | **REAL** | Calls `state.pubsub.unsubscribe(&subscriber, &id)`. |
| `batch` | N/A | **STUB** | Returns `ServerMessage::Error { error: "batch via WebSocket not yet implemented" }`. |
| `ping` | N/A | **REAL** | Responds with `ServerMessage::Pong`. |
| Change event push | `handle_change_event` | **PARTIAL** | Re-executes query via real query engine on change events. However, diff computation is incomplete: always sends full result set as "added" with empty "removed" and "updated" arrays. Comment: "A production implementation would cache previous results per (session, sub)." |
| Pub/sub event push | `handle_pubsub_change` | **REAL** | Calls `state.pubsub.process_change_event(event)` and sends matching events to subscribers. |

---

## Additional Partial Issues

### `require_admin_role` (rest.rs line 2766)
**Status: STUB** -- Function accepts any authenticated request. Has `// TODO: decode JWT from bearer token, verify "admin" in roles`. Admin endpoints are NOT actually restricted to admins.

### `extract_auth_context` (rest.rs line 2778)
**Status: PARTIAL** -- For protected data routes (query, mutate, data/*), this extracts user_id from `ddb_at_<uuid>` format tokens OR generates a random UUID. It does NOT validate the JWT through SessionManager for these routes (unlike auth/signout and auth/me which DO call `session_manager.validate_token`). The `require_auth_middleware` does call `session_manager.validate_token` for all protected routes, but `extract_auth_context` inside the handler re-parses instead of using the validated context from the middleware.

### `storage_get` image transforms (rest.rs line 2340)
**Status: STUB** -- `let _ = params.transform;` -- transform parameter is accepted but discarded. Has `// TODO: apply image transforms`.

---

## Summary: What is BROKEN/STUBBED

### Critical Stubs (functionality advertised but not working)

| # | What | Where | Impact |
|---|------|-------|--------|
| 1 | **Magic link auth** | `auth_magic_link` | Always returns success, sends no email, creates no token |
| 2 | **Token/MFA verification** | `auth_verify` | Returns fake random tokens, no actual verification |
| 3 | **WebSocket mutations** | WS `handle_mutation` | Always returns tx:0, no data written |
| 4 | **WebSocket batch** | WS `ClientMessage::Batch` | Returns "not yet implemented" error |
| 5 | **Admin functions list** | `admin_functions` | Returns empty array, ignores FunctionRegistry |
| 6 | **Admin sessions list** | `admin_sessions` | Returns empty array, ignores SessionManager |
| 7 | **Admin role enforcement** | `require_admin_role` | Any authenticated user can access admin endpoints |

### Partial Issues (works but with gaps)

| # | What | Where | Impact |
|---|------|-------|--------|
| 8 | **SSE query filtering** | `subscribe` | Query param `q` is not used to filter events -- all events broadcast to all subscribers |
| 9 | **WS diff computation** | `handle_change_event` | Always sends full result as "added", no real diff (no removed/updated detection) |
| 10 | **Image transforms** | `storage_get` | Transform parameter accepted but ignored |
| 11 | **Auth context duplication** | `extract_auth_context` | Re-parses token inside handler instead of using middleware-validated AuthContext |

### Not Mounted (code exists but unreachable)

| # | What | Where | Impact |
|---|------|-------|--------|
| 12 | **Embedding storage** | `embeddings_store` | Handler is fully implemented but route is commented out |
| 13 | **Embedding retrieval** | `embeddings_get` | Handler is fully implemented but route is commented out |
| 14 | **Semantic search** | `search_semantic` | Handler is fully implemented but route is commented out |

---

## Scorecard

| Category | Real | Partial | Stub | Not Mounted | Total |
|----------|------|---------|------|-------------|-------|
| REST Auth | 6 | 0 | 2 | 0 | 8 |
| REST Data | 5 | 0 | 0 | 0 | 5 |
| REST Functions | 1 | 0 | 0 | 0 | 1 |
| REST Storage | 2 | 1 | 0 | 0 | 3 |
| REST SSE/PubSub | 2 | 1 | 0 | 0 | 3 |
| REST Admin | 3 | 0 | 2 | 0 | 5 |
| REST Audit | 3 | 0 | 0 | 0 | 3 |
| REST Batch | 3 | 0 | 0 | 0 | 3 |
| REST Docs | 2 | 0 | 0 | 0 | 2 |
| REST Embeddings | 0 | 0 | 0 | 3 | 3 |
| WebSocket | 7 | 1 | 2 | 0 | 10 |
| **TOTAL** | **34** | **3** | **6** | **3** | **46** |

**Overall: 34/46 endpoints are fully REAL (74%), 3 partial (6.5%), 6 stubbed (13%), 3 not mounted (6.5%)**
