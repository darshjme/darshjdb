//! `ddb export` / `ddb import` — Data portability commands.
//!
//! Export all data from a running DarshJDB as JSON or import it back.

use anyhow::{Context, Result};
use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use std::time::Duration;

/// Export all data from a DarshJDB instance.
pub async fn run_export(
    conn: String,
    output: Option<String>,
    token: Option<String>,
    format: ExportFormat,
) -> Result<()> {
    println!("\n  {} DarshJDB Export\n", ">>>".bright_cyan().bold());

    let output_file = output.unwrap_or_else(|| {
        let ts = chrono::Utc::now().format("%Y%m%d_%H%M%S");
        match format {
            ExportFormat::Json => format!("darshjdb_export_{ts}.json"),
            ExportFormat::Jsonl => format!("darshjdb_export_{ts}.jsonl"),
        }
    });

    let spinner = spinner("Connecting to server...");

    let client = reqwest::Client::new();
    let token = token
        .or_else(|| std::env::var("DDB_TOKEN").ok())
        .unwrap_or_default();

    // Fetch all entity types
    spinner.set_message("Fetching schema...");
    let schema_resp = client
        .get(format!("{conn}/api/admin/schema"))
        .bearer_auth(&token)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .context("Failed to fetch schema")?;

    if !schema_resp.status().is_success() {
        let status = schema_resp.status();
        let body = schema_resp.text().await.unwrap_or_default();
        anyhow::bail!("Schema fetch failed: {status} — {body}");
    }

    let schema: serde_json::Value = schema_resp.json().await?;
    spinner.set_message("Exporting data...");

    // Get entity types from schema
    let entity_types: Vec<String> = schema
        .get("entity_types")
        .and_then(|v| v.as_object())
        .map(|obj| obj.keys().cloned().collect())
        .unwrap_or_default();

    let mut all_data = serde_json::Map::new();
    all_data.insert(
        "_meta".to_string(),
        serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "exported_at": chrono::Utc::now().to_rfc3339(),
            "format": format!("{format:?}"),
            "source": conn,
        }),
    );
    all_data.insert("schema".to_string(), schema.clone());

    let mut entities = serde_json::Map::new();

    for entity_type in &entity_types {
        spinner.set_message(format!("Exporting {entity_type}..."));

        let query = serde_json::json!({
            "query": {
                "entity_type": entity_type,
                "limit": 100_000
            }
        });

        let resp = client
            .post(format!("{conn}/api/query"))
            .bearer_auth(&token)
            .json(&query)
            .timeout(Duration::from_secs(60))
            .send()
            .await
            .context(format!("Failed to query {entity_type}"))?;

        if resp.status().is_success() {
            let data: serde_json::Value = resp.json().await?;
            let count = data.as_array().map(|a| a.len()).unwrap_or(0);
            entities.insert(entity_type.clone(), data);
            spinner.set_message(format!("Exported {entity_type} ({count} rows)"));
        } else {
            tracing::warn!("Failed to export {entity_type}: {}", resp.status());
        }
    }

    all_data.insert("entities".to_string(), serde_json::Value::Object(entities));
    spinner.set_message("Writing to file...");

    let export_data = serde_json::Value::Object(all_data);

    match format {
        ExportFormat::Json => {
            let content = serde_json::to_string_pretty(&export_data)
                .context("Failed to serialize export data")?;
            tokio::fs::write(&output_file, content)
                .await
                .context("Failed to write export file")?;
        }
        ExportFormat::Jsonl => {
            let mut content = String::new();
            // Write meta line
            if let Some(meta) = export_data.get("_meta") {
                content.push_str(&serde_json::to_string(meta)?);
                content.push('\n');
            }
            // Write each entity as a line
            if let Some(entities) = export_data.get("entities").and_then(|v| v.as_object()) {
                for (entity_type, rows) in entities {
                    if let Some(arr) = rows.as_array() {
                        for row in arr {
                            let line = serde_json::json!({
                                "_type": entity_type,
                                "_data": row,
                            });
                            content.push_str(&serde_json::to_string(&line)?);
                            content.push('\n');
                        }
                    }
                }
            }
            tokio::fs::write(&output_file, content)
                .await
                .context("Failed to write export file")?;
        }
    }

    spinner.finish_with_message("Export complete");
    println!(
        "  {} Saved to {}\n",
        "-->".bright_green(),
        output_file.bright_yellow()
    );

    Ok(())
}

/// Import data into a DarshJDB instance.
pub async fn run_import(
    conn: String,
    file: String,
    token: Option<String>,
    yes: bool,
) -> Result<()> {
    println!("\n  {} DarshJDB Import\n", ">>>".bright_cyan().bold());

    let token = token
        .or_else(|| std::env::var("DDB_TOKEN").ok())
        .unwrap_or_default();

    let path = std::path::Path::new(&file);
    if !path.exists() {
        anyhow::bail!("Import file not found: {file}");
    }

    let content = tokio::fs::read_to_string(&file)
        .await
        .context("Failed to read import file")?;

    // Detect format
    let is_jsonl = file.ends_with(".jsonl");

    if !yes {
        println!(
            "  {} This will import data from: {}",
            "WARNING:".bright_yellow().bold(),
            file.bright_white()
        );
        let confirm = dialoguer::Confirm::new()
            .with_prompt("  Continue?")
            .default(true)
            .interact()?;

        if !confirm {
            println!("  Aborted.");
            return Ok(());
        }
    }

    let client = reqwest::Client::new();

    if is_jsonl {
        // JSONL format: each line is a record
        let lines: Vec<&str> = content.lines().collect();
        let pb = progress_bar(lines.len() as u64, "Importing");

        for (i, line) in lines.iter().enumerate() {
            let parsed: serde_json::Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => {
                    pb.set_message(format!("Skipping invalid line {}", i + 1));
                    pb.inc(1);
                    continue;
                }
            };

            // Skip meta lines
            if parsed.get("_type").is_none() {
                pb.inc(1);
                continue;
            }

            let entity_type = parsed
                .get("_type")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let data = parsed.get("_data").unwrap_or(&parsed);

            pb.set_message(format!("Importing {entity_type}..."));

            let resp = client
                .post(format!("{conn}/api/mutate"))
                .bearer_auth(&token)
                .json(&serde_json::json!({
                    "operations": [{
                        "op": "set",
                        "entity_type": entity_type,
                        "data": data,
                    }]
                }))
                .send()
                .await;

            if let Err(e) = resp {
                tracing::warn!("Failed to import line {}: {e}", i + 1);
            }

            pb.inc(1);
        }

        pb.finish_with_message("Import complete");
    } else {
        // JSON format: full export object
        let export: serde_json::Value =
            serde_json::from_str(&content).context("Invalid JSON in import file")?;

        if let Some(entities) = export.get("entities").and_then(|v| v.as_object()) {
            let total: u64 = entities
                .values()
                .filter_map(|v| v.as_array())
                .map(|a| a.len() as u64)
                .sum();

            let pb = progress_bar(total, "Importing");

            for (entity_type, rows) in entities {
                if let Some(arr) = rows.as_array() {
                    for row in arr {
                        pb.set_message(format!("Importing {entity_type}..."));

                        let resp = client
                            .post(format!("{conn}/api/mutate"))
                            .bearer_auth(&token)
                            .json(&serde_json::json!({
                                "operations": [{
                                    "op": "set",
                                    "entity_type": entity_type,
                                    "data": row,
                                }]
                            }))
                            .send()
                            .await;

                        if let Err(e) = resp {
                            tracing::warn!("Failed to import {entity_type}: {e}");
                        }

                        pb.inc(1);
                    }
                }
            }

            pb.finish_with_message("Import complete");
        } else {
            anyhow::bail!("Import file does not contain an 'entities' object");
        }
    }

    println!("\n  {} Import complete\n", "-->".bright_green());

    Ok(())
}

// ── Types ──────────────────────────────────────────────────────────

/// Export format.
#[derive(Clone, Debug, Default, clap::ValueEnum)]
pub enum ExportFormat {
    /// Pretty-printed JSON
    #[default]
    Json,
    /// Newline-delimited JSON (one record per line)
    Jsonl,
}

// ── Helpers ────────────────────────────────────────────────────────

fn spinner(msg: &str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::with_template("  {spinner:.cyan} {msg}")
            .expect("valid template")
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
            .expect("valid template")
            .progress_chars("=> "),
    );
    pb.set_prefix(prefix.to_string());
    pb
}
