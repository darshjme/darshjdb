'use client';

/**
 * @module use-presence
 * @description Hook for real-time presence in a DarshJDB room.
 * Peers automatically join on mount and leave on unmount.
 *
 * @example
 * ```tsx
 * import { usePresence } from '@darshjdb/react';
 *
 * interface CursorState {
 *   x: number;
 *   y: number;
 *   name: string;
 * }
 *
 * function Cursors() {
 *   const { peers, publishState } = usePresence<CursorState>('canvas-room');
 *
 *   const handleMouseMove = (e: React.MouseEvent) => {
 *     publishState({ x: e.clientX, y: e.clientY, name: 'Alice' });
 *   };
 *
 *   return (
 *     <div onMouseMove={handleMouseMove}>
 *       {peers.map(p => (
 *         <div
 *           key={p.peerId}
 *           style={{ position: 'fixed', left: p.state.x, top: p.state.y }}
 *         >
 *           {p.state.name}
 *         </div>
 *       ))}
 *     </div>
 *   );
 * }
 * ```
 */

import { useCallback, useEffect, useRef, useSyncExternalStore } from 'react';

import { useDarshanClient } from './provider';
import type { DarshanClientInterface, PresencePeer, Unsubscribe } from './types';

// ---------------------------------------------------------------------------
// Room membership registry
//
// Join and leave are asynchronous, so a naive "join on mount / leave on
// cleanup" pair races: a leave issued for a torn-down effect can land *after*
// the join of the next effect (StrictMode double-mount, or a room id that
// flips back), silently kicking the peer out of a room it is still rendering.
//
// Every room therefore gets a reference count plus a serialised operation
// queue: joins and leaves for the same room never overlap, and a room is only
// left once the last subscriber releases it.
// ---------------------------------------------------------------------------

interface RoomEntry {
  count: number;
  /** Tail of the operation queue, `null` when no operation is in flight. */
  tail: Promise<void> | null;
}

const ROOMS = new WeakMap<DarshanClientInterface, Map<string, RoomEntry>>();

function getRoomEntry(client: DarshanClientInterface, roomId: string): RoomEntry {
  let byRoom = ROOMS.get(client);
  if (!byRoom) {
    byRoom = new Map<string, RoomEntry>();
    ROOMS.set(client, byRoom);
  }

  let entry = byRoom.get(roomId);
  if (!entry) {
    entry = { count: 0, tail: null };
    byRoom.set(roomId, entry);
  }
  return entry;
}

/** Queue `op` after any in-flight operation for the same room. */
function enqueue(entry: RoomEntry, op: () => Promise<void>): Promise<void> {
  const next = entry.tail === null ? op() : entry.tail.then(op, op);
  entry.tail = next;

  const settled = next.catch(() => undefined);
  void settled.then(() => {
    if (entry.tail === next) entry.tail = null;
  });

  return settled;
}

/**
 * Join the room (only the first subscriber issues the join).
 * @returns A promise that settles once the room is joined.
 */
function acquireRoom(client: DarshanClientInterface, roomId: string): Promise<void> {
  const entry = getRoomEntry(client, roomId);
  entry.count += 1;

  if (entry.count > 1) {
    return entry.tail ? entry.tail.catch(() => undefined) : Promise.resolve();
  }

  return enqueue(entry, () => client.joinRoom(roomId));
}

/** Release the room, leaving it once the last subscriber is gone. */
function releaseRoom(client: DarshanClientInterface, roomId: string): void {
  const entry = getRoomEntry(client, roomId);
  if (entry.count === 0) return;

  entry.count -= 1;
  if (entry.count > 0) return;

  void enqueue(entry, () => client.leaveRoom(roomId));
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/** Return value of {@link usePresence}. */
export interface UsePresenceResult<S> {
  /** Current list of peers (excluding self) with their published state. */
  readonly peers: ReadonlyArray<PresencePeer<S>>;
  /** Publish local user state to all peers in the room. */
  readonly publishState: (state: S) => void;
}

// ---------------------------------------------------------------------------
// Internal store
// ---------------------------------------------------------------------------

interface PresenceStore<S> {
  snapshot: ReadonlyArray<PresencePeer<S>>;
  listeners: Set<() => void>;
}

const EMPTY_PEERS: ReadonlyArray<never> = Object.freeze([]);

function createPresenceStore<S>(): PresenceStore<S> {
  return {
    snapshot: EMPTY_PEERS as ReadonlyArray<PresencePeer<S>>,
    listeners: new Set(),
  };
}

function emit<S>(store: PresenceStore<S>): void {
  for (const l of store.listeners) l();
}

// ---------------------------------------------------------------------------
// Hook
// ---------------------------------------------------------------------------

/**
 * Join a presence room, receive peer updates, and publish local state.
 *
 * The hook automatically calls `joinRoom` on mount and `leaveRoom` on
 * unmount (or when `roomId` changes).  Peer list updates are delivered
 * through `useSyncExternalStore` for concurrent-safe rendering.
 *
 * @typeParam S - Shape of the per-peer state object.
 * @param roomId - Unique room identifier to join.
 * @returns A {@link UsePresenceResult} with the current peers and a publish function.
 */
export function usePresence<S = Record<string, unknown>>(
  roomId: string,
): UsePresenceResult<S> {
  const client = useDarshanClient();
  const clientRef = useRef(client);
  clientRef.current = client;

  const storeRef = useRef<PresenceStore<S> | null>(null);
  if (!storeRef.current) {
    storeRef.current = createPresenceStore<S>();
  }
  const store = storeRef.current;

  // -----------------------------------------------------------------------
  // Join / leave + subscription
  // -----------------------------------------------------------------------
  useEffect(() => {
    let unsub: Unsubscribe | null = null;
    let cancelled = false;

    void acquireRoom(client, roomId).then(() => {
      if (cancelled) return;

      unsub = client.onPresenceChange<S>(roomId, (peers) => {
        store.snapshot = peers;
        emit(store);
      });
    });

    return () => {
      cancelled = true;
      unsub?.();
      releaseRoom(client, roomId);
      // Reset peers on leave so stale data is never shown.
      store.snapshot = EMPTY_PEERS as ReadonlyArray<PresencePeer<S>>;
      emit(store);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client, roomId]);

  // -----------------------------------------------------------------------
  // useSyncExternalStore wiring
  // -----------------------------------------------------------------------
  const subscribe = useCallback(
    (onStoreChange: () => void) => {
      store.listeners.add(onStoreChange);
      return () => {
        store.listeners.delete(onStoreChange);
      };
    },
    [store],
  );

  const getSnapshot = useCallback(() => store.snapshot, [store]);
  const getServerSnapshot = useCallback(
    () => EMPTY_PEERS as ReadonlyArray<PresencePeer<S>>,
    [],
  );

  const peers = useSyncExternalStore(subscribe, getSnapshot, getServerSnapshot);

  // -----------------------------------------------------------------------
  // Stable publish callback
  // -----------------------------------------------------------------------
  const publishState = useCallback(
    (state: S) => {
      clientRef.current.publishPresence<S>(roomId, state);
    },
    [roomId],
  );

  return { peers, publishState };
}
