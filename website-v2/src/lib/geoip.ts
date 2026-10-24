/**
 * 访客是否在中国大陆：查 ip2region 官方 IPv4 xdb（github.com/lionsoul2014/ip2region
 * `data/ip2region_v4.xdb`），只用于 /api/download 的下载分流。
 *
 * xdb 布局（结构 2.0 / 3.0 的 IPv4 库相同）：256 字节头；随后 256×256 个向量索引项
 * （按 IP 前两字节定位，各 8 字节：段索引起止偏移，uint32 LE）；段索引项 14 字节：
 * 起始 IP、结束 IP（uint32 LE）、地域串长度（uint16 LE）、地域串偏移（uint32 LE）。
 * 地域串为 `国家|省份|城市|ISP|ISO 代码`；港澳台在库中的 ISO 代码同为 `CN`，按省份字段排除。
 *
 * 文件由 deploy.sh 下载到 `website-v2/geo/` 并只读挂载进容器；缺失或格式不符时记一次错误，
 * 所有访客按非大陆处理（走 GitHub），不影响下载可用性。IPv6 访客同样按非大陆处理。
 */
import { readFileSync } from "node:fs";

const HEADER_LENGTH = 256;
const VECTOR_COLS = 256;
const VECTOR_ENTRY = 8;
const SEGMENT_ENTRY = 14;
/** 库中 ISO 代码为 CN、但不属于中国大陆的省级字段。 */
const NON_MAINLAND: Record<string, true> = { 香港特别行政区: true, 澳门特别行政区: true, 台湾省: true };
const VECTOR_END = HEADER_LENGTH + 256 * VECTOR_COLS * VECTOR_ENTRY;

export const DEFAULT_XDB_PATH = "/app/geo/ip2region_v4.xdb";

/** 解析点分十进制 IPv4；非法或 IPv6 返回 null。 */
function ipv4ToInt(ip: string): number | null {
  const parts = ip.split(".");
  if (parts.length !== 4) return null;
  let value = 0;
  for (const part of parts) {
    if (!/^\d{1,3}$/.test(part)) return null;
    const n = Number(part);
    if (n > 255) return null;
    value = value * 256 + n;
  }
  return value;
}

export class Ip2RegionV4 {
  constructor(private readonly db: Buffer) {
    if (db.length < VECTOR_END) throw new Error("xdb truncated");
    const version = db.readUInt16LE(0);
    // 3.0 头部第 16 字节起为 IP 版本（4 / 6）；2.0 只有 IPv4。
    if (version !== 2 && !(version === 3 && db.readUInt16LE(16) === 4)) {
      throw new Error(`unsupported xdb (structure ${version})`);
    }
  }

  /** 返回地域串；查不到返回空串，非法 IPv4 返回 null。 */
  search(ip: string): string | null {
    const value = ipv4ToInt(ip);
    if (value === null) return null;
    const vector = HEADER_LENGTH + ((value >>> 24) * VECTOR_COLS + ((value >>> 16) & 0xff)) * VECTOR_ENTRY;
    const start = this.db.readUInt32LE(vector);
    const end = this.db.readUInt32LE(vector + 4);
    if (start === 0 || end === 0) return "";
    let low = 0;
    let high = (end - start) / SEGMENT_ENTRY;
    while (low <= high) {
      const mid = (low + high) >> 1;
      const p = start + mid * SEGMENT_ENTRY;
      if (value < this.db.readUInt32LE(p)) {
        high = mid - 1;
      } else if (value > this.db.readUInt32LE(p + 4)) {
        low = mid + 1;
      } else {
        const length = this.db.readUInt16LE(p + 8);
        const offset = this.db.readUInt32LE(p + 10);
        return this.db.toString("utf8", offset, offset + length);
      }
    }
    return "";
  }

  isMainlandChina(ip: string): boolean {
    const fields = this.search(ip)?.split("|");
    return !!fields && fields.length === 5 && fields[4] === "CN" && NON_MAINLAND[fields[1]] !== true;
  }
}

let loaded: Ip2RegionV4 | null | undefined;

function database(): Ip2RegionV4 | null {
  if (loaded !== undefined) return loaded;
  const path = process.env.IP2REGION_XDB || DEFAULT_XDB_PATH;
  try {
    loaded = new Ip2RegionV4(readFileSync(path));
  } catch (error) {
    console.error(`[geoip] ${path} 不可用，大陆分流关闭：${error instanceof Error ? error.message : error}`);
    loaded = null;
  }
  return loaded;
}

/** 访客 IP 是否属于中国大陆；库不可用时恒为 false。 */
export function isMainlandChinaIp(ip: string): boolean {
  return database()?.isMainlandChina(ip) ?? false;
}
