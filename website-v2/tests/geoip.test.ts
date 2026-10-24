import { expect, test } from "bun:test";
import { Ip2RegionV4 } from "../src/lib/geoip";

const HEADER = 256;
const VECTOR_END = HEADER + 256 * 256 * 8;

function ipInt(ip: string): number {
  return ip.split(".").reduce((value, part) => value * 256 + Number(part), 0);
}

/** 按官方 xdb 布局构造最小库：每个区间落在单一 /16，向量索引指向对应段索引项。 */
function buildXdb(segments: [string, string, string][], version = 3, ipVersion = 4): Buffer {
  const regions = segments.map(([, , region]) => Buffer.from(region, "utf8"));
  const segmentStart = VECTOR_END + regions.reduce((size, region) => size + region.length, 0);
  const db = Buffer.alloc(segmentStart + segments.length * 14);
  db.writeUInt16LE(version, 0);
  db.writeUInt16LE(ipVersion, 16);
  let dataPtr = VECTOR_END;
  segments.forEach(([start, end], i) => {
    const region = regions[i];
    region.copy(db, dataPtr);
    const p = segmentStart + i * 14;
    db.writeUInt32LE(ipInt(start), p);
    db.writeUInt32LE(ipInt(end), p + 4);
    db.writeUInt16LE(region.length, p + 8);
    db.writeUInt32LE(dataPtr, p + 10);
    dataPtr += region.length;
    const [a, b] = start.split(".").map(Number);
    const vector = HEADER + (a * 256 + b) * 8;
    if (db.readUInt32LE(vector) === 0) db.writeUInt32LE(p, vector);
    db.writeUInt32LE(p, vector + 4);
  });
  return db;
}

const db = new Ip2RegionV4(buildXdb([
  ["114.114.0.0", "114.114.127.255", "中国|江苏省|南京市|0|CN"],
  ["114.114.128.0", "114.114.255.255", "Japan|Tokyo|Tokyo|0|JP"],
  ["203.198.0.0", "203.198.255.255", "中国|香港特别行政区|0|电讯盈科|CN"],
  ["168.95.0.0", "168.95.255.255", "中国|台湾省|0|中华电信|CN"],
  ["202.175.0.0", "202.175.255.255", "中国|澳门特别行政区|0|澳门电讯|CN"],
]));

test("only mainland China counts; segment boundaries are inclusive", () => {
  expect(db.isMainlandChina("114.114.0.0")).toBe(true);
  expect(db.isMainlandChina("114.114.127.255")).toBe(true);
  expect(db.isMainlandChina("114.114.128.0")).toBe(false);
  for (const ip of ["203.198.7.66", "168.95.1.1", "202.175.3.3"]) {
    expect(db.search(ip)?.endsWith("|CN")).toBe(true);
    expect(db.isMainlandChina(ip)).toBe(false);
  }
});

test("unknown ranges, IPv6 and malformed input are never mainland", () => {
  expect(db.search("8.8.8.8")).toBe("");
  for (const ip of ["8.8.8.8", "::1", "240e:3b7::1", "114.114.1", "114.114.1.256", "114.114.1.1x", ""]) {
    expect(db.isMainlandChina(ip)).toBe(false);
  }
});

test("rejects IPv6 or truncated databases", () => {
  expect(() => new Ip2RegionV4(buildXdb([], 3, 6))).toThrow();
  expect(() => new Ip2RegionV4(Buffer.alloc(1024))).toThrow();
  expect(new Ip2RegionV4(buildXdb([], 2, 0)).search("1.2.3.4")).toBe("");
});
