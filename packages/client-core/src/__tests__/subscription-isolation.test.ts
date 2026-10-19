import { afterEach, describe, expect, it, vi } from 'vitest';
import { DarshJDB } from '../client.js';
import { subscribe } from '../query.js';
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
});
