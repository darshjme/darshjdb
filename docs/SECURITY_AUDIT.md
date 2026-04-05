# DarshJDB Security Audit Report

**Date:** 2026-04-05
**Auditor:** Systems Security Engineer
**Scope:** Full codebase -- Rust server, CLI, TypeScript SDKs, Docker/K8s infrastructure, CI/CD

---

## Executive Summary

DarshJDB demonstrates strong security fundamentals: the Dockerfile runs as a non-root user, password hashing uses Argon2id with OWASP-recommended parameters, JWT tokens use RS256 with key rotation, refresh tokens implement rotation with device fingerprint binding, the storage layer has path traversal protection, rate limiting is implemented, and OAuth state parameters are HMAC-signed with PKCE enforced.

However, there are several findings that must be addressed before production deployment, ranging from hardcoded default credentials in infrastructure files to missing CORS configuration.

---

## 1. Hardcoded Secrets and Default Credentials

### CRITICAL: Default JWT Secret in docker-compose.yml and K8s values

**Files:**
- `docker-compose.yml:10` -- `DDB_JWT_SECRET: ${DDB_JWT_SECRET:-change-me-in-production}`
- `deploy/k8s/values.yaml:55` -- `jwtSecret: change-me-in-production`

**Risk:** If operators deploy without setting the environment variable, the JWT signing secret is a publicly known string. Any attacker can forge valid JWTs and impersonate any user including admins.

**Recommendation:** Remove the default fallback entirely. The server should refuse to start if `DDB_JWT_SECRET` is not set or is set to the placeholder value. Add a startup check in `packages/server/src/main.rs`:

```rust
let jwt_secret = std::env::var("DDB_JWT_SECRET")
    .expect("DDB_JWT_SECRET must be set");
if jwt_secret == "change-me-in-production" || jwt_secret.len() < 32 {
    panic!("DDB_JWT_SECRET is insecure -- set a strong random value");
}
```

### HIGH: Default Postgres Password

**Files:**
- `docker-compose.yml:23` -- `POSTGRES_PASSWORD: ${POSTGRES_PASSWORD:-darshan}`
- `docker-compose.dev.yml` (implicit via base compose)
- `deploy/k8s/values.yaml:44` -- `password: darshan`
- `.github/workflows/ci.yml:22` -- `POSTGRES_PASSWORD: darshan`
- `packages/cli/src/main.rs:249` -- hardcoded `POSTGRES_PASSWORD=darshan` in dev Docker command

**Risk:** The CI password is acceptable for ephemeral test containers. The K8s `values.yaml` default is dangerous -- operators may deploy the Helm chart without overriding.

**Recommendation:**
- K8s `values.yaml`: Remove the default password. Require it as a required value or generate it with `randAlphaNum` in the Helm template.
- `docker-compose.yml`: Already uses env-var substitution which is acceptable, but document prominently that the default must be changed.
- CLI `cmd_dev`: This is dev-only and acceptable, but add a comment noting it is local-only.

### MEDIUM: pgAdmin and Grafana Default Credentials

**File:** `docker-compose.dev.yml`
- Line 24: `PGADMIN_DEFAULT_PASSWORD: admin`
- Line 49: `GF_SECURITY_ADMIN_PASSWORD: admin`

**Risk:** These are in the dev-only compose file, which mitigates the severity. However, if the dev compose is accidentally used in a staging environment, these become exposed admin panels.

**Recommendation:** Use environment variable substitution with no default, or at minimum document that this file must never be used outside local development.

### LOW: Hardcoded Dev Database URL in CLI

**File:** `packages/cli/src/main.rs:298-300`
```rust
.env("DATABASE_URL", "postgres://postgres:darshan@localhost:5432/darshjdb")
```

**Risk:** Dev-only, local-only. Acceptable but worth noting the password is `ddb`.

---

## 2. Command Injection Analysis

### FINDING: No Command Injection Vulnerabilities Detected

The CLI (`packages/cli/src/main.rs`) uses `tokio::process::Command` with argument arrays, not string interpolation into shell commands. Specifically:

- `cmd_dev` (line 233-294): Docker and cargo invoked with `.args([...])` -- safe.
- `cmd_deploy` (line 342-355): Docker build/push with `.args([...])` and the `tag`/`image` variables are passed as discrete arguments, not shell-interpolated -- safe.

The function runtime (`packages/server/src/functions/runtime.rs`) also uses `Command::new()` with `.arg()` chains. Function file paths come from the `FunctionDef.file_path` which is loaded from the registry, not directly from user HTTP input.

**Assessment:** No command injection vectors found. The codebase consistently uses Rust's `Command` API correctly.

### NOTE: Function Runtime Subprocess Security

**File:** `packages/server/src/functions/runtime.rs:253-257`

Deno is invoked with `--allow-net`, `--allow-read`, `--allow-env`. This grants user functions:
- Full network access (could exfiltrate data)
- Full filesystem read access (could read server config, secrets)
- Full environment variable access (could read `DATABASE_URL`, `DDB_JWT_SECRET`)

**Recommendation:**
- `--allow-env`: Restrict to specific variables: `--allow-env=NODE_ENV,DDB_PORT`
- `--allow-read`: Restrict to the functions directory: `--allow-read=./darshan/functions`
- `--allow-net`: Consider restricting to specific domains or blocking metadata endpoints (169.254.169.254)
- Add `--no-prompt` to prevent interactive permission requests

---

## 3. Dockerfile Security Assessment

### GOOD: Non-Root Execution

**File:** `Dockerfile:41-51`

The runtime stage creates a dedicated `ddb` user and group, sets `USER ddb`, and the workdir is owned by this user. This is correct.

### GOOD: Multi-Stage Build

Build tools and source code are not present in the final image. Only compiled binaries and static frontend assets are copied.

### GOOD: Minimal Base Image

Uses `alpine:3.21` with only `ca-certificates` and `tini` installed.

### GOOD: Health Check

Line 59-60: Proper health check with reasonable intervals.

### MINOR: No Read-Only Filesystem Directive

**Recommendation:** Add `--read-only` to Docker run configurations or document that `/app` should be mounted read-only where possible. The `DDB_ADMIN_DIR` points to `/usr/share/darshan/admin` which is static content.

### MINOR: No COPY --chown

Line 45-47: Files are copied as root then `chown -R` is run. Using `COPY --chown=ddb:ddb` is slightly more efficient and avoids the extra layer.

---

## 4. Docker Compose Security Assessment

### HIGH: Postgres Port Exposed to Host

**File:** `docker-compose.yml:25` -- `ports: ["5432:5432"]`

The production compose file exposes Postgres directly on the host. In a production deployment, Postgres should only be accessible via the internal Docker network.

**Recommendation:** Remove the `ports` mapping from the production compose. Keep it only in `docker-compose.dev.yml` (which already has it at line 17).

### MEDIUM: No Network Isolation for Admin Services

**File:** `docker-compose.dev.yml`

pgAdmin (port 5050), Prometheus (port 9090), and Grafana (port 3000) are all exposed on 0.0.0.0. If the dev machine is on a shared network, these are accessible to anyone.

**Recommendation:** Bind to localhost: `"127.0.0.1:5050:80"`, `"127.0.0.1:9090:9090"`, `"127.0.0.1:3000:3000"`.

---

## 5. CORS Configuration -- MISSING

### HIGH: No CORS Layer Configured

The codebase imports `tower-http` with the `cors` feature in `Cargo.toml` (line 39), but **no `CorsLayer` is applied anywhere** in the router setup (`packages/server/src/api/rest.rs`). There is zero CORS configuration.

**Risk:** Without CORS headers, the API cannot be called from browser-based frontend applications (the React, Angular, and Next.js SDKs). Alternatively, if a reverse proxy adds `Access-Control-Allow-Origin: *`, it opens the API to cross-origin attacks.

**Recommendation:** Add explicit CORS configuration in `build_router()`:

```rust
use tower_http::cors::{CorsLayer, Any};

let cors = CorsLayer::new()
    .allow_origin(/* configured allowed origins */)
    .allow_methods([Method::GET, Method::POST, Method::PATCH, Method::DELETE])
    .allow_headers([AUTHORIZATION, CONTENT_TYPE, ACCEPT])
    .allow_credentials(true);

Router::new()
    // ... routes ...
    .layer(cors)
```

The allowed origins should be configurable via environment variable, not hardcoded.

---

## 6. .gitignore Verification

### PASS: .env Files Are Blocked

**File:** `.gitignore:13-15`

```
.env
.env.local
.env.*.local
```

This correctly blocks `.env`, `.env.local`, and `.env.production.local` etc.

### PASS: No .env Files Exist in Repository

Glob scan confirms zero `.env` files are present in the working tree.

### NOTE: Missing Patterns

The `.gitignore` does not block:
- `.env.production` or `.env.staging` (only `.env.*.local` is blocked)
- `ddb.toml` files that may contain `[server].token`

**Recommendation:** Add:
```
.env.*
!.env.example
ddb.toml
```

---

## 7. Authentication and Cryptography Assessment

### STRONG: Password Hashing

**File:** `packages/server/src/auth/providers.rs:42-50`

Argon2id with 64 MiB memory, 3 iterations, parallelism 4. This matches OWASP 2024 recommendations.

### STRONG: Timing-Safe Authentication

**File:** `packages/server/src/auth/providers.rs:96-108`

When an email is not found, a dummy Argon2 verification is performed to prevent timing side-channel attacks that reveal email existence.

### STRONG: JWT Implementation

**File:** `packages/server/src/auth/session.rs`

- RS256 algorithm (asymmetric, more secure than HS256)
- Key rotation with grace period for previous key
- Short access token lifetime (15 minutes)
- Refresh token rotation with device fingerprint binding
- Token theft detection (device fingerprint mismatch triggers full session revocation)
- Refresh tokens stored as SHA-256 hashes only

### STRONG: Magic Link Security

**File:** `packages/server/src/auth/providers.rs:146-244`

- 32-byte random tokens (256 bits of entropy)
- Only SHA-256 hash stored in DB
- 15-minute expiry
- One-time use with atomic consumption (race condition handled)

### STRONG: OAuth2 Security

- PKCE S256 enforced on all providers
- State parameter HMAC-signed to prevent CSRF
- State verification before code exchange

### STRONG: MFA Implementation

**File:** `packages/server/src/auth/mfa.rs`

- TOTP with +/-1 step window
- Recovery codes Argon2id-hashed before storage

### NOTE: No Unsafe Code

Zero `unsafe` blocks in the entire Rust codebase.

---

## 8. Storage Security Assessment

### STRONG: Path Traversal Protection

**File:** `packages/server/src/storage/mod.rs:286-326`

The `resolve_path` function implements defense in depth:
1. Rejects null bytes
2. Rejects empty paths
3. Rejects absolute paths
4. Rejects `..` components via `Component::ParentDir` check
5. Verifies resolved path is still under storage root (canonicalization check)

### GOOD: Content Type Blocking

**File:** `packages/server/src/storage/mod.rs:684-690`

Blocks executable content types: `x-executable`, `x-msdos-program`, `x-msdownload`, `x-sh`, `x-shellscript`.

### NOTE: Upload Size Limit

100 MiB default (`DEFAULT_MAX_UPLOAD_SIZE`). Ensure this is enforced at the Axum layer as well, not just in application code.

---

## 9. Rate Limiting Assessment

### GOOD: Implemented and Tested

**File:** `packages/server/src/auth/middleware.rs`

- Token-bucket algorithm
- Differentiated limits: 100/min authenticated, 20/min anonymous
- Automatic stale bucket cleanup
- Unit tests for both limits and cleanup
- IP-based for anonymous, token-hash-based for authenticated

### NOTE: Token Prefix Rate Limiting

Line 90: Uses first 16 bytes of token for rate limiting bucket key before JWT validation. This is a reasonable design to rate-limit before expensive crypto operations.

---

## 10. CI/CD Security

### GOOD: GitHub Actions Workflow

- Uses pinned action versions (`@v4`, `@v3`, `@v6`)
- Docker credentials use `secrets.GITHUB_TOKEN` (not hardcoded)
- Minimal permissions declared (`contents: read`, `packages: write`)
- Build cache uses GitHub Actions cache (not external)

### NOTE: CI Postgres Password

**File:** `.github/workflows/ci.yml:22` -- `POSTGRES_PASSWORD: darshan`

Acceptable for ephemeral CI containers. These containers are destroyed after the workflow run.

---

## Findings Summary

| # | Severity | Finding | Location |
|---|----------|---------|----------|
| 1 | CRITICAL | Default JWT secret `change-me-in-production` | docker-compose.yml, k8s/values.yaml |
| 2 | HIGH | Postgres port exposed in production compose | docker-compose.yml:25 |
| 3 | HIGH | No CORS layer configured | api/rest.rs |
| 4 | HIGH | K8s Helm values.yaml ships with default DB password | deploy/k8s/values.yaml:44 |
| 5 | MEDIUM | pgAdmin/Grafana default `admin` passwords in dev compose | docker-compose.dev.yml |
| 6 | MEDIUM | Deno sandbox too permissive (--allow-env, --allow-read) | functions/runtime.rs:253-257 |
| 7 | LOW | .gitignore missing `.env.production` pattern | .gitignore |
| 8 | LOW | Hardcoded dev DB password in CLI | cli/src/main.rs:249 |
| 9 | INFO | No COPY --chown in Dockerfile | Dockerfile:45-47 |
| 10 | INFO | No read-only filesystem directive | Dockerfile |

## Positive Findings (No Action Required)

- Non-root Docker container with dedicated user
- Multi-stage Docker build
- Argon2id password hashing with OWASP parameters
- Timing-safe authentication
- RS256 JWT with key rotation
- Refresh token rotation with device fingerprint binding
- Path traversal protection in storage layer
- Content type blocking for executables
- Token-bucket rate limiting
- HMAC-signed OAuth state with PKCE
- Parameterized SQL queries throughout (no SQL injection)
- No command injection vectors
- Zero `unsafe` code blocks
- MFA with hashed recovery codes

---

## Recommended Priority Actions

1. **Immediately:** Add startup validation that rejects `change-me-in-production` as JWT secret
2. **Before production:** Remove Postgres port mapping from `docker-compose.yml`
3. **Before production:** Implement CORS layer with configurable allowed origins
4. **Before production:** Tighten Deno subprocess permissions
5. **Before production:** Remove default password from K8s `values.yaml`
6. **Housekeeping:** Expand `.gitignore` patterns for all `.env.*` variants
