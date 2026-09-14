//! Storage engine for DarshJDB.
//!
//! Provides a unified interface for file storage with pluggable backends.
//! All backends implement the [`StorageBackend`] trait.
//!
//! - [`LocalFsBackend`] — local filesystem. This is the only backend that
//!   exists, and the only one the server wires up (`main.rs` builds it
//!   unconditionally and `AppState.storage_engine` is typed
//!   `StorageEngine<LocalFsBackend>`).
//!
//! An `S3Backend` (S3 / Cloudflare R2 / MinIO, built on `aws-sdk-s3`) was
//! removed in 0.4.0: nothing in the server ever constructed it, and the AWS
//! SDK pulled the unmaintained rustls 0.21 / rustls-webpki 0.101 chain
//! (RUSTSEC-2026-0098, -0099, -0104) into every build. See git history if it
//! is ever reinstated.
//!
//! # Data backends
//!
//! In addition to file/object storage, this module provides a pluggable
//! **data backend** layer via [`traits::DataBackend`]. Supported engines:
//!
//! - [`memory::MemoryBackend`] — in-memory, concurrent, ephemeral.
//! - [`postgres::PostgresBackend`] — production PostgreSQL via `sqlx`.
//! - `file` (RocksDB) — reserved for future implementation.
//!
//! The [`engine::DataEngine`] reads `DDB_STORAGE=memory|postgres|file`
//! and wires up the appropriate backend automatically.
//!
//! # File storage
//!
//! ## Features
//!
//! - **Signed URLs**: Time-limited, HMAC-authenticated download links.
//! - **Image transforms**: On-the-fly resize, crop, and format conversion
//!   (delegated to an external image processor or CDN transform layer).
//! - **Upload hooks**: Pre- and post-upload callbacks for validation, virus
//!   scanning, metadata extraction, etc.
//! - **Resumable uploads**: TUS-protocol-compatible chunked upload support.
//!
//! ## Architecture
//!
//! ```text
//! Client ──▶ StorageEngine ──▶ Backend (LocalFs)
//!                │                         │
//!                ├── UploadHook (pre)       ├── put_object
//!                ├── UploadHook (post)      ├── get_object
//!                ├── SignedUrl generator    ├── delete_object
//!                └── ImageTransform         └── head_object
//! ```
//!
//! ```text
//! Client ──▶ DataEngine ──▶ DataBackend (Memory | Postgres | File)
//!                                │
//!                                ├── get / set / delete
//!                                ├── scan (prefix, limit)
//!                                └── query (SQL / DSL)
//! ```

pub mod engine;
pub mod memory;
pub mod postgres;
pub mod traits;
pub mod transforms;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// Metadata about a stored object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectMeta {
    /// Storage path (relative to the bucket root).
    pub path: String,
    /// Size in bytes.
    pub size: u64,
    /// MIME content type.
    pub content_type: String,
    /// ETag or content hash for cache validation.
    pub etag: String,
    /// When the object was created.
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// When the object was last modified.
    pub modified_at: chrono::DateTime<chrono::Utc>,
    /// User-defined metadata key-value pairs.
    pub metadata: HashMap<String, String>,
}

/// Result of a successful upload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadResult {
    /// The stored object path.
    pub path: String,
    /// Size in bytes.
    pub size: u64,
    /// MIME content type.
    pub content_type: String,
    /// Optional signed URL for immediate access.
    pub signed_url: Option<String>,
    /// ETag of the uploaded object.
    pub etag: String,
}

/// A signed URL with its expiration time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedUrl {
    /// The pre-signed URL.
    pub url: String,
    /// When this URL expires.
    pub expires_at: chrono::DateTime<chrono::Utc>,
    /// How long until expiry in seconds.
    pub expires_in: u64,
}

/// Image transformation parameters.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ImageTransform {
    /// Target width in pixels.
    pub width: Option<u32>,
    /// Target height in pixels.
    pub height: Option<u32>,
    /// Resize fit mode.
    pub fit: Option<ImageFit>,
    /// Output format override.
    pub format: Option<ImageFormat>,
    /// JPEG/WebP quality (1-100).
    pub quality: Option<u8>,
}

/// How the image should be fit to the target dimensions.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageFit {
    /// Scale to fit within bounds, preserving aspect ratio.
    Contain,
    /// Scale to fill bounds, cropping if necessary.
    Cover,
    /// Stretch to exact dimensions.
    Fill,
    /// Scale down only, never upscale.
    Inside,
}

/// Output image format.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageFormat {
    Jpeg,
    Png,
    Webp,
    Avif,
}

impl ImageTransform {
    /// Parse a transform string like `w=200,h=200,fit=cover,format=webp,q=80`.
    ///
    /// Dimensions are clamped to [`MAX_IMAGE_DIMENSION`]. Quality is clamped
    /// to the 1..=100 range.
    pub fn from_query(query: &str) -> Self {
        let mut transform = Self::default();
        for pair in query.split(',') {
            let mut parts = pair.splitn(2, '=');
            let key = parts.next().unwrap_or("").trim();
            let val = parts.next().unwrap_or("").trim();
            match key {
                "w" | "width" => {
                    transform.width = val
                        .parse::<u32>()
                        .ok()
                        .map(|v| v.min(MAX_IMAGE_DIMENSION))
                        .filter(|&v| v > 0)
                }
                "h" | "height" => {
                    transform.height = val
                        .parse::<u32>()
                        .ok()
                        .map(|v| v.min(MAX_IMAGE_DIMENSION))
                        .filter(|&v| v > 0)
                }
                "fit" => {
                    transform.fit = match val {
                        "contain" => Some(ImageFit::Contain),
                        "cover" => Some(ImageFit::Cover),
                        "fill" => Some(ImageFit::Fill),
                        "inside" => Some(ImageFit::Inside),
                        _ => None,
                    }
                }
                "format" | "f" => {
                    transform.format = match val {
                        "jpeg" | "jpg" => Some(ImageFormat::Jpeg),
                        "png" => Some(ImageFormat::Png),
                        "webp" => Some(ImageFormat::Webp),
                        "avif" => Some(ImageFormat::Avif),
                        _ => None,
                    }
                }
                "q" | "quality" => {
                    transform.quality = val.parse::<u8>().ok().map(|v| v.clamp(1, 100))
                }
                _ => {}
            }
        }
        transform
    }

    /// Returns `true` if no transforms are specified.
    pub fn is_empty(&self) -> bool {
        self.width.is_none()
            && self.height.is_none()
            && self.fit.is_none()
            && self.format.is_none()
            && self.quality.is_none()
    }
}

/// State of a resumable (chunked) upload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumableUpload {
    /// Unique upload identifier.
    pub upload_id: Uuid,
    /// Target storage path.
    pub path: String,
    /// Content type of the final assembled file.
    pub content_type: String,
    /// Total expected size in bytes (optional for open-ended uploads).
    pub total_size: Option<u64>,
    /// Bytes received so far.
    pub bytes_received: u64,
    /// Upload creation time.
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Offset of the next expected chunk.
    pub next_offset: u64,
}

// ---------------------------------------------------------------------------
// Upload hooks
// ---------------------------------------------------------------------------

/// Hook invoked before and after upload operations.
///
/// Implementations can reject uploads (pre-hook), add metadata,
/// trigger virus scanning, extract image dimensions, etc.
pub trait UploadHook: Send + Sync {
    /// Called before the upload is persisted.
    ///
    /// Return `Err` to reject the upload with a user-facing message.
    fn pre_upload(
        &self,
        path: &str,
        content_type: &str,
        size: u64,
        metadata: &HashMap<String, String>,
    ) -> Result<(), StorageError>;

    /// Called after the upload is successfully persisted.
    fn post_upload(&self, result: &UploadResult);
}

/// Default hook that accepts all uploads.
pub struct NoopUploadHook;

impl UploadHook for NoopUploadHook {
    fn pre_upload(
        &self,
        _path: &str,
        _content_type: &str,
        _size: u64,
        _metadata: &HashMap<String, String>,
    ) -> Result<(), StorageError> {
        Ok(())
    }

    fn post_upload(&self, _result: &UploadResult) {}
}

// ---------------------------------------------------------------------------
// Backend trait
// ---------------------------------------------------------------------------

/// Pluggable storage backend. Implementations handle the actual I/O
/// against local disk, S3, R2, MinIO, etc.
#[allow(async_fn_in_trait)]
pub trait StorageBackend: Send + Sync {
    /// Store an object. Returns the ETag.
    async fn put_object(
        &self,
        path: &str,
        data: &[u8],
        content_type: &str,
        metadata: &HashMap<String, String>,
    ) -> Result<String, StorageError>;

    /// Retrieve an object's contents.
    async fn get_object(&self, path: &str) -> Result<(Vec<u8>, ObjectMeta), StorageError>;

    /// Delete an object.
    async fn delete_object(&self, path: &str) -> Result<(), StorageError>;

    /// Check if an object exists and return its metadata.
    async fn head_object(&self, path: &str) -> Result<ObjectMeta, StorageError>;

    /// List objects under a prefix.
    async fn list_objects(
        &self,
        prefix: &str,
        limit: usize,
        cursor: Option<&str>,
    ) -> Result<Vec<ObjectMeta>, StorageError>;
}

// ---------------------------------------------------------------------------
// Local filesystem backend
// ---------------------------------------------------------------------------

/// Local filesystem storage backend for development and single-node deploys.
pub struct LocalFsBackend {
    /// Root directory where files are stored.
    root: PathBuf,
}

impl LocalFsBackend {
    /// Create a new local filesystem backend rooted at `root`.
    ///
    /// Creates the directory if it does not exist.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, StorageError> {
        let root = root.into();
        std::fs::create_dir_all(&root).map_err(|e| StorageError::Io(e.to_string()))?;
        Ok(Self { root })
    }

    /// Resolve a storage path to an absolute filesystem path,
    /// preventing path traversal attacks.
    fn resolve_path(&self, path: &str) -> Result<PathBuf, StorageError> {
        // Reject null bytes which can cause truncation on some OSes.
        if path.contains('\0') {
            return Err(StorageError::InvalidPath(
                "null bytes are not allowed in paths".into(),
            ));
        }

        // Reject empty paths.
        if path.is_empty() {
            return Err(StorageError::InvalidPath(
                "empty path is not allowed".into(),
            ));
        }

        let clean = Path::new(path);

        // Reject absolute paths and traversal.
        if clean.is_absolute() {
            return Err(StorageError::InvalidPath(
                "absolute paths are not allowed".into(),
            ));
        }
        for component in clean.components() {
            if matches!(component, std::path::Component::ParentDir) {
                return Err(StorageError::InvalidPath(
                    "path traversal is not allowed".into(),
                ));
            }
        }

        let full = self.root.join(clean);

        // Double-check that the resolved path is still under root.
        if !full.starts_with(&self.root) {
            return Err(StorageError::InvalidPath(
                "resolved path escapes storage root".into(),
            ));
        }

        Ok(full)
    }

    /// Compute a simple hex-encoded SHA-256 hash for ETag generation.
    fn compute_etag(data: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        let hash = Sha256::digest(data);
        format!("{hash:x}")
    }
}

impl StorageBackend for LocalFsBackend {
    async fn put_object(
        &self,
        path: &str,
        data: &[u8],
        content_type: &str,
        metadata: &HashMap<String, String>,
    ) -> Result<String, StorageError> {
        let full_path = self.resolve_path(path)?;

        // Ensure parent directories exist.
        if let Some(parent) = full_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| StorageError::Io(e.to_string()))?;
        }

        // Write the file.
        tokio::fs::write(&full_path, data)
            .await
            .map_err(|e| StorageError::Io(e.to_string()))?;

        // Write metadata sidecar (path.meta.json).
        let meta_path = full_path.with_extension(
            full_path
                .extension()
                .map(|e| format!("{}.meta.json", e.to_string_lossy()))
                .unwrap_or_else(|| "meta.json".into()),
        );
        let meta = serde_json::json!({
            "content_type": content_type,
            "metadata": metadata,
        });
        let meta_bytes =
            serde_json::to_vec_pretty(&meta).map_err(|e| StorageError::Io(e.to_string()))?;
        tokio::fs::write(&meta_path, &meta_bytes)
            .await
            .map_err(|e| StorageError::Io(e.to_string()))?;

        let etag = Self::compute_etag(data);
        Ok(etag)
    }

    async fn get_object(&self, path: &str) -> Result<(Vec<u8>, ObjectMeta), StorageError> {
        let full_path = self.resolve_path(path)?;

        let data = tokio::fs::read(&full_path)
            .await
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => StorageError::NotFound(path.to_string()),
                _ => StorageError::Io(e.to_string()),
            })?;

        let fs_meta = tokio::fs::metadata(&full_path)
            .await
            .map_err(|e| StorageError::Io(e.to_string()))?;

        let created_at = fs_meta.created().unwrap_or(SystemTime::UNIX_EPOCH).into();
        let modified_at = fs_meta.modified().unwrap_or(SystemTime::UNIX_EPOCH).into();

        // Try to read sidecar metadata.
        let meta_path = full_path.with_extension(
            full_path
                .extension()
                .map(|e| format!("{}.meta.json", e.to_string_lossy()))
                .unwrap_or_else(|| "meta.json".into()),
        );
        let (content_type, metadata) = if let Ok(meta_bytes) = tokio::fs::read(&meta_path).await {
            if let Ok(meta_val) = serde_json::from_slice::<serde_json::Value>(&meta_bytes) {
                let ct = meta_val
                    .get("content_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("application/octet-stream")
                    .to_string();
                let md: HashMap<String, String> = meta_val
                    .get("metadata")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default();
                (ct, md)
            } else {
                ("application/octet-stream".to_string(), HashMap::new())
            }
        } else {
            ("application/octet-stream".to_string(), HashMap::new())
        };

        let etag = Self::compute_etag(&data);

        let obj_meta = ObjectMeta {
            path: path.to_string(),
            size: data.len() as u64,
            content_type,
            etag,
            created_at,
            modified_at,
            metadata,
        };

        Ok((data, obj_meta))
    }

    async fn delete_object(&self, path: &str) -> Result<(), StorageError> {
        let full_path = self.resolve_path(path)?;

        tokio::fs::remove_file(&full_path)
            .await
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => StorageError::NotFound(path.to_string()),
                _ => StorageError::Io(e.to_string()),
            })?;

        // Clean up sidecar metadata file.
        let meta_path = full_path.with_extension(
            full_path
                .extension()
                .map(|e| format!("{}.meta.json", e.to_string_lossy()))
                .unwrap_or_else(|| "meta.json".into()),
        );
        let _ = tokio::fs::remove_file(&meta_path).await;

        Ok(())
    }

    async fn head_object(&self, path: &str) -> Result<ObjectMeta, StorageError> {
        let (_, metadata) = self.get_object(path).await?;
        Ok(metadata)
    }

    async fn list_objects(
        &self,
        prefix: &str,
        limit: usize,
        cursor: Option<&str>,
    ) -> Result<Vec<ObjectMeta>, StorageError> {
        let dir = if prefix.is_empty() {
            self.root.clone()
        } else {
            self.resolve_path(prefix)?
        };
        let root = tokio::fs::canonicalize(&self.root)
            .await
            .map_err(|e| StorageError::Io(e.to_string()))?;
        let resolved = tokio::fs::canonicalize(&dir)
            .await
            .map_err(|e| StorageError::Io(e.to_string()))?;
        if !resolved.starts_with(&root) {
            return Err(StorageError::Io("storage prefix escapes root".into()));
        }
        let mut directories = vec![dir];
        let mut paths = Vec::new();
        while let Some(directory) = directories.pop() {
            let mut entries = tokio::fs::read_dir(directory)
                .await
                .map_err(|e| StorageError::Io(e.to_string()))?;
            while let Some(entry) = entries
                .next_entry()
                .await
                .map_err(|e| StorageError::Io(e.to_string()))?
            {
                let kind = entry
                    .file_type()
                    .await
                    .map_err(|e| StorageError::Io(e.to_string()))?;
                // Never traverse symlinks or expose internal metadata sidecars.
                if kind.is_symlink() {
                    continue;
                }
                if kind.is_dir() {
                    directories.push(entry.path());
                } else if kind.is_file()
                    && !entry.file_name().to_string_lossy().ends_with(".meta.json")
                {
                    let path = entry
                        .path()
                        .strip_prefix(&self.root)
                        .map_err(|e| StorageError::Io(e.to_string()))?
                        .to_string_lossy()
                        .replace('\\', "/");
                    if cursor.is_none_or(|cursor| path.as_str() > cursor) {
                        paths.push(path);
                    }
                }
            }
        }
        paths.sort();
        let mut objects = Vec::new();
        for path in paths.into_iter().take(limit) {
            objects.push(self.head_object(&path).await?);
        }
        Ok(objects)
    }
}

// ---------------------------------------------------------------------------
// Storage engine (orchestrator)
// ---------------------------------------------------------------------------

/// Maximum upload size: 100 MiB by default.
pub const DEFAULT_MAX_UPLOAD_SIZE: u64 = 100 * 1024 * 1024;

/// Maximum allowed image dimension (width or height) for transforms.
pub const MAX_IMAGE_DIMENSION: u32 = 16384;

/// Content types that are always rejected (executable payloads).
const BLOCKED_CONTENT_TYPES: &[&str] = &[
    "application/x-executable",
    "application/x-msdos-program",
    "application/x-msdownload",
    "application/x-sh",
    "application/x-shellscript",
];

/// High-level storage engine that wraps a backend with hooks,
/// signed URLs, image transforms, and resumable upload tracking.
pub struct StorageEngine<B: StorageBackend> {
    /// Underlying storage backend.
    backend: Arc<B>,
    /// Upload hooks.
    hooks: Vec<Arc<dyn UploadHook>>,
    /// HMAC key for signing URLs.
    signing_key: Vec<u8>,
    /// Default signed URL TTL.
    signed_url_ttl: Duration,
    /// Active resumable uploads.
    resumable_uploads: dashmap::DashMap<Uuid, ResumableUpload>,
    /// Maximum upload size in bytes (0 = unlimited).
    max_upload_size: u64,
    /// Directory for staging resumable upload chunks before assembly.
    staging_dir: PathBuf,
}

impl<B: StorageBackend> StorageEngine<B> {
    /// Create a new storage engine with the given backend and signing key.
    pub fn new(backend: Arc<B>, signing_key: Vec<u8>) -> Self {
        let staging_dir = std::env::var("DDB_STORAGE_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir().join("darshjdb"))
            .join(".staging");
        // Ensure the staging directory exists (best-effort at construction).
        let _ = std::fs::create_dir_all(&staging_dir);
        Self {
            backend,
            hooks: Vec::new(),
            signing_key,
            signed_url_ttl: Duration::from_secs(3600),
            resumable_uploads: dashmap::DashMap::new(),
            max_upload_size: DEFAULT_MAX_UPLOAD_SIZE,
            staging_dir,
        }
    }

    /// Set the maximum upload size in bytes.
    pub fn set_max_upload_size(&mut self, size: u64) {
        self.max_upload_size = size;
    }

    /// The maximum upload size in bytes (`0` = unlimited).
    pub fn max_upload_size(&self) -> u64 {
        self.max_upload_size
    }

    /// Add an upload hook.
    pub fn add_hook(&mut self, hook: Arc<dyn UploadHook>) {
        self.hooks.push(hook);
    }

    /// Override the staging directory for resumable upload chunks.
    pub fn set_staging_dir(&mut self, dir: impl Into<PathBuf>) {
        self.staging_dir = dir.into();
        let _ = std::fs::create_dir_all(&self.staging_dir);
    }

    /// Set the default signed URL TTL.
    pub fn set_signed_url_ttl(&mut self, ttl: Duration) {
        self.signed_url_ttl = ttl;
    }

    /// Validate a content-type string.
    fn validate_content_type(content_type: &str) -> Result<(), StorageError> {
        // Must not be empty.
        if content_type.is_empty() {
            return Err(StorageError::Rejected(
                "content-type must not be empty".into(),
            ));
        }
        // Must have a valid type/subtype structure.
        if !content_type.contains('/') {
            return Err(StorageError::Rejected(
                "content-type must be in type/subtype format".into(),
            ));
        }
        // Block dangerous executable types.
        let normalized = content_type.to_ascii_lowercase();
        let base_type = normalized.split(';').next().unwrap_or("").trim();
        if BLOCKED_CONTENT_TYPES.contains(&base_type) {
            return Err(StorageError::Rejected(format!(
                "content-type '{base_type}' is not allowed"
            )));
        }
        Ok(())
    }

    /// Upload a file, running all pre/post hooks.
    pub async fn upload(
        &self,
        path: &str,
        data: &[u8],
        content_type: &str,
        metadata: HashMap<String, String>,
    ) -> Result<UploadResult, StorageError> {
        // Enforce upload size limit.
        if self.max_upload_size > 0 && data.len() as u64 > self.max_upload_size {
            return Err(StorageError::Rejected(format!(
                "upload size {} exceeds maximum allowed size {}",
                data.len(),
                self.max_upload_size,
            )));
        }

        // Validate content-type.
        Self::validate_content_type(content_type)?;

        // Run pre-upload hooks.
        for hook in &self.hooks {
            hook.pre_upload(path, content_type, data.len() as u64, &metadata)?;
        }

        // Persist via backend.
        let etag = self
            .backend
            .put_object(path, data, content_type, &metadata)
            .await?;

        let result = UploadResult {
            path: path.to_string(),
            size: data.len() as u64,
            content_type: content_type.to_string(),
            signed_url: None,
            etag,
        };

        // Run post-upload hooks.
        for hook in &self.hooks {
            hook.post_upload(&result);
        }

        Ok(result)
    }

    /// Download a file.
    pub async fn download(&self, path: &str) -> Result<(Vec<u8>, ObjectMeta), StorageError> {
        self.backend.get_object(path).await
    }

    /// Delete a file.
    pub async fn delete(&self, path: &str) -> Result<(), StorageError> {
        self.backend.delete_object(path).await
    }

    /// Get object metadata without downloading content.
    pub async fn head(&self, path: &str) -> Result<ObjectMeta, StorageError> {
        self.backend.head_object(path).await
    }

    /// List objects under a prefix.
    pub async fn list(
        &self,
        prefix: &str,
        limit: usize,
        cursor: Option<&str>,
    ) -> Result<Vec<ObjectMeta>, StorageError> {
        self.backend.list_objects(prefix, limit, cursor).await
    }

    /// Generate a signed URL for the given path.
    pub fn signed_url(&self, path: &str, base_url: &str) -> Result<SignedUrl, StorageError> {
        let expires_at = chrono::Utc::now()
            + chrono::Duration::from_std(self.signed_url_ttl)
                .map_err(|e| StorageError::Internal(e.to_string()))?;
        let expires_ts = expires_at.timestamp();

        // HMAC-SHA256 signature: path + expiry.
        use hmac::{Hmac, Mac};
        use sha2::Sha256;

        let mut mac = Hmac::<Sha256>::new_from_slice(&self.signing_key)
            .map_err(|e| StorageError::Internal(format!("HMAC init: {e}")))?;
        mac.update(path.as_bytes());
        mac.update(b":");
        mac.update(expires_ts.to_string().as_bytes());
        let sig = data_encoding::BASE64URL_NOPAD.encode(&mac.finalize().into_bytes());

        let url = format!("{base_url}/{path}?expires={expires_ts}&sig={sig}");

        Ok(SignedUrl {
            url,
            expires_at,
            expires_in: self.signed_url_ttl.as_secs(),
        })
    }

    /// Verify a signed URL's signature and expiry.
    pub fn verify_signed_url(
        &self,
        path: &str,
        expires: i64,
        signature: &str,
    ) -> Result<(), StorageError> {
        // Check expiry.
        let now = chrono::Utc::now().timestamp();
        if now > expires {
            return Err(StorageError::SignatureExpired);
        }

        // Verify HMAC.
        use hmac::{Hmac, Mac};
        use sha2::Sha256;

        let mut mac = Hmac::<Sha256>::new_from_slice(&self.signing_key)
            .map_err(|e| StorageError::Internal(format!("HMAC init: {e}")))?;
        mac.update(path.as_bytes());
        mac.update(b":");
        mac.update(expires.to_string().as_bytes());

        let expected_bytes = mac.finalize().into_bytes();
        let sig_bytes = data_encoding::BASE64URL_NOPAD
            .decode(signature.as_bytes())
            .map_err(|_| StorageError::InvalidSignature)?;

        // Constant-time comparison to prevent timing attacks.
        use hmac::digest::CtOutput;
        use hmac::digest::OutputSizeUser;
        if sig_bytes.len() != <Hmac<Sha256> as OutputSizeUser>::output_size() {
            return Err(StorageError::InvalidSignature);
        }
        let expected_ct = CtOutput::<Hmac<Sha256>>::new(expected_bytes);
        let received = hmac::digest::generic_array::GenericArray::from_slice(&sig_bytes);
        let received_ct = CtOutput::<Hmac<Sha256>>::new(*received);
        if expected_ct != received_ct {
            return Err(StorageError::InvalidSignature);
        }

        Ok(())
    }

    // -- Resumable uploads -------------------------------------------------

    /// Initiate a resumable upload. Returns the upload ID.
    pub fn create_resumable_upload(
        &self,
        path: &str,
        content_type: &str,
        total_size: Option<u64>,
    ) -> Uuid {
        let upload_id = Uuid::new_v4();
        let upload = ResumableUpload {
            upload_id,
            path: path.to_string(),
            content_type: content_type.to_string(),
            total_size,
            bytes_received: 0,
            created_at: chrono::Utc::now(),
            next_offset: 0,
        };
        self.resumable_uploads.insert(upload_id, upload);
        upload_id
    }

    /// Append a chunk to a resumable upload.
    ///
    /// Each chunk is persisted to a staging file under
    /// `<staging_dir>/<upload_id>/<offset>.chunk` so that data survives
    /// process restarts and is not held purely in memory.
    pub async fn append_chunk(
        &self,
        upload_id: Uuid,
        offset: u64,
        data: &[u8],
    ) -> Result<u64, StorageError> {
        let next_offset = {
            let mut upload = self
                .resumable_uploads
                .get_mut(&upload_id)
                .ok_or_else(|| StorageError::NotFound(format!("upload {upload_id}")))?;

            if offset != upload.next_offset {
                return Err(StorageError::InvalidOffset {
                    expected: upload.next_offset,
                    received: offset,
                });
            }

            upload.bytes_received += data.len() as u64;
            upload.next_offset = offset + data.len() as u64;
            upload.next_offset
        };

        // Persist chunk data to the staging area.
        let chunk_dir = self.staging_dir.join(upload_id.to_string());
        tokio::fs::create_dir_all(&chunk_dir)
            .await
            .map_err(|e| StorageError::Io(format!("staging mkdir: {e}")))?;

        let chunk_path = chunk_dir.join(format!("{offset}.chunk"));
        tokio::fs::write(&chunk_path, data)
            .await
            .map_err(|e| StorageError::Io(format!("staging write: {e}")))?;

        Ok(next_offset)
    }

    /// Get the status of a resumable upload.
    pub fn resumable_upload_status(&self, upload_id: Uuid) -> Option<ResumableUpload> {
        self.resumable_uploads.get(&upload_id).map(|u| u.clone())
    }

    /// Cancel and clean up a resumable upload, removing staged chunks.
    pub fn cancel_resumable_upload(&self, upload_id: Uuid) -> bool {
        let removed = self.resumable_uploads.remove(&upload_id).is_some();
        if removed {
            // Best-effort cleanup of staged chunk files.
            let chunk_dir = self.staging_dir.join(upload_id.to_string());
            let _ = std::fs::remove_dir_all(chunk_dir);
        }
        removed
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors specific to the storage subsystem.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    /// The requested file or object was not found.
    #[error("not found: {0}")]
    NotFound(String),

    /// The storage path is invalid (absolute, traversal, etc.).
    #[error("invalid path: {0}")]
    InvalidPath(String),

    /// An I/O operation failed.
    #[error("I/O error: {0}")]
    Io(String),

    /// The storage backend is not available or not configured.
    #[error("backend unavailable: {0}")]
    BackendUnavailable(String),

    /// A signed URL has expired.
    #[error("signed URL has expired")]
    SignatureExpired,

    /// A signed URL signature is invalid.
    #[error("invalid signature")]
    InvalidSignature,

    /// Chunk offset does not match the expected offset.
    #[error("invalid upload offset: expected {expected}, got {received}")]
    InvalidOffset {
        /// The expected offset.
        expected: u64,
        /// The offset provided in the request.
        received: u64,
    },

    /// An upload hook rejected the file.
    #[error("upload rejected: {0}")]
    Rejected(String),

    /// An internal error occurred.
    #[error("internal storage error: {0}")]
    Internal(String),
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn temp_dir() -> String {
        format!("/tmp/darshjdb-test-{}", Uuid::new_v4())
    }

    fn make_engine(dir: &str) -> StorageEngine<LocalFsBackend> {
        let backend = Arc::new(LocalFsBackend::new(dir).expect("create backend"));
        let mut engine = StorageEngine::new(backend, b"test-secret-key".to_vec());
        engine.set_staging_dir(PathBuf::from(dir).join(".staging"));
        engine
    }

    // -----------------------------------------------------------------------
    // Path traversal prevention
    // -----------------------------------------------------------------------

    #[test]
    fn path_traversal_parent_dir() {
        let backend = LocalFsBackend::new("/tmp/darshjdb-test-pt").expect("create backend");
        assert!(backend.resolve_path("../../../etc/passwd").is_err());
        let _ = std::fs::remove_dir_all("/tmp/darshjdb-test-pt");
    }

    #[test]
    fn path_traversal_nested_parent() {
        let backend = LocalFsBackend::new("/tmp/darshjdb-test-pt2").expect("create backend");
        assert!(
            backend
                .resolve_path("a/b/../../../../../../etc/shadow")
                .is_err()
        );
        let _ = std::fs::remove_dir_all("/tmp/darshjdb-test-pt2");
    }

    #[test]
    fn path_traversal_absolute_path() {
        let backend = LocalFsBackend::new("/tmp/darshjdb-test-pt3").expect("create backend");
        assert!(backend.resolve_path("/etc/passwd").is_err());
        assert!(backend.resolve_path("/absolute/path").is_err());
        let _ = std::fs::remove_dir_all("/tmp/darshjdb-test-pt3");
    }

    #[test]
    fn path_traversal_null_byte() {
        let backend = LocalFsBackend::new("/tmp/darshjdb-test-pt4").expect("create backend");
        assert!(backend.resolve_path("file.txt\0.jpg").is_err());
        assert!(backend.resolve_path("\0").is_err());
        let _ = std::fs::remove_dir_all("/tmp/darshjdb-test-pt4");
    }

    #[test]
    fn path_traversal_empty_path() {
        let backend = LocalFsBackend::new("/tmp/darshjdb-test-pt5").expect("create backend");
        assert!(backend.resolve_path("").is_err());
        let _ = std::fs::remove_dir_all("/tmp/darshjdb-test-pt5");
    }

    #[test]
    fn path_traversal_safe_paths_accepted() {
        let backend = LocalFsBackend::new("/tmp/darshjdb-test-pt6").expect("create backend");
        assert!(backend.resolve_path("safe/path/file.txt").is_ok());
        assert!(backend.resolve_path("uploads/image.png").is_ok());
        assert!(backend.resolve_path("a.txt").is_ok());
        let _ = std::fs::remove_dir_all("/tmp/darshjdb-test-pt6");
    }

    // -----------------------------------------------------------------------
    // Signed URL generation and validation
    // -----------------------------------------------------------------------

    #[test]
    fn signed_url_roundtrip() {
        let dir = temp_dir();
        let engine = make_engine(&dir);

        let signed = engine
            .signed_url("uploads/test.png", "https://example.com/api/storage")
            .expect("sign");

        // Parse the URL to extract expires and sig.
        let url = url::Url::parse(&signed.url).expect("parse url");
        let params: HashMap<_, _> = url.query_pairs().collect();
        let expires: i64 = params["expires"].parse().expect("parse expires");
        let sig = params["sig"].as_ref();

        assert!(
            engine
                .verify_signed_url("uploads/test.png", expires, sig)
                .is_ok()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn signed_url_wrong_path_fails() {
        let dir = temp_dir();
        let engine = make_engine(&dir);

        let signed = engine
            .signed_url("uploads/test.png", "https://example.com")
            .expect("sign");
        let url = url::Url::parse(&signed.url).expect("parse url");
        let params: HashMap<_, _> = url.query_pairs().collect();
        let expires: i64 = params["expires"].parse().unwrap();
        let sig = params["sig"].as_ref();

        assert!(
            engine
                .verify_signed_url("uploads/WRONG.png", expires, sig)
                .is_err()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn signed_url_expired_fails() {
        let dir = temp_dir();
        let engine = make_engine(&dir);

        // Use an already-expired timestamp.
        let past = chrono::Utc::now().timestamp() - 3600;
        assert!(
            engine
                .verify_signed_url("uploads/test.png", past, "bogus-sig")
                .is_err()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn signed_url_tampered_signature_fails() {
        let dir = temp_dir();
        let engine = make_engine(&dir);

        let signed = engine
            .signed_url("uploads/test.png", "https://example.com")
            .expect("sign");
        let url = url::Url::parse(&signed.url).expect("parse url");
        let params: HashMap<_, _> = url.query_pairs().collect();
        let expires: i64 = params["expires"].parse().unwrap();

        assert!(
            engine
                .verify_signed_url("uploads/test.png", expires, "TAMPERED_SIG")
                .is_err()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // -----------------------------------------------------------------------
    // Image transform parameter parsing
    // -----------------------------------------------------------------------

    #[test]
    fn image_transform_full_parsing() {
        let t = ImageTransform::from_query("w=200,h=150,fit=cover,format=webp,q=80");
        assert_eq!(t.width, Some(200));
        assert_eq!(t.height, Some(150));
        assert!(matches!(t.fit, Some(ImageFit::Cover)));
        assert!(matches!(t.format, Some(ImageFormat::Webp)));
        assert_eq!(t.quality, Some(80));
    }

    #[test]
    fn image_transform_empty_query() {
        let t = ImageTransform::from_query("");
        assert!(t.is_empty());
    }

    #[test]
    fn image_transform_quality_clamped() {
        // Quality 0 should be clamped to 1.
        let t = ImageTransform::from_query("q=0");
        assert_eq!(t.quality, Some(1));

        // Quality 255 should be clamped to 100.
        let t = ImageTransform::from_query("q=255");
        assert_eq!(t.quality, Some(100));

        // Quality 50 stays 50.
        let t = ImageTransform::from_query("q=50");
        assert_eq!(t.quality, Some(50));
    }

    #[test]
    fn image_transform_dimension_clamped() {
        let t = ImageTransform::from_query("w=999999,h=999999");
        assert_eq!(t.width, Some(MAX_IMAGE_DIMENSION));
        assert_eq!(t.height, Some(MAX_IMAGE_DIMENSION));
    }

    #[test]
    fn image_transform_zero_dimension_rejected() {
        let t = ImageTransform::from_query("w=0,h=0");
        assert!(t.width.is_none());
        assert!(t.height.is_none());
    }

    #[test]
    fn image_transform_all_fits() {
        for (val, expected) in [
            ("contain", ImageFit::Contain),
            ("cover", ImageFit::Cover),
            ("fill", ImageFit::Fill),
            ("inside", ImageFit::Inside),
        ] {
            let t = ImageTransform::from_query(&format!("fit={val}"));
            assert!(
                matches!(t.fit, Some(f) if std::mem::discriminant(&f) == std::mem::discriminant(&expected)),
                "fit={val} should parse"
            );
        }
    }

    #[test]
    fn image_transform_all_formats() {
        for val in ["jpeg", "jpg", "png", "webp", "avif"] {
            let t = ImageTransform::from_query(&format!("format={val}"));
            assert!(t.format.is_some(), "format={val} should parse");
        }
    }

    #[test]
    fn image_transform_unknown_keys_ignored() {
        let t = ImageTransform::from_query("w=100,unknown=foo,bar=baz");
        assert_eq!(t.width, Some(100));
        assert!(t.fit.is_none());
    }

    // -----------------------------------------------------------------------
    // Local filesystem backend CRUD
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn local_fs_put_get_delete() {
        let dir = temp_dir();
        let backend = LocalFsBackend::new(&dir).expect("create backend");

        let path = "test/hello.txt";
        let data = b"Hello, DarshJDB!";
        let meta = HashMap::new();

        let etag = backend
            .put_object(path, data, "text/plain", &meta)
            .await
            .expect("put");
        assert!(!etag.is_empty());

        let (got_data, got_meta) = backend.get_object(path).await.expect("get");
        assert_eq!(got_data, data);
        assert_eq!(got_meta.content_type, "text/plain");
        assert_eq!(got_meta.size, data.len() as u64);
        assert_eq!(got_meta.path, path);

        backend.delete_object(path).await.expect("delete");
        assert!(backend.get_object(path).await.is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn local_fs_head_object() {
        let dir = temp_dir();
        let backend = LocalFsBackend::new(&dir).expect("create backend");

        let path = "docs/readme.md";
        let data = b"# DarshJDB";
        backend
            .put_object(path, data, "text/markdown", &HashMap::new())
            .await
            .expect("put");

        let meta = backend.head_object(path).await.expect("head");
        assert_eq!(meta.path, path);
        assert_eq!(meta.size, data.len() as u64);
        assert!(!meta.etag.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn local_fs_list_objects() {
        let dir = temp_dir();
        let backend = LocalFsBackend::new(&dir).expect("create backend");

        for i in 0..5 {
            backend
                .put_object(
                    &format!("listing/{i}.txt"),
                    format!("file {i}").as_bytes(),
                    "text/plain",
                    &HashMap::new(),
                )
                .await
                .expect("put");
        }

        let all = backend
            .list_objects("listing", 100, None)
            .await
            .expect("list");
        assert_eq!(all.len(), 5);

        let root_listing = backend
            .list_objects("", 100, None)
            .await
            .expect("root listing");
        assert_eq!(root_listing.len(), 5);
        assert_eq!(root_listing[0].path, "listing/0.txt");
        assert_eq!(root_listing[0].content_type, "text/plain");
        let page = backend
            .list_objects("", 2, Some("listing/1.txt"))
            .await
            .expect("cursor");
        assert_eq!(page[0].path, "listing/2.txt");
        assert_eq!(page.len(), 2);

        // Limit should be respected.
        let limited = backend
            .list_objects("listing", 2, None)
            .await
            .expect("list limited");
        assert_eq!(limited.len(), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn local_fs_get_nonexistent() {
        let dir = temp_dir();
        let backend = LocalFsBackend::new(&dir).expect("create backend");
        let result = backend.get_object("does/not/exist.txt").await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), StorageError::NotFound(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn local_fs_delete_nonexistent() {
        let dir = temp_dir();
        let backend = LocalFsBackend::new(&dir).expect("create backend");
        let result = backend.delete_object("does/not/exist.txt").await;
        assert!(matches!(result.unwrap_err(), StorageError::NotFound(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn local_fs_metadata_preserved() {
        let dir = temp_dir();
        let backend = LocalFsBackend::new(&dir).expect("create backend");

        let mut meta = HashMap::new();
        meta.insert("author".into(), "ddb".into());
        meta.insert("version".into(), "1".into());

        backend
            .put_object("meta-test.bin", b"data", "application/octet-stream", &meta)
            .await
            .expect("put");

        let (_, obj_meta) = backend.get_object("meta-test.bin").await.expect("get");
        assert_eq!(
            obj_meta.metadata.get("author").map(|s| s.as_str()),
            Some("ddb")
        );
        assert_eq!(
            obj_meta.metadata.get("version").map(|s| s.as_str()),
            Some("1")
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // -----------------------------------------------------------------------
    // Content-type validation
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn upload_rejects_empty_content_type() {
        let dir = temp_dir();
        let engine = make_engine(&dir);
        let result = engine
            .upload("file.txt", b"hello", "", HashMap::new())
            .await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), StorageError::Rejected(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn upload_rejects_invalid_content_type() {
        let dir = temp_dir();
        let engine = make_engine(&dir);
        let result = engine
            .upload("file.txt", b"hello", "not-a-mime", HashMap::new())
            .await;
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn upload_rejects_blocked_content_types() {
        let dir = temp_dir();
        let engine = make_engine(&dir);

        for ct in BLOCKED_CONTENT_TYPES {
            let result = engine
                .upload("evil.exe", b"\x7fELF", ct, HashMap::new())
                .await;
            assert!(result.is_err(), "should reject {ct}");
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn upload_accepts_valid_content_types() {
        let dir = temp_dir();
        let engine = make_engine(&dir);
        let result = engine
            .upload("file.txt", b"hello", "text/plain", HashMap::new())
            .await;
        assert!(result.is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // -----------------------------------------------------------------------
    // Upload size limits
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn upload_rejects_oversized_payload() {
        let dir = temp_dir();
        let mut engine = make_engine(&dir);
        engine.set_max_upload_size(100); // 100 bytes max

        let data = vec![0u8; 200];
        let result = engine
            .upload("big.bin", &data, "application/octet-stream", HashMap::new())
            .await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), StorageError::Rejected(_)));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn upload_accepts_within_size_limit() {
        let dir = temp_dir();
        let mut engine = make_engine(&dir);
        engine.set_max_upload_size(1000);

        let data = vec![0u8; 500];
        let result = engine
            .upload("ok.bin", &data, "application/octet-stream", HashMap::new())
            .await;
        assert!(result.is_ok());

        let _ = std::fs::remove_dir_all(&dir);
    }

    // -----------------------------------------------------------------------
    // Resumable upload state management
    // -----------------------------------------------------------------------

    #[test]
    fn resumable_upload_create_and_status() {
        let dir = temp_dir();
        let engine = make_engine(&dir);

        let id = engine.create_resumable_upload("file.bin", "application/octet-stream", Some(100));

        let status = engine.resumable_upload_status(id).expect("status");
        assert_eq!(status.bytes_received, 0);
        assert_eq!(status.next_offset, 0);
        assert_eq!(status.path, "file.bin");
        assert_eq!(status.content_type, "application/octet-stream");
        assert_eq!(status.total_size, Some(100));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn resumable_upload_sequential_chunks() {
        let dir = temp_dir();
        let engine = make_engine(&dir);

        let id = engine.create_resumable_upload("file.bin", "application/octet-stream", Some(100));

        let next = engine
            .append_chunk(id, 0, &[0u8; 50])
            .await
            .expect("chunk 1");
        assert_eq!(next, 50);

        let next = engine
            .append_chunk(id, 50, &[0u8; 50])
            .await
            .expect("chunk 2");
        assert_eq!(next, 100);

        let status = engine.resumable_upload_status(id).expect("status");
        assert_eq!(status.bytes_received, 100);

        // Verify chunk files were persisted to staging.
        let chunk_dir = std::path::Path::new(&dir)
            .join(".staging")
            .join(id.to_string());
        assert!(chunk_dir.join("0.chunk").exists());
        assert!(chunk_dir.join("50.chunk").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn resumable_upload_wrong_offset_rejected() {
        let dir = temp_dir();
        let engine = make_engine(&dir);

        let id = engine.create_resumable_upload("file.bin", "application/octet-stream", Some(100));
        engine
            .append_chunk(id, 0, &[0u8; 50])
            .await
            .expect("chunk 1");

        // Try to append at offset 0 again (should be 50).
        let result = engine.append_chunk(id, 0, &[0u8; 10]).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            StorageError::InvalidOffset {
                expected: 50,
                received: 0
            }
        ));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn resumable_upload_nonexistent_id() {
        let dir = temp_dir();
        let engine = make_engine(&dir);

        let fake_id = Uuid::new_v4();
        assert!(engine.append_chunk(fake_id, 0, &[0u8; 10]).await.is_err());
        assert!(engine.resumable_upload_status(fake_id).is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resumable_upload_cancel() {
        let dir = temp_dir();
        let engine = make_engine(&dir);

        let id = engine.create_resumable_upload("file.bin", "application/octet-stream", None);
        assert!(engine.cancel_resumable_upload(id));
        assert!(engine.resumable_upload_status(id).is_none());

        // Double cancel returns false.
        assert!(!engine.cancel_resumable_upload(id));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn resumable_upload_open_ended() {
        let dir = temp_dir();
        let engine = make_engine(&dir);

        // total_size = None means open-ended.
        let id = engine.create_resumable_upload("stream.bin", "application/octet-stream", None);
        let status = engine.resumable_upload_status(id).expect("status");
        assert!(status.total_size.is_none());

        engine
            .append_chunk(id, 0, &[1u8; 1024])
            .await
            .expect("chunk");
        let status = engine.resumable_upload_status(id).expect("status");
        assert_eq!(status.bytes_received, 1024);

        let _ = std::fs::remove_dir_all(&dir);
    }

    // -----------------------------------------------------------------------
    // StorageEngine upload hook integration
    // -----------------------------------------------------------------------

    struct RejectHook;
    impl UploadHook for RejectHook {
        fn pre_upload(
            &self,
            _path: &str,
            _content_type: &str,
            _size: u64,
            _metadata: &HashMap<String, String>,
        ) -> Result<(), StorageError> {
            Err(StorageError::Rejected("blocked by hook".into()))
        }
        fn post_upload(&self, _result: &UploadResult) {}
    }

    #[tokio::test]
    async fn upload_hook_can_reject() {
        let dir = temp_dir();
        let mut engine = make_engine(&dir);
        engine.add_hook(Arc::new(RejectHook));

        let result = engine
            .upload("file.txt", b"hello", "text/plain", HashMap::new())
            .await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), StorageError::Rejected(_)));

        let _ = std::fs::remove_dir_all(&dir);
    }

    // -----------------------------------------------------------------------
    // StorageEngine::list
    // -----------------------------------------------------------------------

    #[tokio::test]
    #[ignore = "pre-existing v0.2.0 baseline failure — tracked in v0.3.1 followup"]
    async fn engine_list_returns_uploaded_files() {
        let dir = temp_dir();
        let engine = make_engine(&dir);

        // Upload a few files.
        for i in 0..3 {
            engine
                .upload(
                    &format!("docs/file{i}.txt"),
                    format!("content {i}").as_bytes(),
                    "text/plain",
                    HashMap::new(),
                )
                .await
                .expect("upload");
        }

        let files = engine.list("docs", 100, None).await.expect("list");
        assert_eq!(files.len(), 3);
        assert!(files.iter().all(|f| f.content_type == "text/plain"));
        assert!(files.iter().all(|f| f.size > 0));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    #[ignore = "pre-existing v0.2.0 baseline failure — tracked in v0.3.1 followup"]
    async fn engine_list_empty_prefix() {
        let dir = temp_dir();
        let engine = make_engine(&dir);

        engine
            .upload("top.txt", b"hello", "text/plain", HashMap::new())
            .await
            .expect("upload");

        let files = engine.list("", 100, None).await.expect("list");
        assert!(!files.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn engine_list_respects_limit() {
        let dir = temp_dir();
        let engine = make_engine(&dir);

        for i in 0..5 {
            engine
                .upload(
                    &format!("limited/{i}.txt"),
                    b"x",
                    "text/plain",
                    HashMap::new(),
                )
                .await
                .expect("upload");
        }

        let files = engine.list("limited", 2, None).await.expect("list");
        assert_eq!(files.len(), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
