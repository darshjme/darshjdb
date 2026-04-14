// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
//! Unit tests for the Slice 17 typed configuration layer.
//!
//! These tests mutate process env, so every test takes a global lock to
//! serialize execution (Rust's default test runner is multi-threaded).

use super::*;
use std::sync::Mutex;

// Global lock shared by every env-touching test.  Panics in one test
// would poison this lock, so every acquire site recovers from poisoning.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Acquire [`ENV_LOCK`] while tolerating poisoning from prior panicking tests.
fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    match ENV_LOCK.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Snapshot of env vars we care about so each test restores cleanly.
struct EnvGuard {
    keys: Vec<&'static str>,
    saved: Vec<(&'static str, Option<String>)>,
}

impl EnvGuard {
    fn new(keys: &[&'static str]) -> Self {
        let saved = keys
            .iter()
            .map(|k| (*k, std::env::var(*k).ok()))
            .collect();
        // Proactively clear every key so we start from a known baseline.
        for k in keys {
            // SAFETY: tests hold `ENV_LOCK`, so no other thread reads/writes env.
            unsafe {
                std::env::remove_var(k);
            }
        }
        Self {
            keys: keys.to_vec(),
            saved,
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (k, v) in &self.saved {
            // SAFETY: tests hold `ENV_LOCK`.
            unsafe {
                match v {
                    Some(val) => std::env::set_var(k, val),
                    None => std::env::remove_var(k),
                }
            }
        }
        let _ = &self.keys; // silence unused
    }
}

const ALL_KEYS: &[&str] = &[
    "DDB__SERVER__PORT",
    "DDB__SERVER__BIND_ADDR",
    "DDB__SERVER__TLS_CERT_PATH",
    "DDB__SERVER__TLS_KEY_PATH",
    "DDB__DATABASE__URL",
    "DDB__DATABASE__POOL_MAX",
    "DDB__DATABASE__POOL_MIN",
    "DDB__DATABASE__ACQUIRE_TIMEOUT_SECS",
    "DDB__DATABASE__IDLE_TIMEOUT_SECS",
    "DDB__AUTH__JWT_SECRET",
    "DDB__AUTH__JWT_PRIVATE_KEY_PATH",
    "DDB__AUTH__JWT_PUBLIC_KEY_PATH",
    "DDB__AUTH__JWT_EXPIRY_SECONDS",
    "DDB__DEV__MODE",
    "DDB__CORS__ORIGINS",
    "DDB__RULES__FILE_PATH",
    "DDB__EMBEDDING__PROVIDER",
    "DDB__EMBEDDING__API_KEY",
    "DARSH__ANCHOR__CHAIN",
    "DARSH__ANCHOR__EVERY_N_TX",
    // Legacy flat names.
    "DDB_PORT",
    "DDB_BIND_ADDR",
    "DDB_TLS_CERT",
    "DDB_TLS_KEY",
    "DATABASE_URL",
    "DDB_DB_MAX_CONNECTIONS",
    "DDB_DB_MIN_CONNECTIONS",
    "DDB_JWT_SECRET",
    "DDB_JWT_PRIVATE_KEY",
    "DDB_JWT_PUBLIC_KEY",
    "DDB_DEV",
    "DDB_CORS_ORIGINS",
    "DDB_RULES_FILE",
    "DARSH_BLOCKCHAIN_ANCHOR",
    "DARSH_ANCHOR_EVERY_N_TX",
];

#[test]
fn defaults_produce_a_valid_config() {
    let _lock = env_lock();
    let _guard = EnvGuard::new(ALL_KEYS);

    let cfg = load_config().expect("defaults must load");

    assert_eq!(cfg.server.port, 7700);
    assert_eq!(cfg.server.cache_port, 7701);
    assert_eq!(cfg.server.bind_addr, "0.0.0.0");
    assert_eq!(cfg.database.pool_max, 20);
    assert_eq!(cfg.database.pool_min, 2);
    assert_eq!(cfg.database.acquire_timeout_secs, 5);
    assert_eq!(cfg.database.idle_timeout_secs, 600);
    assert_eq!(cfg.database.max_lifetime_sec, 1800);
    assert_eq!(cfg.auth.jwt_expiry_seconds, 900);
    assert_eq!(cfg.auth.refresh_expiry_hours, 720);
    assert!(cfg.cors.origins.is_empty());
    assert!(!cfg.dev.mode);
    assert_eq!(cfg.embedding.provider, "none");
    assert_eq!(cfg.embedding.dimensions, 1536);
    assert_eq!(cfg.storage.backend, "local");
    assert_eq!(cfg.schema.mode, "flexible");
    assert_eq!(cfg.anchor.chain, "none");
    assert_eq!(cfg.anchor.every_n_tx, 1000);
    assert_eq!(cfg.rules.file_path, "./darshan/rules.json");
}

#[test]
fn new_env_var_overrides_server_port() {
    let _lock = env_lock();
    let _guard = EnvGuard::new(ALL_KEYS);

    // SAFETY: lock held.
    unsafe {
        std::env::set_var("DDB__SERVER__PORT", "8080");
    }

    let cfg = load_config().expect("should load");
    assert_eq!(cfg.server.port, 8080);
}

#[test]
fn legacy_flat_env_var_still_works() {
    let _lock = env_lock();
    let _guard = EnvGuard::new(ALL_KEYS);

    // SAFETY: lock held.
    unsafe {
        std::env::set_var("DDB_PORT", "9090");
        std::env::set_var("DDB_DEV", "true");
        std::env::set_var("DATABASE_URL", "postgres://legacy/db");
        std::env::set_var("DDB_CORS_ORIGINS", "https://a.test, https://b.test");
    }

    let cfg = load_config().expect("should load");
    assert_eq!(cfg.server.port, 9090);
    assert!(cfg.dev.mode);
    assert_eq!(cfg.database.url.as_deref(), Some("postgres://legacy/db"));
    assert_eq!(
        cfg.cors.origins,
        vec!["https://a.test".to_string(), "https://b.test".to_string()]
    );
}

#[test]
fn new_prefix_wins_over_legacy_flat() {
    let _lock = env_lock();
    let _guard = EnvGuard::new(ALL_KEYS);

    // SAFETY: lock held.
    unsafe {
        std::env::set_var("DDB_PORT", "1111");
        std::env::set_var("DDB__SERVER__PORT", "2222");
    }

    let cfg = load_config().expect("should load");
    assert_eq!(cfg.server.port, 2222, "new prefixed form must win");
}

#[test]
fn darsh_prefix_maps_to_anchor() {
    let _lock = env_lock();
    let _guard = EnvGuard::new(ALL_KEYS);

    // SAFETY: lock held.
    unsafe {
        std::env::set_var("DARSH_BLOCKCHAIN_ANCHOR", "ethereum");
        std::env::set_var("DARSH_ANCHOR_EVERY_N_TX", "500");
    }

    let cfg = load_config().expect("should load");
    assert_eq!(cfg.anchor.chain, "ethereum");
    assert_eq!(cfg.anchor.every_n_tx, 500);
}

#[test]
fn debug_impl_redacts_secrets() {
    let secret: Secret<String> = Secret::new("super-secret-jwt".to_string());
    let rendered = format!("{secret:?}");
    assert_eq!(rendered, "<redacted>");
    assert!(!rendered.contains("super-secret-jwt"));

    // Full config Debug must also redact.
    let mut cfg = DdbConfig::default();
    cfg.auth.jwt_secret = Some(Secret::new("top-secret-key".to_string()));
    cfg.embedding.api_key = Some(Secret::new("sk-openai-12345".to_string()));
    cfg.cache.cache_password = Some(Secret::new("redis-pass".to_string()));

    let debug_str = format!("{cfg:?}");
    assert!(!debug_str.contains("top-secret-key"));
    assert!(!debug_str.contains("sk-openai-12345"));
    assert!(!debug_str.contains("redis-pass"));
    assert!(debug_str.contains("<redacted>"));
}

#[test]
fn env_var_overrides_tls_paths_via_legacy_shim() {
    let _lock = env_lock();
    let _guard = EnvGuard::new(ALL_KEYS);

    // SAFETY: lock held.
    unsafe {
        std::env::set_var("DDB_TLS_CERT", "/tmp/cert.pem");
        std::env::set_var("DDB_TLS_KEY", "/tmp/key.pem");
    }

    let cfg = load_config().expect("should load");
    assert_eq!(cfg.server.tls_cert_path.as_deref(), Some("/tmp/cert.pem"));
    assert_eq!(cfg.server.tls_key_path.as_deref(), Some("/tmp/key.pem"));
}

#[test]
fn secret_expose_returns_inner_value() {
    let s: Secret<String> = Secret::new("abc123".to_string());
    assert_eq!(s.expose(), "abc123");
    assert_eq!(s.into_inner(), "abc123");
}
