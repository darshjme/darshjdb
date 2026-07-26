# Windows / macOS / Android / iOS support — consolidated build order

Source: four independent read-only specialist audits, 2026-07-25, against this repo.
Every claim below carries a `file:line` citation or is explicitly marked unverified.

---

## 0. Defects found while speccing (not in the original audit)

| # | Defect | Evidence | Severity |
|---|---|---|---|
| D-1 | **TLS listener has no graceful shutdown at all.** `axum_server::bind_rustls(...).serve(...)` is called without `with_graceful_shutdown`, unlike the plain-HTTP branch. Affects **Linux too**, not just Windows. | `main.rs:1305-1308` vs `:1323-1329` | High |
| D-2 | **TS + Python SDKs call `GET /api/health`; the server serves `/health` at root only.** Never worked. Python's tests *mock* `/api/health` and pass — the suite certifies the SDK's own misreading. | `sdks/typescript/src/client.ts:668`, `sdks/python/src/darshjdb/client.py:907` vs `main.rs:1168,1176`; `test_client.py:574-584` | High |
| D-3 | **Sign-in response is a union.** MFA accounts receive `{mfa_required, user_id, mfa_token}` instead of tokens. Every SDK models it as a plain token pair, so any MFA account crashes its client. | `handlers/auth.rs:258-274` | High |
| D-4 | **Device-fingerprint mismatch revokes the whole session.** The client supplies `x-device-fingerprint`; an SDK that regenerates it per launch logs the user out permanently. | `auth/session.rs:475-488`, `handlers/auth.rs:300-315` | High |
| D-5 | Sessions carry a **24h absolute wall-clock cutoff** regardless of activity, independent of the 30-day refresh TTL. Apps must handle daily forced re-login. | `session.rs:286,456-469` | Medium |
| D-6 | **No `.gitattributes`.** Under `core.autocrlf=true` every `.sh` checks out CRLF and dies with `bad interpreter` in WSL/Git-Bash. | `git ls-files --eol` → `i/lf w/crlf` | Medium |
| D-7 | CWD-relative runtime defaults (`./darshan/rules.json`, `./darshan/functions`) silently disable rules and functions when launched from anywhere but the repo root. Under a Windows service, CWD is `System32`. | `config/mod.rs:596,624`; `main.rs:627,798` | Medium |
| D-8 | POSIX `/tmp` fallbacks compiled into production paths resolve to `<drive>:\tmp\` on Windows. Should be `std::env::temp_dir()`. | `main.rs:766-770`, `cmd_start.rs:347`, + 6 test files | Low-Med |

D-1 through D-4 are correctness bugs, not platform work, and belong at the front of the queue.

---

## 1. Architecture decisions

**ADR-M1 — Apple platforms get a native Swift SDK; Kotlin Multiplatform does not ship iOS.**
Reached independently by the Kotlin and macOS specialists. KMP reaches Swift through
Objective-C interop, so suspend functions degrade to completion handlers, sealed classes
lose exhaustiveness, and `Flow` is not `AsyncSequence`; it also costs 6–12 MB per
architecture against roughly zero for URLSession. KMP still covers Android and JVM desktop
from one `commonMain`, leaving `iosMain` addable later without a rewrite.

**ADR-M2 — Contract source of truth is the Rust serde structs, exported via `schemars`.**
utoipa and aide were both rejected as a first step: they require `#[utoipa::path]` and
`ToSchema` across a 6,614-line router with ~120 routes and many ad-hoc `json!` bodies.
Instead, add `#[derive(JsonSchema)]` to roughly 40 wire structs and emit two committed
goldens, `contracts/openapi.json` and `contracts/ws-protocol.json`, with a CI job that
regenerates them and runs `git diff --exit-code`.

The repo currently holds **two** hand-maintained contract mirrors that derive from nothing:
`api/openapi.rs` (3,539 lines of `json!`) and `api/sdk_types.rs` (a string-concatenated
`.d.ts` generator). That is the root cause of the drift class.

**ADR-M3 — Full client codegen is rejected for all five SDKs.** Generated *types* plus
hand-written ergonomics plus mandatory contract tests. Codegen would destroy the
hand-crafted `select('users:darsh')` / `relate()` / live-stream ergonomics that are the
SDKs' actual value.

---

## 2. Why a sixth drift bug will not happen

Four bugs were hand-fixed and a fifth (D-2) still survived. Hand-fixing does not scale.

- **Kotlin:** `JsonNamingStrategy.SnakeCase` and `classDiscriminator = "type"` set globally,
  so a property named `accessToken` *cannot* decode anything but `access_token`. A
  detekt/Konsist rule forbids `@SerialName` on model properties.
- **Swift:** one blessed `DDBJSONCoding` with `.convertFromSnakeCase`; models carry **no**
  manual `CodingKeys` for snake_case fields, so there is no hand-typed string to get wrong.
  Manual keys are permitted only for the WebSocket kebab-case type tags, as enum raw values.
- **All five:** golden fixtures captured from a live server, plus `tests/contract/` suites
  run against a real `ddb-server` across 12 numbered scenarios, including negative
  assertions such as `{operations:[...]}` returning 4xx. A scenario ID missing from a suite
  fails that suite. Mock-based SDK tests are demoted to unit tests with no contract authority.

Protocol note every SDK must encode: the wire has **two casing regimes** — kebab-case
`type` tags (`ws.rs:142,200`) and snake_case fields everywhere else.

---

## 3. Build order

**Wave 1 — blocks the new SDKs, serial:** route D-1 through D-4 into the fix queue (for D-2,
add an `/api/health` alias server-side *and* fix the SDKs, following the existing
`/sql/darshql` alias precedent at `rest.rs:645`); land `contracts/` plus the schemars wiring
and freshness job; build `tests/contract/` and wire TS, Python and PHP into it.

**Wave 2 — parallel, independent of Wave 1:** expand the release matrix to add
`aarch64-unknown-linux-musl` (native on the free `ubuntu-24.04-arm` runner),
`x86_64-apple-darwin` (cross-compiled from macos-latest arm64) and optionally
`aarch64-pc-windows-msvc`; ship `.tar.gz`/`.zip` rather than bare binaries and add
`SHA256SUMS.txt`. **Preserve the triple-named CLI copies** — `self_update` resolves assets by
target triple (`release.yml:90-92`), so renaming breaks `ddb upgrade` for every installed
CLI. Windows host work runs shutdown/SCM fix → `windows-service` + `ddb service` → WiX v5
MSI. macOS host work runs Homebrew tap and formula with a `service do` block (which gives
launchd for free) → codesign and notarytool in CI → optional stapled `.dmg`. Skip `.pkg`.

**Wave 3 — gated on Wave 1:** Swift and Kotlin SDKs in parallel, each shipping its
`tests/contract/{swift,kotlin}` suite **in the same PR**; an SDK without a contract suite is
rejected at review. The README parity table and versioning policy come last, so they are
true on day one.

### Effort

| Workstream | Estimate |
|---|---|
| Contract SSOT + contract-test harness | ~5–6 d |
| Release matrix, artifacts, checksums | ~1 d |
| Windows (shutdown, service, MSI, winget) | ~6–8 d |
| macOS (universal, brew, sign/notarize, launchd) | ~3–4 d |
| Swift SDK v1 | ~8–10 d |
| Kotlin SDK v1 | ~18–19 d |

---

## 4. Owner-held credentials and spend

| Item | Cost | Blocks |
|---|---|---|
| Apple Developer Program | **$99/yr** | Notarized macOS binaries (Gatekeeper). The SDK itself needs none. |
| Windows code signing — Azure Trusted Signing preferred | **~$10/mo** (vs $200–400/yr for an OV cert) | SmartScreen on the MSI and exe. OIDC means no long-lived secrets. Public-trust onboarding requires identity validation; individual eligibility is **unverified**. |
| `TAP_GITHUB_TOKEN` PAT + a `homebrew-tap` repo | free | `brew install ddb` |
| winget-pkgs fork PAT | free | winget publishing |
| Maven Central namespace (`io.github.<user>` auto-verifies) + GPG key | free | Kotlin SDK publishing |
| PyPI trusted publisher, Packagist link, chocolatey.org API key | free | existing and new SDK publishing |
| CI runners (ARM and macOS) | free for public repos | — |

Since the June 2023 CA/Browser Forum changes, **no CA issues file-based PFX certificates** —
code-signing keys must live in hardware or an HSM, so a certificate stored in a GitHub
secret is no longer possible.

---

## 5. Known platform gaps

- **pgvector is absent from embedded Postgres on *every* platform**, not just macOS arm64 —
  zonky bundles ship no pgvector, since it is a third-party extension rather than contrib.
  Degradation is graceful and verified: the pgvector migration runs in its own transaction,
  rolls back on failure, is not recorded, retries on next boot, and never aborts startup
  (`migrations.rs:16-22`). Embedded mode works minus `/api/search/semantic`,
  `/api/search/hybrid` and `/api/embeddings*`; `/api/search/text` still works.
- **pg-embed has no windows-arm64 build** (upstream supports Windows amd64 and i386 only),
  so `embedded-db` is impossible on Windows ARM. Not blocking, since the feature is not in
  `default` and release builds pass no `--features`.
- Embedded Postgres on Windows x64 is *expected* to work but is **untested** — no CI job runs
  `--features embedded-db` on Windows. Do not advertise it until a smoke test exists.
- **Vector search and agent memory — the two features named in the project description — are
  implemented by none of the three existing SDKs.**
- The PHP SDK has no realtime support at all (no WebSocket, no SSE).
- The `x86_64-apple-darwin` cross-compile is dependency-level confidence only; one CI run
  closes it.
