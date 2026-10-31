/**
 * 123 云盘直链发布资产源（纯逻辑，env 绑定见 pan123.ts）。
 *
 * 发布资产由 .github/scripts/pan123_mirror.sh（经东京服务器中转）经 WebDAV 上传到授权目录
 * （123「我的文件/fd-release」，已启用直链空间）下的 `<OSS_RELEASE_PREFIX>/<版本>/<组件>/<file>`
 * （与 OSS 同一对象键，releaseObjectKey）。
 *
 * 下载链接是直链空间 URL 加「URL 鉴权」签名，由官网本地签发、不请求 123；链接不绑请求 IP，
 * 过期由 123 CDN 校验。WebDAV 的 GET 302 链接只认生成它的出口 IP（2026-10 实测 50002），不可用。
 * 302 前仍以 WebDAV PROPFIND 确认资产存在且大小与 GitHub 资产一致（镜像不能回退 GitHub）。
 */

import { createHash, randomInt } from "node:crypto";

/** WebDAV PROPFIND（Depth: 0）响应里的 getcontentlength；命名空间前缀由服务端决定。 */
export function webdavContentLength(xml: string): number | null {
  const m = /<(?:[A-Za-z][\w.-]*:)?getcontentlength>\s*(\d+)\s*</.exec(xml);
  if (!m) return null;
  const size = Number(m[1]);
  return Number.isSafeInteger(size) ? size : null;
}

function encodedKey(key: string): string {
  return key.split("/").map(encodeURIComponent).join("/");
}

export interface DirectLinkConfig {
  /** 直链域名，如 `https://<uid>.cdn.123clouddisk.cn`。 */
  base: string;
  uid: string;
  /** 直链空间文件夹（WebDAV 授权目录）名。 */
  folder: string;
  /** 「URL 鉴权」密钥。 */
  authKey: string;
}

/**
 * 直链空间 URL 鉴权（123「直链鉴权开发指引」）：
 * `<base>/<uid>/<folder>/<key>?auth_key=<过期时刻>-<rand>-<uid>-md5(<路径>-<过期时刻>-<rand>-<uid>-<密钥>)`，
 * 签名用解码后的原始路径（2026-10 实测：用编码后路径签含空格 / `+` 的文件名得 1026 签名错误），
 * URL 里放按段编码的路径；过期时刻为 unix 秒（与阿里云 A 方式不同，不是签发时刻）。
 */
export function signDirectLink(
  config: DirectLinkConfig,
  key: string,
  ttlSec: number,
  nowMs = Date.now(),
  rand = randomInt(0, 1_000_000_000),
): string {
  const path = `/${config.uid}/${config.folder}/${key}`;
  const encoded = `/${config.uid}/${encodedKey(config.folder)}/${encodedKey(key)}`;
  const expires = Math.floor(nowMs / 1000) + ttlSec;
  const hash = createHash("md5").update(`${path}-${expires}-${rand}-${config.uid}-${config.authKey}`).digest("hex");
  return `${config.base.replace(/\/+$/, "")}${encoded}?auth_key=${expires}-${rand}-${config.uid}-${hash}`;
}

/**
 * 返回 `(key, expectedSize) => 资产是否在 123 且大小一致`（WebDAV PROPFIND）。结果按 key + 大小缓存：
 * 存在 1 小时、缺失或失败 60 秒；同一 key 并发合并；在途数达到 `capacity` 视作不可用。
 */
export function createPan123Probe(
  base: string,
  authorization: string,
  request: (input: string, init: RequestInit) => Promise<Response> =
    (input, init) => fetch(input, init),
  now = Date.now,
  capacity = 256,
) {
  const root = base.replace(/\/+$/, "");
  const cache = new Map<string, { until: number; present: boolean }>();
  const inflight = new Map<string, Promise<boolean>>();

  return async (key: string, expectedSize: number): Promise<boolean> => {
    if (!Number.isSafeInteger(expectedSize) || expectedSize < 0) return false;
    const cacheKey = JSON.stringify([key, expectedSize]);
    const hit = cache.get(cacheKey);
    if (hit && now() < hit.until) return hit.present;
    const pending = inflight.get(cacheKey);
    if (pending) return pending;
    if (inflight.size >= capacity) return false;
    const work = Promise.resolve().then(async () => {
      let present = false;
      try {
        const response = await request(`${root}/${encodedKey(key)}`, {
          method: "PROPFIND",
          headers: { Authorization: authorization, Depth: "0" },
          signal: AbortSignal.timeout(5000),
          redirect: "error",
        });
        present = response.status === 207 && webdavContentLength(await response.text()) === expectedSize;
      } catch {
        // 超时、网络与解析错误均按资产不可用处理，交给下一个下载源。
      }
      cache.delete(cacheKey);
      while (cache.size >= capacity) cache.delete(cache.keys().next().value!);
      cache.set(cacheKey, { until: now() + (present ? 3_600_000 : 60_000), present });
      return present;
    }).finally(() => inflight.delete(cacheKey));
    inflight.set(cacheKey, work);
    return work;
  };
}
