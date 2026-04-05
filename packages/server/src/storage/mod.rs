//! Storage engine for DarshanDB.
//!
//! Provides a unified interface for file storage with pluggable backends:
//! local filesystem, Amazon S3, Cloudflare R2, and MinIO. All backends
//! implement the [`StorageBackend`] trait.
//!
//! # Features
//!
//! - **Signed URLs**: Time-limited, HMAC-authenticated download links.
//! - **Image transforms**: On-the-fly resize, crop, and format conversion
//!   (delegated to an external image processor or CDN transform layer).
//! - **Upload hooks**: Pre- and post-upload callbacks for validation, virus
//!   scanning, metadata extraction, etc.
//! - **Resumable uploads**: TUS-protocol-compatible chunked upload support.
//!
//! # Architecture
//!
//! ```text
//! Client ──▶ StorageEngine ──▶ Backend (LocalFs | S3 | R2 | MinIO)
//!                │                         │
//!                ├── UploadHook (pre)       ├── put_object
//!                ├── UploadHook (post)      ├── get_object
//!                ├── SignedUrl generator    ├── delete_object
//!                └── ImageTransform         └── head_object
//! ```

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
    pub fn from_query(query: &str) -> Self {
        let mut transform = Self::default();
        for pair in query.split(',') {
            let mut parts = pair.splitn(2, '=');
            let key = parts.next().unwrap_or("").trim();
            let val = parts.next().unwrap_or("").trim();
            match key {
                "w" | "width" => transform.width = val.parse().ok(),
                "h" | "height" => transform.height = val.parse().ok(),
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
                "q" | "quality" => transform.quality = val.parse().ok(),
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
        let full_path = self.resolve_path(path)?;

        let fs_meta = tokio::fs::metadata(&full_path)
            .await
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => StorageError::NotFound(path.to_string()),
                _ => StorageError::Io(e.to_string()),
            })?;

        let data = tokio::fs::read(&full_path)
            .await
            .map_err(|e| StorageError::Io(e.to_string()))?;

        let created_at = fs_meta.created().unwrap_or(SystemTime::UNIX_EPOCH).into();
        let modified_at = fs_meta.modified().unwrap_or(SystemTime::UNIX_EPOCH).into();

        let etag = Self::compute_etag(&data);

        Ok(ObjectMeta {
            path: path.to_string(),
            size: fs_meta.len(),
            content_type: "application/octet-stream".to_string(),
            etag,
            created_at,
            modified_at,
            metadata: HashMap::new(),
        })
    }

    async fn list_objects(
        &self,
        prefix: &str,
        limit: usize,
        _cursor: Option<&str>,
    ) -> Result<Vec<ObjectMeta>, StorageError> {
        let dir = self.resolve_path(prefix)?;
        let mut entries = Vec::new();

        let mut read_dir = tokio::fs::read_dir(&dir)
            .await
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => StorageError::NotFound(prefix.to_string()),
                _ => StorageError::Io(e.to_string()),
            })?;

        while let Some(entry) = read_dir
            .next_entry()
            .await
            .map_err(|e| StorageError::Io(e.to_string()))?
        {
            if entries.len() >= limit {
                break;
            }

            let file_name = entry.file_name().to_string_lossy().to_string();

            // Skip metadata sidecar files.
            if file_name.ends_with(".meta.json") {
                continue;
            }

            let fs_meta = entry
                .metadata()
                .await
                .map_err(|e| StorageError::Io(e.to_string()))?;

            if fs_meta.is_file() {
                let obj_path = format!(
                    "{}{}{}",
                    prefix,
                    if prefix.ends_with('/') { "" } else { "/" },
                    file_name
                );

                let created_at = fs_meta.created().unwrap_or(SystemTime::UNIX_EPOCH).into();
                let modified_at = fs_meta.modified().unwrap_or(SystemTime::UNIX_EPOCH).into();

                entries.push(ObjectMeta {
                    path: obj_path,
                    size: fs_meta.len(),
                    content_type: "application/octet-stream".to_string(),
                    etag: String::new(),
                    created_at,
                    modified_at,
                    metadata: HashMap::new(),
                });
            }
        }

        Ok(entries)
    }
}

// ---------------------------------------------------------------------------
// S3-compatible backend (stub for S3, R2, MinIO)
// ---------------------------------------------------------------------------

/// Configuration for S3-compatible storage backends.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct S3Config {
    /// S3 endpoint URL (e.g., `https://s3.amazonaws.com` or a MinIO/R2 endpoint).
    pub endpoint: String,
    /// Bucket name.
    pub bucket: String,
    /// AWS region.
    pub region: String,
    /// Access key ID.
    pub access_key_id: String,
    /// Secret access key.
    pub secret_access_key: String,
    /// Optional path prefix within the bucket.
    pub prefix: Option<String>,
    /// Whether to use path-style addressing (for MinIO, etc.).
    pub path_style: bool,
}

/// S3-compatible storage backend.
///
/// Works with Amazon S3, Cloudflare R2, MinIO, and any S3-compatible
/// service. Uses the `aws-sdk-s3` crate under the hood.
///
/// Note: The actual S3 SDK calls will be wired once the `aws-sdk-s3`
/// dependency is added. The trait interface is complete and the local
/// filesystem backend provides a working reference implementation.
pub struct S3Backend {
    config: S3Config,
}

impl S3Backend {
    /// Create a new S3-compatible backend with the given configuration.
    pub fn new(config: S3Config) -> Self {
        Self { config }
    }

    /// Get the effective object key (with prefix if configured).
    fn effective_key(&self, path: &str) -> String {
        match &self.config.prefix {
            Some(prefix) => format!("{prefix}/{path}"),
            None => path.to_string(),
        }
    }
}

impl StorageBackend for S3Backend {
    async fn put_object(
        &self,
        path: &str,
        _data: &[u8],
        _content_type: &str,
        _metadata: &HashMap<String, String>,
    ) -> Result<String, StorageError> {
        let _key = self.effective_key(path);
        // TODO: wire to aws-sdk-s3 PutObject.
        Err(StorageError::BackendUnavailable(
            "S3 backend not yet wired to aws-sdk-s3".into(),
        ))
    }

    async fn get_object(&self, path: &str) -> Result<(Vec<u8>, ObjectMeta), StorageError> {
        let _key = self.effective_key(path);
        Err(StorageError::BackendUnavailable(
            "S3 backend not yet wired to aws-sdk-s3".into(),
        ))
    }

    async fn delete_object(&self, path: &str) -> Result<(), StorageError> {
        let _key = self.effective_key(path);
        Err(StorageError::BackendUnavailable(
            "S3 backend not yet wired to aws-sdk-s3".into(),
        ))
    }

    async fn head_object(&self, path: &str) -> Result<ObjectMeta, StorageError> {
        let _key = self.effective_key(path);
        Err(StorageError::BackendUnavailable(
            "S3 backend not yet wired to aws-sdk-s3".into(),
        ))
    }

    async fn list_objects(
        &self,
        prefix: &str,
        _limit: usize,
        _cursor: Option<&str>,
    ) -> Result<Vec<ObjectMeta>, StorageError> {
        let _key = self.effective_key(prefix);
        Err(StorageError::BackendUnavailable(
            "S3 backend not yet wired to aws-sdk-s3".into(),
        ))
    }
}

// ---------------------------------------------------------------------------
// Storage engine (orchestrator)
// ---------------------------------------------------------------------------

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
}

impl<B: StorageBackend> StorageEngine<B> {
    /// Create a new storage engine with the given backend and signing key.
    pub fn new(backend: Arc<B>, signing_key: Vec<u8>) -> Self {
        Self {
            backend,
            hooks: Vec::new(),
            signing_key,
            signed_url_ttl: Duration::from_secs(3600),
            resumable_uploads: dashmap::DashMap::new(),
        }
    }

    /// Add an upload hook.
    pub fn add_hook(&mut self, hook: Arc<dyn UploadHook>) {
        self.hooks.push(hook);
    }

    /// Set the default signed URL TTL.
    pub fn set_signed_url_ttl(&mut self, ttl: Duration) {
        self.signed_url_ttl = ttl;
    }

    /// Upload a file, running all pre/post hooks.
    pub async fn upload(
        &self,
        path: &str,
        data: &[u8],
        content_type: &str,
        metadata: HashMap<String, String>,
    ) -> Result<UploadResult, StorageError> {
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

        let expected_sig = data_encoding::BASE64URL_NOPAD.encode(&mac.finalize().into_bytes());
        if expected_sig != signature {
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
    pub fn append_chunk(
        &self,
        upload_id: Uuid,
        offset: u64,
        data: &[u8],
    ) -> Result<u64, StorageError> {
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

        // TODO: persist chunk data to a staging area.

        Ok(upload.next_offset)
    }

    /// Get the status of a resumable upload.
    pub fn resumable_upload_status(&self, upload_id: Uuid) -> Option<ResumableUpload> {
        self.resumable_uploads.get(&upload_id).map(|u| u.clone())
    }

    /// Cancel and clean up a resumable upload.
    pub fn cancel_resumable_upload(&self, upload_id: Uuid) -> bool {
        self.resumable_uploads.remove(&upload_id).is_some()
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

    #[test]
    fn image_transform_parsing() {
        let t = ImageTransform::from_query("w=200,h=150,fit=cover,format=webp,q=80");
        assert_eq!(t.width, Some(200));
        assert_eq!(t.height, Some(150));
        assert!(matches!(t.fit, Some(ImageFit::Cover)));
        assert!(matches!(t.format, Some(ImageFormat::Webp)));
        assert_eq!(t.quality, Some(80));
    }

    #[test]
    fn image_transform_empty() {
        let t = ImageTransform::from_query("");
        assert!(t.is_empty());
    }

    #[test]
    fn local_fs_path_traversal_rejected() {
        let backend = LocalFsBackend::new("/tmp/darshandb-test-storage").expect("create backend");
        assert!(backend.resolve_path("../../../etc/passwd").is_err());
        assert!(backend.resolve_path("/absolute/path").is_err());
        assert!(backend.resolve_path("safe/path/file.txt").is_ok());
    }

    #[tokio::test]
    async fn local_fs_roundtrip() {
        let dir = format!("/tmp/darshandb-test-{}", Uuid::new_v4());
        let backend = LocalFsBackend::new(&dir).expect("create backend");

        let path = "test/hello.txt";
        let data = b"Hello, DarshanDB!";
        let meta = HashMap::new();

        let etag = backend
            .put_object(path, data, "text/plain", &meta)
            .await
            .expect("put");
        assert!(!etag.is_empty());

        let (got_data, got_meta) = backend.get_object(path).await.expect("get");
        assert_eq!(got_data, data);
        assert_eq!(got_meta.content_type, "text/plain");

        backend.delete_object(path).await.expect("delete");
        assert!(backend.get_object(path).await.is_err());

        // Clean up.
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn signed_url_roundtrip() {
        let backend: Arc<LocalFsBackend> =
            Arc::new(LocalFsBackend::new("/tmp/darshandb-test-sign").expect("create backend"));
        let engine = StorageEngine::new(backend, b"test-secret-key".to_vec());

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

        // Wrong path should fail.
        assert!(
            engine
                .verify_signed_url("uploads/wrong.png", expires, sig)
                .is_err()
        );

        let _ = std::fs::remove_dir_all("/tmp/darshandb-test-sign");
    }

    #[test]
    fn resumable_upload_lifecycle() {
        let backend: Arc<LocalFsBackend> =
            Arc::new(LocalFsBackend::new("/tmp/darshandb-test-resumable").expect("create backend"));
        let engine = StorageEngine::new(backend, b"key".to_vec());

        let id = engine.create_resumable_upload("file.bin", "application/octet-stream", Some(100));

        let status = engine.resumable_upload_status(id).expect("status");
        assert_eq!(status.bytes_received, 0);
        assert_eq!(status.next_offset, 0);

        let next = engine.append_chunk(id, 0, &[0u8; 50]).expect("chunk 1");
        assert_eq!(next, 50);

        let next = engine.append_chunk(id, 50, &[0u8; 50]).expect("chunk 2");
        assert_eq!(next, 100);

        // Wrong offset should fail.
        assert!(engine.append_chunk(id, 0, &[0u8; 10]).is_err());

        assert!(engine.cancel_resumable_upload(id));
        assert!(engine.resumable_upload_status(id).is_none());

        let _ = std::fs::remove_dir_all("/tmp/darshandb-test-resumable");
    }
}
