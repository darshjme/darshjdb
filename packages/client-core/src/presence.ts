/**
 * Real-time presence system for DarshJDB.
 *
 * Provides join/leave/publish/subscribe semantics with a 50ms publish
 * throttle to prevent excessive network traffic.
 *
 * @module presence
 */

import type { DarshJDB } from './client.js';
import type {
  Peer,
  PresenceMember,
  PresenceSnapshot,
  PresenceCallback,
  ServerMessage,
  Unsubscribe,
} from './types.js';

/* -------------------------------------------------------------------------- */
/*  Constants                                                                 */
/* -------------------------------------------------------------------------- */

/** Minimum interval between publish calls (in milliseconds). */
const PUBLISH_THROTTLE_MS = 50;

/* -------------------------------------------------------------------------- */
/*  PresenceRoom                                                              */
/* -------------------------------------------------------------------------- */

/**
 * A presence room that tracks connected peers and their ephemeral state.
 *
 * @typeParam T - Shape of the per-peer state object.
 *
 * @example
 * ```ts
 * const room = new PresenceRoom<CursorState>(client, 'document-123');
 * await room.join();
 *
 * room.subscribe((snapshot) => {
 *   console.log('Peers:', snapshot.peers);
 * });
 *
 * room.publish({ x: 100, y: 200 });
 * ```
 */
export class PresenceRoom<T = Record<string, unknown>> {
  /** The room identifier. */
  readonly roomId: string;

  private _privateClient: DarshJDB;
  private _privateCallbacks = new Set<PresenceCallback<T>>();
  private _privateSnapshot: PresenceSnapshot<T> = {
    roomId: '',
    peers: [],
    self: null,
  };
  private _privateJoined = false;
  private _privateDisposeReconnect: (() => void) | null = null;

  /* Throttle state */
  private _privateLastPublish = 0;
  private _privatePendingState: T | null = null;
  private _privateThrottleTimer: ReturnType<typeof setTimeout> | null = null;

  constructor(client: DarshJDB, roomId: string) {
    this._privateClient = client;
    this.roomId = roomId;
    this._privateSnapshot = { roomId, peers: [], self: null };
  }

  /* -- Lifecycle ---------------------------------------------------------- */

  /**
   * Join the presence room.
   *
   * Registers the room handler and asks the server for the current snapshot.
   * The room is re-joined automatically after a WebSocket reconnect.
   *
   * @throws If already joined.
   */
  async join(state?: T): Promise<void> {
    if (this._privateJoined) {
      throw new Error(`Already joined room "${this.roomId}"`);
    }

    // The server pushes `pres-snap` / `pres-diff` keyed by room, not by a
    // correlation id, so the handler must be in place before we join.
    this._privateClient.registerPresenceHandler(
      this.roomId,
      (msg: ServerMessage) => {
        this._privateHandleUpdate(msg);
      },
    );

    this._privateClient.notify({
      type: 'pres-join',
      room: this.roomId,
      state: state ?? {},
    });

    this._privateJoined = true;
    this._privateDisposeReconnect = this._privateClient.onReconnected(() => {
      this._privateClient.notify({
        type: 'pres-join',
        room: this.roomId,
        state: this._privateSnapshot.self?.state ?? state ?? {},
      });
    });

    this._privateNotify();
  }

  /**
   * Leave the presence room.
   *
   * Notifies the server and cleans up local state.
   */
  async leave(): Promise<void> {
    if (!this._privateJoined) return;

    if (this._privateThrottleTimer) {
      clearTimeout(this._privateThrottleTimer);
      this._privateThrottleTimer = null;
    }

    this._privateDisposeReconnect?.();
    this._privateDisposeReconnect = null;
    this._privateClient.unregisterPresenceHandler(this.roomId);

    try {
      this._privateClient.notify({ type: 'pres-leave', room: this.roomId });
    } catch {
      /* best effort — server may already consider us gone */
    }

    this._privateJoined = false;
    this._privateSnapshot = { roomId: this.roomId, peers: [], self: null };
    this._privateNotify();
  }

  /* -- State publishing --------------------------------------------------- */

  /**
   * Publish ephemeral state to all peers in the room.
   *
   * Calls are throttled to at most once every 50ms. If called more
   * frequently, only the latest state is sent.
   *
   * @param state - The state object to broadcast.
   */
  publish(state: T): void {
    if (!this._privateJoined) {
      throw new Error(`Not joined to room "${this.roomId}"`);
    }

    const now = Date.now();
    const elapsed = now - this._privateLastPublish;

    if (elapsed >= PUBLISH_THROTTLE_MS) {
      this._privateSendPublish(state);
    } else {
      // Throttle: queue the latest state.
      this._privatePendingState = state;
      if (!this._privateThrottleTimer) {
        this._privateThrottleTimer = setTimeout(() => {
          this._privateThrottleTimer = null;
          if (this._privatePendingState !== null) {
            this._privateSendPublish(this._privatePendingState);
            this._privatePendingState = null;
          }
        }, PUBLISH_THROTTLE_MS - elapsed);
      }
    }
  }

  private _privateSendPublish(state: T): void {
    this._privateLastPublish = Date.now();
    try {
      this._privateClient.notify({
        type: 'pres-state',
        room: this.roomId,
        state,
      });
    } catch (err) {
      console.warn('[DarshJDB Presence] Publish error:', err);
    }

    // Optimistically update self.
    if (this._privateSnapshot.self) {
      this._privateSnapshot.self.state = state;
      this._privateSnapshot.self.lastSeen = Date.now();
      this._privateNotify();
    }
  }

  /* -- Subscription ------------------------------------------------------- */

  /**
   * Subscribe to presence changes in this room.
   *
   * The callback is invoked immediately with the current snapshot,
   * then on every subsequent change.
   *
   * @param callback - Invoked with the latest {@link PresenceSnapshot}.
   * @returns An unsubscribe function.
   */
  subscribe(callback: PresenceCallback<T>): Unsubscribe {
    this._privateCallbacks.add(callback);

    // Deliver current state immediately.
    try {
      callback(this._privateSnapshot);
    } catch {
      /* subscriber error */
    }

    return () => {
      this._privateCallbacks.delete(callback);
    };
  }

  /**
   * Get the current presence snapshot (for one-time reads).
   */
  getSnapshot(): PresenceSnapshot<T> {
    return this._privateSnapshot;
  }

  /**
   * Whether this room is currently joined.
   */
  get joined(): boolean {
    return this._privateJoined;
  }

  /* -- Internal ----------------------------------------------------------- */

  private _privateHandleUpdate(msg: ServerMessage): void {
    if (msg.type === 'pres-snap') {
      this._privateSnapshot = {
        roomId: this.roomId,
        peers: msg.members.map((m) => this._privateToPeer(m)),
        self: this._privateSnapshot.self,
      };
      this._privateNotify();
      return;
    }

    if (msg.type !== 'pres-diff') return;

    const left = new Set(msg.left ?? []);
    const updated = new Map(
      (msg.updated ?? []).map((m) => [m.user_id, this._privateToPeer(m)]),
    );

    const peers = this._privateSnapshot.peers
      .filter((p) => !left.has(p.peerId))
      .map((p) => updated.get(p.peerId) ?? p);

    const known = new Set(peers.map((p) => p.peerId));
    for (const m of msg.joined ?? []) {
      if (!known.has(m.user_id)) peers.push(this._privateToPeer(m));
    }

    this._privateSnapshot = {
      roomId: this.roomId,
      peers,
      self: this._privateSnapshot.self,
    };
    this._privateNotify();
  }

  private _privateToPeer(member: PresenceMember): Peer<T> {
    return {
      peerId: member.user_id,
      userId: member.user_id,
      state: member.state as T,
      lastSeen: Date.now(),
    };
  }

  private _privateNotify(): void {
    for (const cb of this._privateCallbacks) {
      try {
        cb(this._privateSnapshot);
      } catch {
        /* subscriber errors must not break the notification loop */
      }
    }
  }
}
