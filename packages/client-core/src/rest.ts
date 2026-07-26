/**
 * REST/SSE transport fallback for DarshJDB.
 *
 * Provides the same query/mutation/subscription API surface as the WebSocket
 * transport but uses HTTP fetch for one-shot operations and Server-Sent Events
 * for live subscriptions.
 *
 * @module rest
 */

import type { DarshJDB } from './client.js';
import { toDarshJQL } from './query.js';
import { toServerMutations } from './transaction.js';
import type {
  QueryDescriptor,
  QueryResult,
  TxOp,
  TxId,
  SubscriptionCallback,
  Unsubscribe,
} from './types.js';

/* -------------------------------------------------------------------------- */
/*  RestTransport                                                             */
/* -------------------------------------------------------------------------- */

/**
 * REST + SSE transport that mirrors the WebSocket client API surface.
 *
 * Use this when WebSocket connections are unavailable (e.g. restricted
 * network, serverless environments).
 *
 * @example
 * ```ts
 * const rest = new RestTransport(db);
 *
 * const result = await rest.query<User>({
 *   collection: 'users',
 *   where: [{ field: 'age', op: '>=', value: 18 }],
 * });
 *
 * const unsub = rest.subscribe<User>(
 *   { collection: 'users' },
 *   (result) => console.log(result.data),
 * );
 * ```
 */
export class RestTransport {
  private _privateClient: DarshJDB;
  private _privateStreams = new Map<string, AbortController>();
  private _privateSubCounter = 0;

  constructor(client: DarshJDB) {
    this._privateClient = client;
  }

  /* -- Query -------------------------------------------------------------- */

  /**
   * Execute a one-shot query via `POST /api/query`.
   *
   * @typeParam T - Expected document shape.
   * @param descriptor - The query descriptor.
   * @returns The query result set.
   */
  async query<T = Record<string, unknown>>(
    descriptor: QueryDescriptor,
  ): Promise<QueryResult<T>> {
    const resp = await this._privateFetch('/query', {
      method: 'POST',
      body: JSON.stringify({ query: toDarshJQL(descriptor) }),
    });

    const data = (await resp.json()) as { data: T[] };
    return { data: data.data, txId: '' };
  }

  /* -- Transact ----------------------------------------------------------- */

  /**
   * Submit a mutation transaction via `POST /api/mutate`.
   *
   * @param ops - Array of transaction operations.
   * @returns The server-assigned transaction id.
   */
  async transact(ops: TxOp[]): Promise<TxId> {
    const resp = await this._privateFetch('/mutate', {
      method: 'POST',
      body: JSON.stringify({ mutations: toServerMutations(ops) }),
    });

    const data = (await resp.json()) as { tx_id: number | string };
    return String(data.tx_id);
  }

  /* -- Subscribe (SSE) ---------------------------------------------------- */

  /**
   * Subscribe to live query updates via `GET /api/subscribe`.
   *
   * The server's SSE stream carries change notifications rather than result
   * sets, so every notification triggers a re-query and the fresh rows are
   * handed to the callback.
   *
   * @typeParam T - Expected document shape.
   * @param descriptor - The query descriptor.
   * @param callback   - Invoked on every result update.
   * @returns An unsubscribe function that closes the SSE connection.
   */
  subscribe<T = Record<string, unknown>>(
    descriptor: QueryDescriptor,
    callback: SubscriptionCallback<T>,
  ): Unsubscribe {
    const subId = `rest_sub_${(++this._privateSubCounter).toString(36)}`;
    const controller = new AbortController();
    this._privateStreams.set(subId, controller);

    const emit = (): void => {
      this.query<T>(descriptor)
        .then((result) => {
          if (!controller.signal.aborted) callback(result);
        })
        .catch((err: unknown) => {
          if (!controller.signal.aborted) {
            console.warn(`[DarshJDB REST] Query failed for ${subId}:`, err);
          }
        });
    };

    // Deliver the current result set, then follow the change stream.
    emit();

    const params = new URLSearchParams({
      q: JSON.stringify(toDarshJQL(descriptor)),
    });

    void this._privateStream(
      `/subscribe?${params.toString()}`,
      controller.signal,
      (event) => {
        if (event === 'update') emit();
      },
    ).catch((err: unknown) => {
      if (!controller.signal.aborted) {
        console.warn(`[DarshJDB REST] SSE error on subscription ${subId}:`, err);
      }
    });

    let closed = false;

    return () => {
      if (closed) return;
      closed = true;
      controller.abort();
      this._privateStreams.delete(subId);
    };
  }

  /* -- Cleanup ------------------------------------------------------------ */

  /**
   * Close all active SSE connections.
   */
  closeAll(): void {
    for (const [id, controller] of this._privateStreams) {
      controller.abort();
      this._privateStreams.delete(id);
    }
  }

  /* -- Internal ----------------------------------------------------------- */

  private async _privateFetch(
    path: string,
    init: RequestInit,
  ): Promise<Response> {
    const headers: Record<string, string> = {
      'Content-Type': 'application/json',
    };

    const token = this._privateClient.getAuthToken();
    if (token) {
      headers['Authorization'] = `Bearer ${token}`;
    }

    const resp = await fetch(this._privateClient.getRestUrl(path), {
      ...init,
      headers: { ...headers, ...(init.headers as Record<string, string>) },
    });

    if (!resp.ok) {
      const body = await resp.text();
      throw new Error(`REST request failed (${resp.status}): ${body}`);
    }

    return resp;
  }

  /**
   * Consume an SSE endpoint over `fetch`.
   *
   * `EventSource` cannot carry an `Authorization` header and the server
   * authenticates every subscription with a bearer token, so the stream is
   * read and framed manually.
   */
  private async _privateStream(
    path: string,
    signal: AbortSignal,
    onEvent: (event: string, data: string) => void,
  ): Promise<void> {
    const headers: Record<string, string> = { Accept: 'text/event-stream' };
    const token = this._privateClient.getAuthToken();
    if (token) {
      headers['Authorization'] = `Bearer ${token}`;
    }

    const resp = await fetch(this._privateClient.getRestUrl(path), {
      headers,
      signal,
    });

    if (!resp.ok) {
      const body = await resp.text();
      throw new Error(`SSE request failed (${resp.status}): ${body}`);
    }
    if (!resp.body) {
      throw new Error('SSE response has no body');
    }

    const reader = resp.body.getReader();
    const decoder = new TextDecoder();
    let buffer = '';

    while (!signal.aborted) {
      const { done, value } = await reader.read();
      if (done) break;

      buffer += decoder.decode(value, { stream: true });

      let boundary = buffer.indexOf('\n\n');
      while (boundary !== -1) {
        const frame = buffer.slice(0, boundary);
        buffer = buffer.slice(boundary + 2);
        boundary = buffer.indexOf('\n\n');

        let eventName = 'message';
        const dataLines: string[] = [];

        for (const line of frame.split('\n')) {
          if (line.startsWith(':')) continue; // heartbeat comment
          if (line.startsWith('event:')) {
            eventName = line.slice(6).trim();
          } else if (line.startsWith('data:')) {
            dataLines.push(line.slice(5).trim());
          }
        }

        if (dataLines.length > 0) {
          onEvent(eventName, dataLines.join('\n'));
        }
      }
    }
  }
}
