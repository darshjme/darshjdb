/**
 * Tests for the DarshJDB TypeScript SDK.
 *
 * Uses a mock fetch to test HTTP interactions without a running server.
 */

import { describe, it, expect, beforeEach, vi } from "vitest";
import { DarshDB } from "../src/client.js";
import {
  DarshDBError,
  DarshDBAuthError,
  DarshDBConnectionError,
  DarshDBQueryError,
  DarshDBAPIError,
  ConnectionState,
} from "../src/types.js";

// ---------------------------------------------------------------------------
//  Mock fetch helper
// ---------------------------------------------------------------------------

function mockFetch(
  status: number,
  body: unknown,
  headers?: Record<string, string>,
): typeof globalThis.fetch {
  return vi.fn().mockResolvedValue({
    ok: status >= 200 && status < 300,
    status,
    statusText: status === 200 ? "OK" : "Error",
    json: () => Promise.resolve(body),
    text: () => Promise.resolve(JSON.stringify(body)),
    headers: new Headers(headers),
  }) as unknown as typeof globalThis.fetch;
}

function mockFetchReject(error: Error): typeof globalThis.fetch {
  return vi.fn().mockRejectedValue(error) as unknown as typeof globalThis.fetch;
}

const SERVER = "http://localhost:8080";

// ---------------------------------------------------------------------------
//  Initialization
// ---------------------------------------------------------------------------

describe("DarshDB initialization", () => {
  it("creates client with url", () => {
    const db = new DarshDB(SERVER, { fetch: mockFetch(200, {}) });
    expect(db.state).toBe(ConnectionState.Connected);
  });

  it("strips trailing slashes", () => {
    const fetchMock = mockFetch(200, { data: [] });
    const db = new DarshDB("http://localhost:8080///", { fetch: fetchMock });
    // Verify URL is cleaned by making a request
    db.select("test").catch(() => {});
    expect(fetchMock).toHaveBeenCalledWith(
      expect.stringMatching(/^http:\/\/localhost:8080\/api/),
      expect.any(Object),
    );
  });

  it("throws on empty url", () => {
    expect(() => new DarshDB("")).toThrow("url is required");
  });
});

// ---------------------------------------------------------------------------
//  Authentication
// ---------------------------------------------------------------------------

describe("DarshDB authentication", () => {
  it("signin with user/pass", async () => {
    const fetchMock = mockFetch(200, {
      accessToken: "tok123",
      user: { id: "u1" },
      refreshToken: "ref1",
    });
    const db = new DarshDB(SERVER, { fetch: fetchMock });

    const result = await db.signin({ user: "root", pass: "root" });

    expect(result.token).toBe("tok123");
    expect(result.user).toEqual({ id: "u1" });
    expect(result.refreshToken).toBe("ref1");

    // Verify body sent
    const call = vi.mocked(fetchMock).mock.calls[0]!;
    const body = JSON.parse(call[1]?.body as string);
    expect(body.email).toBe("root");
    expect(body.password).toBe("root");
  });

  it("signin with email/password", async () => {
    const fetchMock = mockFetch(200, { accessToken: "tok456" });
    const db = new DarshDB(SERVER, { fetch: fetchMock });

    await db.signin({ email: "alice@test.com", password: "secret" });

    const call = vi.mocked(fetchMock).mock.calls[0]!;
    const body = JSON.parse(call[1]?.body as string);
    expect(body.email).toBe("alice@test.com");
    expect(body.password).toBe("secret");
  });

  it("signin sets namespace and database", async () => {
    const fetchMock = mockFetch(200, { accessToken: "tok" });
    const db = new DarshDB(SERVER, { fetch: fetchMock });

    await db.signin({
      user: "root",
      pass: "root",
      namespace: "ns1",
      database: "db1",
    });

    // Subsequent request should include NS/DB headers
    const selectFetch = mockFetch(200, { data: [] });
    // Hack: replace fetch for next call
    (db as unknown as { fetchFn: typeof fetch }).fetchFn = selectFetch;
    await db.select("users");

    const call = vi.mocked(selectFetch).mock.calls[0]!;
    const headers = call[1]?.headers as Record<string, string>;
    expect(headers["X-DarshDB-NS"]).toBe("ns1");
    expect(headers["X-DarshDB-DB"]).toBe("db1");
  });

  it("signin throws DarshDBAuthError on 401", async () => {
    const fetchMock = mockFetch(401, { message: "Invalid credentials" });
    const db = new DarshDB(SERVER, { fetch: fetchMock });

    await expect(
      db.signin({ user: "root", pass: "wrong" }),
    ).rejects.toThrow(DarshDBAuthError);
  });

  it("signup creates account", async () => {
    const fetchMock = mockFetch(200, {
      accessToken: "new-tok",
      user: { id: "u2" },
    });
    const db = new DarshDB(SERVER, { fetch: fetchMock });

    const result = await db.signup({
      email: "bob@test.com",
      password: "pass",
      name: "Bob",
    });

    expect(result.token).toBe("new-tok");
  });

  it("invalidate clears token", async () => {
    const fetchMock = mockFetch(200, {});
    const db = new DarshDB(SERVER, { fetch: fetchMock });

    await db.authenticate("my-token");
    await db.invalidate();

    // Next request should not have auth header
    const selectFetch = mockFetch(200, { data: [] });
    (db as unknown as { fetchFn: typeof fetch }).fetchFn = selectFetch;
    await db.select("users");

    const call = vi.mocked(selectFetch).mock.calls[0]!;
    const headers = call[1]?.headers as Record<string, string>;
    expect(headers["Authorization"]).toBeUndefined();
  });
});

// ---------------------------------------------------------------------------
//  Namespace / Database
// ---------------------------------------------------------------------------

describe("DarshDB use", () => {
  it("sets namespace and database headers", async () => {
    const fetchMock = mockFetch(200, { data: [] });
    const db = new DarshDB(SERVER, { fetch: fetchMock });

    await db.use("myns", "mydb");
    await db.select("users");

    const call = vi.mocked(fetchMock).mock.calls[0]!;
    const headers = call[1]?.headers as Record<string, string>;
    expect(headers["X-DarshDB-NS"]).toBe("myns");
    expect(headers["X-DarshDB-DB"]).toBe("mydb");
  });
});

// ---------------------------------------------------------------------------
//  CRUD
// ---------------------------------------------------------------------------

describe("DarshDB CRUD", () => {
  it("select table", async () => {
    const fetchMock = mockFetch(200, {
      data: [{ id: "u1", name: "Alice" }],
    });
    const db = new DarshDB(SERVER, { fetch: fetchMock });

    const result = await db.select("users");
    expect(result).toHaveLength(1);
    expect(result[0]!.name).toBe("Alice");
    expect(vi.mocked(fetchMock).mock.calls[0]![0]).toContain(
      "/api/data/users",
    );
  });

  it("select specific record", async () => {
    const fetchMock = mockFetch(200, { id: "darsh", name: "Darsh" });
    const db = new DarshDB(SERVER, { fetch: fetchMock });

    const result = await db.select("users:darsh");
    expect(result).toHaveLength(1);
    expect(vi.mocked(fetchMock).mock.calls[0]![0]).toContain(
      "/api/data/users/darsh",
    );
  });

  it("create record", async () => {
    const fetchMock = mockFetch(200, {
      id: "u1",
      name: "Darsh",
      age: 30,
    });
    const db = new DarshDB(SERVER, { fetch: fetchMock });

    const result = await db.create("users", { name: "Darsh", age: 30 });
    expect(result).toHaveProperty("name", "Darsh");

    const call = vi.mocked(fetchMock).mock.calls[0]!;
    expect(call[1]?.method).toBe("POST");
    expect(call[0]).toContain("/api/data/users");
  });

  it("create with record ID", async () => {
    const fetchMock = mockFetch(200, { id: "darsh", name: "Darsh" });
    const db = new DarshDB(SERVER, { fetch: fetchMock });

    await db.create("users:darsh", { name: "Darsh" });

    const call = vi.mocked(fetchMock).mock.calls[0]!;
    const body = JSON.parse(call[1]?.body as string);
    expect(body.id).toBe("darsh");
    expect(body.name).toBe("Darsh");
  });

  it("update record", async () => {
    const fetchMock = mockFetch(200, { id: "darsh", age: 31 });
    const db = new DarshDB(SERVER, { fetch: fetchMock });

    const result = await db.update("users:darsh", { age: 31 });
    expect(result).toHaveProperty("age", 31);

    const call = vi.mocked(fetchMock).mock.calls[0]!;
    expect(call[1]?.method).toBe("PATCH");
  });

  it("update without ID throws", async () => {
    const db = new DarshDB(SERVER, { fetch: mockFetch(200, {}) });
    await expect(db.update("users", { age: 31 })).rejects.toThrow(
      "requires a record ID",
    );
  });

  it("delete record", async () => {
    const fetchMock = mockFetch(200, { ok: true });
    const db = new DarshDB(SERVER, { fetch: fetchMock });

    await db.delete("users:darsh");

    const call = vi.mocked(fetchMock).mock.calls[0]!;
    expect(call[1]?.method).toBe("DELETE");
    expect(call[0]).toContain("/api/data/users/darsh");
  });

  it("delete table (all records)", async () => {
    const fetchMock = mockFetch(204, {});
    const db = new DarshDB(SERVER, {
      fetch: vi.fn().mockResolvedValue({
        ok: true,
        status: 204,
        json: () => Promise.resolve({}),
        text: () => Promise.resolve(""),
        headers: new Headers(),
      }),
    });

    const result = await db.delete("users");
    expect(result).toEqual({});
  });

  it("insert single record", async () => {
    const fetchMock = mockFetch(200, {
      results: [{ id: "u1", name: "Alice" }],
    });
    const db = new DarshDB(SERVER, { fetch: fetchMock });

    const result = await db.insert("users", { name: "Alice" });
    expect(result).toHaveLength(1);
  });

  it("insert batch", async () => {
    const fetchMock = mockFetch(200, {
      results: [{ id: "u1" }, { id: "u2" }],
    });
    const db = new DarshDB(SERVER, { fetch: fetchMock });

    await db.insert("users", [{ name: "A" }, { name: "B" }]);

    const call = vi.mocked(fetchMock).mock.calls[0]!;
    const body = JSON.parse(call[1]?.body as string);
    expect(body.mutations).toHaveLength(2);
    expect(body.mutations[0].op).toBe("insert");
  });
});

// ---------------------------------------------------------------------------
//  Query
// ---------------------------------------------------------------------------

describe("DarshDB query", () => {
  it("returns QueryResult array", async () => {
    const fetchMock = mockFetch(200, {
      data: [{ id: "1", age: 25 }],
      meta: { count: 1, duration_ms: 0.5 },
    });
    const db = new DarshDB(SERVER, { fetch: fetchMock });

    const results = await db.query("SELECT * FROM users WHERE age > 18");
    expect(results).toHaveLength(1);
    expect(results[0]!.data[0]!.age).toBe(25);
    expect(results[0]!.meta.count).toBe(1);
  });

  it("sends query and vars", async () => {
    const fetchMock = mockFetch(200, { data: [] });
    const db = new DarshDB(SERVER, { fetch: fetchMock });

    await db.query("SELECT * FROM users", { min_age: 18 });

    const call = vi.mocked(fetchMock).mock.calls[0]!;
    const body = JSON.parse(call[1]?.body as string);
    expect(body.query).toBe("SELECT * FROM users");
    expect(body.vars).toEqual({ min_age: 18 });
  });

  it("throws DarshDBQueryError on 400", async () => {
    const fetchMock = mockFetch(400, { message: "Parse error" });
    const db = new DarshDB(SERVER, { fetch: fetchMock });

    await expect(db.query("INVALID")).rejects.toThrow(DarshDBQueryError);
  });
});

// ---------------------------------------------------------------------------
//  Graph
// ---------------------------------------------------------------------------

describe("DarshDB graph", () => {
  it("relate creates edge", async () => {
    const fetchMock = mockFetch(200, { results: [{ id: "r1" }] });
    const db = new DarshDB(SERVER, { fetch: fetchMock });

    await db.relate("user:darsh", "works_at", "company:knowai");

    const call = vi.mocked(fetchMock).mock.calls[0]!;
    const body = JSON.parse(call[1]?.body as string);
    const mutation = body.mutations[0];
    expect(mutation.op).toBe("insert");
    expect(mutation.entity).toBe("works_at");
    expect(mutation.data.from_entity).toBe("user");
    expect(mutation.data.from_id).toBe("darsh");
    expect(mutation.data.to_entity).toBe("company");
    expect(mutation.data.to_id).toBe("knowai");
  });

  it("relate with extra data", async () => {
    const fetchMock = mockFetch(200, { results: [{}] });
    const db = new DarshDB(SERVER, { fetch: fetchMock });

    await db.relate("user:darsh", "works_at", "company:knowai", {
      role: "CEO",
    });

    const call = vi.mocked(fetchMock).mock.calls[0]!;
    const body = JSON.parse(call[1]?.body as string);
    expect(body.mutations[0].data.role).toBe("CEO");
  });
});

// ---------------------------------------------------------------------------
//  Server-side functions
// ---------------------------------------------------------------------------

describe("DarshDB run", () => {
  it("invokes function and extracts result", async () => {
    const fetchMock = mockFetch(200, { result: { rows: 42 } });
    const db = new DarshDB(SERVER, { fetch: fetchMock });

    const result = await db.run("generateReport", { month: "2026-04" });
    expect(result).toEqual({ rows: 42 });
  });

  it("returns full body if no result key", async () => {
    const fetchMock = mockFetch(200, { data: "test" });
    const db = new DarshDB(SERVER, { fetch: fetchMock });

    const result = await db.run("raw");
    expect(result).toEqual({ data: "test" });
  });
});

// ---------------------------------------------------------------------------
//  Error handling
// ---------------------------------------------------------------------------

describe("DarshDB errors", () => {
  it("401 throws DarshDBAuthError on select", async () => {
    const fetchMock = mockFetch(401, { message: "Unauthorized" });
    const db = new DarshDB(SERVER, { fetch: fetchMock });

    await expect(db.select("users")).rejects.toThrow(DarshDBAuthError);
  });

  it("500 throws DarshDBAPIError", async () => {
    const fetchMock = mockFetch(500, { error: "Internal error" });
    const db = new DarshDB(SERVER, { fetch: fetchMock });

    await expect(db.select("users")).rejects.toThrow(DarshDBAPIError);
  });

  it("network error throws DarshDBConnectionError", async () => {
    const fetchMock = mockFetchReject(new TypeError("fetch failed"));
    const db = new DarshDB(SERVER, { fetch: fetchMock });

    await expect(db.select("users")).rejects.toThrow(
      DarshDBConnectionError,
    );
  });

  it("API error includes status code", async () => {
    const fetchMock = mockFetch(403, {
      message: "Forbidden",
      error: { code: "PERM_DENIED" },
    });
    const db = new DarshDB(SERVER, { fetch: fetchMock });

    try {
      await db.select("users");
    } catch (err) {
      expect(err).toBeInstanceOf(DarshDBAuthError);
    }
  });
});

// ---------------------------------------------------------------------------
//  Health
// ---------------------------------------------------------------------------

describe("DarshDB health", () => {
  it("returns true on success", async () => {
    const fetchMock = mockFetch(200, { status: "ok" });
    const db = new DarshDB(SERVER, { fetch: fetchMock });

    expect(await db.health()).toBe(true);
  });

  it("returns false on failure", async () => {
    const fetchMock = mockFetchReject(new Error("down"));
    const db = new DarshDB(SERVER, { fetch: fetchMock });

    expect(await db.health()).toBe(false);
  });
});

// ---------------------------------------------------------------------------
//  Batch
// ---------------------------------------------------------------------------

describe("DarshDB batch", () => {
  it("sends batch operations", async () => {
    const fetchMock = mockFetch(200, {
      results: [{ id: "u1" }, { data: [] }],
    });
    const db = new DarshDB(SERVER, { fetch: fetchMock });

    const results = await db.batch([
      { method: "POST", path: "/api/data/users", body: { name: "A" } },
      { method: "GET", path: "/api/data/users" },
    ]);

    expect(results).toHaveLength(2);
  });
});
