import { useState, useEffect, useCallback } from "react";
import { Routes, Route, useLocation } from "react-router-dom";
import { Sidebar } from "./components/Sidebar";
import { TopBar } from "./components/TopBar";
import { CommandPalette } from "./components/CommandPalette";
import { DataExplorer } from "./pages/DataExplorer";
import { Schema } from "./pages/Schema";
import { Graph } from "./pages/Graph";
import { Functions } from "./pages/Functions";
import { AuthUsers } from "./pages/AuthUsers";
import { Storage } from "./pages/Storage";
import { Logs } from "./pages/Logs";
import { Settings } from "./pages/Settings";

const pageTitles: Record<string, string> = {
  "/": "Data Explorer",
  "/schema": "Schema",
  "/graph": "Graph Explorer",
  "/functions": "Functions",
  "/auth": "Auth & Users",
  "/storage": "Storage",
  "/logs": "Logs",
  "/settings": "Settings",
};

export function App() {
  const [cmdPaletteOpen, setCmdPaletteOpen] = useState(false);
  const location = useLocation();
  const title = pageTitles[location.pathname] || "DarshJDB";

  const handleKeyDown = useCallback((e: KeyboardEvent) => {
    if ((e.metaKey || e.ctrlKey) && e.key === "k") {
      e.preventDefault();
      setCmdPaletteOpen((prev) => !prev);
    }
  }, []);

  useEffect(() => {
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [handleKeyDown]);

  return (
    <div className="flex h-screen overflow-hidden bg-zinc-950">
      <Sidebar />
      <div className="flex flex-col flex-1 min-w-0">
        <TopBar
          title={title}
          onOpenCommandPalette={() => setCmdPaletteOpen(true)}
        />
        <main className="flex-1 overflow-y-auto">
          <Routes>
            <Route path="/" element={<DataExplorer />} />
            <Route path="/schema" element={<Schema />} />
            <Route path="/graph" element={<Graph />} />
            <Route path="/functions" element={<Functions />} />
            <Route path="/auth" element={<AuthUsers />} />
            <Route path="/storage" element={<Storage />} />
            <Route path="/logs" element={<Logs />} />
            <Route path="/settings" element={<Settings />} />
          </Routes>
        </main>
      </div>
      <CommandPalette
        open={cmdPaletteOpen}
        onClose={() => setCmdPaletteOpen(false)}
      />
    </div>
  );
}
