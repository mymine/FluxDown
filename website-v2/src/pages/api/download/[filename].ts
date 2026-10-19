/**
 * Release 下载入口（仅 302，不代理文件流量）。
 * source=auto：中国大陆访客先验证 OSS 再走阿里云 CDN；其余走 Cloudflare 边缘缓存（未命中 OSS 回源、
 * 失败 GitHub）；两者都没有时验证 OSS 后直连。OSS 缺少该资产一律回退 GitHub。
 * source=github：仅 GitHub；source=oss：仅已验证 OSS，不可用返回 503。
 * 无 source / 未知 source：保留旧客户端 OSS 优先、GitHub 兜底行为（大陆访客同样分流到阿里云 CDN）。
 */
import type { APIRoute } from "astro";
import { GITHUB_TOKEN, GITHUB_REPO } from "astro:env/server";
import { getCached, setCached } from "@/lib/api-cache";
import {
  cdnDownloadUrl, cnCdnConfigured, cnCdnDownloadUrl, ossConfigured, ossDownloadUrl, presignOssUrl,
  releaseObjectKey,
} from "@/lib/oss";
import { DownloadLookupCache } from "@/lib/download-cache";
import { createOssProbe } from "@/lib/download-probe";
import {
  DownloadLookupError,
  downloadRedirect,
  downloadSource,
  latestWithAsset,
  isGitHubDownloadUrl,
  mayProbeOss,
  ossUnavailable,
  transientGitHubStatus,
  type DownloadRelease,
} from "@/lib/download-policy";

export const prerender = false;

const GITHUB_HEADERS: Record<string, string> = {
  Accept: "application/vnd.github+json",
  "X-GitHub-Api-Version": "2022-11-28",
  ...(GITHUB_TOKEN ? { Authorization: `Bearer ${GITHUB_TOKEN}` } : {}),
};
const ossHasAsset = createOssProbe((key) => presignOssUrl("HEAD", key, 60));

function releaseCache(): DownloadLookupCache<DownloadRelease[]> {
  const key = "download:lookup-cache:v2";
  let cache = getCached<DownloadLookupCache<DownloadRelease[]>>(key, Infinity);
  if (!cache) {
    cache = new DownloadLookupCache<DownloadRelease[]>();
    setCached(key, cache);
  }
  return cache;
}

async function loadReleases(tag: string): Promise<DownloadRelease[]> {
  let response: Response;
  try {
    response = await fetch(
      `https://api.github.com/repos/${GITHUB_REPO}/releases${
        tag ? `/tags/${encodeURIComponent(tag)}` : "?per_page=30"
      }`,
      { headers: GITHUB_HEADERS, signal: AbortSignal.timeout(8000) },
    );
  } catch {
    throw new DownloadLookupError("GitHub API network failure or timeout", true);
  }
  if (tag && response.status === 404) return [];
  if (!response.ok) {
    throw new DownloadLookupError(
      `GitHub API error ${response.status}`,
      transientGitHubStatus(response.status, response.headers),
    );
  }
  let body: string;
  try {
    // fetch 在响应头到达时就完成；同一 8s signal 也覆盖后续读体。
    body = await response.text();
  } catch {
    throw new DownloadLookupError("GitHub API response body interrupted or timed out", true);
  }
  // 读体与解析分开：完整响应中的 JSON/结构异常不能触发旧数据兜底。
  const data: unknown = JSON.parse(body);
  const releases = tag ? [data] : data;
  if (!Array.isArray(releases) || !releases.every(isRelease)) {
    throw new DownloadLookupError("Invalid GitHub release metadata", false);
  }
  if (tag && releases[0]?.tag_name !== tag) {
    throw new DownloadLookupError("GitHub release tag mismatch", false);
  }
  return releases;
}

function isRelease(value: unknown): value is DownloadRelease {
  if (!value || typeof value !== "object") return false;
  const r = value as DownloadRelease;
  return typeof r.tag_name === "string" && typeof r.draft === "boolean" &&
    typeof r.prerelease === "boolean" && Array.isArray(r.assets) &&
    r.assets.every((a) => a && typeof a.name === "string" &&
      Number.isSafeInteger(a.size) && a.size >= 0 &&
      typeof a.browser_download_url === "string" &&
      isGitHubDownloadUrl(a.browser_download_url, GITHUB_REPO));
}

function jsonError(status: number, error: string): Response {
  return new Response(JSON.stringify({ error }), {
    status,
    headers: { "Content-Type": "application/json", "Cache-Control": "no-store" },
  });
}

export const GET: APIRoute = async ({ params, url, request }) => {
  const { filename } = params;
  if (!filename) return jsonError(400, "Missing filename");
  const tag = url.searchParams.get("tag")?.trim() || "";
  const source = downloadSource(url.searchParams.get("source"));
  // 阿里云 CDN 不会回退 GitHub，所以大陆访客必须先确认 OSS 里有这个资产（结果缓存 1h）。
  // CF-IPCountry 由 Cloudflare 写入；官网只经 Cloudflare 代理对外。
  const mainland = cnCdnConfigured && ossConfigured && request.headers.get("cf-ipcountry") === "CN";

  try {
    const { value: releases, stale } = await releaseCache().lookup(
      tag ? `tag:${tag}` : "releases", () => loadReleases(tag),
    );
    const release = tag ? releases[0] : latestWithAsset(releases, filename);
    if (!release) {
      return jsonError(404, tag ? `Release "${tag}" not found` :
        `No published release contains asset "${filename}"`);
    }
    // 显式 tag 允许预发布，任何来源都禁止草稿。
    if (release.draft) return jsonError(403, `Release "${tag}" is a draft`);
    const asset = release.assets.find((a) => a.name === filename);
    if (!asset) {
      return jsonError(404, `Asset "${filename}" not found in release "${release.tag_name}"`);
    }

    // 只有 GitHub 新鲜元数据确认的 tag + asset 才有资格获得 OSS 签名。
    // 过期缓存只允许 GitHub 公共 URL：撤回/转草稿后由 GitHub 自身拒绝访问。
    if (source === "auto" && !stale && !mainland) {
      const cdn = await cdnDownloadUrl(releaseObjectKey(release.tag_name, asset.name), release.tag_name);
      // 不先 HEAD OSS：即使源站离线，边缘仍可能持有完整缓存。
      if (cdn) return downloadRedirect(cdn, "cdn");
    }
    if (mayProbeOss(source, ossConfigured, stale)) {
      const key = releaseObjectKey(release.tag_name, asset.name);
      if (await ossHasAsset(key, asset.size)) {
        try {
          const target = mainland ? cnCdnDownloadUrl(key) : await ossDownloadUrl(key);
          if (target) return downloadRedirect(target, mainland ? "cn-cdn" : "oss");
        } catch {
          // 签名失败同样遵循强制 OSS 的 503 / auto 与旧客户端的 GitHub 兜底策略。
        }
      }
    }
    if (source === "oss") return ossUnavailable();
    return downloadRedirect(asset.browser_download_url, "github", stale);
  } catch (error) {
    if (source === "oss") return ossUnavailable();
    if (error instanceof DownloadLookupError && error.transient) {
      return jsonError(503, error.message);
    }
    return jsonError(500, "Download metadata lookup failed");
  }
};
