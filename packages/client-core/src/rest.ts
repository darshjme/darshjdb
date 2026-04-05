/**
 * REST/SSE transport fallback for DarshJDB.
 *
 * Provides the same query/mutation/subscription API surface as the WebSocket
 * transport but uses HTTP fetch for one-shot operations and Server-Sent Events
 * (EventSource) for live subscriptions.
 *
 * @module rest
 */

import type { DarshJDB } from './client.js';
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
  private _privateEventSources = new Map<string, EventSource>();
  private _privateSubCounter = 0;

  constructor(client: DarshJDB) {
    this._privateClient = client;
  }

  /* -- Query -------------------------------------------------------------- */

  /**
   * Execute a one-shot query via HTTP POST.
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
      body: JSON.stringify(descriptor),
    });

    const data = (await resp.json()) as { data: T[]; txId: string };
    return { data: data.data, txId: data.txId };
  }

  /* -- Transact ----------------------------------------------------------- */

  /**
   * Submit a mutation transaction via HTTP POST.
   *
   * @param ops - Array of transaction operations.
   * @returns The server-assigned transaction id.
   */
  async transact(ops: TxOp[]): Promise<TxId> {
    const resp = await this._privateFetch('/transact', {
      method: 'POST',
      body: JSON.stringify({ ops }),
    });

    const data = (await resp.json()) as { txId: string };
    return data.txId;
  }

  /* -- Subscribe (SSE) ---------------------------------------------------- */

  /**
   * Subscribe to live query updates via Server-Sent Events.
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

    const params = new URLSearchParams({
      query: JSON.stringify(descriptor),
    });

    const token = this._privateClient.getAuthToken();
    if (token) {
      params.set('token', token);
    }

    const url = this._privateClient.getRestUrl(
      `/subscribe?${params.toString()}`,
    );

    const eventSource = new EventSource(url);
    this._privateEventSources.set(subId, eventSource);

    eventSource.addEventListener('update', (event) => {
      try {
        const payload = JSON.parse(event.data) as {
          data: T[];
          txId: string;
        };
        callback({ data: payload.data, txId: payload.txId });
      } catch {
        /* malformed event data */
      }
    });

    eventSource.addEventListener('error', () => {
      // EventSource auto-reconnects by default.
      // We only log for debugging purposes.
      console.warn(`[DarshJDB REST] SSE error on subscription ${subId}`);
    });

    let closed = false;

    return () => {
      if (closed) return;
      closed = true;
      eventSource.close();
      this._privateEventSources.delete(subId);
    };
  }

  /* -- Cleanup ------------------------------------------------------------ */

  /**
   * Close all active SSE connections.
   */
  closeAll(): void {
    for (const [id, es] of this._privateEventSources) {
      es.close();
      this._privateEventSources.delete(id);
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
}
