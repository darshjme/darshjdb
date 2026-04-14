// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
//! Typed, layered configuration for DarshJDB (Slice 17 — Prometheus).
//!
//! # Design
//!
//! Configuration is a single strongly-typed tree, [`DdbConfig`], grouped by
//! subsystem: server, database, auth, cors, dev, cache, embedding, llm,
//! storage, schema, anchor, memory, rules.  Every field has a documented
//! default so a freshly-cloned repo boots with zero setup.
//!
//! # Sources (last wins)
//!
//! 1. Built-in defaults (via `#[serde(default)]` on every field).
//! 2. `config.toml`    (optional, repo-local)
//! 3. `config.local.toml` (optional, gitignored, per-developer overrides)
//! 4. Environment variables with prefix `DDB__` (double-underscore separator).
//! 5. Environment variables with prefix `DARSH__` (reserved for cross-cutting
//!    knobs such as blockchain anchoring).
//! 6. Backward-compat shim: legacy *flat* env vars (`DATABASE_URL`, `DDB_PORT`,
//!    `DDB_DEV`, `DDB_TLS_CERT`, etc.) are mapped into the typed struct BEFORE
//!    the `config::Config` builder runs, so existing deployments keep working.
//!
//! # Env var mapping examples
//!
//! | Env var                               | Field                           |
//! |---------------------------------------|---------------------------------|
//! | `DDB__SERVER__PORT=8080`              | `server.port`                   |
//! | `DDB__SERVER__BIND_ADDR=0.0.0.0`      | `server.bind_addr`              |
//! | `DDB__DATABASE__POOL_MAX=32`          | `database.pool_max`             |
//! | `DDB__AUTH__JWT_EXPIRY_SECONDS=900`   | `auth.jwt_expiry_seconds`       |
//! | `DDB__CORS__ORIGINS=["https://a"]`    | `cors.origins`                  |
//! | `DDB__EMBEDDING__PROVIDER=openai`     | `embedding.provider`            |
//! | `DARSH__ANCHOR__CHAIN=ethereum`       | `anchor.chain`                  |
//!
//! # Secret handling
//!
//! Secret fields (`jwt_secret`, `api_key`, `cache_password`) are wrapped in
//! [`Secret<T>`], a small newtype whose `Debug` impl prints `<redacted>` so
//! `tracing::info!(?cfg, "...")` at startup is safe.

use serde::{Deserialize, Serialize};
use std::fmt;

// ---------------------------------------------------------------------------
// Secret wrapper — masks the inner value in Debug output.
// ---------------------------------------------------------------------------

/// A newtype that redacts its inner value in `Debug` output.
///
/// Deserializes transparently from the inner type so config files and env
/// vars populate it without ceremony.  `Display` is intentionally NOT
/// implemented to force callers to reach in via [`Secret::expose`].
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(transparent)]
pub struct Secret<T>(T);

impl<T> Secret<T> {
    /// Construct from the raw inner value.
    pub fn new(value: T) -> Self {
        Self(value)
    }

    /// Borrow the inner secret.  Call sites should be audited.
    pub fn expose(&self) -> &T {
        &self.0
    }

    /// Consume and return the inner secret.
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

// ---------------------------------------------------------------------------
// Root configuration
// ---------------------------------------------------------------------------

/// The fully-typed DarshJDB configuration tree.
///
/// Every section is flattened under a named field so env-var overrides work
/// predictably: `DDB__<SECTION>__<FIELD>=value`.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DdbConfig {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub database: DatabaseConfig,
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub cors: CorsConfig,
    #[serde(default)]
    pub dev: DevConfig,
    #[serde(default)]
    pub cache: CacheConfig,
    #[serde(default)]
    pub embedding: EmbeddingConfig,
    #[serde(default)]
    pub llm: LlmConfig,
    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default)]
    pub schema: SchemaConfig,
    #[serde(default)]
    pub anchor: AnchorConfig,
    #[serde(default)]
    pub memory: MemoryConfig,
    #[serde(default)]
    pub rules: RulesConfig,
}

// ---------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    /// Advertised host name (used for log banners, redirects, function URL).
    #[serde(default = "defaults::server_host")]
    pub host: String,
    /// Primary HTTP(S) listen port. Default **7700**.
    #[serde(default = "defaults::server_port")]
    pub port: u16,
    /// Embedded cache port (reserved, default **7701**).
    #[serde(default = "defaults::server_cache_port")]
    pub cache_port: u16,
    /// Bind address for the listener.  Defaults to `0.0.0.0`.
    #[serde(default = "defaults::server_bind_addr")]
    pub bind_addr: String,
    /// Optional TLS certificate path (PEM).  When present with
    /// `tls_key_path`, the server binds with rustls.
    #[serde(default)]
    pub tls_cert_path: Option<String>,
    /// Optional TLS private key path (PEM).
    #[serde(default)]
    pub tls_key_path: Option<String>,
    /// `tracing_subscriber` env filter (e.g. `info`, `ddb_server=debug`).
    #[serde(default = "defaults::server_log_level")]
    pub log_level: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: defaults::server_host(),
            port: defaults::server_port(),
            cache_port: defaults::server_cache_port(),
            bind_addr: defaults::server_bind_addr(),
            tls_cert_path: None,
            tls_key_path: None,
            log_level: defaults::server_log_level(),
        }
    }
}

// ---------------------------------------------------------------------------
// Database
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DatabaseConfig {
    /// Postgres connection URL.  When `None`, [`load_config`] falls back to
    /// `postgres://ddb:ddb@localhost:5432/darshjdb`.
    ///
    /// Wrapped in [`Secret<T>`] because the URL typically embeds the password
    /// (e.g. `postgres://user:password@host/db`).  Since `DdbConfig` derives
    /// `Debug` and `main.rs` logs `?cfg` at startup, a bare `String` here
    /// would ship the connection password to the tracing sink on every boot
    /// (security audit finding F1, 2026-04-15).
    #[serde(default)]
    pub url: Option<Secret<String>>,
    /// Minimum idle pool connections.  Default **2**.
    #[serde(default = "defaults::db_pool_min")]
    pub pool_min: u32,
    /// Maximum pool connections.  Default **20**.
    #[serde(default = "defaults::db_pool_max")]
    pub pool_max: u32,
    /// Seconds to wait to acquire a connection before erroring.  Default **5**.
    #[serde(default = "defaults::db_acquire_timeout_secs")]
    pub acquire_timeout_secs: u64,
    /// Seconds an idle connection may linger before being released.
    /// Default **600** (10 minutes).
    #[serde(default = "defaults::db_idle_timeout_secs")]
    pub idle_timeout_secs: u64,
    /// Hard cap on a connection's lifetime in seconds.  Default **1800**.
    #[serde(default = "defaults::db_max_lifetime_sec")]
    pub max_lifetime_sec: u64,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: None,
            pool_min: defaults::db_pool_min(),
            pool_max: defaults::db_pool_max(),
            acquire_timeout_secs: defaults::db_acquire_timeout_secs(),
            idle_timeout_secs: defaults::db_idle_timeout_secs(),
            max_lifetime_sec: defaults::db_max_lifetime_sec(),
        }
    }
}

// ---------------------------------------------------------------------------
// Auth
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AuthConfig {
    /// Shared HMAC secret for HS256 JWTs (dev/single-node deployments).
    #[serde(default)]
    pub jwt_secret: Option<Secret<String>>,
    /// Path to the RS256 private key PEM (production).  When both this and
    /// `jwt_public_key_path` are set, the server uses RS256 signing.
    #[serde(default)]
    pub jwt_private_key_path: Option<String>,
    /// Path to the RS256 public key PEM (production).
    #[serde(default)]
    pub jwt_public_key_path: Option<String>,
    /// Access-token lifetime in seconds.  Default **900** (15 minutes).
    #[serde(default = "defaults::auth_jwt_expiry_seconds")]
    pub jwt_expiry_seconds: u64,
    /// Refresh-token lifetime in hours.  Default **720** (30 days).
    #[serde(default = "defaults::auth_refresh_expiry_hours")]
    pub refresh_expiry_hours: u64,
    /// Absolute session ceiling in hours.  Default **8760** (1 year).
    #[serde(default = "defaults::auth_session_absolute_hours")]
    pub session_absolute_hours: u64,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            jwt_secret: None,
            jwt_private_key_path: None,
            jwt_public_key_path: None,
            jwt_expiry_seconds: defaults::auth_jwt_expiry_seconds(),
            refresh_expiry_hours: defaults::auth_refresh_expiry_hours(),
            session_absolute_hours: defaults::auth_session_absolute_hours(),
        }
    }
}

// ---------------------------------------------------------------------------
// CORS
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CorsConfig {
    /// Explicit allow-list of origins.  A single entry of `"*"` means
    /// wildcard.  Empty means "use dev-mode heuristics" (localhost-only in
    /// dev, deny-all in prod).
    #[serde(default)]
    pub origins: Vec<String>,
}

// ---------------------------------------------------------------------------
// Dev
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DevConfig {
    /// When `true`, relaxes auth checks and widens CORS to localhost.
    #[serde(default)]
    pub mode: bool,
    /// Dev-mode bind override (rarely used).  Defaults to `127.0.0.1`.
    #[serde(default = "defaults::dev_bind_addr")]
    pub bind_addr: String,
}

impl Default for DevConfig {
    fn default() -> Self {
        Self {
            mode: false,
            bind_addr: defaults::dev_bind_addr(),
        }
    }
}

// ---------------------------------------------------------------------------
// Cache
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CacheConfig {
    /// L1 (in-process) cache size cap in bytes.  Default **128 MiB**.
    #[serde(default = "defaults::cache_l1_max_bytes")]
    pub l1_max_bytes: u64,
    /// Default TTL for L1 entries in seconds.  Default **300**.
    #[serde(default = "defaults::cache_l1_ttl_default_sec")]
    pub l1_ttl_default_sec: u64,
    /// Optional password for the networked cache tier (Redis-compatible).
    #[serde(default)]
    pub cache_password: Option<Secret<String>>,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            l1_max_bytes: defaults::cache_l1_max_bytes(),
            l1_ttl_default_sec: defaults::cache_l1_ttl_default_sec(),
            cache_password: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Embedding
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingConfig {
    /// Provider selector.  One of `none`, `openai`, `ollama`, `anthropic`.
    #[serde(default = "defaults::embedding_provider")]
    pub provider: String,
    /// Model identifier (provider-specific, e.g. `text-embedding-3-small`).
    #[serde(default = "defaults::embedding_model")]
    pub model: String,
    /// API key (OpenAI / Anthropic).
    #[serde(default)]
    pub api_key: Option<Secret<String>>,
    /// Endpoint override (Ollama base URL, OpenAI-compatible proxies).
    #[serde(default)]
    pub endpoint: Option<String>,
    /// Embedding vector dimensions.  Default **1536**.
    #[serde(default = "defaults::embedding_dimensions")]
    pub dimensions: usize,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            provider: defaults::embedding_provider(),
            model: defaults::embedding_model(),
            api_key: None,
            endpoint: None,
            dimensions: defaults::embedding_dimensions(),
        }
    }
}

// ---------------------------------------------------------------------------
// LLM
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LlmConfig {
    /// Provider selector.  One of `none`, `openai`, `anthropic`, `ollama`,
    /// `nvidia`.
    #[serde(default = "defaults::llm_provider")]
    pub provider: String,
    /// Model identifier.
    #[serde(default = "defaults::llm_model")]
    pub model: String,
    /// API key.
    #[serde(default)]
    pub api_key: Option<Secret<String>>,
    /// Base URL override (for self-hosted / proxy deployments).
    #[serde(default)]
    pub base_url: Option<String>,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: defaults::llm_provider(),
            model: defaults::llm_model(),
            api_key: None,
            base_url: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StorageConfig {
    /// Backend kind.  One of `local`, `s3`.
    #[serde(default = "defaults::storage_backend")]
    pub backend: String,
    /// Local filesystem path (used when `backend == "local"`).
    #[serde(default = "defaults::storage_path")]
    pub path: String,
    /// S3 bucket name.
    #[serde(default)]
    pub bucket: Option<String>,
    /// S3 region.
    #[serde(default)]
    pub region: Option<String>,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            backend: defaults::storage_backend(),
            path: defaults::storage_path(),
            bucket: None,
            region: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Schema mode
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SchemaConfig {
    /// Schema enforcement mode.  `flexible` accepts unknown attributes;
    /// `strict` rejects them (SurrealDB SCHEMAFULL equivalent).
    #[serde(default = "defaults::schema_mode")]
    pub mode: String,
}

impl Default for SchemaConfig {
    fn default() -> Self {
        Self {
            mode: defaults::schema_mode(),
        }
    }
}

// ---------------------------------------------------------------------------
// Anchor (blockchain commit)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AnchorConfig {
    /// Chain selector.  One of `none`, `ipfs`, `ethereum`.
    #[serde(default = "defaults::anchor_chain")]
    pub chain: String,
    /// Commit root every N transactions.  Default **1000**.
    #[serde(default = "defaults::anchor_every_n_tx")]
    pub every_n_tx: u64,
}

impl Default for AnchorConfig {
    fn default() -> Self {
        Self {
            chain: defaults::anchor_chain(),
            every_n_tx: defaults::anchor_every_n_tx(),
        }
    }
}

// ---------------------------------------------------------------------------
// Memory (agent memory tiers)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryConfig {
    /// Working-tier size (hot in-memory recall).  Default **64**.
    #[serde(default = "defaults::memory_working_tier_size")]
    pub working_tier_size: usize,
    /// Episodic-tier size (warm recall).  Default **2048**.
    #[serde(default = "defaults::memory_episodic_tier_size")]
    pub episodic_tier_size: usize,
    /// Threshold at which episodic memories get summarised.  Default **256**.
    #[serde(default = "defaults::memory_summarise_threshold")]
    pub summarise_threshold: usize,
    /// λ in the exponential importance decay.  Default **0.001**.
    #[serde(default = "defaults::memory_importance_decay_lambda")]
    pub importance_decay_lambda: f64,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            working_tier_size: defaults::memory_working_tier_size(),
            episodic_tier_size: defaults::memory_episodic_tier_size(),
            summarise_threshold: defaults::memory_summarise_threshold(),
            importance_decay_lambda: defaults::memory_importance_decay_lambda(),
        }
    }
}

// ---------------------------------------------------------------------------
// Rules
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RulesConfig {
    /// Path to the JSON rules file.  Default `./darshan/rules.json`.
    #[serde(default = "defaults::rules_file_path")]
    pub file_path: String,
}

impl Default for RulesConfig {
    fn default() -> Self {
        Self {
            file_path: defaults::rules_file_path(),
        }
    }
}

// ---------------------------------------------------------------------------
// Defaults — a private module so each value has one canonical source.
// ---------------------------------------------------------------------------

mod defaults {
    pub fn server_host() -> String {
        "localhost".to_string()
    }
    pub fn server_port() -> u16 {
        7700
    }
    pub fn server_cache_port() -> u16 {
        7701
    }
    pub fn server_bind_addr() -> String {
        "0.0.0.0".to_string()
    }
    pub fn server_log_level() -> String {
        "info".to_string()
    }

    pub fn db_pool_min() -> u32 {
        2
    }
    pub fn db_pool_max() -> u32 {
        20
    }
    pub fn db_acquire_timeout_secs() -> u64 {
        5
    }
    pub fn db_idle_timeout_secs() -> u64 {
        600
    }
    pub fn db_max_lifetime_sec() -> u64 {
        1800
    }

    pub fn auth_jwt_expiry_seconds() -> u64 {
        900
    }
    pub fn auth_refresh_expiry_hours() -> u64 {
        720
    }
    pub fn auth_session_absolute_hours() -> u64 {
        8760
    }

    pub fn dev_bind_addr() -> String {
        "127.0.0.1".to_string()
    }

    pub fn cache_l1_max_bytes() -> u64 {
        128 * 1024 * 1024
    }
    pub fn cache_l1_ttl_default_sec() -> u64 {
        300
    }

    pub fn embedding_provider() -> String {
        "none".to_string()
    }
    pub fn embedding_model() -> String {
        "text-embedding-3-small".to_string()
    }
    pub fn embedding_dimensions() -> usize {
        1536
    }

    pub fn llm_provider() -> String {
        "none".to_string()
    }
    pub fn llm_model() -> String {
        "gpt-4o-mini".to_string()
    }

    pub fn storage_backend() -> String {
        "local".to_string()
    }
    pub fn storage_path() -> String {
        "./darshan/storage".to_string()
    }

    pub fn schema_mode() -> String {
        "flexible".to_string()
    }

    pub fn anchor_chain() -> String {
        "none".to_string()
    }
    pub fn anchor_every_n_tx() -> u64 {
        1000
    }

    pub fn memory_working_tier_size() -> usize {
        64
    }
    pub fn memory_episodic_tier_size() -> usize {
        2048
    }
    pub fn memory_summarise_threshold() -> usize {
        256
    }
    pub fn memory_importance_decay_lambda() -> f64 {
        0.001
    }

    pub fn rules_file_path() -> String {
        "./darshan/rules.json".to_string()
    }
}

// ---------------------------------------------------------------------------
// Backward-compat shim — map legacy flat env vars into `DDB__*` namespace
// before the `config::Config` builder runs.
// ---------------------------------------------------------------------------

/// Translate legacy flat env vars (`DDB_PORT`, `DATABASE_URL`, `DDB_DEV`,
/// `DDB_TLS_CERT`, etc.) into the new prefixed `DDB__SECTION__FIELD` form.
///
/// Legacy vars are *only* copied when the new form is not already set, so a
/// user running the new scheme always wins.  This function touches
/// `std::env` — call it **before** building `config::Config`.
fn apply_legacy_env_shim() {
    // Helper: only copy when the new form is unset AND the old form is set.
    fn copy_if_unset(new_key: &str, old_key: &str) {
        if std::env::var_os(new_key).is_none()
            && let Ok(val) = std::env::var(old_key)
        {
            // SAFETY: called during startup, before any downstream
            // subsystem spawns a reader for DDB__* / DARSH__* env
            // vars. `load_config` runs inside `#[tokio::main]`'s
            // runtime (so worker threads do exist), but no task
            // reads these variables until observability + typed
            // config assembly complete later in main(). This
            // remains non-racy as long as no Rust 2024 lint-gated
            // callsite is added earlier in main().
            unsafe {
                std::env::set_var(new_key, val);
            }
        }
    }

    // Server.
    copy_if_unset("DDB__SERVER__PORT", "DDB_PORT");
    copy_if_unset("DDB__SERVER__BIND_ADDR", "DDB_BIND_ADDR");
    copy_if_unset("DDB__SERVER__TLS_CERT_PATH", "DDB_TLS_CERT");
    copy_if_unset("DDB__SERVER__TLS_KEY_PATH", "DDB_TLS_KEY");

    // Database.
    copy_if_unset("DDB__DATABASE__URL", "DATABASE_URL");
    copy_if_unset("DDB__DATABASE__POOL_MAX", "DDB_DB_MAX_CONNECTIONS");
    copy_if_unset("DDB__DATABASE__POOL_MIN", "DDB_DB_MIN_CONNECTIONS");
    copy_if_unset(
        "DDB__DATABASE__ACQUIRE_TIMEOUT_SECS",
        "DDB_DB_ACQUIRE_TIMEOUT_SECS",
    );
    copy_if_unset(
        "DDB__DATABASE__IDLE_TIMEOUT_SECS",
        "DDB_DB_IDLE_TIMEOUT_SECS",
    );

    // Auth.
    copy_if_unset("DDB__AUTH__JWT_SECRET", "DDB_JWT_SECRET");
    copy_if_unset("DDB__AUTH__JWT_PRIVATE_KEY_PATH", "DDB_JWT_PRIVATE_KEY");
    copy_if_unset("DDB__AUTH__JWT_PUBLIC_KEY_PATH", "DDB_JWT_PUBLIC_KEY");

    // Dev.
    if std::env::var_os("DDB__DEV__MODE").is_none()
        && let Ok(val) = std::env::var("DDB_DEV")
    {
        let as_bool = matches!(val.as_str(), "1" | "true" | "TRUE" | "yes");
        // SAFETY: called before any downstream reader spawns — see
        // the `copy_if_unset` safety note above.
        unsafe {
            std::env::set_var("DDB__DEV__MODE", if as_bool { "true" } else { "false" });
        }
    }

    // CORS origins: the legacy form is a comma-separated string.  We
    // normalise by trimming each entry before handing it to `config`,
    // because `list_separator(",")` does a byte-level split with no
    // whitespace stripping.
    if std::env::var_os("DDB__CORS__ORIGINS").is_none()
        && let Ok(val) = std::env::var("DDB_CORS_ORIGINS")
    {
        let normalised: String = val
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(",");
        if !normalised.is_empty() {
            // SAFETY: called before any downstream reader spawns —
            // see the `copy_if_unset` safety note above.
            unsafe {
                std::env::set_var("DDB__CORS__ORIGINS", normalised);
            }
        }
    }

    // Rules.
    copy_if_unset("DDB__RULES__FILE_PATH", "DDB_RULES_FILE");

    // Anchor (DARSH_* -> DARSH__*).
    copy_if_unset("DARSH__ANCHOR__CHAIN", "DARSH_BLOCKCHAIN_ANCHOR");
    copy_if_unset("DARSH__ANCHOR__EVERY_N_TX", "DARSH_ANCHOR_EVERY_N_TX");
}

// ---------------------------------------------------------------------------
// Loader
// ---------------------------------------------------------------------------

/// Load the full configuration from defaults + TOML files + env vars.
///
/// Source order (later overrides earlier):
///
/// 1. `Default::default()`
/// 2. `config.toml` (optional)
/// 3. `config.local.toml` (optional)
/// 4. `DDB__*` env vars
/// 5. `DARSH__*` env vars (e.g. `DARSH__ANCHOR__CHAIN`)
///
/// Also eagerly runs `dotenvy::dotenv()` so a repo-local `.env` works out of
/// the box, and applies a backward-compat shim that maps the *legacy* flat
/// env var names (`DDB_PORT`, `DATABASE_URL`, `DDB_DEV`, …) into the new
/// prefixed form.
pub fn load_config() -> Result<DdbConfig, config::ConfigError> {
    // `.env` is best-effort: absence is fine, parse errors are swallowed.
    let _ = dotenvy::dotenv();

    apply_legacy_env_shim();

    let builder = config::Config::builder()
        // Layer 1: defaults (everything serde-default'd).
        .add_source(config::Config::try_from(&DdbConfig::default())?)
        // Layer 2: repo-wide config.
        .add_source(config::File::with_name("config").required(false))
        // Layer 3: per-dev overrides.
        .add_source(config::File::with_name("config.local").required(false))
        // Layer 4: DDB__ env vars.
        .add_source(
            config::Environment::with_prefix("DDB")
                .separator("__")
                .try_parsing(true)
                .list_separator(",")
                .with_list_parse_key("cors.origins"),
        )
        // Layer 5: DARSH__ env vars (blockchain anchoring, cross-cutting).
        .add_source(
            config::Environment::with_prefix("DARSH")
                .separator("__")
                .try_parsing(true),
        );

    builder.build()?.try_deserialize::<DdbConfig>()
}

#[cfg(test)]
mod tests;
