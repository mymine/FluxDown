import { describe, expect, test } from "bun:test";
import { isPageChange } from "./page-nav";

const VIDEO = "https://www.bilibili.com/video/BV1GJ411x7h7/";

describe("isPageChange", () => {
  test("replaceState 只改 query（补追踪参数）不算换页；改路径算", () => {
    expect(isPageChange("replace", VIDEO, `${VIDEO}?vd_source=abc`)).toBe(false);
    expect(isPageChange("replace", VIDEO, "https://www.bilibili.com/video/BV1bW411n7fY/")).toBe(true);
  });

  test("pushState / popstate / 新文档按 query 区分页面（watch?v=A → v=B）", () => {
    expect(isPageChange("push", "https://x.com/watch?v=A", "https://x.com/watch?v=B")).toBe(true);
    expect(isPageChange("pop", "https://x.com/watch?v=B", "https://x.com/watch?v=A")).toBe(true);
    expect(isPageChange("document", "https://x.com/s?q=a", "https://x.com/s?q=b")).toBe(true);
  });

  test("tabs.onUpdated 无法区分同文档改写，只按路径判定", () => {
    expect(isPageChange("tab", VIDEO, `${VIDEO}?vd_source=abc`)).toBe(false);
    expect(isPageChange("tab", VIDEO, "https://www.bilibili.com/video/BV1bW411n7fY/")).toBe(true);
  });

  test("只差 hash 不算换页；首次登记（from 为空）不算换页", () => {
    expect(isPageChange("push", "https://x.com/a", "https://x.com/a#t=30")).toBe(false);
    expect(isPageChange("push", "", "https://x.com/a")).toBe(false);
  });

  test("hash 路由（#/…、#!/…）变化算换页；普通锚点不算", () => {
    expect(isPageChange("push", "https://x.com/app#/video/1", "https://x.com/app#/video/2")).toBe(true);
    expect(isPageChange("replace", "https://x.com/app#!/a", "https://x.com/app#!/b")).toBe(true);
    expect(isPageChange("push", "https://x.com/app#/video/1", "https://x.com/app#/video/1")).toBe(false);
    expect(isPageChange("push", "https://x.com/a#comments", "https://x.com/a#top")).toBe(false);
  });
});