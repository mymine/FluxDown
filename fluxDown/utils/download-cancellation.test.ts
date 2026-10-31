import { describe, expect, test } from "bun:test";
import { cancelBeforeFilenameResolution } from "./download-cancellation";

/** 记录调用顺序的假浏览器：`stateAfterCancel` 是 cancel 请求之后查询到的下载状态。 */
function harness(options: { stateAfterCancel?: string; cancelFails?: boolean } = {}) {
  const events: string[] = [];
  const cancellation = Promise.withResolvers<void>();
  const run = cancelBeforeFilenameResolution(
    7,
    () => {
      events.push("cancel");
      return options.cancelFails ? Promise.reject(new Error("download disappeared")) : cancellation.promise;
    },
    async () => {
      events.push("erase");
      return [];
    },
    () => events.push("suggest"),
    async () => {
      events.push("state");
      return options.stateAfterCancel;
    },
  );
  return { events, run, settleCancel: () => cancellation.resolve() };
}

/** 只推进微任务（状态查询回调），不依赖真实时间。 */
async function microtasks(): Promise<void> {
  for (let i = 0; i < 5; i++) await Promise.resolve();
}

describe("cancelBeforeFilenameResolution", () => {
  test("never suggests for a download the cancel already stopped", async () => {
    const h = harness({ stateAfterCancel: "interrupted" });
    h.settleCancel();

    await expect(h.run).resolves.toBe(true);
    await microtasks();
    expect(h.events).toEqual(["cancel", "state", "erase"]);
  });

  test("releases a still in-progress pipeline without waiting for the cancel reply", async () => {
    const h = harness({ stateAfterCancel: "in_progress" });

    await microtasks();
    expect(h.events).toEqual(["cancel", "state", "suggest"]);
    h.settleCancel();
    await expect(h.run).resolves.toBe(true);
  });

  test("does not claim a failed cancellation and releases an in-progress download", async () => {
    const h = harness({ cancelFails: true, stateAfterCancel: "in_progress" });

    await expect(h.run).resolves.toBe(false);
    await microtasks();
    expect(h.events).toEqual(["cancel", "state", "suggest"]);
  });
});
