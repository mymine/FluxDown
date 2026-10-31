import { describe, expect, test } from "bun:test";
import { carryAcrossRedirect, lookupRequestRecord, recordCapturedRequest } from "./request-record";
import type { CapturedBody, RequestRecordCache } from "./request-record";

const FORM = "https://site.example/download";
const CDN = "https://cdn.example/file.rar?token=1";
const BODY: CapturedBody = { kind: "formData", fields: { id: ["42"] } };

function postedForm(): RequestRecordCache {
  const cache: RequestRecordCache = new Map();
  recordCapturedRequest(cache, FORM, "POST", BODY, 1);
  return cache;
}

describe("request record across redirects", () => {
  for (const status of [301, 302, 303]) {
    test(`${status} turns the follow-up into GET: target carries no POST`, () => {
      const cache = postedForm();
      carryAcrossRedirect(cache, FORM, CDN, status, 2);
      expect(lookupRequestRecord(cache, CDN)).toEqual({});
    });
  }

  for (const status of [307, 308]) {
    test(`${status} preserves method and body on the target`, () => {
      const cache = postedForm();
      carryAcrossRedirect(cache, FORM, CDN, status, 2);
      // 下一跳浏览器不再给出 requestBody 时沿用带过来的体。
      recordCapturedRequest(cache, CDN, "POST", undefined, 3);
      expect(lookupRequestRecord(cache, CDN)).toEqual({ method: "POST", body: BODY });
    });
  }

  test("a direct POST download replays its own transaction", () => {
    expect(lookupRequestRecord(postedForm(), FORM)).toEqual({ method: "POST", body: BODY });
  });

  test("a different method on the same URL does not inherit the old body", () => {
    const cache = postedForm();
    recordCapturedRequest(cache, FORM, "PUT", undefined, 2);
    expect(lookupRequestRecord(cache, FORM)).toEqual({ method: "PUT", body: undefined });
  });
});
