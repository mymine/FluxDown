/**
 * 发布资产在 OSS 中的实际存储键。
 *
 * 阿里云 OSS 禁止经默认域名（*.aliyuncs.com）分发 `.apk` / `.ipa`（400 ApkDownloadForbidden），
 * 而 bucket 未绑定备案域名，所以这两类对象在桶内追加 `.bin` 存储。OSS 同时按对象名、
 * 对象 Content-Type 元数据与 response-content-* 覆盖参数拦截，因此桶内对象的 Content-Type
 * 必须是 application/octet-stream，也不能靠预签名参数改回 apk 类型。对外 URL、GitHub 资产名
 * 与 SHA256SUMS 仍是原名：下载镜像（cdn/oss-proxy.js Worker）回源时换成 `.bin` 存储键，并按
 * `renamedDownloadHeaders` 覆盖 Content-Type / Content-Disposition。
 * 同一规则在 .github/actions/oss-upload/action.yml 的 bash 里复刻，改一处须同步另一处。
 */

const STORAGE_SUFFIX = ".bin";

const BLOCKED_TYPES: Readonly<Record<string, string>> = {
  ".apk": "application/vnd.android.package-archive",
  ".ipa": "application/octet-stream",
};

function blockedExtension(key: string): string | null {
  const lower = key.toLowerCase();
  return Object.keys(BLOCKED_TYPES).find((ext) => lower.endsWith(ext)) ?? null;
}

/** 对外对象键 → 桶内存储键。 */
export function ossStorageKey(key: string): string {
  return blockedExtension(key) ? `${key}${STORAGE_SUFFIX}` : key;
}

/** 改名存储的对象需要覆盖的下载响应头；普通对象返回 null，沿用 OSS 原样响应。 */
export function renamedDownloadHeaders(
  key: string,
): { contentType: string; contentDisposition: string } | null {
  const ext = blockedExtension(key);
  if (!ext) return null;
  const name = key.slice(key.lastIndexOf("/") + 1);
  const ascii = name.replace(/[^\x20-\x7e]|["\\]/g, "_");
  return {
    contentType: BLOCKED_TYPES[ext],
    contentDisposition: `attachment; filename="${ascii}"; filename*=UTF-8''${encodeURIComponent(name)}`,
  };
}
