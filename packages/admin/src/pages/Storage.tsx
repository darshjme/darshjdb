import { useState, useCallback, useEffect } from "react";
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
  RefreshCw,
} from "lucide-react";
import { Badge } from "../components/Badge";
import { mockStorageFiles } from "../lib/mock-data";
import { fetchStorageFiles } from "../lib/api";
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
  const [view, setView] = useState<"grid" | "list">("grid");
  const [search, setSearch] = useState("");
  const [dragOver, setDragOver] = useState(false);
  const [selectedFile, setSelectedFile] = useState<StorageFile | null>(null);
  const [files, setFiles] = useState<StorageFile[]>(mockStorageFiles);
  const [loading, setLoading] = useState(true);
  const [isLive, setIsLive] = useState(false);

  const loadFiles = useCallback(async () => {
    setLoading(true);
    try {
      const res = await fetchStorageFiles();
      if (res.files.length > 0) {
        setFiles(
          res.files.map((f) => ({
            id: f.id,
            name: f.name,
            size: f.size,
            mimeType: f.mimeType,
            url: `/api/storage/${f.path}`,
            uploadedAt: f.uploadedAt,
            uploadedBy: f.metadata?.["uploaded-by"] ?? "Unknown",
          })),
        );
        setIsLive(true);
      } else {
        setFiles(mockStorageFiles);
        setIsLive(false);
      }
    } catch {
      setFiles(mockStorageFiles);
      setIsLive(false);
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

  const handleDrop = useCallback((e: React.DragEvent) => {
    e.preventDefault();
    setDragOver(false);
    // Upload handling would go here
  }, []);

  return (
    <div className="p-6">
      {/* Header */}
      <div className="flex items-center justify-between mb-6">
        <div>
          <h2 className="text-lg font-semibold text-zinc-100">Storage</h2>
          <p className="text-sm text-zinc-500 mt-0.5 flex items-center gap-2">
            {loading ? (
              <span className="flex items-center gap-1.5">
                <Loader2 className="w-3 h-3 animate-spin" />
                Loading...
              </span>
            ) : (
              <>
                {files.length} files, {formatBytes(totalSize)} total
                {isLive ? (
                  <Badge variant="emerald" className="text-[9px]">live</Badge>
                ) : (
                  <Badge variant="zinc" className="text-[9px]">demo</Badge>
                )}
              </>
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
          <button className="btn-primary text-sm">
            <Upload className="w-4 h-4" />
            Upload Files
          </button>
        </div>
      </div>

      {/* Drop zone */}
      <div
        onDragOver={handleDragOver}
        onDragLeave={handleDragLeave}
        onDrop={handleDrop}
        className={cn(
          "border-2 border-dashed rounded-xl p-8 mb-6 text-center transition-all",
          dragOver
            ? "border-amber-500 bg-amber-500/5"
            : "border-zinc-800 hover:border-zinc-700",
        )}
      >
        <Upload className={cn(
          "w-8 h-8 mx-auto mb-3",
          dragOver ? "text-amber-500" : "text-zinc-600",
        )} />
        <p className="text-sm text-zinc-400">
          Drag and drop files here, or{" "}
          <button className="text-amber-500 hover:text-amber-400 font-medium">
            browse
          </button>
        </p>
        <p className="text-xs text-zinc-600 mt-1">Max 100MB per file</p>
      </div>

      {/* Toolbar */}
      <div className="flex items-center gap-3 mb-4">
        <div className="relative flex-1 max-w-sm">
          <Search className="absolute left-3 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-zinc-500" />
          <input
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            placeholder="Search files..."
            className="input-field pl-9 text-xs"
          />
        </div>
        <div className="flex items-center gap-1 bg-zinc-900 rounded-lg p-0.5 border border-zinc-800">
          <button
            onClick={() => setView("grid")}
            className={cn(
              "p-1.5 rounded-md transition-colors",
              view === "grid" ? "bg-zinc-800 text-zinc-100" : "text-zinc-500",
            )}
          >
            <Grid className="w-3.5 h-3.5" />
          </button>
          <button
            onClick={() => setView("list")}
            className={cn(
              "p-1.5 rounded-md transition-colors",
              view === "list" ? "bg-zinc-800 text-zinc-100" : "text-zinc-500",
            )}
          >
            <List className="w-3.5 h-3.5" />
          </button>
        </div>
      </div>

      {/* File display */}
      {view === "grid" ? (
        <div className="grid grid-cols-4 gap-3">
          {filtered.map((file) => {
            const Icon = mimeIcons[file.mimeType] || File;
            const isImage = file.mimeType.startsWith("image/");
            return (
              <button
                key={file.id}
                onClick={() => setSelectedFile(file)}
                className={cn(
                  "glass-panel p-0 text-left transition-all hover:border-zinc-700 group",
                  selectedFile?.id === file.id && "border-amber-500/40",
                )}
              >
                <div className={cn(
                  "aspect-[4/3] flex items-center justify-center rounded-t-lg relative",
                  isImage ? "bg-gradient-to-br from-zinc-800 to-zinc-900" : "bg-zinc-900/50",
                )}>
                  <Icon className={cn(
                    "w-10 h-10",
                    isImage ? "text-amber-500/40" : "text-zinc-700",
                  )} />
                  <div className="absolute top-2 right-2 opacity-0 group-hover:opacity-100 transition-opacity flex gap-1">
                    <button className="p-1 rounded bg-zinc-900/90 text-zinc-400 hover:text-zinc-100" aria-label={`Preview ${file.name}`}>
                      <Eye className="w-3 h-3" />
                    </button>
                    <button className="p-1 rounded bg-zinc-900/90 text-zinc-400 hover:text-zinc-100" aria-label={`Download ${file.name}`}>
                      <Download className="w-3 h-3" />
                    </button>
                  </div>
                </div>
                <div className="p-3">
                  <p className="text-xs font-medium text-zinc-200 truncate">
                    {file.name}
                  </p>
                  <div className="flex items-center justify-between mt-1">
                    <span className="text-[10px] text-zinc-500">
                      {formatBytes(file.size)}
                    </span>
                    <span className="text-[10px] text-zinc-600">
                      {formatRelativeTime(file.uploadedAt)}
                    </span>
                  </div>
                </div>
              </button>
            );
          })}
        </div>
      ) : (
        <div className="glass-panel p-0 overflow-hidden">
          <table className="w-full">
            <thead>
              <tr className="bg-zinc-900/50">
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
                    className="hover:bg-zinc-800/40 transition-colors cursor-pointer"
                    onClick={() => setSelectedFile(file)}
                  >
                    <td className="table-cell">
                      <div className="flex items-center gap-2">
                        <Icon className="w-4 h-4 text-zinc-500" />
                        <span className="text-sm text-zinc-200">{file.name}</span>
                      </div>
                    </td>
                    <td className="table-cell">
                      <Badge variant="zinc" className="text-[10px]">
                        {file.mimeType.split("/")[1]}
                      </Badge>
                    </td>
                    <td className="table-cell text-xs">{formatBytes(file.size)}</td>
                    <td className="table-cell text-xs text-zinc-400">{file.uploadedBy}</td>
                    <td className="table-cell text-xs text-zinc-500">
                      {formatRelativeTime(file.uploadedAt)}
                    </td>
                    <td className="table-cell text-right">
                      <div className="flex items-center justify-end gap-1">
                        <button className="btn-ghost p-1" aria-label={`Download ${file.name}`}>
                          <Download className="w-3.5 h-3.5" />
                        </button>
                        <button className="btn-ghost p-1 text-red-400 hover:text-red-300" aria-label={`Delete ${file.name}`}>
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
      )}
    </div>
  );
}
