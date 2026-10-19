import { describe, expect, test } from "bun:test";
import { bumpHit, mergeHits } from "./rule-hits";
import type { RuleHit } from "./rule-view";

describe("bumpHit", () => {
  test("累加计数并刷新时间", () => {
    const map = new Map<string, RuleHit>();
    bumpHit(map, "a", 1, 10);
    bumpHit(map, "a", 5, 10);
    expect(map.get("a")).toEqual({ count: 2, lastAt: 5 });
  });

  test("超过容量淘汰最久未命中的规则", () => {
    const map = new Map<string, RuleHit>();
    bumpHit(map, "a", 1, 2);
    bumpHit(map, "b", 2, 2);
    bumpHit(map, "a", 3, 2); // a 变为最近
    bumpHit(map, "c", 4, 2);
    expect([...map.keys()].sort()).toEqual(["a", "c"]);
  });
});

describe("mergeHits", () => {
  test("计数相加、时间取较大", () => {
    const map = new Map<string, RuleHit>([["a", { count: 1, lastAt: 10 }]]);
    mergeHits(map, { a: { count: 4, lastAt: 5 }, b: { count: 2, lastAt: 7 } }, 10);
    expect(map.get("a")).toEqual({ count: 5, lastAt: 10 });
    expect(map.get("b")).toEqual({ count: 2, lastAt: 7 });
  });

  test("忽略畸形存储值", () => {
    const map = new Map<string, RuleHit>();
    mergeHits(map, { a: { count: "x" }, b: null, c: { count: 1.5, lastAt: 1 } }, 10);
    mergeHits(map, [1, 2], 10);
    mergeHits(map, undefined, 10);
    expect(map.size).toBe(0);
  });
});
