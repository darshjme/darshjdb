//! `ddb sql` — Interactive DarshQL shell.
//!
//! Connects to a running DarshJDB instance over HTTP and provides a
//! REPL with syntax highlighting, table-formatted output, multi-line
//! query support, and command history.

use anyhow::{Context, Result};
use colored::Colorize;
use comfy_table::{Cell, CellAlignment, Color, Table};
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::HistoryHinter;
use rustyline::{Completer, Helper, Hinter, Validator};
use std::borrow::Cow;

// ── DarshQL Syntax Highlighter ─────────────────────────────────────

/// Keywords recognized by DarshQL for syntax highlighting.
const KEYWORDS: &[&str] = &[
    "SELECT", "FROM", "WHERE", "INSERT", "UPDATE", "DELETE", "CREATE",
    "DROP", "ALTER", "SET", "INTO", "VALUES", "AND", "OR", "NOT",
    "ORDER", "BY", "LIMIT", "OFFSET", "ASC", "DESC", "GROUP",
    "HAVING", "JOIN", "LEFT", "RIGHT", "INNER", "OUTER", "ON",
    "AS", "IN", "IS", "NULL", "TRUE", "FALSE", "LIKE", "BETWEEN",
    "EXISTS", "DISTINCT", "COUNT", "SUM", "AVG", "MIN", "MAX",
    // DarshJDB-specific
    "DEFINE", "NAMESPACE", "DATABASE", "TABLE", "FIELD", "INDEX",
    "UNIQUE", "SEARCH", "SEMANTIC", "HYBRID", "RELATE", "CONTENT",
    "MERGE", "PATCH", "RETURN", "FETCH", "SPLIT", "LET", "BEGIN",
    "COMMIT", "CANCEL", "IF", "THEN", "ELSE", "END", "FOR",
    "LIVE", "KILL", "SLEEP", "INFO", "USE", "SHOW", "PERMISSIONS",
    "TYPE", "ASSERT", "VALUE", "DEFAULT", "READONLY", "FLEXIBLE",
    "TOKENIZER", "ANALYZER", "FUNCTION", "PARAM", "THROW",
    "entity_type", "where_clauses", "order", "limit", "offset",
    "search", "semantic", "hybrid", "nested",
];

/// Simple per-word keyword highlighter for the REPL.
#[derive(Completer, Helper, Hinter, Validator)]
struct DarshQlHelper {
    #[rustyline(Hinter)]
    hinter: HistoryHinter,
}

impl Highlighter for DarshQlHelper {
    fn highlight<'l>(&self, line: &'l str, _pos: usize) -> Cow<'l, str> {
        let mut result = String::with_capacity(line.len() + 64);
        let mut in_string = false;
        let mut string_char = '"';
        let mut word_start = None;

        for (i, ch) in line.char_indices() {
            if in_string {
                result.push(ch);
                if ch == string_char {
                    in_string = false;
                    // Reset coloring would happen naturally
                }
                continue;
            }

            if ch == '"' || ch == '\'' {
                // Flush any pending word
                if let Some(start) = word_start.take() {
                    let word = &line[start..i];
                    result.push_str(&highlight_word(word));
                }
                in_string = true;
                string_char = ch;
                // Green for strings
                result.push_str("\x1b[32m");
                result.push(ch);
                continue;
            }

            if ch.is_alphanumeric() || ch == '_' || ch == '$' {
                if word_start.is_none() {
                    word_start = Some(i);
                }
            } else {
                if let Some(start) = word_start.take() {
                    let word = &line[start..i];
                    result.push_str(&highlight_word(word));
                }
                // Operators and punctuation in default color
                if "{}[]():;,.".contains(ch) {
                    result.push_str("\x1b[37m"); // white for punctuation
                    result.push(ch);
                    result.push_str("\x1b[0m");
                } else {
                    result.push(ch);
                }
            }
        }

        // Flush trailing word
        if let Some(start) = word_start {
            let word = &line[start..];
            result.push_str(&highlight_word(word));
        }

        result.push_str("\x1b[0m");
        Cow::Owned(result)
    }

    fn highlight_char(&self, _line: &str, _pos: usize, _forced: bool) -> bool {
        true
    }

    fn highlight_prompt<'b, 's: 'b, 'p: 'b>(
        &'s self,
        prompt: &'p str,
        _default: bool,
    ) -> Cow<'b, str> {
        Cow::Owned(format!("\x1b[1;36m{prompt}\x1b[0m"))
    }
}

/// Colorize a single word: keywords cyan+bold, numbers magenta, else default.
fn highlight_word(word: &str) -> String {
    let upper = word.to_uppercase();
    if KEYWORDS.iter().any(|kw| *kw == upper) {
        format!("\x1b[1;36m{word}\x1b[0m") // bold cyan
    } else if word.parse::<f64>().is_ok() {
        format!("\x1b[35m{word}\x1b[0m") // magenta for numbers
    } else if word.starts_with('$') {
        format!("\x1b[33m{word}\x1b[0m") // yellow for variables
    } else {
        format!("\x1b[0m{word}")
    }
}

// ── SQL Shell REPL ─────────────────────────────────────────────────

/// Run the interactive DarshQL shell.
pub async fn run(
    conn: String,
    user: Option<String>,
    pass: Option<String>,
    ns: Option<String>,
    db: Option<String>,
    pretty: bool,
) -> Result<()> {
    println!();
    println!(
        "  {}{}{}",
        " DarshJDB ".on_bright_cyan().black().bold(),
        " SQL Shell v".bright_white(),
        env!("CARGO_PKG_VERSION").bright_white()
    );
    println!();
    println!(
        "  {} Connected to {}",
        "-->".bright_green(),
        conn.bright_yellow()
    );
    if let Some(ref n) = ns {
        println!("  {} Namespace: {}", "-->".bright_green(), n.bright_white());
    }
    if let Some(ref d) = db {
        println!("  {} Database:  {}", "-->".bright_green(), d.bright_white());
    }
    println!(
        "  {} Type {} for help, {} to exit",
        "-->".bright_green(),
        ".help".bright_cyan(),
        ".quit".bright_cyan()
    );
    println!();

    // Verify server is reachable
    let client = reqwest::Client::new();
    match client
        .get(format!("{conn}/health"))
        .timeout(std::time::Duration::from_secs(3))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {}
        Ok(resp) => {
            eprintln!(
                "  {} Server returned status {}",
                "!!!".bright_red(),
                resp.status()
            );
        }
        Err(e) => {
            eprintln!(
                "  {} Cannot reach server at {}: {}",
                "!!!".bright_red(),
                conn,
                e
            );
            eprintln!(
                "  {} Is the server running? Start it with: {}",
                "   ".normal(),
                "ddb start".bright_yellow()
            );
            return Ok(());
        }
    }

    // Authenticate if credentials provided
    let token = if let (Some(u), Some(p)) = (&user, &pass) {
        match authenticate(&client, &conn, u, p).await {
            Ok(t) => {
                println!(
                    "  {} Authenticated as {}",
                    "-->".bright_green(),
                    u.bright_white()
                );
                println!();
                Some(t)
            }
            Err(e) => {
                eprintln!(
                    "  {} Authentication failed: {}",
                    "!!!".bright_red(),
                    e
                );
                None
            }
        }
    } else {
        // Try token from env
        std::env::var("DDB_TOKEN").ok()
    };

    // Set up rustyline
    let helper = DarshQlHelper {
        hinter: HistoryHinter::new(),
    };

    let config = rustyline::Config::builder()
        .max_history_size(1000)
        .expect("valid history size")
        .auto_add_history(true)
        .build();

    let mut rl = rustyline::Editor::with_config(config)?;
    rl.set_helper(Some(helper));

    // Load history
    let history_path = dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("darshjdb")
        .join("sql_history");

    if let Some(parent) = history_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = rl.load_history(&history_path);

    let mut multi_line_buffer = String::new();
    let mut query_count: u64 = 0;

    loop {
        let prompt = if multi_line_buffer.is_empty() {
            "ddb> ".to_string()
        } else {
            "  -> ".to_string()
        };

        match rl.readline(&prompt) {
            Ok(line) => {
                let trimmed = line.trim();

                // Handle dot-commands
                if multi_line_buffer.is_empty() && trimmed.starts_with('.') {
                    match handle_dot_command(trimmed) {
                        DotResult::Quit => break,
                        DotResult::Help => continue,
                        DotResult::Clear => {
                            multi_line_buffer.clear();
                            println!("  Buffer cleared.");
                            continue;
                        }
                        DotResult::Status => {
                            print_status(&client, &conn, token.as_deref()).await;
                            continue;
                        }
                        DotResult::Unknown => {
                            eprintln!("  Unknown command: {trimmed}. Type .help for help.");
                            continue;
                        }
                    }
                }

                // Handle empty line
                if trimmed.is_empty() && multi_line_buffer.is_empty() {
                    continue;
                }

                // Accumulate multi-line input
                if !multi_line_buffer.is_empty() {
                    multi_line_buffer.push(' ');
                }
                multi_line_buffer.push_str(trimmed);

                // Check if query is complete (ends with semicolon or is JSON)
                let is_complete = trimmed.ends_with(';')
                    || (multi_line_buffer.starts_with('{')
                        && serde_json::from_str::<serde_json::Value>(&multi_line_buffer).is_ok());

                if !is_complete {
                    continue;
                }

                // Remove trailing semicolon for JSON queries
                let query = multi_line_buffer.trim_end_matches(';').trim().to_string();
                multi_line_buffer.clear();

                if query.is_empty() {
                    continue;
                }

                query_count += 1;
                let start = std::time::Instant::now();

                match execute_query(&client, &conn, &query, token.as_deref(), ns.as_deref(), db.as_deref()).await {
                    Ok(response) => {
                        let elapsed = start.elapsed();
                        if pretty {
                            print_result_table(&response);
                        } else {
                            print_result_json(&response);
                        }
                        println!(
                            "  {} Query #{query_count} executed in {:.2}ms\n",
                            "---".dimmed(),
                            elapsed.as_secs_f64() * 1000.0
                        );
                    }
                    Err(e) => {
                        eprintln!("  {} {}\n", "ERROR:".bright_red().bold(), e);
                    }
                }
            }
            Err(ReadlineError::Interrupted) => {
                if multi_line_buffer.is_empty() {
                    println!("  (Use .quit or Ctrl+D to exit)");
                } else {
                    multi_line_buffer.clear();
                    println!("  Query cancelled.");
                }
            }
            Err(ReadlineError::Eof) => {
                println!("  Goodbye.");
                break;
            }
            Err(e) => {
                eprintln!("  Error: {e}");
                break;
            }
        }
    }

    let _ = rl.save_history(&history_path);
    Ok(())
}

// ── Dot-Commands ───────────────────────────────────────────────────

enum DotResult {
    Quit,
    Help,
    Clear,
    Status,
    Unknown,
}

fn handle_dot_command(cmd: &str) -> DotResult {
    match cmd {
        ".quit" | ".exit" | ".q" => DotResult::Quit,
        ".help" | ".h" => {
            println!();
            println!("  {}", "DarshQL Shell Commands".bright_white().bold());
            println!("  {}", "=".repeat(40).dimmed());
            println!(
                "  {}   {}",
                ".help".bright_cyan(),
                "Show this help message"
            );
            println!(
                "  {}  {}",
                ".clear".bright_cyan(),
                "Clear the multi-line buffer"
            );
            println!(
                "  {} {}",
                ".status".bright_cyan(),
                "Show server status"
            );
            println!(
                "  {}   {}",
                ".quit".bright_cyan(),
                "Exit the shell"
            );
            println!();
            println!("  {}", "Query Syntax".bright_white().bold());
            println!("  {}", "=".repeat(40).dimmed());
            println!("  End queries with a semicolon (;) to execute.");
            println!("  JSON queries are auto-detected when braces balance.");
            println!();
            println!("  {}", "Examples:".bright_white());
            println!(
                "  {}",
                r#"  {"entity_type": "User", "limit": 10};"#.bright_green()
            );
            println!(
                "  {}",
                r#"  SELECT * FROM User WHERE age > 18;"#.bright_green()
            );
            println!();
            DotResult::Help
        }
        ".clear" | ".c" => DotResult::Clear,
        ".status" | ".s" => DotResult::Status,
        _ => DotResult::Unknown,
    }
}

// ── Query Execution ────────────────────────────────────────────────

/// Execute a query against the DarshJDB server.
async fn execute_query(
    client: &reqwest::Client,
    conn: &str,
    query: &str,
    token: Option<&str>,
    _ns: Option<&str>,
    _db: Option<&str>,
) -> Result<serde_json::Value> {
    // Determine if query is JSON (DarshJQL) or text (SQL-like)
    let body = if query.starts_with('{') || query.starts_with('[') {
        // JSON DarshJQL query
        let parsed: serde_json::Value =
            serde_json::from_str(query).context("Invalid JSON query")?;
        serde_json::json!({ "query": parsed })
    } else {
        // SQL-like text query — send as a text query
        serde_json::json!({ "query": { "raw": query } })
    };

    let mut req = client
        .post(format!("{conn}/api/query"))
        .json(&body)
        .timeout(std::time::Duration::from_secs(30));

    if let Some(t) = token {
        req = req.bearer_auth(t);
    }

    let resp = req.send().await.context("Query request failed")?;
    let status = resp.status();

    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("{status} — {body}");
    }

    let result: serde_json::Value = resp.json().await.context("Failed to parse response")?;
    Ok(result)
}

/// Authenticate and get a token.
async fn authenticate(
    client: &reqwest::Client,
    conn: &str,
    user: &str,
    pass: &str,
) -> Result<String> {
    let resp = client
        .post(format!("{conn}/api/auth/signin"))
        .json(&serde_json::json!({
            "email": user,
            "password": pass,
        }))
        .send()
        .await
        .context("Authentication request failed")?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("{body}");
    }

    let body: serde_json::Value = resp.json().await?;
    body.get("token")
        .and_then(|t| t.as_str())
        .map(String::from)
        .ok_or_else(|| anyhow::anyhow!("No token in auth response"))
}

/// Print server status.
async fn print_status(client: &reqwest::Client, conn: &str, token: Option<&str>) {
    let mut req = client
        .get(format!("{conn}/health"))
        .timeout(std::time::Duration::from_secs(3));
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }

    match req.send().await {
        Ok(resp) if resp.status().is_success() => {
            if let Ok(body) = resp.json::<serde_json::Value>().await {
                println!();
                println!("  {}", "Server Status".bright_white().bold());
                println!("  {}", "=".repeat(30).dimmed());
                if let Some(status) = body.get("status").and_then(|v| v.as_str()) {
                    println!(
                        "  Status:  {}",
                        if status == "healthy" {
                            status.bright_green()
                        } else {
                            status.bright_red()
                        }
                    );
                }
                if let Some(version) = body.get("version").and_then(|v| v.as_str()) {
                    println!("  Version: {}", version.bright_white());
                }
                println!();
            }
        }
        Ok(resp) => {
            eprintln!("  Server returned: {}", resp.status());
        }
        Err(e) => {
            eprintln!("  Cannot reach server: {e}");
        }
    }
}

// ── Result Formatting ──────────────────────────────────────────────

/// Print query results as a formatted table.
fn print_result_table(value: &serde_json::Value) {
    // Try to extract an array of results
    let rows = if let Some(arr) = value.as_array() {
        arr
    } else if let Some(data) = value.get("data").or(value.get("results")) {
        if let Some(arr) = data.as_array() {
            arr
        } else {
            // Single result
            println!();
            print_result_json(value);
            return;
        }
    } else {
        println!();
        print_result_json(value);
        return;
    };

    if rows.is_empty() {
        println!("  {} 0 rows returned", "---".dimmed());
        return;
    }

    // Collect all unique keys from the result set
    let mut columns: Vec<String> = Vec::new();
    for row in rows {
        if let Some(obj) = row.as_object() {
            for key in obj.keys() {
                if !columns.contains(key) {
                    columns.push(key.clone());
                }
            }
        }
    }

    if columns.is_empty() {
        // Not objects, just print as JSON array
        for row in rows {
            println!("  {}", serde_json::to_string_pretty(row).unwrap_or_default());
        }
        return;
    }

    // Sort columns: id first, then alphabetical
    columns.sort_by(|a, b| {
        if a == "id" {
            std::cmp::Ordering::Less
        } else if b == "id" {
            std::cmp::Ordering::Greater
        } else {
            a.cmp(b)
        }
    });

    // Build table
    let mut table = Table::new();
    table.set_content_arrangement(comfy_table::ContentArrangement::Dynamic);
    table.load_preset(comfy_table::presets::UTF8_FULL_CONDENSED);

    // Header row
    let headers: Vec<Cell> = columns
        .iter()
        .map(|c| {
            Cell::new(c)
                .fg(Color::Cyan)
                .set_alignment(CellAlignment::Center)
        })
        .collect();
    table.set_header(headers);

    // Data rows
    for row in rows {
        let cells: Vec<Cell> = columns
            .iter()
            .map(|col| {
                let val = row.get(col).unwrap_or(&serde_json::Value::Null);
                let display = format_cell_value(val);
                Cell::new(display)
            })
            .collect();
        table.add_row(cells);
    }

    println!();
    // Indent the table
    for line in table.to_string().lines() {
        println!("  {line}");
    }
    println!(
        "\n  {} {} row(s) returned",
        "---".dimmed(),
        rows.len()
    );
}

/// Format a JSON value for display in a table cell.
fn format_cell_value(val: &serde_json::Value) -> String {
    match val {
        serde_json::Value::Null => "NULL".to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => {
            if s.len() > 50 {
                format!("{}...", &s[..47])
            } else {
                s.clone()
            }
        }
        serde_json::Value::Array(arr) => {
            let repr = serde_json::to_string(arr).unwrap_or_default();
            if repr.len() > 50 {
                format!("[{} items]", arr.len())
            } else {
                repr
            }
        }
        serde_json::Value::Object(obj) => {
            let repr = serde_json::to_string(obj).unwrap_or_default();
            if repr.len() > 50 {
                format!("{{{} keys}}", obj.len())
            } else {
                repr
            }
        }
    }
}

/// Print raw JSON output (when --pretty is false).
fn print_result_json(value: &serde_json::Value) {
    match serde_json::to_string_pretty(value) {
        Ok(pretty) => {
            for line in pretty.lines() {
                println!("  {line}");
            }
        }
        Err(_) => println!("  {value}"),
    }
}
