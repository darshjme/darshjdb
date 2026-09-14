import { ScrollText } from "lucide-react";
export function Logs() {
  return <section className="p-6 max-w-3xl text-ink-secondary"><ScrollText className="text-brand-400 mb-4" /><h2 className="text-lg font-semibold text-ink mb-3">Server logs</h2><p className="text-sm mb-5">This version writes structured logs to the server process. A dashboard log-stream endpoint is not available.</p><div className="glass-panel p-5"><p className="text-sm mb-3">For the Compose deployment, follow the live stream on the server:</p><code className="text-brand-300 text-sm">docker compose logs --follow darshjdb</code><p className="text-xs text-ink-muted mt-4">For a direct binary installation, use its terminal output or your service manager’s journal.</p></div></section>;
}
