import { useState, useEffect, useCallback, useRef } from "react";
import {
  Grid,
  List,
  Upload,
  Search,
  Download,
  Trash2,
  Image,
  FileText,
  File,
  FileCode,
  Archive,
  Video,
  Eye,
  Loader2,
  AlertTriangle,
  RefreshCw,
} from "lucide-react";
import { Badge } from "../components/Badge";
import { fetchStorageFiles } from "../lib/api";
import { apiFetch, API_URL, getToken } from "../lib/http";
import { cn, formatBytes, formatRelativeTime } from "../lib/utils";
import type { StorageFile } from "../types";

const mimeIcons: Record<string, typeof File> = {
  "image/png": Image,
  "image/jpeg": Image,
  "image/svg+xml": Image,
  "application/pdf": FileText,
  "application/zip": Archive,
  "text/csv": FileCode,
  "application/json": FileCode,
  "application/jsonl": FileCode,
  "video/mp4": Video,
};

export function Storage() {
  const picker = useRef<HTMLInputElement>(null);
  const [uploading, setUploading] = useState(false);
  const [view, setView] = useState<"grid" | "list">("grid");
  const [search, setSearch] = useState("");
  const [dragOver, setDragOver] = useState(false);
  const [selectedFile, setSelectedFile] = useState<StorageFile | null>(null);
  const [files, setFiles] = useState<StorageFile[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const loadFiles = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const res = await fetchStorageFiles();
      setFiles(
        res.files.map((f) => ({
          id: f.id,
          name: f.name,
          size: f.size,
          mimeType: f.mimeType,
          url: `/api/storage/${f.path.split("/").map(encodeURIComponent).join("/")}`,
          uploadedAt: f.uploadedAt,
          uploadedBy: f.metadata?.["uploaded-by"] ?? "Unknown",
        })),
      );
    } catch (e) {
      setFiles([]);
      setError(e instanceof Error ? e.message : "Failed to load storage files");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    loadFiles();
  }, [loadFiles]);


  const filtered = files.filter((f) =>
    f.name.toLowerCase().includes(search.toLowerCase()),
  );

  const totalSize = files.reduce((sum, f) => sum + f.size, 0);

  const handleDragOver = useCallback((e: React.DragEvent) => {
    e.preventDefault();
    setDragOver(true);
  }, []);

  const handleDragLeave = useCallback(() => {
    setDragOver(false);
  }, []);

  async function upload(items: FileList | null) {
    if (!items?.length) return;
    setUploading(true); setError(null);
    try {
      for (const file of Array.from(items)) {
        const body = new FormData(); body.append("file", file); body.append("path", `${crypto.randomUUID()}/${file.name}`);
        await apiFetch("/api/storage/upload", { method: "POST", body });
      }
      await loadFiles();
    } catch (e) { setError(e instanceof Error ? e.message : "Upload failed"); }
    finally { setUploading(false); if (picker.current) picker.current.value = ""; }
  }
  async function download(file: StorageFile) {
    try {
      const response = await fetch(`${API_URL}${file.url}`, {headers: {Authorization: `Bearer ${getToken()}`}});
      if (!response.ok) throw new Error(`Download failed (${response.status})`);
      const url = URL.createObjectURL(await response.blob());
      const link = document.createElement("a"); link.href = url; link.download = file.name; link.click();
      setTimeout(() => URL.revokeObjectURL(url), 1000);
    } catch (e) { setError(e instanceof Error ? e.message : "Download failed"); }
  }
  async function remove(file: StorageFile) {
    if (!window.confirm(`Permanently delete ${file.name}?`)) return;
    try { await apiFetch(file.url, {method:"DELETE"}); setSelectedFile(null); await loadFiles(); }
    catch (e) { setError(e instanceof Error ? e.message : "Delete failed"); }
  }
  const handleDrop = (e: React.DragEvent) => { e.preventDefault(); setDragOver(false); if (!uploading) void upload(e.dataTransfer.files); };

  return (
    <div className="p-6">
      <input ref={picker} type="file" multiple hidden onChange={e => { void upload(e.target.files); }} aria-label="Choose files to upload" />
      {uploading && <p role="status" className="text-brand-400 mb-3">Uploading files…</p>}
      {selectedFile && <div className="glass-panel p-4 mb-4 text-sm text-ink-secondary"><strong>{selectedFile.name}</strong><p>{selectedFile.mimeType} · {formatBytes(selectedFile.size)}</p><button className="btn-ghost" onClick={() => setSelectedFile(null)}>Close details</button></div>}
      {/* Header */}
      <div className="flex items-center justify-between mb-6">
        <div>
          <h2 className="text-lg font-semibold text-ink">Storage</h2>
          <p className="text-sm text-ink-muted mt-0.5 flex items-center gap-2">
            {loading ? (
              <span className="flex items-center gap-1.5">
                <Loader2 className="w-3 h-3 animate-spin" />
                Loading...
              </span>
            ) : (
              <>{files.length} files, {formatBytes(totalSize)} total</>
            )}
          </p>
        </div>
        <div className="flex items-center gap-2">
          <button
            onClick={loadFiles}
            className="btn-ghost text-xs"
            title="Refresh"
          >
            <RefreshCw className={cn("w-3.5 h-3.5", loading && "animate-spin")} />
          </button>
          <button className="btn-primary text-sm" disabled={uploading} onClick={() => picker.current?.click()}>
            <Upload className="w-4 h-4" />
            Upload Files
          </button>
        </div>
      </div>

      {error && (
        <div className="flex items-center gap-2 px-4 py-3 mb-6 rounded-lg bg-red-500/10 border border-red-500/20 text-red-700 text-xs">
          <AlertTriangle className="w-3.5 h-3.5 flex-shrink-0" />
          <span>{error}</span>
        </div>
      )}

      {loading && (
        <div className="flex items-center justify-center gap-2 py-16 text-sm text-ink-muted">
          <Loader2 className="w-4 h-4 animate-spin" />
          Loading storage...
        </div>
      )}

      {!loading && !error && (
        <>
          {/* Drop zone */}
          <div
            onDragOver={handleDragOver}
            onDragLeave={handleDragLeave}
            onDrop={handleDrop}
            className={cn(
              "border-2 border-dashed rounded-xl p-8 mb-6 text-center transition-all",
              dragOver
                ? "border-brand-500 bg-brand-500/5"
                : "border-line hover:border-line-strong",
            )}
          >
            <Upload className={cn(
              "w-8 h-8 mx-auto mb-3",
              dragOver ? "text-brand-500" : "text-ink-muted",
            )} />
            <p className="text-sm text-ink-secondary">
              Drag and drop files here, or{" "}
              <button className="text-brand-500 hover:text-brand-400 font-medium">
                browse
              </button>
            </p>
            <p className="text-xs text-ink-muted mt-1">Max 100MB per file</p>
          </div>

          {/* Toolbar */}
          <div className="flex items-center gap-3 mb-4">
            <div className="relative flex-1 max-w-sm">
              <Search className="absolute left-3 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-ink-muted" />
              <input
                value={search}
                onChange={(e) => setSearch(e.target.value)}
                placeholder="Search files..."
                className="input-field pl-9 text-xs"
              />
            </div>
            <div className="flex items-center gap-1 bg-surface-subtle rounded-lg p-0.5 border border-line">
              <button
                aria-label="Grid view"
                onClick={() => setView("grid")}
                className={cn(
                  "p-1.5 rounded-md transition-colors",
                  view === "grid" ? "bg-surface-muted text-ink" : "text-ink-muted",
                )}
              >
                <Grid className="w-3.5 h-3.5" />
              </button>
              <button
                aria-label="List view"
                onClick={() => setView("list")}
                className={cn(
                  "p-1.5 rounded-md transition-colors",
                  view === "list" ? "bg-surface-muted text-ink" : "text-ink-muted",
                )}
              >
                <List className="w-3.5 h-3.5" />
              </button>
            </div>
          </div>

          {/* Empty state */}
          {files.length === 0 && (
            <div className="flex items-center justify-center py-16 text-sm text-ink-muted">
              No files stored yet.
            </div>
          )}

          {/* File display */}
          {files.length > 0 && view === "grid" ? (
            <div className="grid grid-cols-1 sm:grid-cols-2 xl:grid-cols-4 gap-3">
              {filtered.map((file) => {
                const Icon = mimeIcons[file.mimeType] || File;
                const isImage = file.mimeType.startsWith("image/");
                return (
                  <article
                    key={file.id}
                    className={cn(
                      "glass-panel p-0 text-left transition-all hover:border-line-strong group",
                      selectedFile?.id === file.id && "border-brand-500/40",
                    )}
                  >
                    <div className={cn(
                      "aspect-[4/3] flex items-center justify-center rounded-t-lg relative",
                      isImage ? "bg-gradient-to-br from-surface-muted to-surface-subtle" : "bg-surface-subtle/50",
                    )}>
                      <Icon className={cn(
                        "w-10 h-10",
                        isImage ? "text-brand-500/40" : "text-ink",
                      )} />
                      <div className="absolute top-2 right-2 opacity-100 sm:opacity-0 group-hover:opacity-100 group-focus-within:opacity-100 transition-opacity flex gap-1">
                        <button className="p-1 rounded bg-surface-subtle/90 text-ink-secondary hover:text-ink" aria-label={`Details for ${file.name}`} onClick={e => { e.stopPropagation(); setSelectedFile(file); }}>
                          <Eye className="w-3 h-3" />
                        </button>
                        <button className="p-1 rounded bg-surface-subtle/90 text-ink-secondary hover:text-ink" aria-label={`Download ${file.name}`} onClick={e => { e.stopPropagation(); void download(file); }}>
                          <Download className="w-3 h-3" />
                        </button>
                      </div>
                    </div>
                    <div className="p-3">
                      <button onClick={() => setSelectedFile(file)} className="text-xs font-medium text-ink truncate text-left w-full">
                        {file.name}
                      </button>
                      <div className="flex items-center justify-between mt-1">
                        <span className="text-[10px] text-ink-muted">
                          {formatBytes(file.size)}
                        </span>
                        <span className="text-[10px] text-ink-muted">
                          {formatRelativeTime(file.uploadedAt)}
                        </span>
                      </div>
                    </div>
                  </article>
                );
              })}
            </div>
          ) : files.length > 0 ? (
            <div className="glass-panel p-0 overflow-hidden">
              <table className="w-full">
                <thead>
                  <tr className="bg-surface-subtle/50">
                    <th className="table-header text-left">Name</th>
                    <th className="table-header text-left">Type</th>
                    <th className="table-header text-left">Size</th>
                    <th className="table-header text-left">Uploaded by</th>
                    <th className="table-header text-left">Date</th>
                    <th className="table-header text-right">Actions</th>
                  </tr>
                </thead>
                <tbody>
                  {filtered.map((file) => {
                    const Icon = mimeIcons[file.mimeType] || File;
                    return (
                      <tr
                        key={file.id}
                        className="hover:bg-surface-muted/40 transition-colors cursor-pointer"
                        onClick={() => setSelectedFile(file)}
                      >
                        <td className="table-cell">
                          <div className="flex items-center gap-2">
                            <Icon className="w-4 h-4 text-ink-muted" />
                            <span className="text-sm text-ink">{file.name}</span>
                          </div>
                        </td>
                        <td className="table-cell">
                          <Badge variant="zinc" className="text-[10px]">
                            {file.mimeType.split("/")[1]}
                          </Badge>
                        </td>
                        <td className="table-cell text-xs">{formatBytes(file.size)}</td>
                        <td className="table-cell text-xs text-ink-secondary">{file.uploadedBy}</td>
                        <td className="table-cell text-xs text-ink-muted">
                          {formatRelativeTime(file.uploadedAt)}
                        </td>
                        <td className="table-cell text-right">
                          <div className="flex items-center justify-end gap-1">
                            <button className="btn-ghost p-1" aria-label={`Download ${file.name}`} onClick={e => { e.stopPropagation(); void download(file); }}>
                              <Download className="w-3.5 h-3.5" />
                            </button>
                            <button className="btn-ghost p-1 text-red-700 hover:text-red-700" aria-label={`Delete ${file.name}`} onClick={e => { e.stopPropagation(); void remove(file); }}>
                              <Trash2 className="w-3.5 h-3.5" />
                            </button>
                          </div>
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          ) : null}
        </>
      )}
    </div>
  );
}
