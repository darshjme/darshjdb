/**
 * Main DarshJDB client class.
 *
 * Manages the connection lifecycle, transport selection, and MessagePack
 * encoding/decoding for all wire communication.
 *
 * @module client
 */

import { encode, decode } from '@msgpack/msgpack';
import type {
  DarshanConfig,
  ConnectionState,
  ConnectionStateListener,
  TransportMode,
  ClientMessage,
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
  private _privateBackoff = INITIAL_BACKOFF_MS;
  private _privateReconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private _privatePingTimer: ReturnType<typeof setInterval> | null = null;
  private _privateIntentionalClose = false;
  private _privateAuthToken: string | null = null;

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

    if (this.transport === 'rest') {
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
    this._privateRejectAllPending(new Error('Client disconnected'));
    this._privateSetState('disconnected');
  }

  /* -- Messaging ---------------------------------------------------------- */

  /**
   * Send a message to the server and await a correlated response.
   *
   * @param msg - Client message (the `id` field is auto-generated if absent).
   * @param timeoutMs - How long to wait for a response (default 10 000 ms).
   * @returns The correlated {@link ServerMessage}.
   */
  async send(msg: Omit<ClientMessage, 'id'>, timeoutMs = 10_000): Promise<ServerMessage> {
    const id = nextId();
    const fullMsg: ClientMessage = { ...msg, id };

    return new Promise<ServerMessage>((resolve, reject) => {
      const timer = setTimeout(() => {
        this._privatePendingRequests.delete(id);
        reject(new Error(`Request ${id} timed out after ${timeoutMs}ms`));
      }, timeoutMs);

      this._privatePendingRequests.set(id, { resolve, reject, timer });
      this._privateSendRaw(fullMsg);
    });
  }

  /**
   * Register a handler for server-pushed messages on a given subscription id.
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
   * Get the REST base URL (for the REST transport fallback).
   */
  getRestUrl(path: string): string {
    return `${this.serverUrl}/v1/apps/${this.appId}${path}`;
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

      const wsUrl = this.serverUrl
        .replace(/^http/, 'ws')
        .concat(`/v1/apps/${this.appId}/ws`);

      const socket = new WebSocket(wsUrl);
      socket.binaryType = 'arraybuffer';
      this._privateSocket = socket;

      socket.onopen = () => {
        this._privateBackoff = INITIAL_BACKOFF_MS;
        this._privateSetState('authenticating');
        this._privateAuthenticate()
          .then(() => {
            this._privateSetState('connected');
            this._privateStartPing();
            resolve();
          })
          .catch((err) => {
            this.disconnect();
            reject(err);
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
        if (this._privateIntentionalClose) return;
        this._privateSetState('reconnecting');
        this._privateScheduleReconnect();
        // If we never connected, reject the initial promise.
        if (this._privateState !== 'connected') {
          reject(new Error(`WebSocket closed: ${event.code} ${event.reason}`));
        }
      };
    });
  }

  private async _privateAuthenticate(): Promise<void> {
    if (!this._privateAuthToken) {
      // Anonymous connection; server must accept it or reject.
      const resp = await this.send({
        type: 'auth',
        payload: { appId: this.appId, anonymous: true },
      });
      if (resp.type === 'auth-error') {
        throw new Error(
          `Authentication failed: ${JSON.stringify(resp.payload)}`,
        );
      }
      return;
    }

    const resp = await this.send({
      type: 'auth',
      payload: { appId: this.appId, token: this._privateAuthToken },
    });
    if (resp.type === 'auth-error') {
      throw new Error(
        `Authentication failed: ${JSON.stringify(resp.payload)}`,
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

    // Correlated response?
    if (msg.id) {
      const pending = this._privatePendingRequests.get(msg.id);
      if (pending) {
        clearTimeout(pending.timer);
        this._privatePendingRequests.delete(msg.id);
        pending.resolve(msg);
        return;
      }

      // Subscription push?
      const handler = this._privateSubscriptionHandlers.get(msg.id);
      if (handler) {
        handler(msg);
        return;
      }
    }

    // Pong — no action needed.
    if (msg.type === 'pong') return;

    // Uncorrelated error — log as a warning in non-production.
    if (msg.type === 'error') {
      console.warn('[DarshJDB] Server error:', msg.payload);
    }
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

  /* -- Ping / keepalive --------------------------------------------------- */

  private _privateStartPing(): void {
    this._privatePingTimer = setInterval(() => {
      try {
        this._privateSendRaw({
          type: 'ping',
          id: nextId(),
          payload: null,
        });
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
