/**
 * @module use-storage
 * @description Hook for uploading files to DarshJDB storage with
 * reactive progress tracking.
 *
 * @example
 * ```tsx
 * import { useStorage } from '@darshjdb/react';
 *
 * function AvatarUpload() {
 *   const { upload, isUploading, progress, error } = useStorage();
 *
 *   const handleFileChange = async (e: React.ChangeEvent<HTMLInputElement>) => {
 *     const file = e.target.files?.[0];
 *     if (!file) return;
 *
 *     const result = await upload(file, `avatars/${file.name}`);
 *     console.log('Uploaded to:', result.url);
 *   };
 *
 *   return (
 *     <div>
 *       <input type="file" onChange={handleFileChange} disabled={isUploading} />
 *       {isUploading && (
 *         <progress value={progress.fraction} max={1} />
 *       )}
 *       {error && <p>Upload failed: {error.message}</p>}
 *     </div>
 *   );
 * }
 * ```
 */

import { useCallback, useRef, useState } from 'react';

import { useDarshanClient } from './provider';
import type { UploadProgress, UploadResult } from './types';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/** Return value of {@link useStorage}. */
export interface UseStorageResult {
  /**
   * Upload a file or blob to the specified storage path.
   *
   * @param file - The `File` or `Blob` to upload.
   * @param path - Destination path in DarshJDB storage (e.g. `"avatars/photo.jpg"`).
   * @returns The upload result containing the public URL, path, size, and content type.
   * @throws Rejects with an `Error` when the upload fails.
   */
  readonly upload: (file: File | Blob, path: string) => Promise<UploadResult>;
  /** `true` while an upload is in-flight. */
  readonly isUploading: boolean;
  /** Current upload progress. Resets when a new upload starts. */
  readonly progress: UploadProgress;
  /** Non-null when the most recent upload failed. */
  readonly error: Error | null;
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const ZERO_PROGRESS: UploadProgress = Object.freeze({
  bytesTransferred: 0,
  totalBytes: 0,
  fraction: 0,
});

// ---------------------------------------------------------------------------
// Hook
// ---------------------------------------------------------------------------

/**
 * Upload files to DarshJDB storage with reactive progress tracking.
 *
 * The `upload` reference is stable across re-renders.  Progress state
 * updates are batched to avoid excessive re-renders during fast uploads.
 *
 * @returns A {@link UseStorageResult} object.
 */
export function useStorage(): UseStorageResult {
  const client = useDarshanClient();
  const clientRef = useRef(client);
  clientRef.current = client;

  const [isUploading, setIsUploading] = useState(false);
  const [progress, setProgress] = useState<UploadProgress>(ZERO_PROGRESS);
  const [error, setError] = useState<Error | null>(null);

  // Throttle progress updates to at most once every 50ms to prevent
  // render thrashing on fast connections.
  const lastProgressUpdate = useRef(0);
  const pendingProgress = useRef<UploadProgress | null>(null);
  const rafId = useRef<ReturnType<typeof requestAnimationFrame> | null>(null);

  const flushProgress = useCallback(() => {
    if (pendingProgress.current) {
      setProgress(pendingProgress.current);
      pendingProgress.current = null;
    }
    rafId.current = null;
  }, []);

  const onProgress = useCallback(
    (p: UploadProgress) => {
      const now = Date.now();
      // Always flush the final progress event immediately.
      if (p.fraction >= 1 || now - lastProgressUpdate.current >= 50) {
        lastProgressUpdate.current = now;
        setProgress(p);
        pendingProgress.current = null;
      } else {
        pendingProgress.current = p;
        if (!rafId.current) {
          rafId.current = requestAnimationFrame(flushProgress);
        }
      }
    },
    [flushProgress],
  );

  const upload = useCallback(
    async (file: File | Blob, path: string): Promise<UploadResult> => {
      setError(null);
      setProgress(ZERO_PROGRESS);
      setIsUploading(true);

      try {
        const result = await clientRef.current.upload(file, path, {
          onProgress,
        });

        // Ensure we show 100% on completion.
        setProgress({
          bytesTransferred: result.size,
          totalBytes: result.size,
          fraction: 1,
        });

        return result;
      } catch (err) {
        const uploadError =
          err instanceof Error ? err : new Error(String(err));
        setError(uploadError);
        throw uploadError;
      } finally {
        setIsUploading(false);
        // Clean up any pending animation frame.
        if (rafId.current) {
          cancelAnimationFrame(rafId.current);
          rafId.current = null;
        }
      }
    },
    [onProgress],
  );

  return { upload, isUploading, progress, error };
}
