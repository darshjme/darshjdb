/**
 * File storage client for DarshanDB.
 *
 * Supports regular uploads, resumable uploads for files over 5 MB,
 * progress tracking, URL generation, and deletion.
 *
 * @module storage
 */

import type { DarshanDB } from './client.js';
import type { UploadOptions, UploadResult } from './types.js';

/* -------------------------------------------------------------------------- */
/*  Constants                                                                 */
/* -------------------------------------------------------------------------- */

/** Files larger than this threshold use resumable uploads. */
const RESUMABLE_THRESHOLD = 5 * 1024 * 1024; // 5 MB

/** Chunk size for resumable uploads. */
const CHUNK_SIZE = 2 * 1024 * 1024; // 2 MB

/* -------------------------------------------------------------------------- */
/*  StorageClient                                                             */
/* -------------------------------------------------------------------------- */

/**
 * Client for uploading, fetching, and deleting files stored in DarshanDB.
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
  private _privateClient: DarshanDB;

  constructor(client: DarshanDB) {
    this._privateClient = client;
  }

  /* -- Upload ------------------------------------------------------------- */

  /**
   * Upload a file to DarshanDB storage.
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
    const headers = this._privateAuthHeaders();

    const resp = await fetch(
      this._privateClient.getRestUrl(
        `/storage/url?path=${encodeURIComponent(path)}`,
      ),
      { headers },
    );

    if (!resp.ok) {
      const body = await resp.text();
      throw new Error(`Failed to get URL (${resp.status}): ${body}`);
    }

    const data = (await resp.json()) as { url: string };
    return data.url;
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
      this._privateClient.getRestUrl(
        `/storage/files?path=${encodeURIComponent(path)}`,
      ),
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
    if (contentType) {
      formData.append('contentType', contentType);
    }
    if (options.metadata) {
      formData.append('metadata', JSON.stringify(options.metadata));
    }

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
            const result = JSON.parse(xhr.responseText) as UploadResult;
            options.onProgress?.(1);
            resolve(result);
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

    // Step 1: Initiate resumable upload session.
    const initResp = await fetch(
      this._privateClient.getRestUrl('/storage/upload/resumable'),
      {
        method: 'POST',
        headers: { ...headers, 'Content-Type': 'application/json' },
        body: JSON.stringify({
          path,
          contentType,
          size: file.size,
          metadata: options.metadata,
        }),
      },
    );

    if (!initResp.ok) {
      const body = await initResp.text();
      throw new Error(`Resumable init failed (${initResp.status}): ${body}`);
    }

    const { uploadId, chunkSize: serverChunkSize } = (await initResp.json()) as {
      uploadId: string;
      chunkSize?: number;
    };

    const chunkSize = serverChunkSize ?? CHUNK_SIZE;
    const totalChunks = Math.ceil(file.size / chunkSize);
    let uploadedBytes = 0;

    // Step 2: Upload chunks sequentially.
    for (let i = 0; i < totalChunks; i++) {
      const start = i * chunkSize;
      const end = Math.min(start + chunkSize, file.size);
      const chunk = file.slice(start, end);

      const chunkResp = await fetch(
        this._privateClient.getRestUrl(
          `/storage/upload/resumable/${uploadId}/chunk`,
        ),
        {
          method: 'PUT',
          headers: {
            ...headers,
            'Content-Type': 'application/octet-stream',
            'Content-Range': `bytes ${start}-${end - 1}/${file.size}`,
            'X-Chunk-Index': i.toString(),
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

      uploadedBytes += end - start;
      options.onProgress?.(uploadedBytes / file.size);
    }

    // Step 3: Complete the upload.
    const completeResp = await fetch(
      this._privateClient.getRestUrl(
        `/storage/upload/resumable/${uploadId}/complete`,
      ),
      {
        method: 'POST',
        headers,
      },
    );

    if (!completeResp.ok) {
      const body = await completeResp.text();
      throw new Error(`Upload complete failed (${completeResp.status}): ${body}`);
    }

    const result = (await completeResp.json()) as UploadResult;
    options.onProgress?.(1);
    return result;
  }

  /* -- Helpers ------------------------------------------------------------ */

  private _privateAuthHeaders(): Record<string, string> {
    const headers: Record<string, string> = {};
    const token = this._privateClient.getAuthToken();
    if (token) {
      headers['Authorization'] = `Bearer ${token}`;
    }
    return headers;
  }
}
