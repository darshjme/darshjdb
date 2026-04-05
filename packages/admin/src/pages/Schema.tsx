import { useState, useEffect } from "react";
import { Eye, GitBranch, List, ArrowRight, Key, Hash, AlertTriangle } from "lucide-react";
import { Badge } from "../components/Badge";
import { mockEntityTypes } from "../lib/mock-data";
import { fetchSchema } from "../lib/api";
import { cn } from "../lib/utils";
import type { EntityType } from "../types";

const relationships = [
  { from: "documents", to: "users", field: "authorId", type: "many-to-one" },
  { from: "messages", to: "channels", field: "channelId", type: "many-to-one" },
  { from: "messages", to: "users", field: "authorId", type: "many-to-one" },
  { from: "sessions", to: "users", field: "userId", type: "many-to-one" },
  { from: "files", to: "users", field: "uploadedBy", type: "many-to-one" },
];

export function Schema() {
  const [entityTypes, setEntityTypes] = useState<EntityType[]>(mockEntityTypes);
  const [selectedEntity, setSelectedEntity] = useState<EntityType | null>(null);
  const [view, setView] = useState<"diagram" | "list">("diagram");
  const [usingMock, setUsingMock] = useState(false);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const types = await fetchSchema();
        if (cancelled) return;
        if (types.length > 0) {
          setEntityTypes(types);
          setUsingMock(false);
        } else {
          setUsingMock(true);
        }
      } catch {
        if (cancelled) return;
        setEntityTypes(mockEntityTypes);
        setUsingMock(true);
      }
    })();
    return () => { cancelled = true; };
  }, []);

  return (
    <div className="flex h-full">
      {/* Schema visualization */}
      <div className="flex-1 overflow-auto p-6">
        <div className="flex items-center justify-between mb-6">
          <div>
            <h2 className="text-lg font-semibold text-zinc-100">Schema</h2>
            <p className="text-sm text-zinc-500 mt-0.5">
              {entityTypes.length} entity types, {relationships.length} relationships
            </p>
          </div>
          <div className="flex items-center gap-1 bg-zinc-900 rounded-lg p-0.5 border border-zinc-800">
            <button
              onClick={() => setView("diagram")}
              className={cn(
                "px-3 py-1.5 rounded-md text-xs font-medium transition-colors",
                view === "diagram"
                  ? "bg-zinc-800 text-zinc-100"
                  : "text-zinc-500 hover:text-zinc-300",
              )}
            >
              <GitBranch className="w-3.5 h-3.5 inline mr-1.5" />
              Diagram
            </button>
            <button
              onClick={() => setView("list")}
              className={cn(
                "px-3 py-1.5 rounded-md text-xs font-medium transition-colors",
                view === "list"
                  ? "bg-zinc-800 text-zinc-100"
                  : "text-zinc-500 hover:text-zinc-300",
              )}
            >
              <List className="w-3.5 h-3.5 inline mr-1.5" />
              List
            </button>
          </div>
        </div>

        {usingMock && (
          <div className="flex items-center gap-2 px-4 py-2 mb-4 rounded-lg bg-amber-500/10 border border-amber-500/20 text-amber-400 text-xs">
            <AlertTriangle className="w-3.5 h-3.5" />
            <span>Server unreachable -- showing mock schema</span>
          </div>
        )}

        {view === "diagram" ? (
          <div className="grid grid-cols-3 gap-4">
            {entityTypes.map((entity) => (
              <button
                key={entity.name}
                onClick={() => setSelectedEntity(entity)}
                className={cn(
                  "glass-panel p-0 text-left transition-all hover:border-zinc-700",
                  selectedEntity?.name === entity.name && "border-amber-500/50 ring-1 ring-amber-500/20",
                )}
              >
                <div className="flex items-center justify-between px-4 py-3 border-b border-zinc-800/60">
                  <span className="text-sm font-semibold text-zinc-100">
                    {entity.name}
                  </span>
                  <Badge variant="zinc" className="text-[10px]">
                    {entity.count}
                  </Badge>
                </div>
                <div className="px-4 py-2 space-y-0.5">
                  {entity.fields.map((field) => (
                    <div
                      key={field.name}
                      className="flex items-center gap-2 py-1 text-xs"
                    >
                      <div className="flex items-center gap-1 w-5">
                        {field.unique && field.name === "_id" && (
                          <Key className="w-3 h-3 text-amber-500" />
                        )}
                        {field.indexed && field.name !== "_id" && (
                          <Hash className="w-3 h-3 text-sky-500/60" />
                        )}
                      </div>
                      <span className={cn(
                        "font-mono",
                        field.name === "_id" ? "text-amber-500/80" : "text-zinc-300",
                      )}>
                        {field.name}
                      </span>
                      <span className="text-zinc-600 ml-auto font-mono">
                        {field.type}
                      </span>
                    </div>
                  ))}
                </div>

                {/* Show relationships */}
                {relationships
                  .filter((r) => r.from === entity.name)
                  .map((rel) => (
                    <div
                      key={`${rel.from}-${rel.to}`}
                      className="flex items-center gap-2 px-4 py-1.5 text-[10px] text-zinc-500 border-t border-zinc-800/40"
                    >
                      <ArrowRight className="w-3 h-3" />
                      <span>{rel.field}</span>
                      <span className="text-zinc-600">-&gt;</span>
                      <span className="text-sky-400">{rel.to}</span>
                      <Badge variant="zinc" className="ml-auto text-[9px] py-0">
                        {rel.type}
                      </Badge>
                    </div>
                  ))}
              </button>
            ))}
          </div>
        ) : (
          <div className="space-y-3">
            {entityTypes.map((entity) => (
              <div key={entity.name} className="glass-panel">
                <div className="flex items-center justify-between px-4 py-3 border-b border-zinc-800/60">
                  <div className="flex items-center gap-3">
                    <span className="text-sm font-semibold text-zinc-100">{entity.name}</span>
                    <Badge variant="zinc">{entity.fields.length} fields</Badge>
                    <Badge variant="amber">{entity.count} rows</Badge>
                  </div>
                  <button
                    onClick={() => setSelectedEntity(selectedEntity?.name === entity.name ? null : entity)}
                    className="btn-ghost text-xs"
                  >
                    <Eye className="w-3.5 h-3.5" />
                    {selectedEntity?.name === entity.name ? "Hide" : "Details"}
                  </button>
                </div>
                {selectedEntity?.name === entity.name && (
                  <div className="px-4 py-3">
                    <table className="w-full">
                      <thead>
                        <tr>
                          <th className="table-header">Field</th>
                          <th className="table-header">Type</th>
                          <th className="table-header">Required</th>
                          <th className="table-header">Indexed</th>
                          <th className="table-header">Unique</th>
                          <th className="table-header">Default</th>
                        </tr>
                      </thead>
                      <tbody>
                        {entity.fields.map((field) => (
                          <tr key={field.name}>
                            <td className="table-cell font-mono text-xs">{field.name}</td>
                            <td className="table-cell">
                              <Badge variant="sky" className="text-[10px]">{field.type}</Badge>
                            </td>
                            <td className="table-cell">
                              {field.required ? (
                                <span className="text-emerald-400 text-xs">Yes</span>
                              ) : (
                                <span className="text-zinc-600 text-xs">No</span>
                              )}
                            </td>
                            <td className="table-cell">
                              {field.indexed && <Hash className="w-3.5 h-3.5 text-sky-400" />}
                            </td>
                            <td className="table-cell">
                              {field.unique && <Key className="w-3.5 h-3.5 text-amber-500" />}
                            </td>
                            <td className="table-cell font-mono text-xs text-zinc-500">
                              {field.default || "--"}
                            </td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                )}
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
