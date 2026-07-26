/**
 * Main DarshJDB client class.
 *
 * Manages the connection lifecycle, transport selection, and MessagePack
 * encoding/decoding for all wire communication.
 *
 * @module client
 */

import { encode, decode } from '@msgpack/msgpack';
import { RestTransport } from './rest.js';
import type {
  DarshanConfig,
  ConnectionState,
  ConnectionStateListener,
  TransportMode,
  ClientMessage,
  ClientNotification,
  ClientRequest,
  ServerMessage,
} from './types.js';

/* -------------------------------------------------------------------------- */
/*  Constants                                                                 */
/* -------------------------------------------------------------------------- */

const INITIAL_BACKOFF_MS = 500;
const MAX_BACKOFF_MS = 30_000;
const JITTER_FACTOR = 0.3;
const PING_INTERVAL_MS = 25_000;

/* -------------------------------------------------------------------------- */
/*  Helpers                                                                   */
/* -------------------------------------------------------------------------- */

/** Add random jitter to a delay value. */
function withJitter(delay: number): number {
  const jitter = delay * JITTER_FACTOR * (Math.random() * 2 - 1);
  return Math.max(0, delay + jitter);
}

/** Generate a short unique message id. */
let _privateCounter = 0;
function nextId(): string {
  return `m_${Date.now().toString(36)}_${(++_privateCounter).toString(36)}`;
}

/* -------------------------------------------------------------------------- */
/*  DarshJDB Client                                                         */
/* -------------------------------------------------------------------------- */

/**
 * Core DarshJDB client.
 *
 * @example
 * ```ts
 * const db = new DarshJDB({
 *   serverUrl: 'https://db.example.com',
 *   appId: 'my-app',
 * });
 * await db.connect();
 * ```
 */
export class DarshJDB {
  /** Server base URL (no trailing slash). */
  readonly serverUrl: string;

  /** Application identifier. */
  readonly appId: string;

  /** Resolved transport mode. */
  readonly transport: TransportMode;

  /* -- Internal state ----------------------------------------------------- */

  private _privateState: ConnectionState = 'disconnected';
  private _privateSocket: WebSocket | null = null;
  private _privateListeners = new Set<ConnectionStateListener>();
  private _privatePendingRequests = new Map<
    string,
    {
      resolve: (msg: ServerMessage) => void;
      reject: (err: Error) => void;
      timer: ReturnType<typeof setTimeout>;
    }
  >();
  private _privateSubscriptionHandlers = new Map<
    string,
    (msg: ServerMessage) => void
  >();
  private _privatePresenceHandlers = new Map<
    string,
    (msg: ServerMessage) => void
  >();
  private _privateReconnectListeners = new Set<() => void | Promise<void>>();
  private _privateBackoff = INITIAL_BACKOFF_MS;
  private _privateReconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private _privatePingTimer: ReturnType<typeof setInterval> | null = null;
  private _privateIntentionalClose = false;
  private _privateAuthToken: string | null = null;
  private _privateHasConnected = false;
  private _privateRest: RestTransport | null = null;

  constructor(config: DarshanConfig) {
    this.serverUrl = config.serverUrl.replace(/\/+$/, '');
    this.appId = config.appId;
    this.transport = config.transport ?? 'auto';
  }

  /* -- Connection state --------------------------------------------------- */

  /** Current connection state. */
  get state(): ConnectionState {
    return this._privateState;
  }

  /**
   * Register a listener for connection state transitions.
   *
   * @returns A function that removes the listener when called.
   */
  onConnectionStateChange(listener: ConnectionStateListener): () => void {
    this._privateListeners.add(listener);
    return () => {
      this._privateListeners.delete(listener);
    };
  }

  private _privateSetState(next: ConnectionState): void {
    if (next === this._privateState) return;
    const prev = this._privateState;
    this._privateState = next;
    for (const fn of this._privateListeners) {
      try {
        fn(next, prev);
      } catch {
        /* listener errors must not break state machine */
      }
    }
  }

  /* -- Transport selection ------------------------------------------------ */

  /**
   * Whether query/transact traffic is carried over HTTP + SSE instead of
   * the WebSocket. True only for `transport: 'rest'`.
   */
  get usesRest(): boolean {
    return this.transport === 'rest';
  }

  /**
   * The REST/SSE transport for this client, created on first access.
   * Used by the query and transaction layers when {@link usesRest} is true.
   */
  get rest(): RestTransport {
    if (!this._privateRest) {
      this._privateRest = new RestTransport(this);
    }
    return this._privateRest;
  }

  /* -- Auth token (set by auth module) ------------------------------------ */

  /**
   * Set the access token used for authenticating the connection.
   * Typically called by the auth module after sign-in.
   */
  setAuthToken(token: string | null): void {
    this._privateAuthToken = token;
  }

  /* -- Connect / Disconnect ----------------------------------------------- */

  /**
   * Open a connection to the DarshJDB server.
   *
   * Resolves once the connection reaches the `connected` state or rejects
   * if the initial connection fails.
   */
  async connect(): Promise<void> {
    if (
      this._privateState === 'connected' ||
      this._privateState === 'connecting'
    ) {
      return;
    }

    if (this.usesRest) {
      // REST mode has no persistent connection; mark as connected immediately.
      this._privateSetState('connected');
      return;
    }

    return this._privateOpenWebSocket();
  }

  /**
   * Gracefully close the connection.
   */
  disconnect(): void {
    this._privateIntentionalClose = true;
    this._privateClearTimers();
    if (this._privateSocket) {
      this._privateSocket.close(1000, 'client disconnect');
      this._privateSocket = null;
    }
    if (this._privateRest) {
      this._privateRest.closeAll();
    }
    this._privateRejectAllPending(new Error('Client disconnected'));
    this._privateSetState('disconnected');
  }

  /* -- Messaging ---------------------------------------------------------- */

  /**
   * Send a request frame to the server and await its correlated reply.
   *
   * @param msg - Client request frame (the `id` field is auto-generated).
   * @param timeoutMs - How long to wait for a response (default 10 000 ms).
   * @returns The correlated {@link ServerMessage}.
   */
  async send(
    msg: ClientRequest | Extract<ClientMessage, { type: 'auth' }>,
    timeoutMs = 10_000,
  ): Promise<ServerMessage> {
    const id = nextId();
    // The `auth` frame is the one request the server answers without echoing
    // a correlation id, so it is sent verbatim.
    const fullMsg = (
      msg.type === 'auth' ? msg : { ...msg, id }
    ) as ClientMessage;

    return new Promise<ServerMessage>((resolve, reject) => {
      const timer = setTimeout(() => {
        this._privatePendingRequests.delete(id);
        reject(new Error(`Request ${id} timed out after ${timeoutMs}ms`));
      }, timeoutMs);

      this._privatePendingRequests.set(id, { resolve, reject, timer });

      try {
        this._privateSendRaw(fullMsg);
      } catch (err) {
        clearTimeout(timer);
        this._privatePendingRequests.delete(id);
        reject(err as Error);
      }
    });
  }

  /**
   * Send a frame the server never replies to (`ping`, presence updates).
   */
  notify(msg: ClientNotification): void {
    this._privateSendRaw(msg);
  }

  /**
   * Register a handler for server-pushed frames carrying a given `sub_id`.
   */
  registerSubscriptionHandler(
    subId: string,
    handler: (msg: ServerMessage) => void,
  ): void {
    this._privateSubscriptionHandlers.set(subId, handler);
  }

  /**
   * Remove a subscription handler.
   */
  unregisterSubscriptionHandler(subId: string): void {
    this._privateSubscriptionHandlers.delete(subId);
  }

  /**
   * Register a handler for the server's presence frames for a given room.
   */
  registerPresenceHandler(
    room: string,
    handler: (msg: ServerMessage) => void,
  ): void {
    this._privatePresenceHandlers.set(room, handler);
  }

  /**
   * Remove a presence handler.
   */
  unregisterPresenceHandler(room: string): void {
    this._privatePresenceHandlers.delete(room);
  }

  /**
   * Register a callback invoked after every successful reconnect.
   *
   * Server-side subscriptions live on the socket, so they are lost when it
   * drops. The query and presence layers use this hook to re-establish them.
   *
   * @returns A function that removes the callback when called.
   */
  onReconnected(callback: () => void | Promise<void>): () => void {
    this._privateReconnectListeners.add(callback);
    return () => {
      this._privateReconnectListeners.delete(callback);
    };
  }

  /**
   * Build a URL for the server's REST API, which is mounted at `/api`.
   */
  getRestUrl(path: string): string {
    return `${this.serverUrl}/api${path}`;
  }

  /**
   * Get the current auth token (may be null).
   */
  getAuthToken(): string | null {
    return this._privateAuthToken;
  }

  /* -- WebSocket internals ------------------------------------------------ */

  private _privateOpenWebSocket(): Promise<void> {
    return new Promise<void>((resolve, reject) => {
      this._privateSetState('connecting');
      this._privateIntentionalClose = false;

      let settled = false;
      const settle = (err?: Error): void => {
        if (settled) return;
        settled = true;
        if (err) reject(err);
        else resolve();
      };

      const wsUrl = this.serverUrl.replace(/^http/, 'ws').concat('/ws');

      const socket = new WebSocket(wsUrl);
      socket.binaryType = 'arraybuffer';
      this._privateSocket = socket;

      socket.onopen = () => {
        this._privateBackoff = INITIAL_BACKOFF_MS;
        this._privateSetState('authenticating');
        this._privateAuthenticate()
          .then(() => {
            const isReconnect = this._privateHasConnected;
            this._privateHasConnected = true;
            this._privateSetState('connected');
            this._privateStartPing();
            if (isReconnect) this._privateNotifyReconnected();
            settle();
          })
          .catch((err: Error) => {
            this.disconnect();
            settle(err);
          });
      };

      socket.onmessage = (event) => {
        this._privateHandleMessage(event.data as ArrayBuffer);
      };

      socket.onerror = () => {
        /* error details are deliberately hidden by browsers */
      };

      socket.onclose = (event) => {
        this._privateClearTimers();
        this._privateRejectAllPending(
          new Error(`WebSocket closed: ${event.code} ${event.reason}`),
        );
        if (this._privateIntentionalClose) return;
        this._privateSetState('reconnecting');
        this._privateScheduleReconnect();
        // If we never reached `connected`, reject the initial promise.
        settle(new Error(`WebSocket closed: ${event.code} ${event.reason}`));
      };
    });
  }

  private async _privateAuthenticate(): Promise<void> {
    if (!this._privateAuthToken) {
      throw new Error(
        'Cannot authenticate: no access token. Sign in first, or call setAuthToken().',
      );
    }

    const resp = await this.send({
      type: 'auth',
      token: this._privateAuthToken,
    });

    if (resp.type !== 'auth-ok') {
      throw new Error(
        `Authentication failed: ${resp.type === 'auth-err' ? resp.error : resp.type}`,
      );
    }
  }

  private _privateSendRaw(msg: ClientMessage): void {
    if (!this._privateSocket || this._privateSocket.readyState !== WebSocket.OPEN) {
      throw new Error('Cannot send: WebSocket is not open');
    }
    const encoded = encode(msg);
    this._privateSocket.send(encoded);
  }

  private _privateHandleMessage(raw: ArrayBuffer): void {
    const msg = decode(new Uint8Array(raw)) as ServerMessage;

    // The `auth` frame carries no id, so its reply cannot be correlated by
    // one. Resolve the single in-flight request instead.
    if (msg.type === 'auth-ok' || msg.type === 'auth-err') {
      this._privateResolveOldest(msg);
      return;
    }

    // Correlated response?
    if ('id' in msg && msg.id) {
      const pending = this._privatePendingRequests.get(msg.id);
      if (pending) {
        clearTimeout(pending.timer);
        this._privatePendingRequests.delete(msg.id);
        pending.resolve(msg);
        return;
      }
    }

    // Subscription push, keyed by the server-assigned sub_id.
    if (msg.type === 'sub' || msg.type === 'diff') {
      const handler = this._privateSubscriptionHandlers.get(msg.sub_id);
      if (handler) handler(msg);
      return;
    }

    // Presence push, keyed by room.
    if (msg.type === 'pres-snap' || msg.type === 'pres-diff') {
      const handler = this._privatePresenceHandlers.get(msg.room);
      if (handler) handler(msg);
      return;
    }

    // Pong — no action needed.
    if (msg.type === 'pong') return;

    // Uncorrelated error — log as a warning in non-production.
    if (msg.type === 'error') {
      console.warn('[DarshJDB] Server error:', msg.error);
    }
  }

  /** Resolve the oldest in-flight request with an uncorrelated reply. */
  private _privateResolveOldest(msg: ServerMessage): void {
    const first = this._privatePendingRequests.entries().next();
    if (first.done) return;
    const [id, pending] = first.value;
    clearTimeout(pending.timer);
    this._privatePendingRequests.delete(id);
    pending.resolve(msg);
  }

  /* -- Reconnection ------------------------------------------------------- */

  private _privateScheduleReconnect(): void {
    const delay = withJitter(this._privateBackoff);
    this._privateBackoff = Math.min(this._privateBackoff * 2, MAX_BACKOFF_MS);

    this._privateReconnectTimer = setTimeout(() => {
      this._privateOpenWebSocket().catch(() => {
        // Will trigger onclose → scheduleReconnect again.
      });
    }, delay);
  }

  private _privateNotifyReconnected(): void {
    for (const fn of this._privateReconnectListeners) {
      try {
        void Promise.resolve(fn()).catch((err: unknown) => {
          console.warn('[DarshJDB] Re-subscribe failed after reconnect:', err);
        });
      } catch (err) {
        console.warn('[DarshJDB] Re-subscribe failed after reconnect:', err);
      }
    }
  }

  /* -- Ping / keepalive --------------------------------------------------- */

  private _privateStartPing(): void {
    this._privatePingTimer = setInterval(() => {
      try {
        this._privateSendRaw({ type: 'ping' });
      } catch {
        /* swallow – onclose will handle reconnection */
      }
    }, PING_INTERVAL_MS);
  }

  /* -- Cleanup helpers ---------------------------------------------------- */

  private _privateClearTimers(): void {
    if (this._privateReconnectTimer) {
      clearTimeout(this._privateReconnectTimer);
      this._privateReconnectTimer = null;
    }
    if (this._privatePingTimer) {
      clearInterval(this._privatePingTimer);
      this._privatePingTimer = null;
    }
  }

  private _privateRejectAllPending(err: Error): void {
    for (const [id, { reject, timer }] of this._privatePendingRequests) {
      clearTimeout(timer);
      reject(err);
      this._privatePendingRequests.delete(id);
    }
  }
}

/**
 * Encode a value using MessagePack.
 *
 * @param value - Any serialisable value.
 * @returns Encoded bytes.
 */
export function msgpackEncode(value: unknown): Uint8Array {
  return encode(value);
}

/**
 * Decode a MessagePack buffer.
 *
 * @param buffer - Encoded bytes.
 * @returns Decoded value.
 */
export function msgpackDecode(buffer: Uint8Array | ArrayBuffer): unknown {
  return decode(buffer instanceof Uint8Array ? buffer : new Uint8Array(buffer));
}
