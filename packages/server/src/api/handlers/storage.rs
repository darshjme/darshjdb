//! File storage handlers: upload, download, delete.

use axum::body::Body;
use axum::extract::{FromRequest, Path, Query, State};
use axum::http::header::CONTENT_TYPE;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::http::Request;
use serde::Deserialize;
use uuid::Uuid;

use crate::api::error::ApiError;
use crate::api::rest::AppState;

use super::helpers::{
    extract_bearer_token, negotiate_response, negotiate_response_status, storage_err_to_api,
};

// ---------------------------------------------------------------------------
// Upload
// ---------------------------------------------------------------------------

/// `POST /api/storage/upload` -- Upload a file.
///
/// Accepts `multipart/form-data` with a `file` field and optional `path` field.
pub async fn storage_upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Request<Body>,
) -> Result<Response, ApiError> {
    let _token = extract_bearer_token(&headers)?;

    let content_type_str = headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();

    let (file_data, file_content_type, upload_path) =
        if content_type_str.starts_with("multipart/form-data") {
            let mut multipart =
                <axum::extract::Multipart as FromRequest<()>>::from_request(request, &())
                    .await
                    .map_err(|e| ApiError::bad_request(format!("Invalid multipart data: {e}")))?;

            let mut file_data: Option<Vec<u8>> = None;
            let mut file_ct = String::from("application/octet-stream");
            let mut custom_path: Option<String> = None;

            while let Some(field) = multipart
                .next_field()
                .await
                .map_err(|e| ApiError::bad_request(format!("Failed to read field: {e}")))?
            {
                let name = field.name().unwrap_or("").to_string();
                match name.as_str() {
                    "file" => {
                        if let Some(ct) = field.content_type() {
                            file_ct = ct.to_string();
                        }
                        let bytes = field.bytes().await.map_err(|e| {
                            ApiError::bad_request(format!("Failed to read file: {e}"))
                        })?;
                        file_data = Some(bytes.to_vec());
                    }
                    "path" => {
                        let text = field.text().await.map_err(|e| {
                            ApiError::bad_request(format!("Failed to read path: {e}"))
                        })?;
                        if !text.is_empty() {
                            custom_path = Some(text);
                        }
                    }
                    _ => {}
                }
            }

            let data = file_data
                .ok_or_else(|| ApiError::bad_request("Missing 'file' field in multipart upload"))?;
            (data, file_ct, custom_path)
        } else {
            let body_bytes = axum::body::to_bytes(request.into_body(), 100 * 1024 * 1024)
                .await
                .map_err(|e| ApiError::bad_request(format!("Failed to read body: {e}")))?;
            (body_bytes.to_vec(), content_type_str.clone(), None)
        };

    if file_data.is_empty() {
        return Err(ApiError::bad_request("Upload body is empty"));
    }

    let path = upload_path.unwrap_or_else(|| format!("uploads/{}", Uuid::new_v4()));

    let result = state
        .storage_engine
        .upload(
            &path,
            &file_data,
            &file_content_type,
            std::collections::HashMap::new(),
        )
        .await
        .map_err(storage_err_to_api)?;

    let response = serde_json::json!({
        "path": result.path,
        "size": result.size,
        "content_type": result.content_type,
        "etag": result.etag,
        "signed_url": result.signed_url,
    });

    Ok(negotiate_response_status(
        &headers,
        StatusCode::CREATED,
        &response,
    ))
}

// ---------------------------------------------------------------------------
// Download
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct StorageGetParams {
    signed: Option<bool>,
    transform: Option<String>,
    expires: Option<i64>,
    sig: Option<String>,
}

/// `GET /api/storage/*path` -- Download a file or retrieve a signed URL.
pub async fn storage_get(
    State(state): State<AppState>,
    Path(path): Path<String>,
    Query(params): Query<StorageGetParams>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    if path.is_empty() {
        return Err(ApiError::bad_request("Storage path is required"));
    }

    if path.contains("..") {
        return Err(ApiError::bad_request("Path traversal is not allowed"));
    }

    if let (Some(expires), Some(sig)) = (params.expires, &params.sig) {
        state
            .storage_engine
            .verify_signed_url(&path, expires, sig)
            .map_err(storage_err_to_api)?;
    }

    if params.signed.unwrap_or(false) {
        let signed = state
            .storage_engine
            .signed_url(&path, "/api/storage")
            .map_err(storage_err_to_api)?;
        let response = serde_json::json!({
            "signed_url": signed.url,
            "expires_at": signed.expires_at.to_rfc3339(),
            "expires_in": signed.expires_in,
        });
        return Ok(negotiate_response(&headers, &response));
    }

    let (data, meta) = state
        .storage_engine
        .download(&path)
        .await
        .map_err(storage_err_to_api)?;

    let _ = params.transform; // TODO: apply image transforms when image processor is available.

    let mut response = (StatusCode::OK, data).into_response();
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_str(&meta.content_type)
            .unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    if let Ok(etag_val) = HeaderValue::from_str(&format!("\"{}\"", meta.etag)) {
        response.headers_mut().insert("etag", etag_val);
    }
    Ok(response)
}

// ---------------------------------------------------------------------------
// Delete
// ---------------------------------------------------------------------------

/// `DELETE /api/storage/*path` -- Delete a stored file.
pub async fn storage_delete(
    State(state): State<AppState>,
    Path(path): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let _token = extract_bearer_token(&headers)?;

    if path.is_empty() {
        return Err(ApiError::bad_request("Storage path is required"));
    }
    if path.contains("..") {
        return Err(ApiError::bad_request("Path traversal is not allowed"));
    }

    state
        .storage_engine
        .delete(&path)
        .await
        .map_err(storage_err_to_api)?;

    Ok(StatusCode::NO_CONTENT.into_response())
}
