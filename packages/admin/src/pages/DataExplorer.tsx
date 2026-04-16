import { useState, useEffect, useCallback } from "react";
import {
  Play,
  Radio,
  Table2,
  Code,
  RefreshCw,
  Plus,
  Trash2,
  Download,
  AlertTriangle,
  Loader2,
} from "lucide-react";
import { DataTable } from "../components/DataTable";
import { Badge } from "../components/Badge";
import { fetchSchema, fetchEntities, queryDarshJQL } from "../lib/api";
import { cn, formatNumber } from "../lib/utils";
import type { EntityType, EntityRecord } from "../types";

export function DataExplorer() {
  const [entityTypes, setEntityTypes] = useState<EntityType[]>([]);
  const [selectedEntity, setSelectedEntity] = useState<EntityType | null>(null);
  const [records, setRecords] = useState<EntityRecord[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [liveMode, setLiveMode] = useState(false);
  const [showQuery, setShowQuery] = useState(false);
  const [queryText, setQueryText] = useState("");
  const [filter, setFilter] = useState("");

  // ── Load schema (entity types) from API on mount ──────────────────
  useEffect(() => {
    let cancelled = false;
    (async () => {
      setLoading(true);
      setError(null);
      try {
        const types = await fetchSchema();
        if (cancelled) return;
        setEntityTypes(types);
        if (types.length > 0) {
          setSelectedEntity(types[0]);
          setQueryText(`SELECT * FROM "${types[0].name}" LIMIT 100`);
        }
      } catch {
        if (cancelled) return;
        setError("Cannot connect to DarshJDB server. Is the server running?");
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => { cancelled = true; };
  }, []);

  // ── Load records when selected entity changes ─────────────────────
  const loadRecords = useCallback(async (entity: EntityType) => {
    setLoading(true);
    setError(null);
    try {
      const result = await fetchEntities(entity.name);
      setRecords(result.data);
    } catch {
      setError("Cannot connect to DarshJDB server. Is the server running?");
      setRecords([]);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (selectedEntity) loadRecords(selectedEntity);
  }, [selectedEntity, loadRecords]);

  // ── Live polling ──────────────────────────────────────────────────
  useEffect(() => {
    if (!liveMode || !selectedEntity) return;
    const interval = setInterval(() => {
      loadRecords(selectedEntity);
    }, 3000);
    return () => clearInterval(interval);
  }, [liveMode, selectedEntity, loadRecords]);

  // ── Update query text when entity changes ─────────────────────────
  useEffect(() => {
    if (selectedEntity) {
      setQueryText(`SELECT * FROM "${selectedEntity.name}" LIMIT 100`);
    }
  }, [selectedEntity]);

  // ── Run DarshJQL query ───────────────────────────────────────────
  const runQuery = useCallback(async () => {
    if (!selectedEntity) return;
    setLoading(true);
    try {
      // Parse simple SELECT-style into DarshJQL JSON object
      // For now, just fetch entity data; real DarshJQL goes through /api/query
      const results = await queryDarshJQL({
        type: selectedEntity.name,
        $limit: 100,
      });
      setRecords(results);
    } catch {
      // Keep existing records on query failure
    } finally {
      setLoading(false);
    }
  }, [selectedEntity]);

  const columns = selectedEntity
    ? selectedEntity.fields.map((f) => ({
        key: f.name,
        label: f.name,
        sortable: true,
        width: f.name === "_id" ? "140px" : undefined,
        render: f.name === "_id"
          ? (val: unknown) => (
              <span className="font-mono text-xs text-amber-500/80">{String(val)}</span>
            )
          : undefined,
      }))
    : [];

  const filteredEntities = filter
    ? entityTypes.filter((e) => e.name.toLowerCase().includes(filter.toLowerCase()))
    : entityTypes;

  return (
    <div className="flex h-full">
      {/* Entity list panel */}
      <div className="w-56 flex-shrink-0 border-r border-zinc-800 bg-zinc-950/50">
        <div className="p-3 border-b border-zinc-800">
          <input
            placeholder="Filter entities..."
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            className="input-field text-xs py-1.5"
            aria-label="Filter entities"
          />
        </div>
        <div className="overflow-y-auto py-1">
          {loading && entityTypes.length === 0 && (
            <div className="flex items-center justify-center gap-2 py-8 text-xs text-zinc-500">
              <Loader2 className="w-3.5 h-3.5 animate-spin" />
              Loading...
            </div>
          )}
          {filteredEntities.map((entity) => (
            <button
              key={entity.name}
              onClick={() => setSelectedEntity(entity)}
              className={cn(
                "flex items-center justify-between w-full px-3 py-2 text-sm transition-colors",
                selectedEntity?.name === entity.name
                  ? "bg-amber-500/10 text-amber-500"
                  : "text-zinc-400 hover:text-zinc-200 hover:bg-zinc-800/40",
              )}
            >
              <div className="flex items-center gap-2">
                <Table2 className="w-3.5 h-3.5 opacity-60" />
                <span className="font-medium">{entity.name}</span>
              </div>
              <Badge variant="zinc" className="text-[10px]">
                {formatNumber(entity.count)}
              </Badge>
            </button>
          ))}
        </div>
      </div>

      {/* Main content */}
      <div className="flex-1 flex flex-col min-w-0">
        {/* Error banner */}
        {error && (
          <div className="flex items-center gap-2 px-4 py-3 bg-red-500/10 border-b border-red-500/20 text-red-400 text-xs">
            <AlertTriangle className="w-3.5 h-3.5 flex-shrink-0" />
            <span>{error}</span>
          </div>
        )}

        {/* Toolbar */}
        {selectedEntity && (
        <div className="flex items-center justify-between px-4 py-2.5 border-b border-zinc-800">
          <div className="flex items-center gap-3">
            <h2 className="text-sm font-semibold text-zinc-100">
              {selectedEntity.name}
            </h2>
            <Badge variant="zinc">{formatNumber(selectedEntity.count)} rows</Badge>
            {loading && (
              <RefreshCw className="w-3.5 h-3.5 text-zinc-500 animate-spin" />
            )}
          </div>
          <div className="flex items-center gap-2">
            <button
              onClick={() => setLiveMode(!liveMode)}
              className={cn(
                "btn-ghost text-xs gap-1.5",
                liveMode && "text-emerald-400",
              )}
            >
              <Radio className={cn("w-3.5 h-3.5", liveMode && "animate-pulse-slow")} />
              Live
            </button>
            <button
              onClick={() => setShowQuery(!showQuery)}
              className={cn(
                "btn-ghost text-xs gap-1.5",
                showQuery && "text-amber-500",
              )}
            >
              <Code className="w-3.5 h-3.5" />
              Query
            </button>
            <div className="w-px h-5 bg-zinc-800" />
            <button
              className="btn-ghost text-xs"
              aria-label="Refresh data"
              onClick={() => loadRecords(selectedEntity)}
            >
              <RefreshCw className="w-3.5 h-3.5" />
            </button>
            <button className="btn-ghost text-xs" aria-label="Download data">
              <Download className="w-3.5 h-3.5" />
            </button>
            <button className="btn-ghost text-xs" aria-label="Add record">
              <Plus className="w-3.5 h-3.5" />
            </button>
            <button className="btn-ghost text-xs text-red-400 hover:text-red-300" aria-label="Delete selected">
              <Trash2 className="w-3.5 h-3.5" />
            </button>
          </div>
        </div>
        )}

        {/* Query panel */}
        {showQuery && (
          <div className="border-b border-zinc-800 bg-zinc-950/80">
            <div className="p-3">
              <div
                className="bg-zinc-900 border border-zinc-800 rounded-lg overflow-hidden"
              >
                <textarea
                  value={queryText}
                  onChange={(e) => setQueryText(e.target.value)}
                  className="w-full bg-transparent px-4 py-3 font-mono text-sm text-zinc-200 placeholder-zinc-600 resize-none focus:outline-none"
                  rows={3}
                  spellCheck={false}
                  placeholder='SELECT * FROM "users" WHERE role = "admin"'
                  aria-label="SQL query editor"
                  onKeyDown={(e) => {
                    if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
                      e.preventDefault();
                      runQuery();
                    }
                  }}
                />
              </div>
              <div className="flex items-center justify-between mt-2">
                <span className="text-[10px] text-zinc-600">
                  DarshJQL -- use Cmd+Enter to execute
                </span>
                <button className="btn-primary text-xs py-1.5" onClick={runQuery}>
                  <Play className="w-3 h-3" />
                  Run Query
                </button>
              </div>
            </div>
          </div>
        )}

        {/* Data table */}
        <div className="flex-1 overflow-auto">
          {loading && !selectedEntity ? (
            <div className="flex items-center justify-center gap-2 py-16 text-sm text-zinc-500">
              <Loader2 className="w-4 h-4 animate-spin" />
              Loading...
            </div>
          ) : selectedEntity ? (
            <DataTable
              columns={columns}
              data={records}
              pageSize={10}
            />
          ) : !error ? (
            <div className="flex items-center justify-center py-16 text-sm text-zinc-500">
              No entity types found.
            </div>
          ) : null}
        </div>
      </div>
    </div>
  );
}
