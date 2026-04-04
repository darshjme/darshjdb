/**
 * Real-time presence system for DarshanDB.
 *
 * Provides join/leave/publish/subscribe semantics with a 50ms publish
 * throttle to prevent excessive network traffic.
 *
 * @module presence
 */

import type { DarshanDB } from './client.js';
import type {
  Peer,
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

  private _privateClient: DarshanDB;
  private _privateCallbacks = new Set<PresenceCallback<T>>();
  private _privateSnapshot: PresenceSnapshot<T> = {
    roomId: '',
    peers: [],
    self: null,
  };
  private _privateJoined = false;
  private _privateSubId: string | null = null;

  /* Throttle state */
  private _privateLastPublish = 0;
  private _privatePendingState: T | null = null;
  private _privateThrottleTimer: ReturnType<typeof setTimeout> | null = null;

  constructor(client: DarshanDB, roomId: string) {
    this._privateClient = client;
    this.roomId = roomId;
    this._privateSnapshot = { roomId, peers: [], self: null };
  }

  /* -- Lifecycle ---------------------------------------------------------- */

  /**
   * Join the presence room.
   *
   * Registers a server-side subscription and begins receiving peer updates.
   *
   * @throws If already joined.
   */
  async join(): Promise<void> {
    if (this._privateJoined) {
      throw new Error(`Already joined room "${this.roomId}"`);
    }

    const resp = await this._privateClient.send({
      type: 'presence-join',
      payload: { roomId: this.roomId },
    });

    const payload = resp.payload as {
      subId: string;
      peers: Peer<T>[];
      self: Peer<T>;
    };

    this._privateSubId = payload.subId;
    this._privateSnapshot = {
      roomId: this.roomId,
      peers: payload.peers,
      self: payload.self,
    };
    this._privateJoined = true;

    // Register push handler for presence updates.
    this._privateClient.registerSubscriptionHandler(
      this._privateSubId,
      (msg: ServerMessage) => {
        this._privateHandleUpdate(msg);
      },
    );

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

    if (this._privateSubId) {
      this._privateClient.unregisterSubscriptionHandler(this._privateSubId);
    }

    try {
      await this._privateClient.send({
        type: 'presence-leave',
        payload: { roomId: this.roomId },
      });
    } catch {
      /* best effort — server may already consider us gone */
    }

    this._privateJoined = false;
    this._privateSubId = null;
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
    this._privateClient
      .send({
        type: 'presence-publish',
        payload: { roomId: this.roomId, state },
      })
      .catch((err) => {
        console.warn('[DarshanDB Presence] Publish error:', err);
      });

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
    const payload = msg.payload as {
      peers: Peer<T>[];
      self: Peer<T> | null;
    };

    this._privateSnapshot = {
      roomId: this.roomId,
      peers: payload.peers,
      self: payload.self ?? this._privateSnapshot.self,
    };
    this._privateNotify();
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
