import { describe, expect, test } from "bun:test";
import { pageWindow, paginate } from "./pagination";

describe("paginate", () => {
  test("末页只含剩余条目", () => {
    expect(paginate(45, 3, 20)).toEqual({ page: 3, pageCount: 3, start: 40, end: 45 });
  });

  test("列表变短后越界页码钳到最后一页", () => {
    expect(paginate(25, 9, 20)).toEqual({ page: 2, pageCount: 2, start: 20, end: 25 });
  });

  test("空列表固定为第 1 页、零条", () => {
    expect(paginate(0, 4, 20)).toEqual({ page: 1, pageCount: 1, start: 0, end: 0 });
  });
});

describe("pageWindow", () => {
  test("7 页以内全部列出", () => {
    expect(pageWindow(4, 7)).toEqual([1, 2, 3, 4, 5, 6, 7]);
  });

  test("首末页附近窗口固定 7 格，不出现只省略一页的省略号", () => {
    expect(pageWindow(1, 30)).toEqual([1, 2, 3, 4, 5, "gap", 30]);
    expect(pageWindow(4, 30)).toEqual([1, 2, 3, 4, 5, "gap", 30]);
    expect(pageWindow(27, 30)).toEqual([1, "gap", 26, 27, 28, 29, 30]);
    expect(pageWindow(30, 30)).toEqual([1, "gap", 26, 27, 28, 29, 30]);
  });

  test("中间页两侧折叠", () => {
    expect(pageWindow(15, 30)).toEqual([1, "gap", 14, 15, 16, "gap", 30]);
  });
});
