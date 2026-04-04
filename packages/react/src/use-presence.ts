/**
 * @module use-presence
 * @description Hook for real-time presence in a DarshanDB room.
 * Peers automatically join on mount and leave on unmount.
 *
 * @example
 * ```tsx
 * import { usePresence } from '@darshan/react';
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
import type { PresencePeer, Unsubscribe } from './types';

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

    void client.joinRoom(roomId).then(() => {
      if (cancelled) {
        // Already unmounted before join resolved.
        void client.leaveRoom(roomId);
        return;
      }

      unsub = client.onPresenceChange<S>(roomId, (peers) => {
        store.snapshot = peers;
        emit(store);
      });
    });

    return () => {
      cancelled = true;
      unsub?.();
      void client.leaveRoom(roomId);
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
