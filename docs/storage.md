# File Storage

DarshJDB includes an S3-compatible file storage system. Upload files, generate signed URLs, and apply image transforms -- all through the same API.

## Storage Backends

| Backend | Config Value | Description |
|---------|-------------|-------------|
| Local filesystem | `local` | Default for development. Files stored in `./ddb-storage/` |
| Amazon S3 | `s3` | Production-ready object storage |
| Cloudflare R2 | `r2` | S3-compatible, zero egress fees |
| MinIO | `minio` | Self-hosted S3-compatible storage |

### Configuration

```bash
# Backend selection
DDB_STORAGE_BACKEND=s3

# S3 / R2 / MinIO credentials
DDB_S3_BUCKET=my-app-files
DDB_S3_REGION=us-east-1
DDB_S3_ACCESS_KEY=AKIA...
DDB_S3_SECRET_KEY=wJal...
DDB_S3_ENDPOINT=https://s3.amazonaws.com  # Override for R2/MinIO

# Local storage path (only for local backend)
DDB_STORAGE_PATH=./ddb-storage

# Signed URL expiry (default: 1 hour)
DDB_STORAGE_URL_EXPIRY=3600
```

## Uploading Files

### Client SDK

```typescript
// Upload a file
const file = document.querySelector('input[type="file"]').files[0];
const result = await db.storage.upload(file, {
  path: 'avatars/user-123.jpg',
});
// result.url → signed URL to the uploaded file

// Upload with metadata
const result = await db.storage.upload(file, {
  path: 'documents/report.pdf',
  metadata: { uploadedBy: currentUser.id, category: 'reports' },
});
```

### React

```tsx
function AvatarUpload() {
  const { upload, isUploading, progress } = db.useUpload();

  const handleChange = async (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    if (!file) return;

    const result = await upload(file, { path: `avatars/${currentUser.id}` });
    console.log('Uploaded:', result.url);
  };

  return (
    <div>
      <input type="file" accept="image/*" onChange={handleChange} />
      {isUploading && <progress value={progress} max={100} />}
    </div>
  );
}
```

### cURL

```bash
# Upload a file
curl -X POST http://localhost:7700/api/storage/upload \
  -H "Authorization: Bearer TOKEN" \
  -F "file=@photo.jpg" \
  -F "path=images/photo.jpg"

# Response:
# { "path": "images/photo.jpg", "url": "https://...", "size": 245760, "contentType": "image/jpeg" }
```

## Retrieving Files

```typescript
// Get a signed URL (expires after DDB_STORAGE_URL_EXPIRY seconds)
const url = await db.storage.getUrl('avatars/user-123.jpg');

// Get file metadata
const meta = await db.storage.getMeta('documents/report.pdf');
// { path, size, contentType, metadata, createdAt, updatedAt }
```

### REST API

```bash
# Get signed URL
curl http://localhost:7700/api/storage/avatars/user-123.jpg \
  -H "Authorization: Bearer TOKEN"

# List files in a directory
curl http://localhost:7700/api/storage?prefix=avatars/ \
  -H "Authorization: Bearer TOKEN"
```

## Image Transforms

Apply transforms on the fly by appending query parameters to signed URLs. Transforms are cached on the server.

```typescript
// Resize to 200x200, crop to fit
const url = await db.storage.getUrl('avatars/user-123.jpg', {
  width: 200,
  height: 200,
  fit: 'cover',
});

// Convert to WebP
const url = await db.storage.getUrl('photos/landscape.png', {
  format: 'webp',
  quality: 80,
});
```

### Available Transforms

| Parameter | Values | Description |
|-----------|--------|-------------|
| `width` | `1-4096` | Target width in pixels |
| `height` | `1-4096` | Target height in pixels |
| `fit` | `cover`, `contain`, `fill`, `inside`, `outside` | Resize strategy |
| `format` | `webp`, `avif`, `jpeg`, `png` | Output format |
| `quality` | `1-100` | Compression quality |
| `blur` | `0.3-1000` | Gaussian blur sigma |

## Resumable Uploads

For large files, use resumable uploads via the tus protocol:

```typescript
const upload = db.storage.createResumableUpload(file, {
  path: 'videos/recording.mp4',
  chunkSize: 5 * 1024 * 1024, // 5MB chunks
});

upload.on('progress', (percent) => {
  console.log(`${percent}% uploaded`);
});

upload.on('complete', (result) => {
  console.log('Upload complete:', result.url);
});

// Start (or resume after interruption)
upload.start();

// Pause
upload.pause();

// Resume
upload.resume();
```

## Deleting Files

```typescript
await db.storage.delete('avatars/old-avatar.jpg');

// Delete multiple files
await db.storage.deleteMany(['temp/file1.txt', 'temp/file2.txt']);
```

### REST API

```bash
curl -X DELETE http://localhost:7700/api/storage/avatars/old-avatar.jpg \
  -H "Authorization: Bearer TOKEN"
```

## Storage Permissions

Control who can upload, read, and delete files:

```typescript
// darshan/permissions.ts
export default {
  storage: {
    // Only authenticated users can upload
    upload: (ctx) => !!ctx.auth,

    // Users can only read their own files
    read: (ctx, { path }) => path.startsWith(`avatars/${ctx.auth.userId}`),

    // Only admins can delete
    delete: (ctx) => ctx.auth.role === 'admin',

    // Limit upload size (in bytes)
    maxFileSize: 10 * 1024 * 1024, // 10MB

    // Restrict allowed MIME types
    allowedTypes: ['image/jpeg', 'image/png', 'image/webp', 'application/pdf'],
  },
};
```

## Server Functions

Access storage from server functions:

```typescript
import { mutation, v } from '@darshjdb/server';

export const processUpload = mutation({
  args: { path: v.string() },
  handler: async (ctx, { path }) => {
    // Read a file
    const data = await ctx.storage.get(path);

    // Generate a signed URL
    const url = await ctx.storage.getUrl(path, { expiresIn: 3600 });

    // Copy a file
    await ctx.storage.copy(path, `backups/${path}`);

    // Delete a file
    await ctx.storage.delete(path);
  },
});
```

---

[Previous: Presence](presence.md) | [Next: Migration Guide](migration.md) | [All Docs](README.md)
