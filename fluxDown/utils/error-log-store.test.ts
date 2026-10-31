import { describe, expect, test } from "bun:test";
import {
  ERROR_LOG_KEY,
  ERROR_LOG_MAX_AGE_MS,
  ErrorLogStore,
  MAX_ERROR_LOG_ENTRIES,
  formatErrorArgs,
} from "./error-log-store";

class MemoryStorage {
  data: Record<string, unknown> = {};
  async get(key: string) {
    return { [key]: this.data[key] };
  }
  async set(items: Record<string, unknown>) {
    Object.assign(this.data, structuredClone(items));
  }
}

describe("ErrorLogStore", () => {
  test("a repeated error is counted once and moves to the newest position", async () => {
    let now = 1;
    const store = new ErrorLogStore(new MemoryStorage(), () => now);
    await store.append({ level: "error", message: "A" }, "background");
    await store.append({ level: "warn", message: "B" }, "content");
    now = 5;
    await store.append({ level: "error", message: "A" }, "background");

    expect(await store.snapshot()).toEqual([
      { level: "warn", message: "B", source: "content", firstAt: 1, lastAt: 1, count: 1 },
      { level: "error", message: "A", source: "background", firstAt: 1, lastAt: 5, count: 2 },
    ]);
  });

  test("concurrent reports from several contexts are all kept", async () => {
    const store = new ErrorLogStore(new MemoryStorage());
    await Promise.all([
      store.append({ level: "error", message: "x" }, "popup"),
      store.append({ level: "error", message: "y" }, "sidepanel"),
      store.append({ level: "error", message: "z" }, "content"),
    ]);
    expect((await store.snapshot()).map((e) => e.message)).toEqual(["x", "y", "z"]);
  });

  test("keeps only the newest entries within the retention window", async () => {
    let now = 0;
    const storage = new MemoryStorage();
    const store = new ErrorLogStore(storage, () => now);
    for (let i = 0; i <= MAX_ERROR_LOG_ENTRIES; i++) await store.append({ level: "error", message: `e${i}` }, "background");
    const kept = await store.snapshot();
    expect(kept).toHaveLength(MAX_ERROR_LOG_ENTRIES);
    expect(kept[0].message).toBe("e1");

    now = ERROR_LOG_MAX_AGE_MS + 1;
    expect(await store.snapshot()).toEqual([]);
    await store.clear();
    expect(storage.data[ERROR_LOG_KEY]).toEqual([]);
  });

  test("rejects malformed reports and drops corrupted stored rows", async () => {
    const storage = new MemoryStorage();
    storage.data[ERROR_LOG_KEY] = [{ level: "fatal", message: "?", source: "background", firstAt: 1, lastAt: 1, count: 1 }];
    const store = new ErrorLogStore(storage, () => 2);
    await expect(store.append({ level: "error", message: "" }, "popup")).rejects.toThrow();
    expect(await store.snapshot()).toEqual([]);
  });
});

describe("formatErrorArgs", () => {
  test("keeps the error name, message and cause on one line", () => {
    const error = new Error("send failed", { cause: new TypeError("port closed") });
    expect(formatErrorArgs(["[FluxDown] download:", error, { id: 3 }])).toBe(
      '[FluxDown] download: Error: send failed (cause: TypeError: port closed) {"id":3}',
    );
  });
});
