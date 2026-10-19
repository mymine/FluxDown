import { describe, expect, test } from "bun:test";
import { RecentResponses, propagate, tag, tagsOf } from "./provenance";

describe("provenance 标记传递", () => {
  test("ArrayBuffer.slice 与 Uint8Array.set 之后标记仍可传递", () => {
    const src = new Uint8Array([1, 2, 3, 4]);
    tag(src, "https://cdn.example.com/seg-1.m4s");

    // slice 得到新 buffer：宿主 hook 会在拷贝点调用 propagate
    const sliced = src.buffer.slice(0, 2);
    expect(tagsOf(sliced)).toBeUndefined();
    propagate(src, sliced);
    expect([...(tagsOf(sliced) ?? [])]).toEqual(["https://cdn.example.com/seg-1.m4s"]);

    // set 把 src 拷进另一块内存
    const dst = new Uint8Array(8);
    dst.set(src, 2);
    propagate(src, dst);
    expect(tagsOf(dst)?.has("https://cdn.example.com/seg-1.m4s")).toBe(true);
  });

  test("subarray 共享同一 buffer，标记相同", () => {
    const whole = new Uint8Array(16);
    tag(whole, "https://cdn.example.com/a.m4s");
    const part = whole.subarray(4, 8);
    expect(tagsOf(part)).toBe(tagsOf(whole));
  });

  test("没有标记的来源不会污染目标", () => {
    const a = new Uint8Array(4);
    const b = new Uint8Array(4);
    propagate(a, b);
    expect(tagsOf(b)).toBeUndefined();
  });
});

describe("RecentResponses", () => {
  test("withinWindow 只返回窗口内的响应", () => {
    const recent = new RecentResponses();
    recent.push("https://x/old.m4s", 10, 1_000);
    recent.push("https://x/new.m4s", 10, 9_000);
    recent.push("https://x/future.m4s", 10, 20_000);
    const urls = recent.withinWindow(10_000, 3_000).map((r) => r.url);
    expect(urls).toEqual(["https://x/new.m4s"]);
  });

  test("超出容量时淘汰最旧项", () => {
    const recent = new RecentResponses(2);
    recent.push("a", 1, 1);
    recent.push("b", 1, 2);
    recent.push("c", 1, 3);
    expect(recent.withinWindow(3, 10).map((r) => r.url)).toEqual(["b", "c"]);
  });
});
