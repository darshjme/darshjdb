/** Adapts the framework-neutral core API to the Angular injection facade. */
import {
  AuthClient, DarshJDB, PresenceRoom, queryOnce, subscribe, submitOps, generateId,
  type QueryDescriptor, type WhereClause, type OrderClause, type User,
} from '@darshjdb/client';
import type { DarshanConfig } from './types';
import type { DarshanClient, AuthResult, AuthUser, PresenceHandle } from './tokens';

function object(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Expected an object');
  return value as Record<string, unknown>;
}

function descriptor(collection: string, query: Record<string, unknown>): QueryDescriptor {
  return {
    collection,
    where: query.where === undefined ? undefined : Array.isArray(query.where)
      ? query.where as WhereClause[]
      : Object.entries(object(query.where)).map(([field, value]) => ({ field, op: '=', value })),
    order: query.order === undefined ? undefined : Array.isArray(query.order)
      ? query.order as OrderClause[]
      : Object.entries(object(query.order)).map(([field, direction]) => {
        if (direction !== 'asc' && direction !== 'desc') throw new Error('Order must be asc or desc');
        return { field, direction };
      }),
    select: query.select as string[] | undefined,
    limit: query.limit as number | undefined,
    offset: query.offset as number | undefined,
  };
}

function user(value: User | null): AuthUser | null {
  return value ? { id: value.id, email: value.email, displayName: value.displayName, roles: [] } : null;
}

export function createDarshanClient(config: DarshanConfig): DarshanClient {
  const db = new DarshJDB({ serverUrl: config.serverUrl, appId: config.appId });
  const auth = new AuthClient(db);
  const rooms = new Set<PresenceHandle>();
  const result = (value: User): AuthResult => {
    const tokens = auth.getTokens();
    if (!tokens) throw new Error('Authentication returned no session');
    return { user: user(value)!, ...tokens, expiresAt: new Date(tokens.expiresAt).toISOString() };
  };
  const credentials = (value: Record<string, unknown>) => {
    if (typeof value.email !== 'string' || typeof value.password !== 'string') {
      throw new Error('This adapter requires email/password credentials');
    }
    return { email: value.email, password: value.password,
      ...(typeof value.displayName === 'string' ? { displayName: value.displayName } : {}) };
  };
  return {
    async connect() { await auth.init(); if (db.getAuthToken()) await db.connect(); },
    disconnect() { for (const room of rooms) room.leave(); db.disconnect(); },
    get connected() { return db.state === 'connected'; },
    subscribe<T>(collection: string, query: Record<string, unknown>, callback: (value: { data: T; error: import('./tokens').DarshanClientError | null }) => void) {
      return subscribe(db, descriptor(collection, query), (value) => callback({
        data: value.data as T,
        error: value.error ? { code: value.error.name, message: value.error.message } : null,
      }));
    },
    async query<T>(collection: string, query: Record<string, unknown>) {
      return (await queryOnce(db, descriptor(collection, query))).data as T;
    },
    async mutate<T>(collection: string, mutation: Record<string, unknown>) {
      let txId: string;
      if ('insert' in mutation) {
        txId = await submitOps(db, [{ kind: 'set', entity: collection, id: generateId(), data: object(mutation.insert) }]);
      } else if ('update' in mutation) {
        const update = object(mutation.update);
        if (typeof update.id !== 'string') throw new Error('Update requires an id');
        txId = await submitOps(db, [{ kind: 'merge', entity: collection, id: update.id, data: object(update.data) }]);
      } else if (typeof mutation.delete === 'string') {
        txId = await submitOps(db, [{ kind: 'delete', entity: collection, id: mutation.delete }]);
      } else throw new Error('Expected insert, update: {id, data}, or delete: id mutation');
      return { txId } as T;
    },
    async signIn(value) { const signedIn = await auth.signIn(credentials(value)); await db.connect(); return result(signedIn); },
    async signUp(value) { const signedUp = await auth.signUp(credentials(value)); await db.connect(); return result(signedUp); },
    async signOut() { for (const room of rooms) room.leave(); await auth.signOut(); },
    getUser() { return user(auth.getUser()); },
    getToken() { return db.getAuthToken(); },
    onAuthStateChange(callback) { return auth.onAuthStateChange((event) => callback(user(event.user))); },
    joinPresence<TData>(roomId: string, data: TData, callback: (state: import('./tokens').PresenceStateRaw<TData>) => void): PresenceHandle<TData> {
      const room = new PresenceRoom<TData>(db, roomId);
      let state = data;
      let left = false;
      const stop = room.subscribe((snapshot) => callback({
        users: snapshot.peers.map((peer) => ({ userId: peer.userId ?? peer.peerId, data: peer.state, lastSeen: new Date(peer.lastSeen).toISOString() })),
        count: snapshot.peers.length,
      }));
      const handle: PresenceHandle<TData> = {
        update(value) { state = { ...state, ...value }; room.publish(state); },
        leave() { if (left) return; left = true; stop(); rooms.delete(handle as PresenceHandle); void room.leave(); },
      };
      rooms.add(handle as PresenceHandle);
      void room.join().then(() => { if (left) return room.leave(); room.publish(state); }).catch((error: unknown) => {
        handle.leave(); console.error('[DarshJDB] Presence join failed:', error);
      });
      return handle;
    },
  };
}
