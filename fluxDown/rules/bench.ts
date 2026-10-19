/**
 * 规则引擎热路径微基准（不是测试，不进 `bun test`）。
 *
 *   bun rules/bench.ts                    # 当前引擎
 *   bun rules/bench.ts <dir> --legacy     # 对比旧实现：<dir> 含旧版 engine.ts（packs 为 RulePack[]）
 *
 * 输出每个场景的 ns/调用；运行前先预热。同一份 builtin.json / official.json 用于两边，数字可直接对比。
 */
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import type * as EngineModule from "./engine";
import type { ResponseInput } from "./engine";

const args = process.argv.slice(2);
const legacy = args.includes("--legacy");
const dir = resolve(args.find((a) => !a.startsWith("--")) ?? dirname(fileURLToPath(import.meta.url)));
// 对比旧实现时引擎目录由命令行在运行时选定，无法静态导入。
const mod = (await import(`${dir}/engine.ts`)) as typeof EngineModule;

const NOW = Date.parse("2026-01-01T00:00:00Z");
const packs = [mod.BUILTIN_PACK, mod.BUNDLED_OFFICIAL_PACK];
const compileOnce = () =>
  mod.compileRules({
    packs: (legacy ? packs : packs.map((pack) => ({ tier: pack.packId, pack }))) as never,
    overrides: {},
    revocations: null,
    options: { sniffImages: false, communityEnabled: true, now: NOW },
  });

const PAGE = "https://www.bilibili.com/video/BV1xx";
const base = { pageUrl: PAGE, size: 0, attachment: false };
/** 典型响应流量：媒体分片、统计 / 广告噪声、站点接口、无扩展名 MIME 资源。 */
const responses: ResponseInput[] = [
  { ...base, url: "https://cdn.example.com/v/seg-001.m4s?sig=abc&expires=123", resourceType: "media", mime: "video/mp4" },
  { ...base, url: "https://cdn.example.com/v/movie.mp4", resourceType: "media", mime: "video/mp4" },
  { ...base, url: "https://cdn.example.com/live/index.m3u8?token=x", resourceType: "xmlhttprequest", mime: "application/vnd.apple.mpegurl" },
  { ...base, url: "https://www.google-analytics.com/collect?v=1&t=pageview", resourceType: "xmlhttprequest", mime: "image/gif" },
  { ...base, url: "https://stats.g.doubleclick.net/j/collect?a=1", resourceType: "xmlhttprequest", mime: "text/plain" },
  { ...base, url: "https://api.example.com/api/v2/feed/list?page=3&size=20", resourceType: "xmlhttprequest", mime: "application/json" },
  { ...base, url: "https://api.example.com/log/track/event", resourceType: "xmlhttprequest", mime: "text/plain" },
  { ...base, url: "https://static.example.com/assets/app.3f2a1b.js", resourceType: "other", mime: "application/javascript" },
  { ...base, url: "https://dl.example.com/files/archive.zip", resourceType: "other", mime: "application/zip", attachment: true },
  { ...base, url: "https://dl.example.com/stream/1234567", resourceType: "media", mime: "video/mp4" },
  { ...base, url: "https://img.example.com/p/cover.jpg", resourceType: "other", mime: "image/jpeg" },
  { ...base, url: "https://cdn.example.com/a/b/c/d/e/f/unknown.bin?x=1", resourceType: "other", mime: "application/octet-stream" },
];
const dedupeUrls = responses.map((r) => r.url);

function measure(name: string, fn: () => void, iterations: number): void {
  for (let i = 0; i < Math.max(1, iterations / 10); i++) fn();
  const t0 = performance.now();
  for (let i = 0; i < iterations; i++) fn();
  const ms = performance.now() - t0;
  console.log(`${name.padEnd(34)} ${((ms * 1e6) / iterations).toFixed(0).padStart(9)} ns/op`);
}

// 编译（页面 / 内容脚本每帧都会做一次）
measure("compileRules(bundled)", compileOnce, 30);

const engine = compileOnce();
const N = 20000;
let sink = 0;

measure(`classifyResponse ×${responses.length}`, () => {
  for (const r of responses) sink += engine.classifyResponse(r).decision.length;
}, N);
measure(`isIgnored ×${responses.length}`, () => {
  for (const r of responses) sink += engine.isIgnored(r.url, r.pageUrl, r.attachment) ? 1 : 0;
}, N);
measure(`classify(url, mime) ×${responses.length}`, () => {
  for (const r of responses) sink += engine.classify(r.url, r.mime).length;
}, N);
measure(`dedupeKey ×${dedupeUrls.length}`, () => {
  for (const u of dedupeUrls) sink += engine.dedupeKey(u, PAGE).length;
}, N);
measure("bodyWatchers(pageUrl)", () => {
  sink += engine.bodyWatchers(PAGE).length;
}, N);
measure("fileExtension ×4", () => {
  sink += engine.fileExtension("https://pbs.twimg.com/media/A?format=jpg", PAGE).length;
  sink += engine.fileExtension("https://x.com/a.mp4", PAGE).length;
  sink += engine.fileExtension("https://x.com/dl?file=a.zip", PAGE).length;
  sink += engine.fileExtension("https://x.com/dl", PAGE).length;
}, N);

if (sink < 0) console.log(sink);
