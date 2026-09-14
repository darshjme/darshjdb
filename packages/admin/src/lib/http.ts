/** Same-origin API transport; credentials are entered at runtime, never bundled. */
export const API_URL = (import.meta.env.VITE_DDB_URL || "").replace(/\/$/, "");
const SESSION_KEY = "darshjdb.admin.session";
export const AUTH_CHANGED = "ddb-auth-changed";
export function getToken(): string { return sessionStorage.getItem(SESSION_KEY) || ""; }
export function setToken(token: string) {
  if (token) sessionStorage.setItem(SESSION_KEY, token);
  else sessionStorage.removeItem(SESSION_KEY);
  window.dispatchEvent(new Event(AUTH_CHANGED));
}
export class ApiError extends Error {
  constructor(public status: number, public body: string, public path: string) {
    super(`API ${status} on ${path}: ${body}`); this.name = "ApiError";
  }
}
export async function apiFetch<T>(path: string, init?: RequestInit, token = getToken()): Promise<T> {
  const headers = new Headers(init?.headers);
  headers.set("Accept", "application/json");
  if (init?.body && !(init.body instanceof FormData)) headers.set("Content-Type", "application/json");
  if (token) headers.set("Authorization", `Bearer ${token}`);
  const res = await fetch(`${API_URL}${path}`, { ...init, headers });
  if (!res.ok) {
    // An old request must not invalidate a newly signed-in session.
    if (res.status === 401 && token && token === getToken()) setToken("");
    const body = await res.text().catch(() => res.statusText);
    let message = body || res.statusText;
    try { const parsed = JSON.parse(body); message = parsed.error?.message || parsed.message || message; } catch { /* Plain-text server error. */ }
    throw new ApiError(res.status, message, path);
  }
  if (res.status === 204) return undefined as T;
  return res.json() as Promise<T>;
}
export interface AdminUser { user_id: string; email: string; roles: string[] }
export async function verifyAdmin(token = getToken()): Promise<AdminUser> {
  const user = await apiFetch<AdminUser>("/api/auth/me", undefined, token);
  if (!user.roles.includes("admin")) throw new Error("This account does not have the admin role. Ask your server administrator for access.");
  return user;
}
export async function signIn(email: string, password: string): Promise<AdminUser> {
  const pair = await apiFetch<{ access_token: string }>("/api/auth/signin", {
    method: "POST", body: JSON.stringify({ email, password }),
  }, "");
  const user = await verifyAdmin(pair.access_token);
  setToken(pair.access_token);
  return user;
}
export async function signOut() {
  try { await apiFetch("/api/auth/signout", { method: "POST" }); }
  finally { setToken(""); }
}
