import { describe, expect, test } from "bun:test";
import { compileBundled } from "@/rules/engine";
import type { DetectedResource } from "@/utils/resource-types";
import type { HostCapabilities } from "./protocol";
import { groupTrackPairs, handoffFor } from "./view-model";

const engine = compileBundled();
const PAGE = "https://example.com/watch";

/** 从 engine.trackKind 判轨（与生产路径一致）。 */
function kindOf(r: DetectedResource) {
  return engine.trackKind({ url: r.url, filename: r.filename, mime: r.mimeType, pageUrl: PAGE });
}

describe("groupTrackPairs — 离散音视频轨道分组", () => {
  let seq = 0;
  function res(overrides: Partial<DetectedResource>): DetectedResource {
    seq += 1;
    return {
      id: `id-${seq}`,
      url: `https://cdn.example.com/track-${seq}`,
      filename: `track-${seq}`,
      type: "video",
      size: 1_000_000,
      detectedBy: "webRequest",
      detectedAt: seq,
      tabId: 1,
      pageUrl: PAGE,
      confidence: "high",
      ...overrides,
    };
  }

  test("1 视频 + 1 音频（均有 mime）配成一组", () => {
    const video = res({ mimeType: "video/mp4", size: 9_000_000, quality: "1080p" });
    const audio = res({ mimeType: "audio/mp4", size: 3_000_000 });
    const groups = groupTrackPairs([video, audio], kindOf);
    expect(groups).toHaveLength(1);
    expect(groups[0].quality).toBe("1080p");
    expect(groups[0].videoUrl).toBe(video.url);
    expect(groups[0].audioUrl).toBe(audio.url);
  });

  test("多个视频清晰度共享体积最大的音频轨，按视频体积降序", () => {
    const v1080 = res({ mimeType: "video/mp4", size: 30_000_000, quality: "1080p" });
    const v720 = res({ mimeType: "video/mp4", size: 20_000_000, quality: "720p" });
    const v480 = res({ mimeType: "video/mp4", size: 12_000_000, quality: "480p" });
    const audio = res({ mimeType: "audio/mp4", size: 3_000_000 });
    const groups = groupTrackPairs([v720, audio, v1080, v480], kindOf);
    expect(groups.map((g) => g.quality)).toEqual(["1080p", "720p", "480p"]);
    for (const g of groups) expect(g.audioUrl).toBe(audio.url);
  });

  test("只有视频轨时不配音频", () => {
    const video = res({ mimeType: "video/mp4", size: 8_000_000, quality: "720p" });
    const groups = groupTrackPairs([video], kindOf);
    expect(groups).toHaveLength(1);
    expect(groups[0].videoUrl).toBe(video.url);
    expect(groups[0].audioUrl).toBeUndefined();
  });

  test("mime 缺失且 ≥2 条、大小均已知：最小的一条视为音频轨", () => {
    const big = res({ size: 9_000_000 });
    const small = res({ size: 2_000_000 });
    const groups = groupTrackPairs([big, small], kindOf);
    expect(groups).toHaveLength(1);
    expect(groups[0].videoUrl).toBe(big.url);
    expect(groups[0].audioUrl).toBe(small.url);
  });

  test("非媒体类型被忽略", () => {
    const doc = res({ type: "document", mimeType: "application/pdf" });
    const video = res({ mimeType: "video/mp4", size: 8_000_000, quality: "720p" });
    const groups = groupTrackPairs([doc, video], kindOf);
    expect(groups).toHaveLength(1);
    expect(groups[0].videoUrl).toBe(video.url);
  });
});

describe("handoffFor", () => {
  const caps: HostCapabilities = {
    protocol: 2,
    pluginsSupported: true,
    connected: true,
    pluginRulesSequence: 0,
    actions: [],
    pageScriptsSequence: null,
    pageScriptsEpoch: null,
    resolvers: [
      { identity: "fluxdown@ytdlp", name: "yt-dlp", enabled: true, loaded: true, urls: ["*://www.youtube.com/*"] },
      { identity: "x@off", name: "Off", enabled: false, loaded: true, urls: ["*://example.com/*"] },
    ],
  };

  test("命中启用插件为 match；只命中禁用插件为 disabled；否则 no-match", () => {
    expect(handoffFor({ remoteMode: "off", caps, pageUrl: "https://www.youtube.com/watch?v=1" })).toEqual({
      kind: "match",
      identity: "fluxdown@ytdlp",
      name: "yt-dlp",
    });
    expect(handoffFor({ remoteMode: "off", caps, pageUrl: "https://example.com/a" }).kind).toBe("disabled");
    expect(handoffFor({ remoteMode: "off", caps, pageUrl: "https://other.org/" })).toEqual({
      kind: "no-match",
      installed: 2,
    });
  });

  test("不可用原因按固定顺序判定", () => {
    const url = "https://www.youtube.com/";
    expect(handoffFor({ remoteMode: "always", caps, pageUrl: url })).toEqual({ kind: "unavailable", reason: "remote-mode" });
    expect(handoffFor({ remoteMode: "off", caps: { ...caps, connected: false }, pageUrl: url })).toEqual({ kind: "unavailable", reason: "disconnected" });
    expect(handoffFor({ remoteMode: "off", caps: { ...caps, protocol: 1 }, pageUrl: url })).toEqual({ kind: "unavailable", reason: "old-app" });
    expect(handoffFor({ remoteMode: "off", caps: { ...caps, pluginsSupported: false }, pageUrl: url })).toEqual({ kind: "unavailable", reason: "plugins-unsupported" });
  });
});
