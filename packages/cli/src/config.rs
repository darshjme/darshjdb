//! CLI configuration — resolves settings from file, env, and CLI args.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::PathBuf;

/// Resolved configuration for the CLI session.
#[derive(Debug, Clone)]
pub struct Config {
    /// Base URL of the DarshJDB server.
    pub url: String,
    /// Authentication token (may be empty for local-only commands).
    pub token: String,
}

impl Config {
    /// Returns an error if no authentication token is configured.
    /// Call this before any command that contacts a remote server.
    pub fn require_token(&self) -> Result<&str> {
        if self.token.is_empty() {
            anyhow::bail!(
                "No authentication token configured.\n\
                 Set DDB_TOKEN, pass --token, or add [server].token to ddb.toml."
            );
        }
        Ok(&self.token)
    }
}

/// On-disk configuration format (`ddb.toml`).
#[derive(Debug, Deserialize, Default)]
struct FileConfig {
    server: Option<ServerConfig>,
}

#[derive(Debug, Deserialize)]
struct ServerConfig {
    url: Option<String>,
    token: Option<String>,
}

impl Config {
    /// Recognized log levels for the `logs --level` filter.
    pub const VALID_LOG_LEVELS: &[&str] = &["debug", "info", "warn", "error"];
}

impl Config {
    /// Load configuration with precedence: CLI args > env vars > config file.
    pub fn load(cli_url: Option<&str>, cli_token: Option<&str>) -> Result<Self> {
        let file_cfg = Self::load_file().unwrap_or_default();

        let url = cli_url
            .map(String::from)
            .or_else(|| std::env::var("DDB_URL").ok())
            .or_else(|| file_cfg.server.as_ref().and_then(|s| s.url.clone()))
            .unwrap_or_else(|| "http://localhost:4820".to_string());

        let token = cli_token
            .map(String::from)
            .or_else(|| std::env::var("DDB_TOKEN").ok())
            .or_else(|| file_cfg.server.as_ref().and_then(|s| s.token.clone()))
            .unwrap_or_default();

        Ok(Self { url, token })
    }

    /// Locate and parse `ddb.toml` by walking up from the current directory.
    fn load_file() -> Result<FileConfig> {
        let path = Self::find_config_file()?;
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        let cfg: FileConfig = toml::from_str(&content)
            .with_context(|| format!("Failed to parse {}", path.display()))?;
        Ok(cfg)
    }

    /// Walk up from CWD looking for `ddb.toml`.
    fn find_config_file() -> Result<PathBuf> {
        let mut dir = std::env::current_dir()?;
        loop {
            let candidate = dir.join("ddb.toml");
            if candidate.exists() {
                return Ok(candidate);
            }
            if !dir.pop() {
                anyhow::bail!("No ddb.toml found (searched from CWD to root)");
            }
        }
    }
}
