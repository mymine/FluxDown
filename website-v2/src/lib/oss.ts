/**
 * 阿里云 OSS 发布资产源。
 *
 * 发布资产由 .github/actions/oss-upload 上传到 `oss://<bucket>/<prefix>/<tag>/<file>`，
 * bucket 保持私有（对象不可公共读）。下载 302 目标：
 * - 中国大陆访客（Cloudflare `CF-IPCountry: CN`）：阿里云 CDN（`CN_CDN_BASE`，URL 鉴权 A 方式，
 *   OSS 私有 Bucket 回源）；
 * - 其他访客：Cloudflare 边缘缓存（`OSS_CDN_BASE`，website-v2/cdn/oss-proxy.js 回源私有桶，
 *   命中不计 OSS 外网流出），配置 `OSS_CDN_SIGNING_SECRET` 后链接带签名；
 * - 两者都未配置：官网签出短期预签名 URL 直出 OSS。
 * 签名算法：https://help.aliyun.com/zh/oss/developer-reference/signature-version-1
 */

import { createHash, createHmac, randomBytes } from "node:crypto";
import {
  CN_CDN_AUTH_KEY,
  CN_CDN_BASE,
  OSS_ACCESS_KEY_ID,
  OSS_ACCESS_KEY_SECRET,
  OSS_BUCKET,
  OSS_CDN_BASE,
  OSS_CDN_SIGNING_SECRET,
  OSS_ENDPOINT,
  OSS_RELEASE_PREFIX,
} from "astro:env/server";
import { downloadSignature } from "@/lib/download-signature";
import { ossStorageKey } from "@/lib/oss-storage";

/**
 * 官网签发的下载链接有效期（秒）。阿里云 CDN「URL 鉴权 → 鉴权 URL 有效时长」须配置为同一值：
 * A 方式的过期时间 = 签发时刻 + CDN 侧有效时长。分段下载在过期前发起的请求不受影响，
 * 过期后客户端重新请求 `/api/download` 即拿到新链接。
 */
export const DOWNLOAD_URL_TTL_SEC = 3 * 3600;

/** 未配置 AK/SK 时关闭直连 OSS；独立配置的 CDN 仍可使用 Worker 凭据回源。 */
export const ossConfigured = !!(OSS_ACCESS_KEY_ID && OSS_ACCESS_KEY_SECRET);

/**
 * 发布资产的对象键（无前导斜杠）：`<prefix>/<版本>/<组件>/<file>`。
 * 目录由 release tag 推导：`v0.4.8` → `v0.4.8/app/`（历史目录名；统一 release 起
 * 该目录承载这个 vX.Y.Z release 的全部组件资产），历史组件 release `extension-v…` /
 * `server-v…` / `cli-v…` / `mobile-v…` → 同名组件目录；版本目录保留 `v` 前缀。
 * 同一套规则在 .github/actions/oss-upload/action.yml 的 bash 里复刻，改一处须同步另一处。
 */
export function releaseObjectKey(tag: string, filename: string): string {
  const m = /^(?:([a-z]+)-)?(v.+)$/.exec(tag);
  const component = m?.[1] ?? "app";
  const version = m?.[2] ?? tag;
  return `${OSS_RELEASE_PREFIX}/${version}/${component}/${filename}`;
}

function encodedPath(key: string): string {
  return `/${key.split("/").map(encodeURIComponent).join("/")}`;
}

const WORKER_BASE = (OSS_CDN_BASE ?? "").replace(/\/+$/, "");

/** Cloudflare Worker 链接：配置了签名密钥时追加 `exp` + `sig`，Worker 侧缺签名或过期返回 403。 */
async function signedWorkerUrl(base: string, key: string): Promise<URL> {
  const path = encodedPath(key);
  const url = new URL(`${base}${path}`);
  if (OSS_CDN_SIGNING_SECRET) {
    const exp = Math.floor(Date.now() / 1000) + DOWNLOAD_URL_TTL_SEC;
    url.searchParams.set("exp", String(exp));
    url.searchParams.set("sig", await downloadSignature(OSS_CDN_SIGNING_SECRET, path, exp));
  }
  return url;
}

/**
 * 自动下载链的 Cloudflare 入口。tag 显式携带，避免 app-v… 与 v… 共用 app 目录时
 * 猜错 GitHub release；Worker 校验 tag 与对象键映射，不接受任意回退 URL。
 */
export async function cdnDownloadUrl(key: string, tag: string): Promise<string | null> {
  if (!WORKER_BASE) return null;
  const url = await signedWorkerUrl(WORKER_BASE, key);
  url.searchParams.set("fallback", "github");
  url.searchParams.set("tag", tag);
  return url.toString();
}

const CN_BASE = (CN_CDN_BASE ?? "").replace(/\/+$/, "");

/** 阿里云 CDN 基址与鉴权主 KEY 都配置时才分流中国大陆访客。 */
export const cnCdnConfigured = !!(CN_BASE && CN_CDN_AUTH_KEY);

/**
 * 中国大陆访客的阿里云 CDN 链接（URL 鉴权 A 方式：`auth_key=<签发时间>-<rand>-0-<md5>`，
 * md5 串为 `<编码后路径>-<签发时间>-<rand>-0-<密钥>`）。CDN 鉴权通过后去掉 auth_key 再算缓存键，
 * 签名不影响命中率。`.apk` / `.ipa` 由 CDN 回源 URI 改写到桶内 `.bin` 对象。
 * 调用前须保证 `cnCdnConfigured`。https://help.aliyun.com/zh/cdn/user-guide/type-a-signing
 */
export function cnCdnDownloadUrl(key: string): string {
  const path = encodedPath(key);
  const timestamp = Math.floor(Date.now() / 1000);
  const rand = randomBytes(16).toString("hex");
  const hash = createHash("md5").update(`${path}-${timestamp}-${rand}-0-${CN_CDN_AUTH_KEY}`).digest("hex");
  return `${CN_BASE}${path}?auth_key=${timestamp}-${rand}-0-${hash}`;
}

/**
 * 生成 `ttlSec` 秒内有效的预签名 URL。`key` 为对外对象键，签名前换成桶内存储键
 * （见 oss-storage.ts）。签名串用原始 key，URL 路径按段编码
 * （OSS 用解码后的资源路径校验签名）。调用前须保证 `ossConfigured`。
 */
export function presignOssUrl(
  method: "GET" | "HEAD",
  key: string,
  ttlSec: number,
): string {
  const storageKey = ossStorageKey(key);
  const expires = Math.floor(Date.now() / 1000) + ttlSec;
  const stringToSign = `${method}\n\n\n${expires}\n/${OSS_BUCKET}/${storageKey}`;
  const signature = createHmac("sha1", OSS_ACCESS_KEY_SECRET ?? "")
    .update(stringToSign)
    .digest("base64");
  const query = new URLSearchParams({
    OSSAccessKeyId: OSS_ACCESS_KEY_ID ?? "",
    Expires: String(expires),
    Signature: signature,
  });
  return `https://${OSS_BUCKET}.${OSS_ENDPOINT}${encodedPath(storageKey)}?${query}`;
}

/**
 * 已验证存在的发布资产的下载 URL：优先 Cloudflare Worker（URL 恒定，便于多分段 Range 与边缘缓存），
 * 未配置 `OSS_CDN_BASE` 时回退 1h 预签名直链。改名存储的对象（`.apk` / `.ipa`）
 * 无法直出：OSS 默认域名既拒绝原名，也拒绝用 response-content-* 把文件名 / 类型
 * 改回 apk，直链只能得到 `.bin`，此时返回 null 交由调用方回退。
 * 调用前须保证 `ossConfigured`。
 */
export async function ossDownloadUrl(key: string): Promise<string | null> {
  if (WORKER_BASE) return (await signedWorkerUrl(WORKER_BASE, key)).toString();
  if (ossStorageKey(key) !== key) return null;
  return presignOssUrl("GET", key, 3600);
}
