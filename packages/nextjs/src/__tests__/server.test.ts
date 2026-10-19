import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { mutateServer, queryServer } from '../server.js';

beforeEach(() => {
  vi.stubEnv('DDB_URL', 'https://db.example.com');
  vi.stubEnv('DDB_ADMIN_TOKEN', 'test-only-token');
});
afterEach(() => { vi.unstubAllEnvs(); vi.unstubAllGlobals(); });

describe('server REST contracts', () => {
  it('wraps the typed DarshJQL query and preserves cache options and result envelope', async () => {
    const result = { data: [{ id: 'one' }], meta: { count: 1 } };
    const fetch = vi.fn().mockResolvedValue({ ok: true, json: async () => result });
    vi.stubGlobal('fetch', fetch);
    const query = { type: 'todos', $where: [{ attribute: 'done', op: 'Eq' as const, value: false }] };
    expect(await queryServer(query, { revalidate: 60, tags: ['todos'] })).toEqual(result);
    expect(fetch.mock.calls[0]![0]).toBe('https://db.example.com/api/query');
    const request = fetch.mock.calls[0]![1];
    expect(JSON.parse(request.body)).toEqual({ query });
    expect(request.headers.Authorization).toBe('Bearer test-only-token');
    expect(request.next).toEqual({ revalidate: 60, tags: ['todos'] });
  });

  it('maps every public mutation verb into the server transaction envelope', async () => {
    const fetch = vi.fn().mockResolvedValue({ ok: true, json: async () => ({ affected: 3 }) });
    vi.stubGlobal('fetch', fetch);
    await mutateServer([
      { entity: 'todos', op: 'set', data: { title: 'new' } },
      { entity: 'todos', op: 'merge', id: '8d67a240-4f83-4a1f-9fbe-3312162ce260', data: { done: true } },
      { entity: 'todos', op: 'delete', id: '2f6732bf-4040-4a6e-bf26-b961042ed832' },
    ]);
    expect(fetch.mock.calls[0]![0]).toBe('https://db.example.com/api/mutate');
    expect(JSON.parse(fetch.mock.calls[0]![1].body)).toEqual({ mutations: [
      { entity: 'todos', op: 'insert', data: { title: 'new' } },
      { entity: 'todos', op: 'update', id: '8d67a240-4f83-4a1f-9fbe-3312162ce260', data: { done: true } },
      { entity: 'todos', op: 'delete', id: '2f6732bf-4040-4a6e-bf26-b961042ed832' },
    ] });
  });

  it('surfaces server rejections', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: false, status: 403, text: async () => 'forbidden' }));
    await expect(queryServer({ type: 'todos' })).rejects.toThrow('Query failed (403): forbidden');
    await expect(mutateServer([{ entity: 'todos', op: 'set', data: {} }])).rejects.toThrow('Mutation failed (403): forbidden');
  });
});
