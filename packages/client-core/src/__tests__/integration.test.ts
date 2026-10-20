/** Live authenticated HTTP, SSE and WebSocket regression tests.
 * Run against a disposable normal-auth server: DDB_URL=http://127.0.0.1:7700 npm test.
 * Idle revocation cases also require DDB_TEST_DATABASE_URL and psql (disposable DB only).
 * No endpoint/auth failure is treated as a passing test. Fixtures use unique users.
 */
import { describe, it, expect, beforeAll, afterAll, vi } from 'vitest';
import { execFileSync } from 'node:child_process';
import { DarshJDB } from '../client.js';
import { AuthClient } from '../auth.js';
import { TransactionBuilder, generateId, submitOps } from '../transaction.js';
import { subscribe, queryOnce, SubscriptionEndedError } from '../query.js';
import type { TokenStorage, QueryResult } from '../types.js';
const URL = process.env.DDB_URL;
const live = URL ? describe : describe.skip;
class MemoryTokenStorage implements TokenStorage {
  private values = new Map<string, string>();
  get(key: string) { return this.values.get(key) ?? null; }
  set(key: string, value: string) { this.values.set(key, value); }
  remove(key: string) { this.values.delete(key); }
}
async function json(response: Response) {
  expect(response.ok, `HTTP ${response.status}: ${await response.clone().text()}`).toBe(true);
  return response.json();
}
async function until(predicate: () => boolean, timeoutMs = 7000) {
  const end = Date.now() + timeoutMs;
  while (!predicate() && Date.now() < end) await new Promise(r => setTimeout(r, 20));
  expect(predicate()).toBe(true);
}
// HTTP-to-database round trips may cross an SSH tunnel in release validation.
live('DarshJDB Client Integration', { timeout: 15000 }, () => {
  let db: DarshJDB;
  let a: { user_id: string; access_token: string };
  let b: typeof a;
  const collection = `live_${Date.now()}`;
  const idA = generateId(), idB = generateId();
  const password = 'DisposableLiveTest123!';
  const email = `live-a-${Date.now()}@darshjdb.test`;
  const clients: DarshJDB[] = [];
  const request = (path: string, body?: unknown, token = a?.access_token) => fetch(`${URL}${path}`, {
    method: body === undefined ? 'GET' : 'POST',
    headers: { 'Content-Type': 'application/json', ...(token ? { Authorization: `Bearer ${token}` } : {}) },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  });
  const makeClient = async (token: string, transport: 'rest' | 'ws' = 'rest') => {
    const client = new DarshJDB({ serverUrl: URL!, appId: 'live-test', transport });
    clients.push(client); client.setAuthToken(token); await client.connect(); return client;
  };
  beforeAll(async () => {
    a = await json(await request('/api/auth/signup', { email, password }, ''));
    b = await json(await request('/api/auth/signup', { email: `live-b-${Date.now()}@darshjdb.test`, password }, ''));
    db = await makeClient(a.access_token);
    for (const [identity, id, label] of [[a, idA, 'alice'], [b, idB, 'bob']] as const) {
      await json(await request('/api/mutate', { mutations: [{ op: 'insert', entity: collection, id, data: { label, owner_id: identity.user_id } }] }, identity.access_token));
    }
  }, 20000);
  afterAll(() => clients.forEach(client => client.disconnect()));
  it('health endpoint returns ok', async () => expect(await json(await request('/health'))).toMatchObject({ status: 'ok', version: expect.any(String) }));
  it('REST client connects', () => expect(db.state).toBe('connected'));
  it('builds API URLs', () => expect(db.getRestUrl('/query')).toBe(`${URL}/api/query`));
  it('serves OpenAPI', async () => expect(await json(await request('/api/openapi.json'))).toMatchObject({ openapi: expect.any(String), paths: expect.any(Object) }));
  it('creates an owned entity through data API', async () => {
    const result = await json(await request(`/api/data/${collection}`, { label: 'data-api', owner_id: a.user_id }));
    expect(result).toBeDefined();
  });
  it('lists only owned entities through data API', async () => {
    const result = await json(await request(`/api/data/${collection}`));
    expect(JSON.stringify(result)).toContain('alice'); expect(JSON.stringify(result)).not.toContain('bob');
  });
  it('queries exact owned data', async () => {
    const result = await db.rest.query({ collection, where: [{ field: 'id', op: '=', value: idA }] });
    expect(result.data).toEqual([expect.objectContaining({ id: idA, label: 'alice' })]);
  });
  it('submits SDK transactions and reads their values', async () => {
    const id = generateId();
    await db.rest.transact([{ kind: 'set', entity: collection, id, data: { label: 'sdk-write', owner_id: a.user_id } }]);
    expect((await db.rest.query({ collection, where: [{ field: 'id', op: '=', value: id }] })).data).toEqual([expect.objectContaining({ label: 'sdk-write' })]);
  });
  it('signs in the same registered user', async () => {
    const result = await json(await request('/api/auth/signin', { email, password }, ''));
    expect(result.user_id).toBe(a.user_id); expect(result.access_token).toBeTruthy();
    a = result; db.setAuthToken(a.access_token); await db.connect();
  }, 10000);
  it('AuthClient signup and signout work', async () => {
    const client = await makeClient(a.access_token);
    const auth = new AuthClient(client, new MemoryTokenStorage());
    const user = await auth.signUp({ email: `sdk-${Date.now()}@darshjdb.test`, password });
    expect(user.id).toBeTruthy(); expect(auth.getTokens()).toBeTruthy();
    await auth.signOut(); expect(client.getAuthToken()).toBeNull();
  }, 10000);
  it('rejects unauthenticated admin access and nonadmin raw event feeds', async () => {
    expect((await request('/api/admin/schema', undefined, '')).status).toBe(401);
    expect((await request('/api/events?channel=*')).status).toBe(403);
    expect((await request('/api/events/publish', { channel: 'live-test', data: {} })).status).toBe(403);
  });
  it('transaction builder retains both operations', () => {
    const builder = new TransactionBuilder(); const id = generateId();
    builder.proxy[collection]![id]!.set({ label: 'x' }); builder.proxy[collection]![id]!.merge({ label: 'y' });
    expect(builder.ops.map(op => op.kind)).toEqual(['set', 'merge']);
  });
  it('opens an authenticated SSE stream', async () => {
    const controller = new AbortController();
    try {
      const response = await fetch(`${URL}/api/subscribe?q=${encodeURIComponent(JSON.stringify({ type: collection }))}`, { headers: { Authorization: `Bearer ${a.access_token}` }, signal: controller.signal });
      expect(response.status).toBe(200); expect(response.headers.get('content-type')).toContain('text/event-stream');
    } finally { controller.abort(); }
  });
  it('accepts MessagePack negotiation', async () => {
    const response = await fetch(`${URL}/api/query`, { method: 'POST', headers: { Authorization: `Bearer ${a.access_token}`, 'Content-Type': 'application/json', Accept: 'application/msgpack' }, body: JSON.stringify({ query: { type: collection } }) });
    expect(response.status).toBe(200); expect((await response.arrayBuffer()).byteLength).toBeGreaterThan(0);
  });
  it('denies cross-owner reads and unauthenticated queries', async () => {
    expect((await db.rest.query({ collection, where: [{ field: 'id', op: '=', value: idB }] })).data).toEqual([]);
    expect((await request('/api/query', { query: { type: collection } }, '')).status).toBe(401);
  });
  it('rejects invalid WebSocket credentials without decode errors', async () => {
    const client = new DarshJDB({ serverUrl: URL!, appId: 'live-test', transport: 'ws' });
    clients.push(client); client.setAuthToken('invalid-test-token');
    await expect(client.connect()).rejects.toThrow();
    expect(client.state).toBe('disconnected');
  });
  for (const transport of ['rest', 'ws'] as const) {
    it(`${transport}: separate principals and credential retirement isolate real subscriptions`, async () => {
      const first = await makeClient(a.access_token, transport), second = await makeClient(b.access_token, transport);
      const left: QueryResult[] = [], right: QueryResult[] = [];
      const offLeft = subscribe(first, { collection }, value => left.push(value));
      const offRight = subscribe(second, { collection }, value => right.push(value));
      try {
        await until(() => left.some(value => value.data.length > 0) && right.some(value => value.data.length > 0));
        expect(JSON.stringify(left)).not.toContain('bob'); expect(JSON.stringify(right)).not.toContain('alice');
        await expect(submitOps(first, [{ kind: 'merge', entity: collection, id: idB, data: { label: 'unauthorized' } }])).rejects.toThrow();
        await expect(submitOps(first, [{ kind: 'merge', entity: collection, id: idA, data: { owner_id: b.user_id } }])).rejects.toThrow();
        expect((await queryOnce(first, { collection, where: [{ field: 'id', op: '=', value: idA }] })).data).toEqual([expect.objectContaining({ owner_id: a.user_id, label: 'alice' })]);
        first.setAuthToken(b.access_token);
        expect(left.at(-1)?.data).toEqual([]); expect(left.at(-1)?.error).toBeInstanceOf(SubscriptionEndedError);
        const retiredCount = left.length;
        await first.connect();
        expect((await queryOnce(first, { collection })).data).toEqual([expect.objectContaining({ id: idB, label: 'bob' })]);
        await submitOps(second, [{ kind: 'merge', entity: collection, id: idB, data: { label: 'bob-updated' } }]);
        await until(() => right.some(value => value.data.some(row => row.label === 'bob-updated')));
        expect(left).toHaveLength(retiredCount);
        second.disconnect(); expect(right.at(-1)?.data).toEqual([]); expect(right.at(-1)?.error).toBeInstanceOf(SubscriptionEndedError);
      } finally { offLeft(); offRight(); first.disconnect(); second.disconnect(); }
      await json(await request('/api/mutate', { mutations: [{ op: 'update', entity: collection, id: idB, data: { label: 'bob' } }] }, b.access_token));
    }, 35000);
  }
  for (const transport of ['rest', 'ws'] as const) {
  it(`${transport}: retires an already-open subscription after server-side session revocation`, async () => {
    const client = await makeClient(a.access_token, transport);
    const received: QueryResult[] = [];
    const stop = subscribe(client, { collection }, result => received.push(result));
    try {
      await until(() => received.some(result => result.data.length > 0));
      a = await json(await request('/api/auth/signin', { email, password }, ''));
      await json(await request('/api/mutate', { mutations: [{ op: 'update', entity: collection, id: idB, data: { label: 'bob' } }] }, b.access_token));
      if (transport === 'ws') await expect(queryOnce(client, { collection })).rejects.toThrow();
      await until(() => received.at(-1)?.error instanceof SubscriptionEndedError);
      expect(received.at(-1)?.data).toEqual([]);
    } finally { stop(); client.disconnect(); }
  }, 20000);
  }

  for (const transport of ['rest', 'ws'] as const) {
    it.skipIf(!process.env.DDB_TEST_DATABASE_URL)(`${transport}: retires an idle subscription after revocation without a data mutation or client query`, async () => {
      const client = await makeClient(a.access_token, transport);
      const received: QueryResult[] = [];
      let completedRefreshes = 0;
      const realQuery = client.rest.query.bind(client.rest);
      const spy = vi.spyOn(client.rest, 'query').mockImplementation(async (...args) => {
        const result = await realQuery(...args);
        completedRefreshes++;
        return result;
      });
      const stop = subscribe(client, { collection }, result => received.push(result));
      try {
        await until(() => received.some(result => result.data.length > 0));
        // Drain the SSE initial invalidation before revocation. Direct SQL avoids
        // any API event that could accidentally wake a non-polling old server.
        if (transport === 'rest') await until(() => completedRefreshes >= 2);
        expect(a.user_id).toMatch(/^[0-9a-f-]{36}$/i);
        try {
          const updated = execFileSync('psql', ['-X', '-d', process.env.DDB_TEST_DATABASE_URL!, '-v', 'ON_ERROR_STOP=1', '-c',
          `UPDATE sessions SET revoked = true, revoked_at = now() WHERE user_id = '${a.user_id}'::uuid`],
          { stdio: 'pipe' }).toString();
          expect(updated).toMatch(/UPDATE [1-9][0-9]*/);
        } catch {
          throw new Error('Direct session revocation failed; check isolated DDB_TEST_DATABASE_URL and psql');
        }
        await until(() => received.at(-1)?.error instanceof SubscriptionEndedError, 20000);
        expect(received.at(-1)?.data).toEqual([]);
      } finally {
        stop(); client.disconnect(); spy.mockRestore();
        a = await json(await request('/api/auth/signin', { email, password }, ''));
      }
    }, 35000);
  }

});
