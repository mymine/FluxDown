import { beforeEach, expect, test } from "bun:test";
import { BAN_MS, MAX_DOWNLOADS, WINDOW_MS, banRemaining, recordDownload, resetDownloadBans } from "../src/lib/download-ban";

const IP = "114.114.114.114";
const T0 = 1_000_000_000_000;

beforeEach(() => resetDownloadBans());

test("the 21st distinct download within 15 minutes bans the IP for 12 hours", () => {
  for (let i = 0; i < MAX_DOWNLOADS; i++) {
    expect(recordDownload(IP, `f${i}`, T0 + i * 1000)).toBe(0);
  }
  expect(recordDownload(IP, "f20", T0 + 20_000)).toBe(BAN_MS);
  expect(banRemaining(IP, T0 + 20_000 + BAN_MS - 1)).toBe(1);
  expect(recordDownload(IP, "f0", T0 + 20_000 + BAN_MS - 1)).toBe(1);
  // 封禁到期后重新计数。
  expect(recordDownload(IP, "f0", T0 + 20_000 + BAN_MS)).toBe(0);
  expect(recordDownload("8.8.8.8", "f0", T0 + 20_000)).toBe(0);
});

test("downloads older than the 15-minute window no longer count", () => {
  for (let i = 0; i < MAX_DOWNLOADS; i++) recordDownload(IP, `f${i}`, T0 + i);
  // 第一次计数滑出窗口：窗口内仍是 20 次，不封禁。
  expect(recordDownload(IP, "late", T0 + WINDOW_MS)).toBe(0);
  // 同一时刻再来一次，窗口内变成 21 次。
  expect(recordDownload(IP, "late-2", T0 + WINDOW_MS)).toBe(BAN_MS);
});

test("repeated requests for the same file within a minute count once (segmented downloaders)", () => {
  for (let i = 0; i < 100; i++) {
    expect(recordDownload(IP, "app.zip", T0 + i * 500)).toBe(0);
  }
  for (let i = 0; i < MAX_DOWNLOADS - 1; i++) recordDownload(IP, `f${i}`, T0 + 60_000);
  expect(recordDownload(IP, "one-more", T0 + 60_001)).toBe(BAN_MS);
});

test("the same file requested again after a minute counts again", () => {
  expect(recordDownload(IP, "app.zip", T0)).toBe(0);
  for (let i = 0; i < MAX_DOWNLOADS - 2; i++) recordDownload(IP, `f${i}`, T0 + 60_000);
  // 隔满 1 分钟再请求同一文件计为第 20 次，再多一次即封禁。
  expect(recordDownload(IP, "app.zip", T0 + 60_000)).toBe(0);
  expect(recordDownload(IP, "other", T0 + 60_000)).toBe(BAN_MS);
});
