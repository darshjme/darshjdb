/**
 * Live query stream implementation using WebSocket.
 *
 * Provides an EventEmitter-style API for subscribing to real-time
 * changes from DarshJDB.
 */

import {
  DarshDBAuthError,
  DarshDBConnectionError,
  DarshDBError,
  DarshDBQueryError,
  LiveAction,
  type LiveNotification,
  type LiveStream,
} from "./types.js";

type Listener<T> = (data: T) => void;

/**
 * WebSocket-backed live query stream.
 *
 * Connects to the DarshJDB WebSocket endpoint, authenticates,
 * subscribes to a query, and emits change/error events.
 */
export class LiveQueryStream<T = Record<string, unknown>>
  implements LiveStream<T>
{
  private ws: WebSocket | null = null;
  private changeListeners: Listener<LiveNotification<T>>[] = [];
  private errorListeners: Listener<Error>[] = [];
  private onceChangeListeners: Set<Listener<LiveNotification<T>>> = new Set();
  private closed = false;

  constructor(
    private readonly wsUrl: string,
    private readonly token: string | null,
    private readonly query: string,
  ) {
    this.connect();
  }

  on(event: "change", callback: Listener<LiveNotification<T>>): void;
  on(event: "error", callback: Listener<Error>): void;
  on(event: string, callback: Listener<unknown>): void {
    if (event === "change") {
      this.changeListeners.push(
        callback as Listener<LiveNotification<T>>,
      );
    } else if (event === "error") {
      this.errorListeners.push(callback as Listener<Error>);
    }
  }

  once(event: "change", callback: Listener<LiveNotification<T>>): void;
  once(event: string, callback: Listener<unknown>): void {
    if (event === "change") {
      const cb = callback as Listener<LiveNotification<T>>;
      this.onceChangeListeners.add(cb);
      this.changeListeners.push(cb);
    }
  }

  off(event: string, callback: Listener<unknown>): void {
    if (event === "change") {
      this.changeListeners = this.changeListeners.filter(
        (cb) => cb !== callback,
      );
      this.onceChangeListeners.delete(
        callback as Listener<LiveNotification<T>>,
      );
    } else if (event === "error") {
      this.errorListeners = this.errorListeners.filter(
        (cb) => cb !== callback,
      );
    }
  }

  close(): void {
    this.closed = true;
    if (this.ws) {
      this.ws.close();
      this.ws = null;
    }
    this.changeListeners = [];
    this.errorListeners = [];
    this.onceChangeListeners.clear();
  }

  // -----------------------------------------------------------------------
  //  Internal
  // -----------------------------------------------------------------------

  private connect(): void {
    if (this.closed) return;

    try {
      this.ws = new WebSocket(this.wsUrl);
    } catch (err) {
      this.emitError(
        new DarshDBConnectionError(
          `Failed to connect to WebSocket: ${err}`,
        ),
      );
      return;
    }

    this.ws.onopen = () => {
      // Authenticate
      if (this.token) {
        this.ws?.send(
          JSON.stringify({ type: "auth", token: this.token }),
        );
      } else {
        // No token — subscribe directly
        this.sendSubscribe();
      }
    };

    this.ws.onmessage = (event) => {
      try {
        const msg = JSON.parse(String(event.data)) as Record<
          string,
          unknown
        >;
        this.handleMessage(msg);
      } catch {
        // Ignore non-JSON messages
      }
    };

    this.ws.onerror = () => {
      this.emitError(
        new DarshDBConnectionError("WebSocket connection error"),
      );
    };

    this.ws.onclose = () => {
      if (!this.closed) {
        this.emitError(
          new DarshDBConnectionError("WebSocket connection closed"),
        );
      }
    };
  }

  private sendSubscribe(): void {
    const isQuery = this.query.trim().toUpperCase().startsWith("SELECT");
    const queryPayload = isQuery
      ? this.query
      : `SELECT * FROM ${this.query}`;

    this.ws?.send(
      JSON.stringify({
        type: "sub",
        id: `live_${Date.now()}`,
        query: { query: queryPayload },
      }),
    );
  }

  private handleMessage(msg: Record<string, unknown>): void {
    const type = msg["type"] as string;

    switch (type) {
      case "auth-ok":
        this.sendSubscribe();
        break;

      case "auth-err":
        this.emitError(
          new DarshDBAuthError(
            (msg["error"] as string) ?? "WebSocket auth failed",
          ),
        );
        this.close();
        break;

      case "sub-ok":
        // Subscription confirmed — initial data may be in msg.initial
        break;

      case "sub-err":
        this.emitError(
          new DarshDBQueryError(
            (msg["error"] as string) ?? "Subscription failed",
            this.query,
          ),
        );
        break;

      case "diff": {
        const changes = (msg["changes"] ?? {}) as Record<string, unknown[]>;
        const actionMap: Record<string, LiveAction> = {
          inserted: LiveAction.Create,
          updated: LiveAction.Update,
          deleted: LiveAction.Delete,
        };
        for (const [key, action] of Object.entries(actionMap)) {
          const records = (changes[key] ?? []) as T[];
          for (const record of records) {
            this.emitChange({ action, result: record });
          }
        }
        break;
      }

      case "pub-event": {
        const eventType = (msg["event"] as string) ?? "updated";
        const actionMapPub: Record<string, LiveAction> = {
          created: LiveAction.Create,
          updated: LiveAction.Update,
          deleted: LiveAction.Delete,
        };
        this.emitChange({
          action: actionMapPub[eventType] ?? LiveAction.Update,
          result: msg as unknown as T,
        });
        break;
      }

      case "pong":
        break;

      case "error":
        this.emitError(
          new DarshDBError(
            (msg["error"] as string) ?? "Unknown WebSocket error",
          ),
        );
        break;
    }
  }

  private emitChange(notification: LiveNotification<T>): void {
    for (const listener of [...this.changeListeners]) {
      listener(notification);
      if (this.onceChangeListeners.has(listener)) {
        this.onceChangeListeners.delete(listener);
        this.changeListeners = this.changeListeners.filter(
          (cb) => cb !== listener,
        );
      }
    }
  }

  private emitError(error: Error): void {
    for (const listener of this.errorListeners) {
      listener(error);
    }
  }
}
