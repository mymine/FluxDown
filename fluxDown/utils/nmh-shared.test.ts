import { describe, expect, test } from "bun:test";
import type { NmhPortLike } from "./nmh-dedicated";
import { createSharedChannel } from "./nmh-shared";

interface FakePort extends NmhPortLike {
  sent: Array<{ action: string; msg_id: number }>;
  disconnected: boolean;
  /** 中继按 msg_id 应答。 */
  reply(action: string): void;
  /** 浏览器侧断开（NMH 进程退出）。 */
  drop(): void;
}

function harness() {
  const ports: FakePort[] = [];
  const channel = createSharedChannel({
    connect() {
      const messageListeners: Array<(message: unknown) => void> = [];
      const disconnectListeners: Array<() => void> = [];
      const port: FakePort = {
        sent: [],
        disconnected: false,
        postMessage(message) {
          port.sent.push(message as { action: string; msg_id: number });
        },
        disconnect() {
          port.disconnected = true;
        },
        onMessage: { addListener: (listener) => void messageListeners.push(listener) },
        onDisconnect: { addListener: (listener) => void disconnectListeners.push(listener) },
        reply(action) {
          const message = port.sent.find((m) => m.action === action);
          if (!message) throw new Error(`no ${action} sent`);
          for (const listener of messageListeners) listener({ success: true, msg_id: message.msg_id });
        },
        drop() {
          for (const listener of disconnectListeners) listener();
        },
      };
      ports.push(port);
      return port;
    },
    disconnectReason: () => undefined,
    record: () => {},
  });
  return { ports, channel };
}

describe("shared NMH channel", () => {
  test("a request stuck at the head of the serial queue recycles the port, so later requests reach a fresh host", async () => {
    const { ports, channel } = harness();
    const stuck = channel.send("capabilities", {}, 20);
    const queued = channel.send("tasks", {}, 5_000);

    expect(await stuck).toMatchObject({ success: false, message: "timeout" });
    // 排在卡死请求后面的请求不再空等自己的时限。
    expect(await queued).toMatchObject({ success: false, message: "port disconnected" });
    expect(ports[0].disconnected).toBe(true);

    const next = channel.send("ping", {}, 1_000);
    expect(ports).toHaveLength(2);
    ports[1].reply("ping");
    expect(await next).toMatchObject({ success: true });
  });

  test("a request that only timed out behind a legitimately slow request keeps the port", async () => {
    const { ports, channel } = harness();
    const slow = channel.send("sniff_rules", {}, 1_000);
    const short = channel.send("tasks", {}, 20);

    expect(await short).toMatchObject({ success: false, message: "timeout" });
    expect(ports[0].disconnected).toBe(false);

    ports[0].reply("sniff_rules");
    expect(await slow).toMatchObject({ success: true });
    // 超时请求的迟到应答按序出队，后续请求继续用同一连接。
    ports[0].reply("tasks");
    const after = channel.send("ping", {}, 1_000);
    expect(ports).toHaveLength(1);
    ports[0].reply("ping");
    expect(await after).toMatchObject({ success: true });
  });

  test("a late disconnect of a recycled port does not fail requests on the new port", async () => {
    const { ports, channel } = harness();
    expect(await channel.send("capabilities", {}, 20)).toMatchObject({ message: "timeout" });

    const fresh = channel.send("ping", {}, 1_000);
    ports[0].drop();
    ports[1].reply("ping");
    expect(await fresh).toMatchObject({ success: true });
  });
});
