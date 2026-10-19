import { describe, expect, test } from "bun:test";
import { estimateUsdtMicros, formatUsdt } from "../src/components/pay/money";

describe("usdt money", () => {
  test("full price maps to the configured USDT price", () => {
    expect(estimateUsdtMicros(4900, 4900, 7_000_000)).toBe(7_000_000);
  });
  test("campaign price rounds up to 0.01 USDT", () => {
    expect(estimateUsdtMicros(3900, 4900, 7_000_000)).toBe(5_580_000);
  });
  test("tiny amounts floor at 0.01 USDT", () => {
    expect(estimateUsdtMicros(1, 4900, 10_000)).toBe(10_000);
    expect(estimateUsdtMicros(0, 4900, 7_000_000)).toBe(10_000);
  });
  test("formatUsdt", () => {
    expect(formatUsdt(5_580_000)).toBe("5.58 USDT");
  });
});
