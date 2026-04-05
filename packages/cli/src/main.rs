//! DarshanDB CLI — `ddb`
//!
//! The primary command-line interface for developing with, deploying,
//! and administrating DarshanDB instances.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use std::time::Duration;

mod config;

// ── CLI structure ───────────────────────────────────────────────────

#[derive(Parser)]
#[command(
    name = "ddb",
    about = "DarshanDB CLI — develop, deploy, and manage your database",
    version,
    propagate_version = true
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// DarshanDB server URL (overrides config and DARSHAN_URL env)
    #[arg(long, global = true, env = "DARSHAN_URL")]
    url: Option<String>,

    /// Authentication token (overrides config and DARSHAN_TOKEN env)
    #[arg(long, global = true, env = "DARSHAN_TOKEN")]
    token: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start a local development server
    Dev {
        /// Port to listen on
        #[arg(short, long, default_value = "7700")]
        port: u16,

        /// Watch for file changes and hot-reload functions
        #[arg(long, default_value = "true")]
        watch: bool,
    },

    /// Build and deploy a Docker image to production
    Deploy {
        /// Docker image tag
        #[arg(short, long, default_value = "latest")]
        tag: String,

        /// Docker registry (e.g., ghcr.io/darshjme/darshandb)
        #[arg(short, long)]
        registry: Option<String>,

        /// Skip confirmation prompt
        #[arg(long)]
        yes: bool,
    },

    /// Push local functions to the server
    Push {
        /// Functions directory
        #[arg(short, long, default_value = "darshan/functions")]
        dir: String,

        /// Dry run — show what would be pushed without pushing
        #[arg(long)]
        dry_run: bool,
    },

    /// Pull schema from server and generate TypeScript types
    Pull {
        /// Output directory for generated types
        #[arg(short, long, default_value = "darshan/generated")]
        output: String,
    },

    /// Run a seed file against the database
    Seed {
        /// Path to seed file (TypeScript or JSON)
        #[arg(default_value = "darshan/seed.ts")]
        file: String,
    },

    /// Run database migrations
    Migrate {
        /// Migrations directory
        #[arg(short, long, default_value = "darshan/migrations")]
        dir: String,

        /// Roll back the last N migrations
        #[arg(long)]
        rollback: Option<u32>,

        /// Show migration status without running
        #[arg(long)]
        status: bool,
    },

    /// Tail structured logs from the server
    Logs {
        /// Number of recent lines to show
        #[arg(short = 'n', long, default_value = "100")]
        lines: u32,

        /// Follow log output (like tail -f)
        #[arg(short, long)]
        follow: bool,

        /// Filter by log level (debug, info, warn, error)
        #[arg(short, long)]
        level: Option<String>,
    },

    /// Authentication and user management
    Auth {
        #[command(subcommand)]
        command: AuthCommands,
    },

    /// Create a database backup
    Backup {
        /// Output file path
        #[arg(short, long)]
        output: Option<String>,

        /// Include file storage blobs in backup
        #[arg(long)]
        include_storage: bool,
    },

    /// Restore a database from backup
    Restore {
        /// Backup file path
        file: String,

        /// Skip confirmation prompt
        #[arg(long)]
        yes: bool,
    },

    /// Show server health and status information
    Status,

    /// Initialize a new DarshanDB project
    Init {
        /// Project name
        #[arg(default_value = ".")]
        name: String,
    },
}

#[derive(Subcommand)]
enum AuthCommands {
    /// Create an admin user
    CreateAdmin {
        /// Admin email
        #[arg(short, long)]
        email: String,

        /// Admin password (prompted if not provided)
        #[arg(short, long)]
        password: Option<String>,
    },

    /// List all users
    ListUsers {
        /// Maximum number of users to display
        #[arg(short, long, default_value = "50")]
        limit: u32,
    },

    /// Revoke all sessions for a user
    RevokeUser {
        /// User ID or email
        user: String,
    },
}

// ── Main ────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "darshan_cli=info".into()),
        )
        .init();

    let cli = Cli::parse();
    let cfg = config::Config::load(cli.url.as_deref(), cli.token.as_deref())?;

    match cli.command {
        Commands::Dev { port, watch } => cmd_dev(port, watch).await,
        Commands::Deploy { tag, registry, yes } => cmd_deploy(&tag, registry.as_deref(), yes).await,
        Commands::Push { dir, dry_run } => cmd_push(&cfg, &dir, dry_run).await,
        Commands::Pull { output } => cmd_pull(&cfg, &output).await,
        Commands::Seed { file } => cmd_seed(&cfg, &file).await,
        Commands::Migrate {
            dir,
            rollback,
            status,
        } => cmd_migrate(&cfg, &dir, rollback, status).await,
        Commands::Logs {
            lines,
            follow,
            level,
        } => cmd_logs(&cfg, lines, follow, level.as_deref()).await,
        Commands::Auth { command } => cmd_auth(&cfg, command).await,
        Commands::Backup {
            output,
            include_storage,
        } => cmd_backup(&cfg, output.as_deref(), include_storage).await,
        Commands::Restore { file, yes } => cmd_restore(&cfg, &file, yes).await,
        Commands::Status => cmd_status(&cfg).await,
        Commands::Init { name } => cmd_init(&name).await,
    }
}

// ── Command implementations ────────────────────────────────────────

async fn cmd_dev(port: u16, watch: bool) -> Result<()> {
    println!("\n  {} DarshanDB dev server\n", ">>>".bright_cyan().bold());

    let spinner = spinner("Starting server...");

    // Check if Docker is available for Postgres
    let docker_available = tokio::process::Command::new("docker")
        .arg("info")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);

    if docker_available {
        spinner.set_message("Starting Postgres via Docker...");
        let pg_status = tokio::process::Command::new("docker")
            .args([
                "run",
                "-d",
                "--name",
                "darshandb-dev-pg",
                "-e",
                "POSTGRES_PASSWORD=darshan",
                "-e",
                "POSTGRES_DB=darshandb",
                "-p",
                "5432:5432",
                "pgvector/pgvector:pg16",
            ])
            .output()
            .await;

        match pg_status {
            Ok(out) if out.status.success() => {
                spinner.set_message("Postgres started");
            }
            _ => {
                spinner.set_message("Postgres container may already be running, continuing...");
            }
        }
    } else {
        spinner.set_message("Docker not found — ensure Postgres is running manually");
    }

    spinner.finish_with_message("Postgres ready");

    println!(
        "  {} Listening on {}",
        "-->".bright_green(),
        format!("http://localhost:{port}").bright_yellow()
    );
    println!(
        "  {} API docs at {}",
        "-->".bright_green(),
        format!("http://localhost:{port}/api/docs").bright_yellow()
    );

    if watch {
        println!(
            "  {} Watching for file changes (functions hot-reload enabled)",
            "-->".bright_green()
        );
    }

    println!();

    // Build and run the server binary
    let mut cmd = tokio::process::Command::new("cargo");
    cmd.args(["run", "-p", "darshandb-server", "--"])
        .env("DARSHAN_PORT", port.to_string())
        .env(
            "DATABASE_URL",
            "postgres://postgres:darshan@localhost:5432/darshandb",
        );

    if watch {
        cmd.env("DARSHAN_WATCH", "true");
    }

    let status = cmd
        .status()
        .await
        .context("Failed to start DarshanDB server")?;

    if !status.success() {
        anyhow::bail!("Server exited with status: {}", status);
    }

    Ok(())
}

async fn cmd_deploy(tag: &str, registry: Option<&str>, yes: bool) -> Result<()> {
    println!("\n  {} DarshanDB deploy\n", ">>>".bright_cyan().bold());

    let image = match registry {
        Some(r) => format!("{r}:{tag}"),
        None => format!("darshandb:{tag}"),
    };

    if !yes {
        println!("  Will build and push: {}", image.bright_yellow());
        let confirm = dialoguer::Confirm::new()
            .with_prompt("  Continue?")
            .default(true)
            .interact()?;

        if !confirm {
            println!("  Aborted.");
            return Ok(());
        }
    }

    let pb = progress_bar(3, "Deploying");

    pb.set_message("Building Docker image...");
    let build = tokio::process::Command::new("docker")
        .args(["build", "-t", &image, "-f", "Dockerfile", "."])
        .status()
        .await
        .context("Docker build failed")?;

    if !build.success() {
        anyhow::bail!("Docker build failed");
    }
    pb.inc(1);

    if registry.is_some() {
        pb.set_message("Pushing image...");
        let push = tokio::process::Command::new("docker")
            .args(["push", &image])
            .status()
            .await
            .context("Docker push failed")?;

        if !push.success() {
            anyhow::bail!("Docker push failed");
        }
    }
    pb.inc(1);

    pb.set_message("Done");
    pb.inc(1);
    pb.finish_with_message("Deploy complete");

    println!(
        "\n  {} Image: {}\n",
        "-->".bright_green(),
        image.bright_yellow()
    );

    Ok(())
}

async fn cmd_push(cfg: &config::Config, dir: &str, dry_run: bool) -> Result<()> {
    println!("\n  {} Push functions\n", ">>>".bright_cyan().bold());
    cfg.require_token()?;

    let functions_dir = std::path::Path::new(dir);
    if !functions_dir.exists() {
        anyhow::bail!("Functions directory not found: {dir}");
    }

    let mut files = Vec::new();
    let mut entries = tokio::fs::read_dir(functions_dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "ts" || e == "js") {
            files.push(path);
        }
    }

    if files.is_empty() {
        println!("  No function files found in {dir}");
        return Ok(());
    }

    println!("  Found {} function(s):", files.len());
    for f in &files {
        let name = f.file_stem().unwrap_or_default().to_string_lossy();
        let prefix = if dry_run { "  (dry)" } else { "  " };
        println!("  {} {}", prefix.dimmed(), name.bright_white());
    }

    if dry_run {
        println!("\n  Dry run complete — no changes made.");
        return Ok(());
    }

    let client = reqwest::Client::new();
    let pb = progress_bar(files.len() as u64, "Pushing");

    for file in &files {
        let name = file
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        pb.set_message(format!("Pushing {name}..."));

        let content = tokio::fs::read_to_string(file).await?;
        let resp = client
            .post(format!("{}/api/admin/functions", cfg.url))
            .bearer_auth(&cfg.token)
            .json(&serde_json::json!({
                "name": name,
                "source": content,
            }))
            .send()
            .await
            .context("Failed to push function")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Push failed for {name}: {status} — {body}");
        }

        pb.inc(1);
    }

    pb.finish_with_message("All functions pushed");
    println!();

    Ok(())
}

async fn cmd_pull(cfg: &config::Config, output: &str) -> Result<()> {
    println!(
        "\n  {} Pull schema & generate types\n",
        ">>>".bright_cyan().bold()
    );
    cfg.require_token()?;

    let spinner = spinner("Fetching schema...");

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/api/admin/schema", cfg.url))
        .bearer_auth(&cfg.token)
        .send()
        .await
        .context("Failed to fetch schema")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Schema fetch failed: {status} — {body}");
    }

    let schema: serde_json::Value = resp.json().await?;
    spinner.finish_with_message("Schema fetched");

    // Generate TypeScript types from schema
    let out_dir = std::path::Path::new(output);
    tokio::fs::create_dir_all(out_dir).await?;

    let mut ts_output = String::from("// Auto-generated by `ddb pull` — do not edit.\n\n");

    if let Some(types) = schema.get("entity_types").and_then(|v| v.as_object()) {
        for (type_name, type_def) in types {
            let interface_name = to_pascal_case(type_name);
            ts_output.push_str(&format!("export interface {interface_name} {{\n"));
            ts_output.push_str("  id: string;\n");

            if let Some(attrs) = type_def.get("attributes").and_then(|v| v.as_object()) {
                for (attr_name, attr_def) in attrs {
                    let ts_type = map_value_type_to_ts(attr_def);
                    let optional = attr_def
                        .get("required")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let mark = if optional { "" } else { "?" };
                    ts_output.push_str(&format!("  {attr_name}{mark}: {ts_type};\n"));
                }
            }

            ts_output.push_str("}\n\n");
        }
    }

    let types_path = out_dir.join("schema.ts");
    tokio::fs::write(&types_path, &ts_output).await?;

    println!(
        "  {} Generated types at {}",
        "-->".bright_green(),
        types_path.display().to_string().bright_yellow()
    );
    println!();

    Ok(())
}

async fn cmd_seed(cfg: &config::Config, file: &str) -> Result<()> {
    println!("\n  {} Seed database\n", ">>>".bright_cyan().bold());
    cfg.require_token()?;

    let path = std::path::Path::new(file);
    if !path.exists() {
        anyhow::bail!("Seed file not found: {file}");
    }

    let spinner = spinner("Running seed...");

    let content = tokio::fs::read_to_string(path).await?;
    let client = reqwest::Client::new();

    // If JSON, send directly as mutations
    if file.ends_with(".json") {
        let data: serde_json::Value =
            serde_json::from_str(&content).context("Invalid JSON in seed file")?;

        let resp = client
            .post(format!("{}/api/mutate", cfg.url))
            .bearer_auth(&cfg.token)
            .json(&data)
            .send()
            .await
            .context("Seed request failed")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Seed failed: {status} — {body}");
        }
    } else {
        // For TS/JS seed files, invoke via the functions runtime
        let resp = client
            .post(format!("{}/api/fn/seed", cfg.url))
            .bearer_auth(&cfg.token)
            .json(&serde_json::json!({ "source": content }))
            .send()
            .await
            .context("Seed function invocation failed")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Seed failed: {status} — {body}");
        }
    }

    spinner.finish_with_message("Seed complete");
    println!();

    Ok(())
}

async fn cmd_migrate(
    cfg: &config::Config,
    dir: &str,
    rollback: Option<u32>,
    status: bool,
) -> Result<()> {
    println!("\n  {} Migrations\n", ">>>".bright_cyan().bold());

    cfg.require_token()?;
    let client = reqwest::Client::new();

    if status {
        let resp = client
            .get(format!("{}/api/admin/migrations", cfg.url))
            .bearer_auth(&cfg.token)
            .send()
            .await
            .context("Failed to fetch migration status")?;

        let body: serde_json::Value = resp.json().await?;
        println!("  Migration status:");
        let pretty = serde_json::to_string_pretty(&body)
            .context("Failed to format migration status as JSON")?;
        println!("  {}", pretty.dimmed());
        return Ok(());
    }

    if let Some(n) = rollback {
        let spinner = spinner(&format!("Rolling back {n} migration(s)..."));

        let resp = client
            .post(format!("{}/api/admin/migrations/rollback", cfg.url))
            .bearer_auth(&cfg.token)
            .json(&serde_json::json!({ "count": n }))
            .send()
            .await
            .context("Rollback failed")?;

        if !resp.status().is_success() {
            let status_code = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Rollback failed: {status_code} — {body}");
        }

        spinner.finish_with_message(format!("Rolled back {n} migration(s)"));
        return Ok(());
    }

    // Run forward migrations
    let migrations_dir = std::path::Path::new(dir);
    if !migrations_dir.exists() {
        anyhow::bail!("Migrations directory not found: {dir}");
    }

    let mut files: Vec<_> = std::fs::read_dir(migrations_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .is_some_and(|ext| ext == "sql" || ext == "json")
        })
        .collect();

    files.sort_by_key(|e| e.file_name());

    let pb = progress_bar(files.len() as u64, "Migrating");

    for entry in &files {
        let name = entry.file_name().to_string_lossy().to_string();
        pb.set_message(format!("Running {name}..."));

        let content = std::fs::read_to_string(entry.path())?;
        let resp = client
            .post(format!("{}/api/admin/migrations/run", cfg.url))
            .bearer_auth(&cfg.token)
            .json(&serde_json::json!({
                "name": name,
                "content": content,
            }))
            .send()
            .await
            .context("Migration request failed")?;

        if !resp.status().is_success() {
            let status_code = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Migration {name} failed: {status_code} — {body}");
        }

        pb.inc(1);
    }

    pb.finish_with_message("All migrations applied");
    println!();

    Ok(())
}

async fn cmd_logs(
    cfg: &config::Config,
    lines: u32,
    follow: bool,
    level: Option<&str>,
) -> Result<()> {
    cfg.require_token()?;
    let client = reqwest::Client::new();

    if let Some(l) = level
        && !config::Config::VALID_LOG_LEVELS.contains(&l)
    {
        anyhow::bail!(
            "Invalid log level '{l}'. Valid levels: {}",
            config::Config::VALID_LOG_LEVELS.join(", ")
        );
    }

    let mut url = format!("{}/api/admin/logs?lines={lines}", cfg.url);
    if follow {
        url.push_str("&follow=true");
    }
    if let Some(l) = level {
        // level is validated above so this is safe against injection
        url.push_str(&format!("&level={l}"));
    }

    let resp = client
        .get(&url)
        .bearer_auth(&cfg.token)
        .send()
        .await
        .context("Failed to fetch logs")?;

    // For both follow and non-follow, read the full response text.
    // In follow mode the server holds the connection open (SSE), so
    // reqwest will stream chunks as they arrive via `chunk()`.
    if follow {
        let mut resp = resp;
        while let Some(chunk) = resp.chunk().await? {
            print!("{}", String::from_utf8_lossy(&chunk));
        }
    } else {
        let body = resp.text().await?;
        println!("{body}");
    }

    Ok(())
}

async fn cmd_auth(cfg: &config::Config, command: AuthCommands) -> Result<()> {
    cfg.require_token()?;
    let client = reqwest::Client::new();

    match command {
        AuthCommands::CreateAdmin { email, password } => {
            let password = match password {
                Some(p) => p,
                None => dialoguer::Password::new()
                    .with_prompt("Admin password")
                    .with_confirmation("Confirm password", "Passwords do not match")
                    .interact()?,
            };

            let spinner = spinner("Creating admin user...");

            let resp = client
                .post(format!("{}/api/admin/users", cfg.url))
                .bearer_auth(&cfg.token)
                .json(&serde_json::json!({
                    "email": email,
                    "password": password,
                    "roles": ["admin"],
                }))
                .send()
                .await
                .context("Failed to create admin")?;

            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                anyhow::bail!("Create admin failed: {status} — {body}");
            }

            let body: serde_json::Value = resp.json().await?;
            spinner.finish_with_message("Admin created");

            println!(
                "\n  {} Admin user ID: {}\n",
                "-->".bright_green(),
                body.get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .bright_yellow()
            );
        }

        AuthCommands::ListUsers { limit } => {
            let resp = client
                .get(format!("{}/api/admin/users?limit={limit}", cfg.url))
                .bearer_auth(&cfg.token)
                .send()
                .await
                .context("Failed to list users")?;

            let body: serde_json::Value = resp.json().await?;
            let pretty = serde_json::to_string_pretty(&body)
                .context("Failed to format user list as JSON")?;
            println!("{pretty}");
        }

        AuthCommands::RevokeUser { user } => {
            let spinner = spinner(&format!("Revoking sessions for {user}..."));

            let resp = client
                .post(format!("{}/api/admin/users/revoke", cfg.url))
                .bearer_auth(&cfg.token)
                .json(&serde_json::json!({ "user": user }))
                .send()
                .await
                .context("Failed to revoke user")?;

            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                anyhow::bail!("Revoke failed: {status} — {body}");
            }

            spinner.finish_with_message("All sessions revoked");
        }
    }

    Ok(())
}

async fn cmd_backup(
    cfg: &config::Config,
    output: Option<&str>,
    include_storage: bool,
) -> Result<()> {
    println!("\n  {} Backup\n", ">>>".bright_cyan().bold());

    cfg.require_token()?;

    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let default_output = format!("darshandb_backup_{timestamp}.tar.gz");
    let output = output.unwrap_or(&default_output);

    let spinner = spinner("Creating backup...");

    let mut url = format!("{}/api/admin/backup", cfg.url);
    if include_storage {
        url.push_str("?include_storage=true");
    }

    let client = reqwest::Client::new();
    let resp = client
        .get(&url)
        .bearer_auth(&cfg.token)
        .send()
        .await
        .context("Backup request failed")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Backup failed: {status} — {body}");
    }

    let bytes = resp.bytes().await?;
    tokio::fs::write(output, &bytes).await?;

    spinner.finish_with_message("Backup complete");
    println!(
        "  {} Saved to {}\n",
        "-->".bright_green(),
        output.bright_yellow()
    );

    Ok(())
}

async fn cmd_restore(cfg: &config::Config, file: &str, yes: bool) -> Result<()> {
    println!("\n  {} Restore\n", ">>>".bright_cyan().bold());
    cfg.require_token()?;

    if !yes {
        println!(
            "  {} This will overwrite the current database!",
            "WARNING:".bright_red().bold()
        );
        let confirm = dialoguer::Confirm::new()
            .with_prompt("  Continue?")
            .default(false)
            .interact()?;

        if !confirm {
            println!("  Aborted.");
            return Ok(());
        }
    }

    let spinner = spinner("Restoring from backup...");

    let data = tokio::fs::read(file)
        .await
        .context("Failed to read backup file")?;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/api/admin/restore", cfg.url))
        .bearer_auth(&cfg.token)
        .header("Content-Type", "application/octet-stream")
        .body(data)
        .send()
        .await
        .context("Restore request failed")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Restore failed: {status} — {body}");
    }

    spinner.finish_with_message("Restore complete");
    println!();

    Ok(())
}

async fn cmd_status(cfg: &config::Config) -> Result<()> {
    println!("\n  {} DarshanDB Status\n", ">>>".bright_cyan().bold());
    cfg.require_token()?;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/api/admin/health", cfg.url))
        .bearer_auth(&cfg.token)
        .timeout(Duration::from_secs(5))
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => {
            let body: serde_json::Value = r.json().await?;

            println!(
                "  {} Server: {}",
                "-->".bright_green(),
                "healthy".bright_green().bold()
            );

            if let Some(version) = body.get("version").and_then(|v| v.as_str()) {
                println!("  {} Version: {}", "-->".bright_green(), version);
            }
            if let Some(uptime) = body.get("uptime_seconds").and_then(|v| v.as_u64()) {
                let hours = uptime / 3600;
                let mins = (uptime % 3600) / 60;
                println!("  {} Uptime: {}h {}m", "-->".bright_green(), hours, mins);
            }
            if let Some(db) = body.get("database").and_then(|v| v.as_str()) {
                println!("  {} Database: {}", "-->".bright_green(), db);
            }
            if let Some(conns) = body.get("active_connections").and_then(|v| v.as_u64()) {
                println!("  {} Connections: {}", "-->".bright_green(), conns);
            }
            if let Some(entities) = body.get("entity_count").and_then(|v| v.as_u64()) {
                println!("  {} Entities: {}", "-->".bright_green(), entities);
            }
            if let Some(triples) = body.get("triple_count").and_then(|v| v.as_u64()) {
                println!("  {} Triples: {}", "-->".bright_green(), triples);
            }
        }
        Ok(r) => {
            println!(
                "  {} Server responded with: {}",
                "-->".bright_red(),
                r.status()
            );
        }
        Err(e) => {
            println!(
                "  {} Server unreachable: {}",
                "-->".bright_red(),
                e.to_string().dimmed()
            );
            println!("  {} URL: {}", "   ".normal(), cfg.url.bright_yellow());
        }
    }

    println!();
    Ok(())
}

async fn cmd_init(name: &str) -> Result<()> {
    println!(
        "\n  {} Initialize DarshanDB project\n",
        ">>>".bright_cyan().bold()
    );

    let project_dir = if name == "." {
        std::env::current_dir()?
    } else {
        let dir = std::env::current_dir()?.join(name);
        tokio::fs::create_dir_all(&dir).await?;
        dir
    };

    let darshan_dir = project_dir.join("ddb");
    tokio::fs::create_dir_all(darshan_dir.join("functions")).await?;
    tokio::fs::create_dir_all(darshan_dir.join("migrations")).await?;
    tokio::fs::create_dir_all(darshan_dir.join("generated")).await?;

    // Create ddb.toml
    let config_content = r#"# DarshanDB project configuration

[server]
url = "http://localhost:7700"

[functions]
dir = "darshan/functions"

[migrations]
dir = "darshan/migrations"

[codegen]
output = "darshan/generated"
"#;

    tokio::fs::write(project_dir.join("ddb.toml"), config_content).await?;

    // Create example function
    let example_fn = r#"import { query, mutation } from "@darshan/server";

// Example query function
export const listTodos = query(async (ctx) => {
  return await ctx.db.query("todo").collect();
});

// Example mutation function
export const createTodo = mutation(async (ctx, args: { title: string }) => {
  return await ctx.db.insert("todo", {
    title: args.title,
    completed: false,
    createdAt: Date.now(),
  });
});
"#;

    tokio::fs::write(darshan_dir.join("functions/todos.ts"), example_fn).await?;

    // Create example seed
    let seed_content = r#"import { seed } from "@darshan/server";

export default seed(async (ctx) => {
  await ctx.db.insert("todo", { title: "Learn DarshanDB", completed: false, createdAt: Date.now() });
  await ctx.db.insert("todo", { title: "Build something great", completed: false, createdAt: Date.now() });
  console.log("Seed complete: 2 todos created");
});
"#;

    tokio::fs::write(darshan_dir.join("seed.ts"), seed_content).await?;

    println!("  {} Created project structure:", "-->".bright_green());
    println!("      ddb.toml");
    println!("      darshan/functions/todos.ts");
    println!("      darshan/migrations/");
    println!("      darshan/generated/");
    println!("      darshan/seed.ts");
    println!(
        "\n  {} Run {} to start developing\n",
        "-->".bright_green(),
        "ddb dev".bright_yellow()
    );

    Ok(())
}

// ── Helpers ─────────────────────────────────────────────────────────

fn spinner(msg: &str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::with_template("  {spinner:.cyan} {msg}")
            .expect("hard-coded spinner template must be valid")
            .tick_chars("-\\|/"),
    );
    pb.enable_steady_tick(Duration::from_millis(80));
    pb.set_message(msg.to_string());
    pb
}

fn progress_bar(total: u64, prefix: &str) -> ProgressBar {
    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::with_template("  {prefix:.cyan} [{bar:30.cyan/dim}] {pos}/{len} {msg}")
            .expect("hard-coded progress bar template must be valid")
            .progress_chars("=> "),
    );
    pb.set_prefix(prefix.to_string());
    pb
}

fn to_pascal_case(s: &str) -> String {
    s.split(['_', '-', '/'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                None => String::new(),
                Some(c) => c.to_uppercase().to_string() + &chars.as_str().to_lowercase(),
            }
        })
        .collect()
}

fn map_value_type_to_ts(attr_def: &serde_json::Value) -> &str {
    let types = attr_def.get("value_types").and_then(|v| v.as_array());

    match types {
        Some(arr) if arr.len() == 1 => match arr[0].as_str().unwrap_or("") {
            "String" => "string",
            "Int" | "Float" | "Number" => "number",
            "Boolean" => "boolean",
            "Reference" => "string",
            "Json" => "Record<string, unknown>",
            "DateTime" => "string",
            "Bytes" => "Uint8Array",
            _ => "unknown",
        },
        _ => "unknown",
    }
}
