/**
 * 123 云盘下载源的 env 绑定（逻辑见 download-pan123.ts）。
 * WebDAV 账号 / 应用密码（123「第三方挂载」里只授权发布目录的应用）只用来 PROPFIND 确认资产；
 * 下载链接由直链空间「URL 鉴权」密钥本地签名。任一项缺失则关闭该下载源。
 */

import {
  PAN123_AUTH_KEY,
  PAN123_DIRECT_BASE,
  PAN123_DIRECT_FOLDER,
  PAN123_UID,
  PAN123_WEBDAV_PASSWORD,
  PAN123_WEBDAV_URL,
  PAN123_WEBDAV_USER,
} from "astro:env/server";
import { createPan123Probe, signDirectLink } from "@/lib/download-pan123";

/**
 * 123 直链有效期：更新器（≥ 0.6.0）拿到 302 立即单连接下载，每次续传 / 重试都重新请求
 * /api/download，所以只需覆盖「签发 → 发起请求」加两端时钟偏差；签了不绑 IP，越短转发价值越低。
 */
const PAN123_URL_TTL_SEC = 10 * 60;

export const pan123Configured =
  !!(PAN123_WEBDAV_USER && PAN123_WEBDAV_PASSWORD && PAN123_UID && PAN123_AUTH_KEY);

const authorization = `Basic ${Buffer.from(`${PAN123_WEBDAV_USER ?? ""}:${PAN123_WEBDAV_PASSWORD ?? ""}`).toString("base64")}`;
const hasAsset = createPan123Probe(PAN123_WEBDAV_URL, authorization);
const direct = {
  base: PAN123_DIRECT_BASE || `https://${PAN123_UID}.cdn.123clouddisk.cn`,
  uid: PAN123_UID ?? "",
  folder: PAN123_DIRECT_FOLDER,
  authKey: PAN123_AUTH_KEY ?? "",
};

/**
 * 对象键（releaseObjectKey）对应资产的 123 直链（鉴权签名，PAN123_URL_TTL_SEC 后过期）；
 * 资产缺失、大小不符或 123 不可用时为 null。调用前须保证 `pan123Configured`。
 */
export async function pan123DownloadUrl(key: string, size: number): Promise<string | null> {
  return await hasAsset(key, size) ? signDirectLink(direct, key, PAN123_URL_TTL_SEC) : null;
}
