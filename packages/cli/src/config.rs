//! CLI configuration — resolves settings from file, env, and CLI args.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::PathBuf;

/// Resolved configuration for the CLI session.
#[derive(Debug, Clone)]
pub struct Config {
    /// Base URL of the DarshanDB server.
    pub url: String,
    /// Authentication token.
    pub token: String,
}

/// On-disk configuration format (`darshan.toml`).
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
    /// Load configuration with precedence: CLI args > env vars > config file.
    pub fn load(cli_url: Option<&str>, cli_token: Option<&str>) -> Result<Self> {
        let file_cfg = Self::load_file().unwrap_or_default();

        let url = cli_url
            .map(String::from)
            .or_else(|| std::env::var("DARSHAN_URL").ok())
            .or_else(|| file_cfg.server.as_ref().and_then(|s| s.url.clone()))
            .unwrap_or_else(|| "http://localhost:4820".to_string());

        let token = cli_token
            .map(String::from)
            .or_else(|| std::env::var("DARSHAN_TOKEN").ok())
            .or_else(|| file_cfg.server.as_ref().and_then(|s| s.token.clone()))
            .unwrap_or_default();

        Ok(Self { url, token })
    }

    /// Locate and parse `darshan.toml` by walking up from the current directory.
    fn load_file() -> Result<FileConfig> {
        let path = Self::find_config_file()?;
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        let cfg: FileConfig = toml::from_str(&content)
            .with_context(|| format!("Failed to parse {}", path.display()))?;
        Ok(cfg)
    }

    /// Walk up from CWD looking for `darshan.toml`.
    fn find_config_file() -> Result<PathBuf> {
        let mut dir = std::env::current_dir()?;
        loop {
            let candidate = dir.join("darshan.toml");
            if candidate.exists() {
                return Ok(candidate);
            }
            if !dir.pop() {
                anyhow::bail!("No darshan.toml found (searched from CWD to root)");
            }
        }
    }
}
