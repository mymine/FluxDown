/**
 * Release 下载入口（仅 302，不代理文件流量）。
 * 镜像 = dl-cn（Cloudflare Worker / 备案后阿里云 CDN，私有 OSS 回源，不能回退 GitHub），302 前必须确认 OSS 有该资产且大小一致。
 * 只有中国大陆访客（ip2region 按 X-Client-IP 判定）且像正常客户端（24 小时内读过发布清单、
 * 非脚本 User-Agent，见 download-gate.ts）才走镜像；其余访客、脚本与镜像缺资产时走 GitHub。
 * source=github：仅 GitHub；source=oss：可走镜像的访客在镜像不可用时返回 503，其余同 auto。
 */
import type { APIRoute } from "astro";
import { GITHUB_TOKEN, GITHUB_REPO } from "astro:env/server";
import { getCached, setCached } from "@/lib/api-cache";
import { getClientIp } from "@/lib/client-ip";
import { recordDownload } from "@/lib/download-ban";
import { mayUseMirror } from "@/lib/download-gate";
import { isMainlandChinaIp } from "@/lib/geoip";
import { cdnConfigured, cdnDownloadUrl, ossConfigured, presignOssHeadUrl, releaseObjectKey } from "@/lib/oss";
import { DownloadLookupCache } from "@/lib/download-cache";
import { createOssProbe } from "@/lib/download-probe";
import {
  DownloadLookupError,
  downloadRedirect,
  downloadSource,
  latestWithAsset,
  isGitHubDownloadUrl,
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
const ossHasAsset = createOssProbe((key) => presignOssHeadUrl(key, 60));

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

export const GET: APIRoute = async ({ params, url, request, clientAddress }) => {
  const { filename } = params;
  if (!filename) return jsonError(400, "Missing filename");
  const tag = url.searchParams.get("tag")?.trim() || "";
  const source = downloadSource(url.searchParams.get("source"));
  const ip = getClientIp(request, clientAddress);
  const banned = recordDownload(ip, `${tag}/${filename}`);
  if (banned > 0) {
    return new Response(JSON.stringify({ error: "Too many downloads from this IP; try again later." }), {
      status: 429,
      headers: {
        "Content-Type": "application/json",
        "Cache-Control": "no-store",
        "Retry-After": String(Math.ceil(banned / 1000)),
      },
    });
  }
  const wantsMirror = source !== "github" && isMainlandChinaIp(ip) &&
    mayUseMirror(ip, request.headers.get("user-agent"));

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

    // 只有 GitHub 新鲜元数据确认的 tag + asset 才有资格获得 CDN 签名；HEAD 结果缓存 1h。
    // 过期缓存只允许 GitHub 公共 URL：撤回/转草稿后由 GitHub 自身拒绝访问。
    if (wantsMirror && cdnConfigured && ossConfigured && !stale) {
      const key = releaseObjectKey(release.tag_name, asset.name);
      if (await ossHasAsset(key, asset.size)) return downloadRedirect(cdnDownloadUrl(key), "cdn");
    }
    if (source === "oss" && wantsMirror) return ossUnavailable();
    return downloadRedirect(asset.browser_download_url, "github", stale);
  } catch (error) {
    if (source === "oss" && wantsMirror) return ossUnavailable();
    if (error instanceof DownloadLookupError && error.transient) {
      return jsonError(503, error.message);
    }
    return jsonError(500, "Download metadata lookup failed");
  }
};
