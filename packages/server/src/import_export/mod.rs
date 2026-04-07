//! Import/Export system for DarshJDB.
//!
//! Provides streaming CSV and JSON import/export with automatic type
//! inference, field mapping, and batched UNNEST bulk inserts. Designed
//! for data migration and interoperability with external systems.
//!
//! # Architecture
//!
//! - **Streaming throughout** — neither imports nor exports buffer the
//!   entire dataset in memory. CSV/JSON rows are parsed incrementally
//!   and flushed in configurable batches; exports stream rows directly
//!   from a Postgres cursor to the HTTP response body.
//!
//! - **EAV mapping** — incoming tabular/JSON records are decomposed
//!   into `(entity_id, attribute, value, value_type)` triples using
//!   the same UNNEST bulk-insert path as `/admin/bulk-load`.
//!
//! - **Progress tracking** — every import returns an [`ImportResult`]
//!   with counts of processed, imported, and skipped rows plus a
//!   per-error log so callers can diagnose partial failures.

pub mod csv_export;
pub mod csv_import;
pub mod handlers;
pub mod json_export;
pub mod json_import;
pub mod mapping;

use serde::{Deserialize, Serialize};

// ── Shared result types ───────────────────────────────────────────────

/// Outcome of an import operation (CSV or JSON).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportResult {
    /// Total rows/objects read from the input.
    pub rows_processed: usize,
    /// Rows successfully converted to triples and inserted.
    pub rows_imported: usize,
    /// Rows skipped due to parse or validation errors.
    pub rows_skipped: usize,
    /// Per-row error descriptions (row index + message).
    pub errors: Vec<ImportError>,
    /// Number of triples written across all batches.
    pub triples_written: usize,
    /// Wall-clock duration in milliseconds.
    pub duration_ms: u64,
}

/// A single row-level error encountered during import.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportError {
    /// Zero-based row index in the source file.
    pub row: usize,
    /// Human-readable description of the problem.
    pub message: String,
}

/// Outcome of an export operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportResult {
    /// Number of entities written to the output.
    pub entities_exported: usize,
    /// Wall-clock duration in milliseconds.
    pub duration_ms: u64,
}
