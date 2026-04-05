import { useState } from "react";
import { ArrowUpDown, ArrowUp, ArrowDown, ChevronLeft, ChevronRight } from "lucide-react";
import { cn, formatRelativeTime } from "../lib/utils";

interface Column<T> {
  key: string;
  label: string;
  sortable?: boolean;
  render?: (value: unknown, row: T) => React.ReactNode;
  width?: string;
}

interface DataTableProps<T extends Record<string, unknown>> {
  columns: Column<T>[];
  data: T[];
  pageSize?: number;
  onRowClick?: (row: T) => void;
  emptyMessage?: string;
}

type SortDirection = "asc" | "desc" | null;

export function DataTable<T extends Record<string, unknown>>({
  columns,
  data,
  pageSize = 10,
  onRowClick,
  emptyMessage = "No data available",
}: DataTableProps<T>) {
  const [sortKey, setSortKey] = useState<string | null>(null);
  const [sortDir, setSortDir] = useState<SortDirection>(null);
  const [page, setPage] = useState(0);

  const handleSort = (key: string) => {
    if (sortKey === key) {
      if (sortDir === "asc") setSortDir("desc");
      else if (sortDir === "desc") {
        setSortKey(null);
        setSortDir(null);
      }
    } else {
      setSortKey(key);
      setSortDir("asc");
    }
  };

  const sorted = [...data].sort((a, b) => {
    if (!sortKey || !sortDir) return 0;
    const aVal = a[sortKey];
    const bVal = b[sortKey];
    if (aVal == null) return 1;
    if (bVal == null) return -1;
    const cmp = aVal < bVal ? -1 : aVal > bVal ? 1 : 0;
    return sortDir === "asc" ? cmp : -cmp;
  });

  const totalPages = Math.ceil(sorted.length / pageSize);
  const paged = sorted.slice(page * pageSize, (page + 1) * pageSize);

  const SortIcon = ({ col }: { col: string }) => {
    if (sortKey !== col) return <ArrowUpDown className="w-3 h-3 text-zinc-600" />;
    return sortDir === "asc" ? (
      <ArrowUp className="w-3 h-3 text-amber-500" />
    ) : (
      <ArrowDown className="w-3 h-3 text-amber-500" />
    );
  };

  if (data.length === 0) {
    return (
      <div className="flex items-center justify-center py-16 text-zinc-500 text-sm">
        {emptyMessage}
      </div>
    );
  }

  return (
    <div className="flex flex-col">
      <div className="overflow-x-auto">
        <table className="w-full">
          <thead>
            <tr className="bg-zinc-900/50">
              {columns.map((col) => (
                <th
                  key={col.key}
                  className={cn(
                    "table-header text-left",
                    col.sortable && "cursor-pointer select-none hover:text-zinc-300",
                  )}
                  style={col.width ? { width: col.width } : undefined}
                  onClick={() => col.sortable && handleSort(col.key)}
                >
                  <div className="flex items-center gap-1.5">
                    {col.label}
                    {col.sortable && <SortIcon col={col.key} />}
                  </div>
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {paged.map((row, i) => (
              <tr
                key={i}
                className={cn(
                  "hover:bg-zinc-800/40 transition-colors",
                  onRowClick && "cursor-pointer",
                )}
                onClick={() => onRowClick?.(row)}
              >
                {columns.map((col) => (
                  <td key={col.key} className="table-cell">
                    {col.render
                      ? col.render(row[col.key], row)
                      : renderValue(row[col.key])}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      {totalPages > 1 && (
        <div className="flex items-center justify-between px-4 py-3 border-t border-zinc-800">
          <span className="text-xs text-zinc-500">
            Showing {page * pageSize + 1}-{Math.min((page + 1) * pageSize, data.length)} of{" "}
            {data.length}
          </span>
          <div className="flex items-center gap-1">
            <button
              onClick={() => setPage(Math.max(0, page - 1))}
              disabled={page === 0}
              className="btn-ghost disabled:opacity-30"
              aria-label="Previous page"
            >
              <ChevronLeft className="w-4 h-4" />
            </button>
            {Array.from({ length: Math.min(totalPages, 5) }, (_, i) => {
              const p = totalPages <= 5 ? i : Math.max(0, Math.min(page - 2, totalPages - 5)) + i;
              return (
                <button
                  key={p}
                  onClick={() => setPage(p)}
                  className={cn(
                    "w-8 h-8 rounded-lg text-xs font-medium transition-colors",
                    p === page
                      ? "bg-amber-500/10 text-amber-500"
                      : "text-zinc-500 hover:text-zinc-300 hover:bg-zinc-800/60",
                  )}
                >
                  {p + 1}
                </button>
              );
            })}
            <button
              onClick={() => setPage(Math.min(totalPages - 1, page + 1))}
              disabled={page === totalPages - 1}
              className="btn-ghost disabled:opacity-30"
              aria-label="Next page"
            >
              <ChevronRight className="w-4 h-4" />
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

function renderValue(val: unknown): React.ReactNode {
  if (val === null || val === undefined) {
    return <span className="text-zinc-600 italic">null</span>;
  }
  if (typeof val === "boolean") {
    return (
      <span className={val ? "text-emerald-400" : "text-red-400"}>
        {String(val)}
      </span>
    );
  }
  if (typeof val === "number") {
    if (val > 1_600_000_000_000 && val < 2_000_000_000_000) {
      return <span className="text-zinc-400">{formatRelativeTime(val)}</span>;
    }
    return <span className="text-sky-400 font-mono text-xs">{val}</span>;
  }
  if (typeof val === "string") {
    if (val.length > 60) return <span title={val}>{val.slice(0, 60)}...</span>;
    return <span>{val}</span>;
  }
  return <span className="text-zinc-500 font-mono text-xs">{JSON.stringify(val)}</span>;
}
