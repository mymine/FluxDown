import { describe, expect, test } from "bun:test";
import { docExpiry } from "./doc-expiry";

const DAY = 24 * 60 * 60 * 1000;
const now = Date.parse("2026-10-18T00:00:00Z");

describe("docExpiry", () => {
  test("no or unparsable expiry is none", () => {
    expect(docExpiry(undefined, now)).toEqual({ state: "none" });
    expect(docExpiry("not a date", now)).toEqual({ state: "none" });
  });

  test("past or present instants are expired", () => {
    expect(docExpiry(new Date(now - DAY).toISOString(), now)).toEqual({ state: "expired" });
    expect(docExpiry(new Date(now).toISOString(), now)).toEqual({ state: "expired" });
  });

  test("warns within a week and rounds partial days up", () => {
    expect(docExpiry(new Date(now + 2.2 * DAY).toISOString(), now)).toEqual({ state: "soon", days: 3 });
    expect(docExpiry(new Date(now + 7 * DAY).toISOString(), now)).toEqual({ state: "soon", days: 7 });
    expect(docExpiry(new Date(now + 20 * DAY).toISOString(), now)).toEqual({ state: "ok", days: 20 });
  });
});
