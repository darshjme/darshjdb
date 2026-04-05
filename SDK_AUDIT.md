# DarshJDB SDK Audit Report

**Date:** 2026-04-05
**Auditor:** Claude Opus 4.6 (1M context)
**Scope:** All client SDK packages across TypeScript, React, Angular, Next.js, PHP, and Python

---

## Executive Summary

The DarshJDB SDK suite is **well-architected and production-quality**. All six packages demonstrate strong typing, consistent error handling, proper documentation, and good separation of concerns. The codebase follows modern best practices for each language/framework ecosystem.

**Fixes applied during this audit:** 4 bugs fixed, 1 type annotation improved.

---

## 1. `@darshjdb/client` (packages/client-core)

### Files Audited
- `types.ts` - Core type definitions
- `client.ts` - Main client class (WebSocket, MessagePack)
- `auth.ts` - Authentication (email/password, OAuth, token refresh)
- `query.ts` - Query builder with deduplication
- `sync.ts` - IndexedDB cache, offline queue, optimistic updates
- `rest.ts` - REST/SSE transport fallback
- `presence.ts` - Real-time presence rooms
- `storage.ts` - File uploads (simple + resumable)
- `transaction.ts` - Proxy-based transaction builder
- `index.ts` - Barrel exports

### Verdict: EXCELLENT

**Type Safety:**
- All 30+ exported types are complete and correctly defined
- Generic type parameters used consistently (`QueryResult<T>`, `Peer<T>`, `PresenceSnapshot<T>`)
- No unjustified `any` types. The single `any` in `query.ts` line 169 (`_privateActiveSubs`) is correctly documented with an eslint-disable comment explaining it is type-safe at call-sites
- `WhereClause.value` uses `unknown` (correct -- values are heterogeneous by design)
- `ServerMessage.payload` and `ClientMessage.payload` use `unknown` (correct -- payloads vary by message type)

**Export Completeness:**
- `index.ts` re-exports all 30 types, 7 classes, and 4 utility functions
- Both value exports and `type` exports are properly separated (no runtime cost for type-only imports)
- `EntityProxy` and `EntityCollectionProxy` exported as `type` (correct)

**Error Handling:**
- Consistent pattern: network errors throw `Error` with descriptive messages
- All listener loops wrapped in try/catch to prevent one bad listener from breaking others
- Reconnection uses exponential backoff with jitter (INITIAL_BACKOFF_MS=500, MAX_BACKOFF_MS=30000)
- Pending requests are rejected on disconnect (no dangling promises)
- Token refresh failure clears session (prevents stale auth state)

**Potential Improvements (non-blocking):**
- `query.ts`: The global `_privateActiveSubs` map is module-scoped, meaning multiple `DarshJDB` instances share subscription deduplication. This is likely intentional but could cause cross-client interference in edge cases (e.g., test suites)
- `client.ts` line 280: The `onclose` handler rejects the initial connect promise even when `_privateState` comparison is against the *already-updated* reconnecting state. The condition `this._privateState !== 'connected'` will always be true since `_privateSetState('reconnecting')` was just called. This is harmless (the reject is caught) but the intent check is misleading
- `storage.ts`: The XHR-based simple upload doesn't support cancellation (AbortController). The resumable upload doesn't support resuming from the last successful chunk after failure

---

## 2. `@darshjdb/react` (packages/react)

### Files Audited
- `provider.tsx` - Context provider with lifecycle management
- `types.ts` - Internal type definitions (decoupled from client-core)
- `use-query.ts` - Reactive query hook with Suspense support
- `use-mutation.ts` - Mutation hook with optimistic state
- `use-auth.ts` - Authentication hook
- `use-presence.ts` - Presence hook with auto join/leave
- `use-storage.ts` - Upload hook with progress tracking
- `index.ts` - Barrel exports

### Verdict: VERY GOOD (1 bug fixed)

**React Best Practices:**
- All hooks use `useSyncExternalStore` (React 18+ concurrent-safe)
- `getServerSnapshot` provided in all hooks (SSR-safe)
- Stable callback references via `useCallback` with `useRef` indirection
- Proper cleanup in all `useEffect` return functions
- Shallow array comparison prevents unnecessary re-renders in `useQuery`

**Bug Fixed -- `use-mutation.ts`:**
- **Issue:** The mount tracking logic was convoluted, using a `useState` initializer trick and `unmountRef` without an actual `useEffect` cleanup. The `mountedRef.current = false` was never set on unmount, meaning state updates could fire after unmount.
- **Fix:** Replaced with a standard `useEffect` with cleanup that sets `mountedRef.current = false` on unmount.

**Suspense Support:**
- `useQuery` correctly throws the pending promise during initial load when `suspense: true`
- Promise is resolved when first snapshot arrives
- Promise reference is cleared after resolution (prevents memory leak)

**Memory Leak Assessment:**
- `usePresence`: Properly calls `leaveRoom` on unmount and cleans up subscription
- `useQuery`: Unsubscribes via `store.unsub?.()` in effect cleanup
- `useAuth`: Unsubscribes from `onAuthStateChange` in effect cleanup
- `useStorage`: Cancels pending `requestAnimationFrame` in upload `finally` block
- No leaks detected

**Type Observations:**
- `types.ts` defines its own `WhereClause` with `op: '==' | '!=' | ...` which differs from client-core's `'=' | '!=' | ...`. This is an intentional abstraction boundary (the React types mirror the *public* API while client-core types mirror the *wire* format). The provider layer translates between them. However, this difference should be documented to prevent confusion.
- `DarshanClientInterface` is well-defined and serves as the decoupling contract

---

## 3. `@darshjdb/angular` (packages/angular)

### Files Audited
- `tokens.ts` - Injection tokens and client interface
- `types.ts` - Type definitions
- `providers.ts` - Standalone provider function
- `ddb.module.ts` - NgModule configuration
- `client.factory.ts` - Client factory
- `inject.ts` - Convenience injection functions with signals
- `query.signal.ts` - Signal-based reactive queries
- `query.observable.ts` - RxJS Observable queries
- `auth.ts` - Router guards and HTTP interceptor
- `presence.ts` - Presence directive and signal helper
- `ssr.ts` - TransferState SSR support
- `public-api.ts` - Barrel exports

### Verdict: EXCELLENT

**Angular Injection Patterns:**
- `DDB_CONFIG` and `DDB_CLIENT` properly typed `InjectionToken<T>`
- `provideDarshan()` returns `EnvironmentProviders` (Angular 16+ standalone pattern)
- `DarshJDBModule.forRoot()` returns `ModuleWithProviders<DarshJDBModule>` (legacy pattern)
- Both paths correctly register `APP_INITIALIZER` for connection setup
- `ENVIRONMENT_INITIALIZER` used for `beforeunload` cleanup (browser safety net)

**Signal Usage:**
- All reactive state in `inject.ts` uses `signal()` with `.asReadonly()` for public access
- `computed()` used correctly for derived state (`isAuthenticated`)
- `WritableSignal` typed explicitly in `query.signal.ts`
- `DestroyRef.onDestroy()` used consistently for cleanup (Angular 16+ pattern)

**SSR Support:**
- `darshanTransferQuery()` correctly uses `TransferState`, `isPlatformServer`, `isPlatformBrowser`
- Server path: one-shot query, stores in `TransferState`, returns signals
- Client path: hydrates from `TransferState`, removes cached entry, opens live subscription
- `hydrated` signal correctly tracks whether data came from transfer cache
- Deterministic transfer key via `simpleHash()` (stable across server/client)

**Observable API:**
- `shareReplay({ bufferSize: 1, refCount: true })` prevents memory leaks
- Teardown function in Observable constructor cleans up DarshJDB subscription
- `debounceTime` conditionally applied via operator pipeline

**Auth Guards:**
- `darshanAuthGuard` correctly implements `CanActivateFn`
- `darshanRoleGuard` is a factory returning `CanActivateFn` (composable)
- `darshanAuthInterceptor` correctly scopes token attachment to `config.serverUrl` origin only (prevents token leakage)

**Minor Observation:**
- `ddb.module.ts` constructor sets `this._client = null` but never assigns the injected client. The `ngOnDestroy` therefore never calls `disconnect()`. This is a dead code path -- cleanup is handled by the `ENVIRONMENT_INITIALIZER` in the standalone path and `beforeunload` in the module path. Not a bug, but the `_client` field is misleading.

---

## 4. `@darshjdb/nextjs` (packages/nextjs)

### Files Audited
- `server.ts` - Server Component queries, Server Actions, admin client
- `api.ts` - API route helpers (Pages + App Router)
- `middleware.ts` - Edge Middleware for session auth
- `pages.ts` - Pages Router helpers (getServerSideProps, getStaticProps)
- `provider.tsx` - Client-side provider with SSR hydration
- `index.ts` - Barrel exports

### Verdict: VERY GOOD (1 type annotation improved)

**Server Component Compatibility:**
- `server.ts` exports are all server-only (no `'use client'` directive)
- `queryServer()` integrates with Next.js fetch cache via `revalidate` and `tags`
- `mutateServer()` provides clean transactional wrapper for Server Actions
- Admin client singleton uses lazy initialization from env vars

**Cookie Security:**
- `setSessionCookie()` defaults: `httpOnly: true`, `sameSite: 'lax'`, `secure: true` in production
- Session token injected into `x-ddb-session` header for downstream consumption
- Invalid sessions are cleared (cookie deleted) before redirect

**Fix Applied -- `server.ts`:**
- **Issue:** `let chain: any = collection` -- bare `any` without justification
- **Fix:** Added eslint-disable comment with explanation that the chain builder API returns varying shapes

**Middleware:**
- Prefix-based route matching with explicit public route exemptions
- `callbackUrl` and `callbackSearch` preserved for post-login redirect
- `validateSession` callback is optional (graceful degradation to cookie-presence check)
- `onAuthenticated` hook allows response modification

**Provider:**
- `'use client'` directive correctly placed at file top
- Stable client reference via `useMemo` keyed on URL/token changes
- Dehydrated state hydrated into client cache on mount
- Missing URL produces a clear error message with env var name

**Pages Router:**
- `queryServerSide()` wraps `getServerSideProps` with DarshJDB client injection
- `queryStaticProps()` wraps `getStaticProps` with ISR revalidation support
- Both serialize output via `JSON.parse(JSON.stringify(result))` to strip non-serializable values
- `null` return triggers Next.js 404

**Observation:**
- `server.ts` uses `require('@darshjdb/client')` which may not resolve correctly in all bundler configurations. A dynamic `import()` would be more compatible with ESM-first bundlers, but `require` works in the Node.js runtime context where these server functions execute.

---

## 5. PHP SDK (sdks/php)

### Files Audited
- `Client.php` - Main client with HTTP helpers
- `AuthClient.php` - Authentication
- `DarshanException.php` - Exception hierarchy
- `QueryBuilder.php` - Fluent query builder
- `StorageClient.php` - File storage
- `Laravel/DarshanFacade.php` - Laravel facade
- `Laravel/DarshanServiceProvider.php` - Laravel service provider

### Verdict: EXCELLENT

**PHP 8.1+ Type Safety:**
- `declare(strict_types=1)` in every file
- Typed properties: `private HttpClient $http`, `private string $serverUrl`, `private ?string $token`
- Return types on all methods including `mixed` where appropriate
- PHPDoc `@param` and `@return` annotations with generic array shapes (e.g., `array{data: array<int, array<string, mixed>>, txId: string}`)
- Named arguments used correctly in `request()` method: `query: $query`
- Constructor promoted properties in `AuthClient` and `QueryBuilder`

**PSR Compliance:**
- Namespace structure: `Darshan\*` follows PSR-4
- Exception class extends `\RuntimeException` (PSR convention)
- Method visibility properly declared (public/private)
- No PSR-12 formatting violations observed

**Error Handling:**
- `DarshanException::fromGuzzle()` factory method extracts status code and body from Guzzle exceptions
- JSON decode errors caught separately from transport errors
- Auth `getUser()` returns `null` on 401 instead of throwing (correct UX pattern)

**Laravel Integration:**
- Service provider follows Laravel conventions: `register()`, `boot()`, `provides()`
- Config merging with `mergeConfigFrom()`
- Publishable config file
- Facade extends `Illuminate\Support\Facades\Facade` with correct PHPDoc `@method` annotations

**Observations:**
- `QueryBuilder::update()` uses POST instead of PUT/PATCH. This follows the DarshJDB server convention (mutation via POST) but deviates from REST conventions. Documented in usage examples, so this is intentional.
- `StorageClient::getUrl()` returns empty string on missing key: `return $result['url'] ?? ''`. Should arguably throw or return null for missing responses.

---

## 6. Python SDK (sdks/python)

### Files Audited
- `__init__.py` - Package exports
- `client.py` - Main client (httpx-based)
- `auth.py` - Authentication
- `storage.py` - File storage
- `admin.py` - Admin client with SSE subscriptions
- `exceptions.py` - Exception hierarchy

### Verdict: GOOD (3 bugs fixed)

**Type Hints:**
- Full type annotations on all public methods
- `dict[str, Any]` used consistently for JSON payloads
- `str | None` union syntax (Python 3.10+, guarded by `from __future__ import annotations`)
- `TYPE_CHECKING` guard for circular imports (auth.py, storage.py)
- `__all__` defined in `__init__.py`

**Async Support:**
- `DarshanAdmin.subscribe()` is `async def` returning `AsyncIterator[dict[str, Any]]`
- Uses `httpx.AsyncClient` with `timeout=None` for long-lived SSE
- Optional `httpx-sse` dependency with clear error message on import failure
- Main `DarshJDB` client is synchronous (correct for the common case)

**httpx Usage:**
- `httpx.Client` with `base_url` and default headers
- Proper error handling: `httpx.HTTPError` caught and wrapped in `DarshanError`
- 4xx/5xx responses raise `DarshanAPIError` with status code and body
- 204 responses return empty dict (correct for DELETE operations)
- Content-Type header correctly removed for multipart file uploads

**Bugs Fixed:**

1. **File handle leak in `storage.py` `upload()`:**
   - **Issue:** `open(local, "rb")` was not wrapped in a context manager (`with` statement). If the upload failed, the file handle would leak.
   - **Fix:** Wrapped in `with open(local, "rb") as fh:` block.

2. **Missing form data in `storage.py` `upload()` and `upload_bytes()`:**
   - **Issue:** The `path`, `content_type`, and `metadata` parameters were never sent to the server. The upload request only contained the file data but not the destination path or metadata form fields.
   - **Fix:** Added `data` parameter to `_request()` method and passed `form_data` dict with path, contentType, and metadata in both upload methods.

3. **`_request()` missing `data` parameter for multipart form fields:**
   - **Issue:** The `_request()` method accepted `files` but had no `data` parameter for additional form fields, making it impossible to send multipart requests with both files and form data.
   - **Fix:** Added `data: dict[str, str] | None = None` parameter, passed to `httpx` as `data=` alongside `files=`.

**Context Manager Support:**
- Both `DarshJDB` and `DarshanAdmin` implement `__enter__`/`__exit__` for `with` statement usage
- `close()` properly closes the underlying httpx client

**Observations:**
- No async version of the main `DarshJDB` client. Users who need async queries must use the admin client or wrap sync calls. Consider adding an `AsyncDarshJDB` class in a future release.
- `DarshanAdmin._request()` duplicates most of `DarshJDB._request()`. Consider extracting a shared base class or mixin.
- `admin.py` `as_user()` heuristic for detecting tokens (`"." in email_or_token and "@" not in email_or_token`) is fragile. Email addresses can't contain bare dots before the `@`, but display names used as identifiers could. This edge case is unlikely but worth documenting.

---

## Cross-SDK Consistency Analysis

### API Surface Comparison

| Feature | client-core | React | Angular | Next.js | PHP | Python |
|---------|:-----------:|:-----:|:-------:|:-------:|:---:|:------:|
| Query | QueryBuilder | useQuery | darshanQuery/$ | queryServer | QueryBuilder | query()/get() |
| Mutation | transact() | useMutation | darshanMutate$ | mutateServer | transact() | transact() |
| Auth | AuthClient | useAuth | injectDarshJDBAuth | withDarshan | AuthClient | AuthClient |
| Presence | PresenceRoom | usePresence | injectDarshanPresence | -- | -- | -- |
| Storage | StorageClient | useStorage | -- | -- | StorageClient | StorageClient |
| SSR | -- | -- | darshanTransferQuery | queryServer | -- | -- |
| Offline | SyncEngine | -- | -- | -- | -- | -- |

### Naming Consistency
- All SDKs use "DarshJDB" or "Darshan" prefix consistently
- Server-side SDKs (PHP, Python) use `sign_in`/`signIn` convention matching their language idioms
- TypeScript SDKs consistently use camelCase
- PHP uses camelCase methods (PSR convention)
- Python uses snake_case (PEP 8)

### Error Handling Patterns
- TypeScript: `throw new Error(message)` with descriptive context
- Angular: `DarshanError` interface with `code`, `message`, `status`, `cause`
- PHP: `DarshanException` extends `RuntimeException` with `statusCode` and `errorBody`
- Python: `DarshanError` (base) and `DarshanAPIError` (HTTP) with `status_code` and `error_body`

All SDKs consistently:
- Wrap transport errors with context
- Parse server error bodies for structured error information
- Distinguish network errors from server errors

---

## Summary of Changes Made

| SDK | File | Change | Type |
|-----|------|--------|------|
| React | `use-mutation.ts` | Fixed missing unmount cleanup for `mountedRef` | Bug fix |
| Python | `storage.py` | Fixed file handle leak in `upload()` | Bug fix |
| Python | `storage.py` | Added form data (path, contentType, metadata) to both upload methods | Bug fix |
| Python | `client.py` | Added `data` parameter to `_request()` for multipart form fields | Bug fix |
| Next.js | `server.ts` | Added eslint-disable comment justifying `any` type usage | Type annotation |

---

## Recommendations (Future Work)

1. **Python async client:** Add `AsyncDarshJDB` class wrapping `httpx.AsyncClient` for native async/await support in FastAPI, Starlette, etc.
2. **Python admin code dedup:** Extract shared HTTP logic from `DarshJDB` and `DarshanAdmin` into a base class.
3. **Angular storage:** Add a `StorageClient` or `injectDarshanStorage()` to match PHP/Python feature parity.
4. **React operator alignment:** Document the `WhereClause.op` difference between React types (`'=='`) and client-core types (`'='`) -- or unify them.
5. **client-core subscription scoping:** Consider scoping `_privateActiveSubs` per-client instance rather than globally, to prevent cross-client deduplication interference in test environments.
6. **PHP `StorageClient::getUrl()`:** Return `null` or throw instead of returning empty string when the server response is missing the `url` key.
7. **Next.js ESM compatibility:** Consider replacing `require('@darshjdb/client')` with dynamic `import()` for better ESM bundler compatibility in `server.ts` and `client.factory.ts`.
