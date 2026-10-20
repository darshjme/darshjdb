import { afterEach, describe, expect, it, vi } from 'vitest';
import { DarshJDB } from '../client.js';
import { subscribe, queryOnce } from '../query.js';
import type { ServerMessage } from '../types.js';

function client(owner: string) {
  const db = new DarshJDB({ serverUrl: `https://${owner}.example.com`, appId: owner });
  let count = 0;
  const send = vi.spyOn(db, 'send').mockImplementation(async (message) => {
    if (message.type === 'sub') return {
      type: 'sub-ok', id: 'request', sub_id: `${owner}-${++count}`,
      initial: [{ id: owner }],
    } as ServerMessage;
    return { type: 'unsub-ok', id: 'request' } as ServerMessage;
  });
  return { db, send };
}

const flush = async () => { await Promise.resolve(); await Promise.resolve(); };

describe('subscription isolation', () => {
  afterEach(() => vi.unstubAllGlobals());
  it('keeps identical queries, pushes, reconnects and teardown scoped to their client', async () => {
    const a = client('a');
    const b = client('b');
    const receivedA = vi.fn();
    const receivedB = vi.fn();
    const handlersA = vi.spyOn(a.db, 'registerSubscriptionHandler');
    const stopA = subscribe(a.db, { collection: 'notes' }, receivedA);
    const stopB = subscribe(b.db, { collection: 'notes' }, receivedB);
    await flush();
    expect(a.send).toHaveBeenCalledTimes(1);
    expect(b.send).toHaveBeenCalledTimes(1);
    expect(receivedA.mock.lastCall?.[0].data).toEqual([{ id: 'a' }]);
    expect(receivedB.mock.lastCall?.[0].data).toEqual([{ id: 'b' }]);
    handlersA.mock.calls[0]![1]({ type: 'sub', sub_id: 'a-1', added: [{ id: 'a-private' }] } as ServerMessage);
    expect(receivedB).toHaveBeenCalledTimes(1);
    (a.db as unknown as { _privateNotifyReconnected(): void })._privateNotifyReconnected();
    await flush();
    expect(a.send).toHaveBeenCalledTimes(2);
    expect(b.send).toHaveBeenCalledTimes(1);
    stopA();
    expect(a.send).toHaveBeenLastCalledWith({ type: 'unsub', sub_id: 'a-2' });
    expect(b.send).toHaveBeenCalledTimes(1);
    stopB();
  });

  it('opens separate filtered queries but shares equal queries on the same client', async () => {
    const { db, send } = client('filters');
    const first = vi.fn();
    const second = vi.fn();
    const stop1 = subscribe(db, { collection: 'notes', where: [{ field: 'owner', op: '=', value: 'a' }] }, first);
    const stop2 = subscribe(db, { collection: 'notes', where: [{ field: 'owner', op: '=', value: 'b' }] }, second);
    const stop3 = subscribe(db, { collection: 'notes', where: [{ field: 'owner', op: '=', value: 'a' }] }, vi.fn());
    await flush();
    expect(send).toHaveBeenCalledTimes(2);
    expect(send.mock.calls.map(([m]) => m.type === 'sub' ? m.query : null)).toEqual([
      { type: 'notes', $where: [{ attribute: 'owner', op: 'Eq', value: 'a' }] },
      { type: 'notes', $where: [{ attribute: 'owner', op: 'Eq', value: 'b' }] },
    ]);
    stop1();
    expect(send).toHaveBeenCalledTimes(2);
    stop3();
    expect(send).toHaveBeenLastCalledWith({ type: 'unsub', sub_id: 'filters-1' });
    stop2();
  });

  it('retires cached rows and handlers when credentials change on the same client', async () => {
    const { db, send } = client('session');
    db.setAuthToken('first-token');
    const oldCallback = vi.fn();
    const stopOld = subscribe(db, { collection: 'notes' }, oldCallback);
    await flush();
    db.setAuthToken('second-token');
    expect(oldCallback.mock.lastCall?.[0].data).toEqual([]);
    expect(oldCallback.mock.lastCall?.[0].error.message).toContain('Credentials changed');
    const nextCallback = vi.fn();
    const stopNext = subscribe(db, { collection: 'notes' }, nextCallback);
    expect(nextCallback).not.toHaveBeenCalled();
    await flush();
    expect(send).toHaveBeenCalledTimes(2);
    stopOld();
    expect(send).toHaveBeenCalledTimes(2);
    db.setAuthToken(null);
    expect(nextCallback.mock.lastCall?.[0].data).toEqual([]);
    stopNext();
  });

  it('retires REST subscriptions and suppresses results already in flight on logout', async () => {
    const db = new DarshJDB({ serverUrl: 'https://db.example.com', appId: 'rest', transport: 'rest' });
    db.setAuthToken('rest-user');
    let finishQuery!: (response: unknown) => void;
    vi.stubGlobal('fetch', vi.fn((url: string) => url.endsWith('/query')
      ? new Promise((resolve) => { finishQuery = resolve; })
      : new Promise(() => {})));
    const callback = vi.fn();
    const stop = subscribe(db, { collection: 'notes' }, callback);
    db.setAuthToken(null);
    expect(callback).toHaveBeenCalledOnce();
    expect(callback.mock.lastCall?.[0].error.name).toBe('SubscriptionEndedError');
    finishQuery({ ok: true, json: async () => ({ data: [{ id: 'previous-user' }] }) });
    await flush();
    await flush();
    expect(callback).toHaveBeenCalledOnce();
    stop();
  });

  it('ignores a stale registration response after a newer reconnect completes', async () => {
    const { db, send } = client('race');
    let resolve!: (message: ServerMessage) => void;
    send.mockImplementationOnce(() => new Promise((done) => { resolve = done; }));
    const callback = vi.fn();
    const stop = subscribe(db, { collection: 'notes' }, callback);
    (db as unknown as { _privateNotifyReconnected(): void })._privateNotifyReconnected();
    await flush();
    expect(callback).toHaveBeenCalledTimes(1);
    resolve({ type: 'sub-ok', id: 'old', sub_id: 'old-session', initial: [{ id: 'stale' }] });
    await flush();
    expect(callback).toHaveBeenCalledTimes(1);
    expect(send).toHaveBeenLastCalledWith({ type: 'unsub', sub_id: 'old-session' });
    stop();
  });

  it('releases an in-flight registration after its last subscriber leaves', async () => {
    const { db, send } = client('pending');
    let resolve!: (message: ServerMessage) => void;
    send.mockImplementationOnce(() => new Promise((done) => { resolve = done; }));
    const callback = vi.fn();
    const stop = subscribe(db, { collection: 'notes' }, callback);
    stop();
    resolve({ type: 'sub-ok', id: 'request', sub_id: 'late', initial: [{ id: 'private' }] });
    await flush();
    expect(callback).not.toHaveBeenCalled();
    expect(send).toHaveBeenLastCalledWith({ type: 'unsub', sub_id: 'late' });
  });
  function restStream() {
    const db = new DarshJDB({ serverUrl: 'https://db.example.com', appId: 'stream', transport: 'rest' });
    db.setAuthToken('alice');
    let stream!: ReadableStreamDefaultController<Uint8Array>;
    const pending: Array<(value: Response) => void> = [];
    vi.stubGlobal('fetch', vi.fn((url: string) => url.endsWith('/query')
      ? new Promise<Response>(resolve => pending.push(resolve))
      : Promise.resolve(new Response(new ReadableStream<Uint8Array>({ start(controller) { stream = controller; } })))));
    const answer = (index: number, id: string) => pending[index]!(new Response(JSON.stringify({ data: [{ id }] })));
    return { db, pending, answer, close: () => stream.close(), frame: (text: string) => stream.enqueue(new TextEncoder().encode(text)) };
  }

  it('retires cached REST rows on clean SSE EOF and suppresses pending results', async () => {
    const h = restStream(), callback = vi.fn();
    const stop = subscribe(h.db, { collection: 'notes' }, callback);
    h.answer(0, 'alice');
    await vi.waitFor(() => expect(callback.mock.lastCall?.[0].data).toEqual([{ id: 'alice' }]));
    h.frame('event: update\ndata: {}\n\n');
    await vi.waitFor(() => expect(h.pending).toHaveLength(2));
    h.close();
    await vi.waitFor(() => expect(callback.mock.lastCall?.[0].error?.name).toBe('SubscriptionEndedError'));
    expect(callback.mock.lastCall?.[0].data).toEqual([]);
    const count = callback.mock.calls.length;
    h.answer(1, 'late-alice');
    await new Promise(resolve => setTimeout(resolve, 0));
    expect(callback).toHaveBeenCalledTimes(count);
    stop();
  });

  it('recognizes chunk-split CRLF SSE revocation and suppresses old query responses', async () => {
    const h = restStream(), callback = vi.fn();
    const stop = subscribe(h.db, { collection: 'notes' }, callback);
    h.frame('event: auth-error\r');
    h.frame('\ndata: {}\r\n\r');
    h.frame('\n');
    await vi.waitFor(() => expect(callback.mock.lastCall?.[0].error?.name).toBe('SubscriptionEndedError'));
    h.answer(0, 'late-alice');
    await new Promise(resolve => setTimeout(resolve, 0));
    expect(callback).toHaveBeenCalledOnce();
    expect(callback.mock.lastCall?.[0].data).toEqual([]);
    stop(); h.close();
  });

  it('rejects a pending one-shot REST query across an Alice/Bob/Alice session change', async () => {
    const h = restStream();
    const result = h.db.rest.query({ collection: 'notes' });
    const rejected = expect(result).rejects.toThrow('session');
    h.db.setAuthToken('bob'); h.db.setAuthToken('alice');
    h.answer(0, 'old-session');
    await rejected;
  });

  it('does not overwrite a newer REST refresh with an older in-flight response', async () => {
    const h = restStream(), callback = vi.fn();
    const stop = subscribe(h.db, { collection: 'notes' }, callback);
    h.frame('event: update\ndata: {}\n\n');
    await vi.waitFor(() => expect(h.pending).toHaveLength(2));
    h.answer(1, 'new');
    await vi.waitFor(() => expect(callback.mock.lastCall?.[0].data).toEqual([{ id: 'new' }]));
    h.answer(0, 'old');
    await new Promise(resolve => setTimeout(resolve, 0));
    expect(callback).toHaveBeenCalledOnce();
    stop(); h.close();
  });

  it('does not retire a newer REST snapshot when an older refresh fails', async () => {
    const h = restStream(), callback = vi.fn();
    const stop = subscribe(h.db, { collection: 'notes' }, callback);
    h.frame('event: update\ndata: {}\n\n');
    await vi.waitFor(() => expect(h.pending).toHaveLength(2));
    h.answer(1, 'new');
    await vi.waitFor(() => expect(callback.mock.lastCall?.[0].data).toEqual([{ id: 'new' }]));
    h.pending[0]!(new Response('temporary upstream failure', { status: 503 }));
    await new Promise(resolve => setTimeout(resolve, 0));
    expect(callback).toHaveBeenCalledOnce();
    expect(callback.mock.lastCall?.[0].data).toEqual([{ id: 'new' }]);
    stop(); h.close();
  });

  it('rejects a resolved WebSocket query when credentials change before its continuation', async () => {
    const { db } = client('ws-session');
    db.setAuthToken('alice');
    const result = queryOnce(db, { collection: 'notes' });
    const rejected = expect(result).rejects.toThrow('session');
    db.setAuthToken('bob'); db.setAuthToken('alice');
    await rejected;
  });

  it('replaces a WS row snapshot when field policy removes a previously visible field', async () => {
    const { db, send } = client('policy');
    send.mockResolvedValueOnce({ type: 'sub-ok', id: 'request', sub_id: 'policy-1', initial: [{ id: 'note', public: 'visible', secret: 'old-secret' }] } as ServerMessage);
    const handlers = vi.spyOn(db, 'registerSubscriptionHandler');
    const callback = vi.fn();
    const stop = subscribe(db, { collection: 'notes' }, callback);
    await flush();
    expect(callback.mock.lastCall?.[0].data[0].secret).toBe('old-secret');
    handlers.mock.calls[0]![1]({ type: 'sub', sub_id: 'policy-1', updated: [{ id: 'note', public: 'visible' }] } as ServerMessage);
    expect(callback.mock.lastCall?.[0].data).toEqual([{ id: 'note', public: 'visible' }]);
    stop();
  });

});
