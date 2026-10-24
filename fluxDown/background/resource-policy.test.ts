import { describe, expect, test } from "bun:test";
import type { DetectedResource } from "@/utils/resource-types";
import { computeConfidence, isWorthShowing } from "./resource-policy";

function resource(overrides: Partial<DetectedResource>): DetectedResource {
  return {
    id: "r",
    url: "https://example.com/a",
    filename: "a",
    type: "video",
    size: -1,
    detectedBy: "webRequest",
    detectedAt: 0,
    tabId: 1,
    pageUrl: "https://example.com/",
    confidence: "medium",
    ...overrides,
  };
}

describe("computeConfidence", () => {
  test("attachment 与 stream / torrent 恒为 high", () => {
    expect(computeConfidence("other", 1, "webRequest", true)).toBe("high");
    expect(computeConfidence("stream", -1, "webRequest")).toBe("high");
    expect(computeConfidence("torrent", -1, "webRequest")).toBe("high");
  });

  test("已知类型：超过阈值 high，大小未知 medium，低于阈值 low", () => {
    expect(computeConfidence("video", 600 * 1024, "webRequest")).toBe("high");
    expect(computeConfidence("video", -1, "webRequest")).toBe("medium");
    expect(computeConfidence("video", 1024, "webRequest")).toBe("low");
  });

  test("不设大小下限的类型（文档 / 压缩包）：大小已知即 high，不会因有 Content-Length 被默认视图隐藏", () => {
    expect(computeConfidence("document", 1, "webRequest")).toBe("high");
    expect(computeConfidence("document", 5 * 1024 * 1024, "webRequest")).toBe("high");
    expect(computeConfidence("archive", 300 * 1024, "webRequest")).toBe("high");
    expect(computeConfidence("document", -1, "webRequest")).toBe("medium");
  });

  test("图片与 other 的分档", () => {
    expect(computeConfidence("image", 200 * 1024, "webRequest")).toBe("medium");
    expect(computeConfidence("image", 20 * 1024, "webRequest")).toBe("low");
    expect(computeConfidence("other", 2 * 1024 * 1024, "webRequest")).toBe("medium");
    expect(computeConfidence("other", 60 * 1024, "webRequest")).toBe("low");
  });
});

describe("isWorthShowing", () => {
  test("magnet 始终展示", () => {
    expect(isWorthShowing(resource({ type: "magnet", url: "magnet:?xt=urn:btih:1" }))).toBe(true);
  });

  test("低于阈值的音视频照样入库（可信度 low，由视图决定是否展示）；其它类型按阈值过滤", () => {
    expect(isWorthShowing(resource({ type: "video", size: 1024 }))).toBe(true);
    expect(computeConfidence("video", 1024, "webRequest")).toBe("low");
    expect(isWorthShowing(resource({ type: "other", size: 1024 }))).toBe(false);
    expect(isWorthShowing(resource({ type: "other", size: 1024, isAttachment: true }))).toBe(true);
  });

  test("只在 API JSON 里出现过的地址为 low，被页面真正请求后按大小重新评估", () => {
    expect(computeConfidence("stream", -1, "api-scan")).toBe("low");
    expect(computeConfidence("archive", -1, "api-scan")).toBe("low");
    expect(computeConfidence("archive", -1, "fetch-intercept")).toBe("medium");
  });

  test("大小未知放行；文档无阈值", () => {
    expect(isWorthShowing(resource({ type: "video", size: -1 }))).toBe(true);
    expect(isWorthShowing(resource({ type: "document", size: 10 }))).toBe(true);
  });

  test("小于 10KB 的图片被过滤", () => {
    expect(isWorthShowing(resource({ type: "image", size: -1 }))).toBe(true);
    expect(isWorthShowing(resource({ type: "image", size: 5 * 1024, isAttachment: true }))).toBe(false);
  });
});
