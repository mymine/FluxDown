/**
 * 发布资产在 OSS 中的实际存储键。
 *
 * 阿里云 OSS 禁止经默认域名（*.aliyuncs.com）分发 `.apk` / `.ipa`（400 ApkDownloadForbidden），
 * 而 bucket 未绑定备案域名，所以这两类对象在桶内追加 `.bin` 存储。OSS 同时按对象名、
 * 对象 Content-Type 元数据与 response-content-* 覆盖参数拦截，因此桶内对象的 Content-Type
 * 必须是 application/octet-stream，也不能靠预签名参数改回 apk 类型。对外 URL、GitHub 资产名
 * 与 SHA256SUMS 仍是原名：下载镜像（ESA 边缘函数 cdn/esa-dl-cn.js）回源时换成 `.bin` 存储键，并覆盖
 * Content-Type / Content-Disposition。
 * 同一规则在 cdn/esa-dl-cn.js 与 .github/actions/oss-upload/action.yml 的 bash 里各复刻一份，改一处须同步另两处。
 */

const STORAGE_SUFFIX = ".bin";

const BLOCKED_EXTENSIONS = [".apk", ".ipa"];

function isBlocked(key: string): boolean {
  const lower = key.toLowerCase();
  return BLOCKED_EXTENSIONS.some((ext) => lower.endsWith(ext));
}

/** 对外对象键 → 桶内存储键。 */
export function ossStorageKey(key: string): string {
  return isBlocked(key) ? `${key}${STORAGE_SUFFIX}` : key;
}
