/**
 * @module create-client
 * @description Adapts the framework-agnostic `DarshJDB` core client (exported
 * by `@darshjdb/client`) to the {@link DarshanClientInterface} contract that
 * the React hooks consume.
 *
 * @example
 * ```ts
 * import { createDarshanClient } from '@darshjdb/react';
 *
 * const client = createDarshanClient({
 *   serverUrl: 'https://db.example.com',
 *   appId: 'my-app',
 * });
 * ```
 */

import {
  AuthClient,
  DarshJDB,
  PresenceRoom,
  StorageClient,
  generateId,
  queryOnce,
  subscribe as subscribeQuery,
  transact,
  type AuthStateEvent,
  type EntityCollectionProxy,
  type EntityProxy,
  type Peer,
  type QueryDescriptor,
  type QueryResult,
  type User,
} from '@darshjdb/client';

import type {
  AuthState,
  AuthUnsubscribe,
  AuthUser,
  DarshanClientInterface,
  DarshanClientOptions,
  MutationOperation,
  PresencePeer,
  Query,
  QuerySnapshot,
  Unsubscribe,
  UploadProgress,
  UploadResult,
} from './types';

// ---------------------------------------------------------------------------
// Shape mapping between core types and the React-facing types
// ---------------------------------------------------------------------------

function toDescriptor<T>(query: Query<T>): QueryDescriptor {
  return {
    collection: query.collection,
    where: query.where ? ([...query.where] as QueryDescriptor['where']) : undefined,
    order: query.orderBy ? ([...query.orderBy] as QueryDescriptor['order']) : undefined,
    limit: query.limit,
    offset: query.offset,
    select: query.select ? [...query.select] : undefined,
  };
}

function toAuthUser(user: User | null): AuthUser | null {
  if (!user) return null;
  return {
    id: user.id,
    email: user.email ?? null,
    displayName: user.displayName ?? null,
    photoUrl: user.avatarUrl ?? null,
    metadata: user.metadata ?? {},
  };
}

/** The transaction proxy materialises collections and entities on access. */
function entityOf(
  tx: Record<string, EntityCollectionProxy>,
  collection: string,
  id: string,
): EntityProxy {
  return (tx[collection] as EntityCollectionProxy)[id] as EntityProxy;
}

function toPresencePeers<S>(peers: ReadonlyArray<Peer>): ReadonlyArray<PresencePeer<S>> {
  return peers.map((peer) => ({
    peerId: peer.peerId,
    state: peer.state as S,
    lastSeen: peer.lastSeen,
  }));
}

// ---------------------------------------------------------------------------
// Adapter
// ---------------------------------------------------------------------------

class DarshanClientAdapter implements DarshanClientInterface {
  private readonly db: DarshJDB;
  private readonly auth: AuthClient;
  private readonly storage: StorageClient;
  private readonly rooms = new Map<string, PresenceRoom>();

  constructor(options: DarshanClientOptions) {
    this.db = new DarshJDB({ serverUrl: options.serverUrl, appId: options.appId });
    this.auth = new AuthClient(this.db);
    this.storage = new StorageClient(this.db);
  }

  /* -- Lifecycle ---------------------------------------------------------- */

  async connect(): Promise<void> {
    await this.db.connect();
    await this.auth.init();
  }

  disconnect(): void {
    for (const room of this.rooms.values()) {
      void room.leave();
    }
    this.rooms.clear();
    this.db.disconnect();
  }

  /* -- Queries ------------------------------------------------------------ */

  subscribe<T>(query: Query<T>, callback: (snapshot: QuerySnapshot<T>) => void): Unsubscribe {
    return subscribeQuery<T>(this.db, toDescriptor(query), (result: QueryResult<T>) => {
      callback({ data: result.data, error: null });
    });
  }

  async query<T>(query: Query<T>): Promise<ReadonlyArray<T>> {
    const result = await queryOnce<T>(this.db, toDescriptor(query));
    return result.data;
  }

  /* -- Mutations ---------------------------------------------------------- */

  async mutate(
    operations: MutationOperation | ReadonlyArray<MutationOperation>,
  ): Promise<void> {
    const ops = Array.isArray(operations)
      ? (operations as ReadonlyArray<MutationOperation>)
      : [operations as MutationOperation];

    if (ops.length === 0) return;

    await transact(this.db, (tx) => {
      for (const op of ops) {
        switch (op.type) {
          case 'insert':
            entityOf(tx, op.collection, generateId()).set(op.data);
            break;
          case 'update':
            entityOf(tx, op.collection, op.id).merge(op.data);
            break;
          case 'delete':
            entityOf(tx, op.collection, op.id).delete();
            break;
        }
      }
    });
  }

  /* -- Auth --------------------------------------------------------------- */

  async signIn(credentials: { email: string; password: string }): Promise<AuthUser> {
    return toAuthUser(await this.auth.signIn(credentials)) as AuthUser;
  }

  async signUp(credentials: {
    email: string;
    password: string;
    displayName?: string;
  }): Promise<AuthUser> {
    return toAuthUser(await this.auth.signUp(credentials)) as AuthUser;
  }

  async signOut(): Promise<void> {
    await this.auth.signOut();
  }

  getAuthState(): AuthState {
    return { user: toAuthUser(this.auth.getUser()), isLoading: false };
  }

  onAuthStateChange(callback: (state: AuthState) => void): AuthUnsubscribe {
    return this.auth.onAuthStateChange((event: AuthStateEvent) => {
      callback({ user: toAuthUser(event.user), isLoading: false });
    });
  }

  /* -- Presence ----------------------------------------------------------- */

  async joinRoom(roomId: string): Promise<void> {
    if (this.rooms.has(roomId)) return;

    const room = new PresenceRoom(this.db, roomId);
    this.rooms.set(roomId, room);

    try {
      await room.join();
    } catch (err) {
      this.rooms.delete(roomId);
      throw err;
    }
  }

  async leaveRoom(roomId: string): Promise<void> {
    const room = this.rooms.get(roomId);
    if (!room) return;

    this.rooms.delete(roomId);
    await room.leave();
  }

  publishPresence<S>(roomId: string, state: S): void {
    this.rooms.get(roomId)?.publish(state as Record<string, unknown>);
  }

  onPresenceChange<S>(
    roomId: string,
    callback: (peers: ReadonlyArray<PresencePeer<S>>) => void,
  ): Unsubscribe {
    const room = this.rooms.get(roomId);
    if (!room) return () => {};

    return room.subscribe((snapshot) => {
      callback(toPresencePeers<S>(snapshot.peers));
    });
  }

  /* -- Storage ------------------------------------------------------------ */

  async upload(
    file: File | Blob,
    path: string,
    options: { onProgress?: (progress: UploadProgress) => void } = {},
  ): Promise<UploadResult> {
    const { onProgress } = options;
    const totalBytes = file.size;

    const result = await this.storage.upload(path, file, {
      onProgress: onProgress
        ? (fraction: number) =>
            onProgress({
              bytesTransferred: Math.round(fraction * totalBytes),
              totalBytes,
              fraction,
            })
        : undefined,
    });

    return {
      url: result.url,
      path: result.path,
      size: result.size,
      contentType: result.contentType,
    };
  }
}

/**
 * Create a {@link DarshanClientInterface} backed by the `DarshJDB` core client.
 *
 * @param options - Server URL and application identifier.
 * @returns A client instance ready to be passed to `<DarshanProvider client={...}>`.
 */
export function createDarshanClient(
  options: DarshanClientOptions,
): DarshanClientInterface {
  return new DarshanClientAdapter(options);
}
