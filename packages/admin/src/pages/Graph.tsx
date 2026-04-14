import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import ForceGraph2D, {
  type ForceGraphMethods,
  type NodeObject,
  type LinkObject,
} from "react-force-graph-2d";
import {
  AlertTriangle,
  Network,
  RefreshCw,
  ArrowRight,
  ArrowLeft,
  ArrowLeftRight,
  Maximize2,
  X,
} from "lucide-react";
import { Badge } from "../components/Badge";
import { fetchSchema } from "../lib/api";
import {
  traverse,
  neighbours,
  parseRecordId,
  type Direction,
  type GraphData,
  type GraphEdge,
  type GraphNode,
} from "../lib/graph-api";
import type { EntityType } from "../types";
import { cn } from "../lib/utils";

// ---------------------------------------------------------------------------
// Palette — deterministic colour per entity type
// ---------------------------------------------------------------------------

const palette = [
  "#f59e0b", // amber-500
  "#38bdf8", // sky-400
  "#34d399", // emerald-400
  "#a78bfa", // purple-400
  "#f472b6", // pink-400
  "#facc15", // yellow-400
  "#fb923c", // orange-400
  "#60a5fa", // blue-400
  "#4ade80", // green-400
  "#f87171", // red-400
];

function colourForTable(table: string): string {
  let h = 0;
  for (let i = 0; i < table.length; i++) {
    h = (h * 31 + table.charCodeAt(i)) >>> 0;
  }
  return palette[h % palette.length];
}

// ---------------------------------------------------------------------------
// Graph node / link shapes that feed react-force-graph-2d.
//
// `react-force-graph-2d` mutates these objects (x/y/vx/vy) in place, so we
// keep them as plain objects and let the lib attach runtime fields.
// ---------------------------------------------------------------------------

interface FGNode extends GraphNode {
  color: string;
}

type FGLink = LinkObject<FGNode, GraphEdge>;

// ---------------------------------------------------------------------------
// Page
// ---------------------------------------------------------------------------

export function Graph() {
  const canvasWrapRef = useRef<HTMLDivElement | null>(null);
  const fgRef = useRef<ForceGraphMethods<FGNode, FGLink> | undefined>(undefined);

  // Controls ----------------------------------------------------------------
  const [entityTypes, setEntityTypes] = useState<EntityType[]>([]);
  const [startTable, setStartTable] = useState<string>("");
  const [startId, setStartId] = useState<string>("");
  const [relation, setRelation] = useState<string>("");
  const [direction, setDirection] = useState<Direction>("out");
  const [maxDepth, setMaxDepth] = useState<number>(2);

  // Data --------------------------------------------------------------------
  const [data, setData] = useState<GraphData | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [selectedNode, setSelectedNode] = useState<FGNode | null>(null);

  // Canvas dimensions -------------------------------------------------------
  const [dims, setDims] = useState({ width: 800, height: 600 });

  useEffect(() => {
    const el = canvasWrapRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => {
      const r = el.getBoundingClientRect();
      setDims({
        width: Math.max(320, Math.floor(r.width)),
        height: Math.max(320, Math.floor(r.height)),
      });
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // Load schema so the entity-type picker is populated -----------------------
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const types = await fetchSchema();
        if (cancelled) return;
        setEntityTypes(types);
        if (types.length > 0 && !startTable) {
          setStartTable(types[0].name);
        }
      } catch {
        // Schema unreachable -- leave dropdown empty, users can still type.
      }
    })();
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Traversal runner --------------------------------------------------------
  const runTraversal = useCallback(
    async (overrides?: { start?: string }) => {
      const startComposite =
        overrides?.start ?? (startTable && startId ? `${startTable}:${startId}` : "");
      if (!startComposite) {
        setError("Enter a start entity (table + id) before traversing.");
        return;
      }

      setLoading(true);
      setError(null);
      try {
        const result = await traverse({
          start: startComposite,
          direction,
          edge_type: relation || undefined,
          max_depth: maxDepth,
        });
        setData(result);
        setSelectedNode(null);
      } catch (err) {
        setError(err instanceof Error ? err.message : "Traversal failed");
        setData(null);
      } finally {
        setLoading(false);
      }
    },
    [startTable, startId, direction, relation, maxDepth],
  );

  // Convert our GraphData into the shape react-force-graph-2d consumes ------
  const graphPayload = useMemo(() => {
    if (!data) return { nodes: [] as FGNode[], links: [] as FGLink[] };

    const nodes: FGNode[] = data.nodes.map((n) => ({
      ...n,
      color: colourForTable(n.table),
    }));

    // Only keep edges whose endpoints exist in the node set (traversal is
    // truncated by max_depth/max_nodes, so dangling edges are possible).
    const ids = new Set(nodes.map((n) => n.id));
    const links: FGLink[] = data.edges
      .filter((e) => ids.has(e.source) && ids.has(e.target))
      .map((e) => ({
        id: e.id,
        source: e.source,
        target: e.target,
        relation: e.relation,
      }));

    return { nodes, links };
  }, [data]);

  // Handler: click a node → inspector --------------------------------------
  const handleNodeClick = useCallback((node: NodeObject<FGNode>) => {
    setSelectedNode(node as FGNode);
    // Centre/zoom on the clicked node.
    const fg = fgRef.current;
    if (fg && typeof node.x === "number" && typeof node.y === "number") {
      fg.centerAt(node.x, node.y, 400);
      fg.zoom(2.5, 400);
    }
  }, []);

  // Handler: expand from selected node -------------------------------------
  const handleExpand = useCallback(async () => {
    if (!selectedNode) return;
    const parsed = parseRecordId(selectedNode.id);
    if (!parsed) return;

    setLoading(true);
    setError(null);
    try {
      const newEdges = await neighbours(parsed.table, parsed.entityId, relation || undefined);
      setData((prev) => {
        if (!prev) return prev;
        // Merge edges -----------------------------------------------------
        const edgeIds = new Set(prev.edges.map((e) => e.id));
        const merged: GraphEdge[] = [...prev.edges];
        for (const e of newEdges) {
          if (!edgeIds.has(e.id)) merged.push(e);
        }
        // Merge nodes: any new endpoint becomes a node at depth+1 --------
        const existing = new Map(prev.nodes.map((n) => [n.id, n]));
        const parentDepth = selectedNode.depth;
        for (const e of newEdges) {
          for (const endpointId of [e.source, e.target]) {
            if (existing.has(endpointId)) continue;
            const p = parseRecordId(endpointId);
            if (!p) continue;
            existing.set(endpointId, {
              id: endpointId,
              table: p.table,
              entityId: p.entityId,
              depth: parentDepth + 1,
              label: endpointId,
            });
          }
        }
        return {
          ...prev,
          nodes: Array.from(existing.values()),
          edges: merged,
        };
      });
    } catch (err) {
      setError(err instanceof Error ? err.message : "Expand failed");
    } finally {
      setLoading(false);
    }
  }, [selectedNode, relation]);

  // ─────────────────────────────────────────────────────────────────────
  // Render
  // ─────────────────────────────────────────────────────────────────────

  const nodeCount = graphPayload.nodes.length;
  const edgeCount = graphPayload.links.length;
  const isEmpty = !loading && !error && data !== null && nodeCount === 0;
  const isInitial = !loading && !error && data === null;

  return (
    <div className="flex h-full">
      {/* Main column ---------------------------------------------------- */}
      <div className="flex-1 flex flex-col min-w-0 p-6">
        {/* Header */}
        <div className="flex items-center justify-between mb-4">
          <div>
            <h2 className="text-lg font-semibold text-zinc-100">Graph Explorer</h2>
            <p className="text-sm text-zinc-500 mt-0.5">
              Traverse record-link edges from any entity. BFS up to depth 5.
            </p>
          </div>
        </div>

        {/* Control bar */}
        <div className="glass-panel p-4 mb-4">
          <div className="flex flex-wrap items-end gap-3">
            {/* Start entity type */}
            <div className="flex flex-col gap-1 min-w-[140px]">
              <label className="text-[10px] uppercase tracking-wide text-zinc-500">
                Entity type
              </label>
              <select
                value={startTable}
                onChange={(e) => setStartTable(e.target.value)}
                className="bg-zinc-900 border border-zinc-800 rounded-md px-2 py-1.5 text-xs text-zinc-200 focus:outline-none focus:border-amber-500/50"
              >
                {entityTypes.length === 0 && (
                  <option value="">(no schema)</option>
                )}
                {entityTypes.map((et) => (
                  <option key={et.name} value={et.name}>
                    {et.name}
                  </option>
                ))}
              </select>
            </div>

            {/* Entity id */}
            <div className="flex flex-col gap-1 min-w-[200px] flex-1">
              <label className="text-[10px] uppercase tracking-wide text-zinc-500">
                Entity id
              </label>
              <input
                type="text"
                value={startId}
                onChange={(e) => setStartId(e.target.value)}
                placeholder="e.g. darsh or a uuid"
                className="bg-zinc-900 border border-zinc-800 rounded-md px-2 py-1.5 text-xs text-zinc-200 font-mono focus:outline-none focus:border-amber-500/50"
              />
            </div>

            {/* Relation filter */}
            <div className="flex flex-col gap-1 min-w-[160px]">
              <label className="text-[10px] uppercase tracking-wide text-zinc-500">
                Relation
              </label>
              <input
                type="text"
                value={relation}
                onChange={(e) => setRelation(e.target.value)}
                placeholder="any"
                className="bg-zinc-900 border border-zinc-800 rounded-md px-2 py-1.5 text-xs text-zinc-200 font-mono focus:outline-none focus:border-amber-500/50"
              />
            </div>

            {/* Direction toggle */}
            <div className="flex flex-col gap-1">
              <label className="text-[10px] uppercase tracking-wide text-zinc-500">
                Direction
              </label>
              <div className="flex items-center gap-1 bg-zinc-900 rounded-md p-0.5 border border-zinc-800">
                <button
                  type="button"
                  onClick={() => setDirection("out")}
                  className={cn(
                    "px-2 py-1 rounded text-[11px] font-medium transition-colors flex items-center gap-1",
                    direction === "out"
                      ? "bg-zinc-800 text-zinc-100"
                      : "text-zinc-500 hover:text-zinc-300",
                  )}
                  title="Outgoing edges"
                >
                  <ArrowRight className="w-3 h-3" />
                  Out
                </button>
                <button
                  type="button"
                  onClick={() => setDirection("in")}
                  className={cn(
                    "px-2 py-1 rounded text-[11px] font-medium transition-colors flex items-center gap-1",
                    direction === "in"
                      ? "bg-zinc-800 text-zinc-100"
                      : "text-zinc-500 hover:text-zinc-300",
                  )}
                  title="Incoming edges"
                >
                  <ArrowLeft className="w-3 h-3" />
                  In
                </button>
                <button
                  type="button"
                  onClick={() => setDirection("both")}
                  className={cn(
                    "px-2 py-1 rounded text-[11px] font-medium transition-colors flex items-center gap-1",
                    direction === "both"
                      ? "bg-zinc-800 text-zinc-100"
                      : "text-zinc-500 hover:text-zinc-300",
                  )}
                  title="Both directions"
                >
                  <ArrowLeftRight className="w-3 h-3" />
                  Both
                </button>
              </div>
            </div>

            {/* Depth slider */}
            <div className="flex flex-col gap-1 min-w-[140px]">
              <label className="text-[10px] uppercase tracking-wide text-zinc-500 flex items-center justify-between">
                <span>Depth</span>
                <span className="text-zinc-300 font-mono">{maxDepth}</span>
              </label>
              <input
                type="range"
                min={1}
                max={5}
                step={1}
                value={maxDepth}
                onChange={(e) => setMaxDepth(Number(e.target.value))}
                className="accent-amber-500 h-[22px]"
              />
            </div>

            {/* Refresh */}
            <button
              type="button"
              onClick={() => runTraversal()}
              disabled={loading || !startTable || !startId}
              className="btn-ghost bg-amber-500/10 border border-amber-500/30 text-amber-400 hover:bg-amber-500/20 disabled:opacity-40 disabled:cursor-not-allowed text-xs h-[34px] px-3"
            >
              <RefreshCw className={cn("w-3.5 h-3.5", loading && "animate-spin")} />
              {loading ? "Traversing..." : "Traverse"}
            </button>
          </div>
        </div>

        {/* Error banner */}
        {error && (
          <div className="flex items-start justify-between gap-3 px-4 py-2.5 mb-4 rounded-lg bg-red-500/10 border border-red-500/20 text-red-400 text-xs">
            <div className="flex items-start gap-2 min-w-0">
              <AlertTriangle className="w-3.5 h-3.5 flex-shrink-0 mt-0.5" />
              <span className="break-words">{error}</span>
            </div>
            <button
              onClick={() => runTraversal()}
              className="btn-ghost text-[11px] text-red-300 hover:text-red-200 flex-shrink-0"
            >
              Retry
            </button>
          </div>
        )}

        {/* Canvas */}
        <div
          ref={canvasWrapRef}
          className="flex-1 glass-panel p-0 relative overflow-hidden min-h-[420px]"
        >
          {loading && (
            <div className="absolute inset-0 flex items-center justify-center z-10 bg-zinc-950/40 backdrop-blur-sm">
              <div className="flex items-center gap-2 text-xs text-zinc-400">
                <RefreshCw className="w-4 h-4 animate-spin" />
                Loading graph...
              </div>
            </div>
          )}

          {isInitial && (
            <div className="absolute inset-0 flex items-center justify-center">
              <div className="flex flex-col items-center gap-2 text-zinc-500">
                <Network className="w-10 h-10 opacity-40" />
                <p className="text-sm">Pick a start entity and hit Traverse.</p>
              </div>
            </div>
          )}

          {isEmpty && (
            <div className="absolute inset-0 flex items-center justify-center">
              <div className="flex flex-col items-center gap-2 text-zinc-500">
                <Network className="w-10 h-10 opacity-40" />
                <p className="text-sm">No nodes reachable from this start.</p>
                <p className="text-[11px] text-zinc-600">
                  Try increasing depth, switching direction, or clearing the relation filter.
                </p>
              </div>
            </div>
          )}

          {!isInitial && !isEmpty && (
            <ForceGraph2D<FGNode, GraphEdge>
              ref={fgRef}
              graphData={graphPayload}
              width={dims.width}
              height={dims.height}
              backgroundColor="rgba(0,0,0,0)"
              nodeId="id"
              nodeLabel={(n) => `${n.table}:${n.entityId} (depth ${n.depth})`}
              nodeColor={(n) => n.color}
              nodeRelSize={5}
              nodeCanvasObject={(node, ctx, globalScale) => {
                const n = node as NodeObject<FGNode>;
                const label = n.label ?? "";
                const radius = 5;
                ctx.beginPath();
                ctx.arc(n.x ?? 0, n.y ?? 0, radius, 0, 2 * Math.PI, false);
                ctx.fillStyle = n.color ?? "#a1a1aa";
                ctx.fill();
                ctx.lineWidth = 1 / globalScale;
                ctx.strokeStyle = "#09090b";
                ctx.stroke();

                if (globalScale > 1.2) {
                  const fontSize = 10 / globalScale;
                  ctx.font = `${fontSize}px ui-sans-serif, system-ui`;
                  ctx.textAlign = "center";
                  ctx.textBaseline = "top";
                  ctx.fillStyle = "#e4e4e7";
                  ctx.fillText(label, n.x ?? 0, (n.y ?? 0) + radius + 2);
                }
              }}
              linkColor={() => "rgba(161,161,170,0.35)"}
              linkWidth={1}
              linkDirectionalArrowLength={3}
              linkDirectionalArrowRelPos={1}
              linkLabel={(l) => (l as FGLink & { relation?: string }).relation ?? ""}
              onNodeClick={handleNodeClick}
              cooldownTicks={120}
            />
          )}
        </div>

        {/* Stats footer */}
        <div className="flex items-center gap-3 mt-3 text-[11px] text-zinc-500">
          <span>
            Nodes: <span className="text-zinc-300 font-mono">{nodeCount}</span>
          </span>
          <span>
            Edges: <span className="text-zinc-300 font-mono">{edgeCount}</span>
          </span>
          {data && (
            <>
              <span>
                Examined:{" "}
                <span className="text-zinc-300 font-mono">{data.edgesExamined}</span>
              </span>
              <span>
                Fetch: <span className="text-zinc-300 font-mono">{data.fetchMs}ms</span>
              </span>
              {data.truncated && (
                <Badge variant="amber" className="text-[9px]">
                  truncated
                </Badge>
              )}
            </>
          )}
        </div>
      </div>

      {/* Inspector sidebar ---------------------------------------------- */}
      <aside className="w-80 border-l border-zinc-800 bg-zinc-950 flex flex-col">
        <div className="flex items-center justify-between px-4 h-14 border-b border-zinc-800">
          <h3 className="text-sm font-semibold text-zinc-100">Inspector</h3>
          {selectedNode && (
            <button
              type="button"
              onClick={() => setSelectedNode(null)}
              className="btn-ghost text-xs"
              aria-label="Close inspector"
            >
              <X className="w-3.5 h-3.5" />
            </button>
          )}
        </div>

        <div className="flex-1 overflow-y-auto p-4 space-y-4">
          {!selectedNode && (
            <p className="text-xs text-zinc-500">
              Click a node in the graph to inspect it.
            </p>
          )}

          {selectedNode && (
            <>
              <div>
                <label className="text-[10px] uppercase tracking-wide text-zinc-500">
                  Entity type
                </label>
                <div className="flex items-center gap-2 mt-1">
                  <span
                    className="w-2.5 h-2.5 rounded-full flex-shrink-0"
                    style={{ backgroundColor: selectedNode.color }}
                  />
                  <Badge variant="zinc">{selectedNode.table}</Badge>
                </div>
              </div>

              <div>
                <label className="text-[10px] uppercase tracking-wide text-zinc-500">
                  Entity id
                </label>
                <div className="mt-1 font-mono text-xs text-zinc-200 break-all">
                  {selectedNode.entityId}
                </div>
              </div>

              <div>
                <label className="text-[10px] uppercase tracking-wide text-zinc-500">
                  Depth from start
                </label>
                <div className="mt-1 font-mono text-xs text-zinc-200">
                  {selectedNode.depth}
                </div>
              </div>

              <div className="pt-2 border-t border-zinc-800/60 space-y-2">
                <button
                  type="button"
                  onClick={handleExpand}
                  disabled={loading}
                  className="btn-ghost w-full justify-center bg-zinc-900 border border-zinc-800 hover:border-amber-500/30 text-xs h-8 disabled:opacity-40 disabled:cursor-not-allowed"
                >
                  <Maximize2 className="w-3.5 h-3.5" />
                  Expand neighbours
                </button>
                <button
                  type="button"
                  onClick={() => {
                    const p = parseRecordId(selectedNode.id);
                    if (!p) return;
                    setStartTable(p.table);
                    setStartId(p.entityId);
                    runTraversal({ start: selectedNode.id });
                  }}
                  disabled={loading}
                  className="btn-ghost w-full justify-center bg-zinc-900 border border-zinc-800 hover:border-amber-500/30 text-xs h-8 disabled:opacity-40 disabled:cursor-not-allowed"
                >
                  <Network className="w-3.5 h-3.5" />
                  Re-root traversal here
                </button>
              </div>
            </>
          )}
        </div>
      </aside>
    </div>
  );
}
