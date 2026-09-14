import { useEffect, useState, type ReactNode, type FormEvent } from "react";
import { Database, ArrowRight, Loader2 } from "lucide-react";
import { AUTH_CHANGED, API_URL, getToken, signIn, verifyAdmin } from "../lib/http";

export function AuthGate({ children }: { children: ReactNode }) {
  const [ready, setReady] = useState(false);
  const [checking, setChecking] = useState(Boolean(getToken()));
  const [error, setError] = useState("");
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    let cancelled = false;
    let generation = 0;
    const check = async () => {
      const current = ++generation;
      if (!getToken()) { setReady(false); setChecking(false); return; }
      setChecking(true);
      try { await verifyAdmin(); if (!cancelled && current === generation) { setReady(true); setError(""); } }
      catch (e) { if (!cancelled && current === generation) { setReady(false); setError(e instanceof Error ? e.message : "Could not verify this session."); } }
      finally { if (!cancelled && current === generation) setChecking(false); }
    };
    void check(); window.addEventListener(AUTH_CHANGED, check);
    return () => { cancelled = true; window.removeEventListener(AUTH_CHANGED, check); };
  }, []);
  async function submit(event: FormEvent) {
    event.preventDefault(); setBusy(true); setError("");
    try { await signIn(email, password); setPassword(""); }
    catch (e) { setError(e instanceof Error ? e.message : "Unable to sign in."); }
    finally { setBusy(false); }
  }
  if (ready) return children;
  return <main className="min-h-screen bg-surface text-ink flex items-center justify-center p-6">
    <section className="w-full max-w-md">
      <div className="flex items-center gap-3 mb-10"><Database className="text-brand-400" /><span className="font-semibold tracking-tight">DarshJDB</span><span className="text-ink-muted text-sm">/ Console</span></div>
      <p className="text-xs uppercase tracking-widest text-brand-400 mb-3">Your data, under your control</p>
      <h1 className="text-3xl font-semibold tracking-tight mb-3">Sign in to your workspace</h1>
      <p className="text-ink-secondary text-sm mb-8">Use an administrator account on this server to explore data and manage your database.</p>
      {checking ? <p role="status" className="flex items-center gap-2"><Loader2 className="animate-spin w-4 h-4" />Checking your session…</p> : <form onSubmit={submit} className="space-y-5">
        <div><label htmlFor="email" className="block text-sm mb-2">Email</label><input id="email" className="input-field" type="email" autoComplete="username" required value={email} onChange={e => setEmail(e.target.value)} /></div>
        <div><label htmlFor="password" className="block text-sm mb-2">Password</label><input id="password" className="input-field" type="password" autoComplete="current-password" required value={password} onChange={e => setPassword(e.target.value)} /></div>
        {error && <p role="alert" className="text-sm text-red-700 bg-red-500/10 border border-red-500/20 rounded-lg p-3 break-words">{error}</p>}
        <button disabled={busy} className="btn-primary w-full justify-center" type="submit">{busy ? "Signing in…" : "Sign in"}<ArrowRight className="w-4 h-4" /></button>
      </form>}
      <p className="mt-8 pt-5 border-t border-line text-xs text-ink-muted break-all">Server: {API_URL || window.location.origin}</p>
    </section>
  </main>;
}
