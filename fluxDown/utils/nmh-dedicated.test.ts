import { describe, expect, test } from "bun:test";
import { createDedicatedPool } from "./nmh-dedicated";
import type { NmhPortLike } from "./nmh-dedicated";

interface FakePort extends NmhPortLike {
  sent: Array<{ action: string; msg_id: number; [key: string]: unknown }>;
  disconnected: boolean;
  reply(msgId: number, patch?: object): void;
  drop(): void;
}

function fakePort(): FakePort {
  const messageListeners: Array<(m: unknown) => void> = [];
  const disconnectListeners: Array<() => void> = [];
  const port: FakePort = {
    sent: [],
    disconnected: false,
    postMessage(message) {
      port.sent.push(message as FakePort["sent"][number]);
    },
    disconnect() {
      port.disconnected = true;
    },
    onMessage: { addListener: (l) => void messageListeners.push(l) },
    onDisconnect: { addListener: (l) => void disconnectListeners.push(l) },
    reply(msgId, patch = {}) {
      for (const l of messageListeners) l({ success: true, msg_id: msgId, data: { value: msgId }, ...patch });
    },
    drop() {
      for (const l of disconnectListeners) l();
    },
  };
  return port;
}

/** 手动推进的定时器。 */
function fakeTimers() {
  let now = 0;
  let nextId = 1;
  const timers = new Map<number, { at: number; run: () => void }>();
  return {
    setTimer(run: () => void, ms: number): unknown {
      const id = nextId++;
      timers.set(id, { at: now + ms, run });
      return id;
    },
    clearTimer(handle: unknown): void {
      timers.delete(handle as number);
    },
    advance(ms: number): void {
      now += ms;
      for (const [id, timer] of [...timers]) {
        if (timer.at <= now) {
          timers.delete(id);
          timer.run();
        }
      }
    },
    pending: () => timers.size,
  };
}

function setup(idleMs = 30_000) {
  const ports: FakePort[] = [];
  const timers = fakeTimers();
  const pool = createDedicatedPool({
    connect: () => {
      const port = fakePort();
      ports.push(port);
      return port;
    },
    idleMs,
    setTimer: timers.setTimer,
    clearTimer: timers.clearTimer,
  });
  return { pool, ports, timers };
}

describe("dedicated NMH pool", () => {
  test("one port per key; same-key requests queue one at a time", async () => {
    const { pool, ports } = setup();
    const a1 = pool.call("a@x", "plugin_invoke", { n: 1 }, 90_000);
    const a2 = pool.call("a@x", "plugin_invoke", { n: 2 }, 90_000);
    const b1 = pool.call("b@x", "plugin_invoke", { n: 3 }, 90_000);
    expect(ports).toHaveLength(2);
    expect(ports[0].sent.map((m) => m.n)).toEqual([1]);
    expect(ports[1].sent.map((m) => m.n)).toEqual([3]);

    ports[0].reply(ports[0].sent[0].msg_id);
    expect((await a1).success).toBe(true);
    expect(ports[0].sent.map((m) => m.n)).toEqual([1, 2]);
    ports[0].reply(ports[0].sent[1].msg_id, { data: { value: "two" } });
    expect((await a2).data).toEqual({ value: "two" });
    ports[1].reply(ports[1].sent[0].msg_id);
    expect((await b1).success).toBe(true);
    expect(ports).toHaveLength(2);
  });

  test("replies with a foreign msg_id are ignored", async () => {
    const { pool, ports } = setup();
    const call = pool.call("a@x", "plugin_invoke", {}, 90_000);
    ports[0].reply(9999);
    ports[0].reply(ports[0].sent[0].msg_id, { success: false, message: "rpc not exposed to page: x" });
    const result = await call;
    expect(result.success).toBe(false);
    expect(result.message).toBe("rpc not exposed to page: x");
  });

  test("idle port is disconnected after idleMs; a later call opens a fresh port", async () => {
    const { pool, ports, timers } = setup(30_000);
    const first = pool.call("a@x", "plugin_invoke", {}, 90_000);
    ports[0].reply(ports[0].sent[0].msg_id);
    await first;
    expect(pool.openPorts()).toBe(1);
    timers.advance(29_999);
    expect(ports[0].disconnected).toBe(false);
    timers.advance(1);
    expect(ports[0].disconnected).toBe(true);
    expect(pool.openPorts()).toBe(0);

    const second = pool.call("a@x", "plugin_invoke", {}, 90_000);
    expect(ports).toHaveLength(2);
    ports[1].reply(ports[1].sent[0].msg_id);
    expect((await second).success).toBe(true);
  });

  test("an in-flight request keeps the port open past idleMs", async () => {
    const { pool, ports, timers } = setup(1_000);
    const first = pool.call("a@x", "plugin_invoke", {}, 90_000);
    ports[0].reply(ports[0].sent[0].msg_id);
    await first;
    const second = pool.call("a@x", "plugin_invoke", {}, 90_000);
    timers.advance(60_000);
    expect(ports[0].disconnected).toBe(false);
    ports[0].reply(ports[0].sent[1].msg_id);
    expect((await second).success).toBe(true);
  });

  test("timeout fails the request, drops the port, and queued requests continue on a new port", async () => {
    const { pool, ports, timers } = setup();
    const slow = pool.call("a@x", "plugin_invoke", { n: 1 }, 90_000);
    const queued = pool.call("a@x", "plugin_invoke", { n: 2 }, 90_000);
    timers.advance(90_000);
    expect(await slow).toEqual({ success: false, message: "timeout", data: undefined });
    expect(ports[0].disconnected).toBe(true);
    expect(ports).toHaveLength(2);
    expect(ports[1].sent.map((m) => m.n)).toEqual([2]);
    ports[1].reply(ports[1].sent[0].msg_id);
    expect((await queued).success).toBe(true);
  });

  test("port disconnect rejects the in-flight request", async () => {
    const { pool, ports } = setup();
    const call = pool.call("a@x", "plugin_invoke", {}, 90_000);
    ports[0].drop();
    expect(await call).toEqual({ success: false, message: "port disconnected", data: undefined });
  });

  test("connect failure fails every queued request", async () => {
    const pool = createDedicatedPool({
      connect: () => {
        throw new Error("no host");
      },
    });
    const [a, b] = await Promise.all([
      pool.call("a@x", "plugin_invoke", {}, 1_000),
      pool.call("a@x", "plugin_invoke", {}, 1_000),
    ]);
    expect(a.message).toBe("native_messaging_unavailable");
    expect(b.message).toBe("native_messaging_unavailable");
  });
});
