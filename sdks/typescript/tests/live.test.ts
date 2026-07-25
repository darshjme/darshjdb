/**
 * Tests for the live query stream.
 *
 * Uses a fake WebSocket to assert the wire contract the server speaks
 * (see `ClientMessage` / `ServerMessage` in packages/server/src/api/ws.rs).
 */

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { DarshDB } from "../src/client.js";
import { LiveQueryStream } from "../src/live.js";
import { LiveAction, type LiveNotification } from "../src/types.js";

class FakeWebSocket {
  static instances: FakeWebSocket[] = [];

  onopen: (() => void) | null = null;
  onmessage: ((event: { data: string }) => void) | null = null;
  onerror: (() => void) | null = null;
  onclose: (() => void) | null = null;
  readonly sent: string[] = [];
  closed = false;

  constructor(public readonly url: string) {
    FakeWebSocket.instances.push(this);
  }

  send(data: string): void {
    this.sent.push(data);
  }

  close(): void {
    this.closed = true;
  }

  /** Deliver a server frame to the stream. */
  emit(msg: Record<string, unknown>): void {
    this.onmessage?.({ data: JSON.stringify(msg) });
  }
}

beforeEach(() => {
  FakeWebSocket.instances = [];
  vi.stubGlobal("WebSocket", FakeWebSocket);
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("LiveQueryStream subscribe", () => {
  it("sends a DarshanQL query object in the sub frame", () => {
    new LiveQueryStream("ws://localhost:8080/ws", null, { type: "users" });

    const ws = FakeWebSocket.instances[0]!;
    ws.onopen?.();

    const frame = JSON.parse(ws.sent[0]!);
    expect(frame.type).toBe("sub");
    expect(frame.id).toMatch(/^live_/);
    expect(frame.query).toEqual({ type: "users" });
  });

  it("authenticates before subscribing when a token is set", () => {
    new LiveQueryStream("ws://localhost:8080/ws", "tok", { type: "users" });

    const ws = FakeWebSocket.instances[0]!;
    ws.onopen?.();
    expect(JSON.parse(ws.sent[0]!)).toEqual({ type: "auth", token: "tok" });

    ws.emit({ type: "auth-ok", session_id: "s1" });
    expect(JSON.parse(ws.sent[1]!).type).toBe("sub");
  });

  it("db.live() converts a table name into DarshanQL", async () => {
    const db = new DarshDB("http://localhost:8080");
    await db.live("users");

    const ws = FakeWebSocket.instances[0]!;
    expect(ws.url).toBe("ws://localhost:8080/ws");
    ws.onopen?.();
    expect(JSON.parse(ws.sent[0]!).query).toEqual({ type: "users" });
  });
});

describe("LiveQueryStream change events", () => {
  it("emits changes from the server's 'sub' frame", () => {
    const stream = new LiveQueryStream("ws://localhost:8080/ws", null, {
      type: "users",
    });
    const changes: LiveNotification[] = [];
    stream.on("change", (n) => changes.push(n));

    const ws = FakeWebSocket.instances[0]!;
    ws.onopen?.();
    ws.emit({
      type: "sub",
      sub_id: "s1",
      added: [{ _id: "u1" }],
      updated: [{ _id: "u2" }],
      removed: [{ _id: "u3" }],
    });

    expect(changes).toEqual([
      { action: LiveAction.Create, result: { _id: "u1" } },
      { action: LiveAction.Update, result: { _id: "u2" } },
      { action: LiveAction.Delete, result: { _id: "u3" } },
    ]);
  });

  it("tolerates frames that omit empty change lists", () => {
    const stream = new LiveQueryStream("ws://localhost:8080/ws", null, {
      type: "users",
    });
    const changes: LiveNotification[] = [];
    stream.on("change", (n) => changes.push(n));

    const ws = FakeWebSocket.instances[0]!;
    ws.onopen?.();
    ws.emit({ type: "sub", sub_id: "s1", updated: [{ _id: "u2" }] });

    expect(changes).toEqual([
      { action: LiveAction.Update, result: { _id: "u2" } },
    ]);
  });

  it("reports subscription errors", () => {
    const stream = new LiveQueryStream("ws://localhost:8080/ws", null, {
      type: "users",
    });
    const errors: Error[] = [];
    stream.on("error", (err) => errors.push(err));

    const ws = FakeWebSocket.instances[0]!;
    ws.onopen?.();
    ws.emit({ type: "sub-err", id: "1", error: "query requires 'type'" });

    expect(errors).toHaveLength(1);
    expect(errors[0]!.message).toBe("query requires 'type'");
  });
});
