/**
 * File storage client for DarshJDB.
 *
 * Supports regular uploads, resumable uploads for files over 5 MB,
 * progress tracking, URL generation, and deletion.
 *
 * @module storage
 */

import type { DarshJDB } from './client.js';
import type { UploadOptions, UploadResult } from './types.js';

/* -------------------------------------------------------------------------- */
/*  Constants                                                                 */
/* -------------------------------------------------------------------------- */

/** Files larger than this threshold use resumable uploads. */
const RESUMABLE_THRESHOLD = 5 * 1024 * 1024; // 5 MB

/** Chunk size for resumable uploads. */
const CHUNK_SIZE = 2 * 1024 * 1024; // 2 MB

/** Percent-encode each segment of a storage path, keeping the separators. */
function encodePath(path: string): string {
  return path.split('/').map(encodeURIComponent).join('/');
}

/** Response body of `POST /api/storage/upload`. */
interface UploadResponse {
  path: string;
  size: number;
  content_type: string;
  signed_url?: string | null;
}

/* -------------------------------------------------------------------------- */
/*  StorageClient                                                             */
/* -------------------------------------------------------------------------- */

/**
 * Client for uploading, fetching, and deleting files stored in DarshJDB.
 *
 * @example
 * ```ts
 * const storage = new StorageClient(db);
 *
 * // Upload a file
 * const result = await storage.upload('avatars/profile.png', file, {
 *   onProgress: (p) => console.log(`${(p * 100).toFixed(0)}%`),
 * });
 *
 * // Get a signed URL
 * const url = await storage.getUrl('avatars/profile.png');
 *
 * // Delete
 * await storage.delete('avatars/profile.png');
 * ```
 */
export class StorageClient {
  private _privateClient: DarshJDB;

  constructor(client: DarshJDB) {
    this._privateClient = client;
  }

  /* -- Upload ------------------------------------------------------------- */

  /**
   * Upload a file to DarshJDB storage.
   *
   * Files under 5 MB are uploaded in a single request. Larger files
   * use a resumable, chunked upload protocol.
   *
   * @param path    - Storage path (e.g. `'avatars/profile.png'`).
   * @param file    - File or Blob to upload.
   * @param options - Upload options (content type, progress callback, metadata).
   * @returns Upload result with URL and metadata.
   */
  async upload(
    path: string,
    file: File | Blob,
    options: UploadOptions = {},
  ): Promise<UploadResult> {
    if (file.size > RESUMABLE_THRESHOLD) {
      return this._privateResumableUpload(path, file, options);
    }
    return this._privateSimpleUpload(path, file, options);
  }

  /* -- URL ---------------------------------------------------------------- */

  /**
   * Get a (potentially signed) URL for a stored file.
   *
   * @param path - Storage path.
   * @returns The file URL.
   */
  async getUrl(path: string): Promise<string> {
    const headers = { ...this._privateAuthHeaders(), Accept: 'application/json' };

    const resp = await fetch(
      `${this._privateClient.getRestUrl(`/storage/${encodePath(path)}`)}?signed=true`,
      { headers },
    );

    if (!resp.ok) {
      const body = await resp.text();
      throw new Error(`Failed to get URL (${resp.status}): ${body}`);
    }

    const data = (await resp.json()) as { signed_url: string };
    return data.signed_url;
  }

  /* -- Delete ------------------------------------------------------------- */

  /**
   * Delete a file from storage.
   *
   * @param path - Storage path of the file to delete.
   */
  async delete(path: string): Promise<void> {
    const headers = this._privateAuthHeaders();

    const resp = await fetch(
      this._privateClient.getRestUrl(`/storage/${encodePath(path)}`),
      { method: 'DELETE', headers },
    );

    if (!resp.ok) {
      const body = await resp.text();
      throw new Error(`Failed to delete file (${resp.status}): ${body}`);
    }
  }

  /* -- Simple upload ------------------------------------------------------ */

  private async _privateSimpleUpload(
    path: string,
    file: File | Blob,
    options: UploadOptions,
  ): Promise<UploadResult> {
    const contentType =
      options.contentType ?? (file instanceof File ? file.type : 'application/octet-stream');

    const formData = new FormData();
    formData.append('file', file);
    formData.append('path', path);

    const xhr = new XMLHttpRequest();

    return new Promise<UploadResult>((resolve, reject) => {
      if (options.onProgress) {
        xhr.upload.addEventListener('progress', (e) => {
          if (e.lengthComputable) {
            options.onProgress!(e.loaded / e.total);
          }
        });
      }

      xhr.addEventListener('load', () => {
        if (xhr.status >= 200 && xhr.status < 300) {
          try {
            const body = JSON.parse(xhr.responseText) as UploadResponse;
            options.onProgress?.(1);
            resolve(this._privateToResult(body, contentType));
          } catch {
            reject(new Error('Invalid upload response'));
          }
        } else {
          reject(new Error(`Upload failed (${xhr.status}): ${xhr.responseText}`));
        }
      });

      xhr.addEventListener('error', () => {
        reject(new Error('Upload network error'));
      });

      xhr.addEventListener('abort', () => {
        reject(new Error('Upload aborted'));
      });

      xhr.open('POST', this._privateClient.getRestUrl('/storage/upload'));
      // Set auth header (FormData sets Content-Type automatically with boundary).
      const token = this._privateClient.getAuthToken();
      if (token) {
        xhr.setRequestHeader('Authorization', `Bearer ${token}`);
      }
      xhr.send(formData);
    });
  }

  /* -- Resumable upload --------------------------------------------------- */

  private async _privateResumableUpload(
    path: string,
    file: File | Blob,
    options: UploadOptions,
  ): Promise<UploadResult> {
    const headers = this._privateAuthHeaders();
    const contentType =
      options.contentType ?? (file instanceof File ? file.type : 'application/octet-stream');

    const totalChunks = Math.max(1, Math.ceil(file.size / CHUNK_SIZE));

    // Step 1: Initiate the chunked upload session.
    const initResp = await fetch(
      this._privateClient.getRestUrl('/storage/upload/init'),
      {
        method: 'POST',
        headers: { ...headers, 'Content-Type': 'application/json' },
        body: JSON.stringify({
          path,
          content_type: contentType,
          total_chunks: totalChunks,
          file_size: file.size,
        }),
      },
    );

    if (!initResp.ok) {
      const body = await initResp.text();
      throw new Error(`Resumable init failed (${initResp.status}): ${body}`);
    }

    const { upload_id: uploadId } = (await initResp.json()) as {
      upload_id: string;
    };

    let uploadedBytes = 0;
    let final: { path: string } | null = null;

    // Step 2: Upload chunks sequentially. The server assembles and completes
    // the upload itself once the last outstanding chunk lands.
    for (let i = 0; i < totalChunks; i++) {
      const start = i * CHUNK_SIZE;
      const end = Math.min(start + CHUNK_SIZE, file.size);
      const chunk = file.slice(start, end);

      const chunkResp = await fetch(
        this._privateClient.getRestUrl(
          `/storage/upload/${uploadId}/chunk/${i}`,
        ),
        {
          method: 'PUT',
          headers: {
            ...headers,
            'Content-Type': 'application/octet-stream',
            Accept: 'application/json',
          },
          body: chunk,
        },
      );

      if (!chunkResp.ok) {
        const body = await chunkResp.text();
        throw new Error(
          `Chunk upload failed (${chunkResp.status}): ${body}`,
        );
      }

      const status = (await chunkResp.json()) as {
        status: string;
        path: string;
      };
      if (status.status === 'completed') {
        final = status;
      }

      uploadedBytes += end - start;
      options.onProgress?.(uploadedBytes / file.size);
    }

    if (!final) {
      throw new Error(
        `Resumable upload ${uploadId} did not complete after ${totalChunks} chunks`,
      );
    }

    options.onProgress?.(1);
    return this._privateToResult(
      { path: final.path, size: file.size, content_type: contentType },
      contentType,
    );
  }

  /* -- Helpers ------------------------------------------------------------ */

  private _privateToResult(
    body: UploadResponse,
    fallbackContentType: string,
  ): UploadResult {
    return {
      path: body.path,
      url:
        body.signed_url ??
        this._privateClient.getRestUrl(`/storage/${encodePath(body.path)}`),
      size: body.size,
      contentType: body.content_type || fallbackContentType,
    };
  }

  private _privateAuthHeaders(): Record<string, string> {
    const headers: Record<string, string> = {};
    const token = this._privateClient.getAuthToken();
    if (token) {
      headers['Authorization'] = `Bearer ${token}`;
    }
    return headers;
  }
}
