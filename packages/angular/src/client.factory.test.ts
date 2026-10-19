import { afterEach, describe, expect, it, vi } from 'vitest';
import { DarshJDB, PresenceRoom } from '@darshjdb/client';
import { createDarshanClient } from './client.factory';

afterEach(() => { vi.restoreAllMocks(); vi.unstubAllGlobals(); });
const factory = () => createDarshanClient({ serverUrl: 'https://db.example.com', appId: 'test' });

describe('Angular core adapter', () => {
  it('translates query filters, returns rows and forwards terminal retirement errors', async () => {
    const send = vi.spyOn(DarshJDB.prototype, 'send').mockResolvedValue({ type: 'sub-ok', id: 'r', sub_id: 's', initial: [{ id: 'private' }] });
    const client = factory();
    const callback = vi.fn();
    const stop = client.subscribe('notes', { where: { owner: 'a' }, order: { createdAt: 'desc' } }, callback);
    await Promise.resolve();
    expect(send).toHaveBeenCalledWith({ type: 'sub', query: { type: 'notes', $where: [{ attribute: 'owner', op: 'Eq', value: 'a' }], $order: [{ attribute: 'createdAt', direction: 'Desc' }] } });
    expect(callback.mock.lastCall?.[0].data).toEqual([{ id: 'private' }]);
    expect(await client.query('notes', {})).toEqual([{ id: 'private' }]);
    client.disconnect();
    expect(callback.mock.lastCall?.[0]).toMatchObject({ data: [], error: { code: 'SubscriptionEndedError' } });
    stop();
  });

  it('maps the documented mutation forms into core operations', async () => {
    const send = vi.spyOn(DarshJDB.prototype, 'send').mockResolvedValue({ type: 'mut-ok', id: 'r', tx: 42 });
    const client = factory();
    expect(await client.mutate('notes', { insert: { title: 'new' } })).toEqual({ txId: '42' });
    expect(send.mock.lastCall?.[0]).toMatchObject({ type: 'mut', ops: [{ op: 'insert', entity: 'notes', data: { title: 'new' } }] });
    await client.mutate('notes', { update: { id: 'one', data: { title: 'edit' } } });
    expect(send.mock.lastCall?.[0]).toEqual({ type: 'mut', ops: [{ op: 'update', entity: 'notes', id: 'one', data: { title: 'edit' } }] });
    await client.mutate('notes', { delete: 'one' });
    expect(send.mock.lastCall?.[0]).toEqual({ type: 'mut', ops: [{ op: 'delete', entity: 'notes', id: 'one' }] });
    await expect(client.mutate('notes', { update: { data: {} } })).rejects.toThrow('requires an id');
  });

  it('adapts auth results and connects only after setting credentials', async () => {
    const connect = vi.spyOn(DarshJDB.prototype, 'connect').mockImplementation(async function () {
      expect(this.getAuthToken()).toBe('access');
    });
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: true, json: async () => ({ user_id: 'u', access_token: 'access', refresh_token: 'refresh', expires_in: 3600 }) }));
    const client = factory();
    const result = await client.signIn({ email: 'test@example.com', password: 'test' });
    expect(result.user.id).toBe('u');
    expect(result.accessToken).toBe('access');
    expect(connect).toHaveBeenCalledOnce();
    await client.signOut();
    expect(client.getUser()).toBeNull();
    expect(client.getToken()).toBeNull();
  });
  it('adapts presence updates and leaves rooms on disconnect', async () => {
    vi.spyOn(PresenceRoom.prototype, 'join').mockResolvedValue();
    const publish = vi.spyOn(PresenceRoom.prototype, 'publish').mockImplementation(() => {});
    const leave = vi.spyOn(PresenceRoom.prototype, 'leave').mockResolvedValue();
    vi.spyOn(PresenceRoom.prototype, 'subscribe').mockImplementation(callback => {
      callback({ roomId: 'room', self: null, peers: [{ peerId: 'p', userId: 'u', state: { x: 1 }, lastSeen: 0 }] });
      return () => {};
    });
    const client = factory();
    const callback = vi.fn();
    const room = client.joinPresence('room', { x: 1 }, callback);
    await Promise.resolve();
    expect(callback).toHaveBeenCalledWith({ count: 1, users: [{ userId: 'u', data: { x: 1 }, lastSeen: '1970-01-01T00:00:00.000Z' }] });
    room.update({ x: 2 });
    expect(publish).toHaveBeenLastCalledWith({ x: 2 });
    client.disconnect();
    expect(leave).toHaveBeenCalledOnce();
  });

});
