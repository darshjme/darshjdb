//! HTTP handlers for import/export operations.
//!
//! Provides multipart upload for CSV/JSON import, streaming download
//! for CSV/JSON export, and import job status tracking.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::error::ApiError;
use crate::api::rest::AppState;

use super::csv_export::{CsvExportConfig, export_csv};
use super::csv_import::{CsvImportConfig, import_csv};
use super::json_export::{JsonExportConfig, export_json};
use super::json_import::{JsonImportConfig, import_json};
use super::ImportResult;

// ── Job tracking ──────────────────────────────────────────────────────

/// Status of an import job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportJobStatus {
    /// Unique job identifier.
    pub job_id: String,
    /// Current state of the job.
    pub state: JobState,
    /// Import result (populated when state is `Completed`).
    pub result: Option<ImportResult>,
    /// Error message (populated when state is `Failed`).
    pub error: Option<String>,
}

/// Possible states for an import job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobState {
    /// Job is queued or actively running.
    Running,
    /// Job completed successfully.
    Completed,
    /// Job failed with an error.
    Failed,
}

/// In-memory job status tracker.
///
/// For production deployments with multiple server instances, this
/// would be backed by Redis or Postgres. The in-memory DashMap is
/// sufficient for single-node and development.
pub type JobTracker = Arc<DashMap<String, ImportJobStatus>>;

/// Create a new job tracker instance.
pub fn new_job_tracker() -> JobTracker {
    Arc::new(DashMap::new())
}

// ── Query parameters ──────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ExportQuery {
    /// Entity type to export (required).
    #[serde(rename = "type")]
    pub entity_type: String,

    /// Comma-separated list of fields to include (optional).
    /// When empty, all attributes are exported.
    #[serde(default)]
    pub fields: Option<String>,

    /// CSV delimiter (default: `,`).
    #[serde(default)]
    pub delimiter: Option<String>,

    /// Whether to pretty-print JSON (default: false).
    #[serde(default)]
    pub pretty: Option<bool>,

    /// JSON export format: `array` or `ndjson` (default: `array`).
    #[serde(default)]
    pub format: Option<String>,
}

// ── Router ────────────────────────────────────────────────────────────

/// Build the import/export sub-router.
///
/// All routes are nested under `/api/import` and `/api/export` by the
/// caller in `build_router`.
pub fn import_export_routes(job_tracker: JobTracker) -> Router<AppState> {
    Router::new()
        .route("/import/csv", post(import_csv_handler))
        .route("/import/json", post(import_json_handler))
        .route("/export/csv", get(export_csv_handler))
        .route("/export/json", get(export_json_handler))
        .route("/import/status/{job_id}", get(import_status_handler))
        .layer(axum::Extension(job_tracker))
}

// ── Import handlers ───────────────────────────────────────────────────

/// `POST /api/import/csv` — Upload CSV and import into an entity type.
///
/// Accepts `multipart/form-data` with:
/// - `file`: the CSV file
/// - `config`: JSON-encoded [`CsvImportConfig`] (optional)
///
/// For large files, the import runs asynchronously and returns a job ID
/// that can be polled via `GET /api/import/status/{job_id}`.
async fn import_csv_handler(
    State(state): State<AppState>,
    axum::Extension(tracker): axum::Extension<JobTracker>,
    mut multipart: Multipart,
) -> Result<Response, ApiError> {
    let mut file_data: Option<Vec<u8>> = None;
    let mut config = CsvImportConfig::default();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::bad_request(format!("Multipart error: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "file" => {
                file_data = Some(
                    field
                        .bytes()
                        .await
                        .map_err(|e| ApiError::bad_request(format!("Failed to read file: {e}")))?
                        .to_vec(),
                );
            }
            "config" => {
                let text = field
                    .text()
                    .await
                    .map_err(|e| ApiError::bad_request(format!("Failed to read config: {e}")))?;
                config = serde_json::from_str(&text)
                    .map_err(|e| ApiError::bad_request(format!("Invalid config JSON: {e}")))?;
            }
            _ => {}
        }
    }

    let data = file_data.ok_or_else(|| ApiError::bad_request("Missing 'file' field"))?;

    if config.entity_type.is_empty() {
        return Err(ApiError::bad_request(
            "entity_type is required in config",
        ));
    }

    // For files larger than 1MB, run async and return job ID.
    let large_threshold = 1_048_576;
    if data.len() > large_threshold {
        let job_id = Uuid::new_v4().to_string();
        tracker.insert(
            job_id.clone(),
            ImportJobStatus {
                job_id: job_id.clone(),
                state: JobState::Running,
                result: None,
                error: None,
            },
        );

        let pool = state.pool.clone();
        let tracker_clone = tracker.clone();
        let job_id_clone = job_id.clone();

        tokio::spawn(async move {
            match import_csv(&pool, &data, &config).await {
                Ok(result) => {
                    tracker_clone.insert(
                        job_id_clone.clone(),
                        ImportJobStatus {
                            job_id: job_id_clone,
                            state: JobState::Completed,
                            result: Some(result),
                            error: None,
                        },
                    );
                }
                Err(e) => {
                    tracker_clone.insert(
                        job_id_clone.clone(),
                        ImportJobStatus {
                            job_id: job_id_clone,
                            state: JobState::Failed,
                            result: None,
                            error: Some(e.to_string()),
                        },
                    );
                }
            }
        });

        let response = serde_json::json!({
            "ok": true,
            "async": true,
            "job_id": job_id,
            "message": "Large file import started. Poll /api/import/status/{job_id} for progress."
        });

        return Ok((StatusCode::ACCEPTED, axum::Json(response)).into_response());
    }

    // Synchronous import for smaller files.
    let result = import_csv(&state.pool, &data, &config)
        .await
        .map_err(|e| ApiError::internal(format!("CSV import failed: {e}")))?;

    let response = serde_json::json!({
        "ok": true,
        "async": false,
        "rows_processed": result.rows_processed,
        "rows_imported": result.rows_imported,
        "rows_skipped": result.rows_skipped,
        "triples_written": result.triples_written,
        "duration_ms": result.duration_ms,
        "errors": result.errors,
    });

    Ok((StatusCode::OK, axum::Json(response)).into_response())
}

/// `POST /api/import/json` — Upload JSON and import into an entity type.
///
/// Accepts `multipart/form-data` with:
/// - `file`: the JSON/NDJSON file
/// - `config`: JSON-encoded [`JsonImportConfig`] (optional)
async fn import_json_handler(
    State(state): State<AppState>,
    axum::Extension(tracker): axum::Extension<JobTracker>,
    mut multipart: Multipart,
) -> Result<Response, ApiError> {
    let mut file_data: Option<Vec<u8>> = None;
    let mut config = JsonImportConfig::default();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::bad_request(format!("Multipart error: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "file" => {
                file_data = Some(
                    field
                        .bytes()
                        .await
                        .map_err(|e| ApiError::bad_request(format!("Failed to read file: {e}")))?
                        .to_vec(),
                );
            }
            "config" => {
                let text = field
                    .text()
                    .await
                    .map_err(|e| ApiError::bad_request(format!("Failed to read config: {e}")))?;
                config = serde_json::from_str(&text)
                    .map_err(|e| ApiError::bad_request(format!("Invalid config JSON: {e}")))?;
            }
            _ => {}
        }
    }

    let data = file_data.ok_or_else(|| ApiError::bad_request("Missing 'file' field"))?;

    if config.entity_type.is_empty() {
        return Err(ApiError::bad_request(
            "entity_type is required in config",
        ));
    }

    // For files larger than 1MB, run async.
    let large_threshold = 1_048_576;
    if data.len() > large_threshold {
        let job_id = Uuid::new_v4().to_string();
        tracker.insert(
            job_id.clone(),
            ImportJobStatus {
                job_id: job_id.clone(),
                state: JobState::Running,
                result: None,
                error: None,
            },
        );

        let pool = state.pool.clone();
        let tracker_clone = tracker.clone();
        let job_id_clone = job_id.clone();

        tokio::spawn(async move {
            match import_json(&pool, &data, &config).await {
                Ok(result) => {
                    tracker_clone.insert(
                        job_id_clone.clone(),
                        ImportJobStatus {
                            job_id: job_id_clone,
                            state: JobState::Completed,
                            result: Some(result),
                            error: None,
                        },
                    );
                }
                Err(e) => {
                    tracker_clone.insert(
                        job_id_clone.clone(),
                        ImportJobStatus {
                            job_id: job_id_clone,
                            state: JobState::Failed,
                            result: None,
                            error: Some(e.to_string()),
                        },
                    );
                }
            }
        });

        let response = serde_json::json!({
            "ok": true,
            "async": true,
            "job_id": job_id,
            "message": "Large file import started. Poll /api/import/status/{job_id} for progress."
        });

        return Ok((StatusCode::ACCEPTED, axum::Json(response)).into_response());
    }

    // Synchronous import.
    let result = import_json(&state.pool, &data, &config)
        .await
        .map_err(|e| ApiError::internal(format!("JSON import failed: {e}")))?;

    let response = serde_json::json!({
        "ok": true,
        "async": false,
        "rows_processed": result.rows_processed,
        "rows_imported": result.rows_imported,
        "rows_skipped": result.rows_skipped,
        "triples_written": result.triples_written,
        "duration_ms": result.duration_ms,
        "errors": result.errors,
    });

    Ok((StatusCode::OK, axum::Json(response)).into_response())
}

// ── Export handlers ───────────────────────────────────────────────────

/// `GET /api/export/csv?type={entity_type}` — Export entities as CSV download.
async fn export_csv_handler(
    State(state): State<AppState>,
    Query(params): Query<ExportQuery>,
) -> Result<Response, ApiError> {
    let fields: Vec<String> = params
        .fields
        .as_deref()
        .unwrap_or("")
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let csv_config = CsvExportConfig {
        delimiter: params
            .delimiter
            .as_deref()
            .and_then(|d| d.as_bytes().first().copied())
            .unwrap_or(b','),
        ..Default::default()
    };

    let (data, result) = export_csv(&state.pool, &params.entity_type, &fields, &csv_config)
        .await
        .map_err(|e| ApiError::internal(format!("CSV export failed: {e}")))?;

    let filename = format!("{}.csv", params.entity_type);
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "text/csv; charset=utf-8")
        .header(
            CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", filename),
        )
        .header("X-Export-Count", result.entities_exported.to_string())
        .header("X-Export-Duration-Ms", result.duration_ms.to_string())
        .body(Body::from(data))
        .map_err(|e| ApiError::internal(format!("Failed to build response: {e}")))?;

    Ok(response)
}

/// `GET /api/export/json?type={entity_type}` — Export entities as JSON download.
async fn export_json_handler(
    State(state): State<AppState>,
    Query(params): Query<ExportQuery>,
) -> Result<Response, ApiError> {
    let json_config = JsonExportConfig {
        pretty: params.pretty.unwrap_or(false),
        format: match params.format.as_deref() {
            Some("ndjson") => super::json_export::JsonExportFormat::Ndjson,
            _ => super::json_export::JsonExportFormat::Array,
        },
    };

    let (data, result) = export_json(&state.pool, &params.entity_type, &json_config)
        .await
        .map_err(|e| ApiError::internal(format!("JSON export failed: {e}")))?;

    let content_type = match json_config.format {
        super::json_export::JsonExportFormat::Ndjson => "application/x-ndjson; charset=utf-8",
        super::json_export::JsonExportFormat::Array => "application/json; charset=utf-8",
    };

    let extension = match json_config.format {
        super::json_export::JsonExportFormat::Ndjson => "ndjson",
        super::json_export::JsonExportFormat::Array => "json",
    };

    let filename = format!("{}.{}", params.entity_type, extension);
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, content_type)
        .header(
            CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", filename),
        )
        .header("X-Export-Count", result.entities_exported.to_string())
        .header("X-Export-Duration-Ms", result.duration_ms.to_string())
        .body(Body::from(data))
        .map_err(|e| ApiError::internal(format!("Failed to build response: {e}")))?;

    Ok(response)
}

// ── Status handler ────────────────────────────────────────────────────

/// `GET /api/import/status/{job_id}` — Check import job progress.
async fn import_status_handler(
    Path(job_id): Path<String>,
    axum::Extension(tracker): axum::Extension<JobTracker>,
) -> Result<Response, ApiError> {
    let status = tracker
        .get(&job_id)
        .map(|entry| entry.value().clone())
        .ok_or_else(|| ApiError::not_found(format!("Import job '{job_id}' not found")))?;

    let response = serde_json::json!({
        "ok": true,
        "job_id": status.job_id,
        "state": status.state,
        "result": status.result,
        "error": status.error,
    });

    Ok((StatusCode::OK, axum::Json(response)).into_response())
}
