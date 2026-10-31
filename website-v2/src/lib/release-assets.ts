/**
 * GitHub Release → 组件归属与选取（/api/release 的纯逻辑，无运行时依赖，可单测）。
 *
 * 两代发布形态并存，必须同时识别：
 * - 统一 release（当前）：一个 `vX.Y.Z` release 承载全部组件资产，每个组件上传完成后
 *   附带哨兵 `SHA256SUMS-<组件>.txt`（release.yml 的 release-upload action 最后上传）。
 *   只有哨兵齐全的组件才算发布完整——部分上传（中途失败）的组件不会被选中。
 * - 拆分 release（历史）：`vX.Y.Z` 仅桌面端，另有 `server-v*` / `cli-v*` / `mobile-v*` /
 *   `extension-v*` 组件 release；没有哨兵，按资产名判定组件归属。
 */

export interface GitHubAsset {
  name: string;
  size: number;
  download_count: number;
  url: string; // API URL, 需要 token 才能下载
  browser_download_url: string;
}

export interface GitHubRelease {
  tag_name: string;
  name: string;
  published_at: string;
  draft: boolean;
  prerelease: boolean;
  assets: GitHubAsset[];
}

export type ReleaseComponent = "app" | "extension" | "server" | "cli" | "mobile" | "ios";

/** iOS 侧载 IPA 的资产名后缀（.github/workflows/ios-sideload.yml 产出，改名须同步）。 */
export const IOS_SIDELOAD_SUFFIX = "-ios-sideload-unsigned.ipa";

/**
 * 统一 release 说明的机器头部（已发布组件清单 + 服务器 / CLI 安装说明），由
 * .github/scripts/release_publish.py 维护，只给 GitHub release 页面看。
 * /api/changelog 返回前剥掉，客户端拿到的 body 仍只有更新说明。
 */
const RELEASE_HEADER_RE =
  /<!-- fluxdown:release:begin[^>]*-->[\s\S]*?<!-- fluxdown:release:end -->\n*/g;

export function stripReleaseHeader(body: string): string {
  return body.replace(RELEASE_HEADER_RE, "");
}

/** 统一 release 的组件完成哨兵 `SHA256SUMS-<组件>.txt`（与 .github/actions/release-upload 一致）。 */
const SENTINEL_RE = /^SHA256SUMS-[a-z]+\.txt$/;

/** 按资产名的组件归属判定：历史拆分 release（无哨兵）与下方 NAME_FALLBACK 组件使用。 */
const LEGACY_MATCHERS: Record<ReleaseComponent, (name: string) => boolean> = {
  app: (n) => n.endsWith("-setup.exe") || n.endsWith("-portable.zip"),
  extension: (n) =>
    n.endsWith("-chrome.zip") ||
    n.endsWith("-extension.zip") ||
    n.endsWith("-firefox.xpi"),
  server: (n) => n.startsWith("FluxDown-Server-"),
  cli: (n) => n.startsWith("FluxDown-CLI-"),
  mobile: (n) => n.includes("-android-") && n.endsWith(".apk"),
  ios: (n) => n.endsWith(IOS_SIDELOAD_SUFFIX),
};

/**
 * 组件接入哨兵之前就附在统一 release 上的资产：iOS 侧载 IPA 在并入 release.yml 之前由独立
 * workflow 直接挂到 release（没有 SHA256SUMS-ios.txt）。它是单文件资产，不存在「半套」，
 * 统一 release 缺该组件哨兵时仍按资产名识别；有哨兵时以哨兵为准。
 */
const NAME_FALLBACK: Partial<Record<ReleaseComponent, true>> = { ios: true };

/** 该 release 是否完整包含某组件：有该组件哨兵即包含；统一 release 缺哨兵时只有 NAME_FALLBACK 组件按资产名判定。 */
export function hasComponent(
  release: GitHubRelease,
  component: ReleaseComponent,
): boolean {
  const names = release.assets.map((a) => a.name);
  if (names.includes(`SHA256SUMS-${component}.txt`)) return true;
  if (names.some((n) => SENTINEL_RE.test(n)) && !NAME_FALLBACK[component]) {
    return false;
  }
  return names.some(LEGACY_MATCHERS[component]);
}

/**
 * 组件 release 的 tag 形态：桌面端只认 `vX.Y.Z`；其余组件同时接受统一 release 的
 * `vX.Y.Z` 与历史组件前缀 `<组件>-vX.Y.Z`。frontier 额外放行 `-rc.N` 等预发布后缀。
 */
export function componentTagRe(
  component: ReleaseComponent,
  includePrerelease: boolean,
): RegExp {
  const prefix = component === "app" ? "" : `(?:${component}-)?`;
  const pre = includePrerelease ? "(?:-[\\w.]+)?" : "";
  return new RegExp(`^${prefix}v\\d+\\.\\d+\\.\\d+${pre}$`);
}

/** 去掉组件前缀与 `v`：`server-v0.4.8` / `v0.5.0-rc.1` → `0.4.8` / `0.5.0-rc.1`。 */
export function releaseVersion(tag: string): string {
  return tag.replace(/^(?:cli|mobile|server|extension|website)-/, "").replace(/^v/, "");
}

/**
 * SemVer 2.0 精度比较两个 release tag（去组件前缀后），a>b 时返回 >0。
 * 处理 frontier 的 `-rc.N` 预发布后缀——普通数字切分会误判。
 */
export function cmpReleaseTag(a: string, b: string): number {
  const [ca, pa = ""] = releaseVersion(a).split("-", 2);
  const [cb, pb = ""] = releaseVersion(b).split("-", 2);
  const na = ca.split(".").map((s) => Number.parseInt(s, 10) || 0);
  const nb = cb.split(".").map((s) => Number.parseInt(s, 10) || 0);
  for (let i = 0; i < 3; i++) {
    const d = (na[i] ?? 0) - (nb[i] ?? 0);
    if (d !== 0) return d;
  }
  // core 相等：无预发布 > 有预发布（SemVer 2.0 §11.3）
  if (!pa && !pb) return 0;
  if (!pa) return 1;
  if (!pb) return -1;
  const ida = pa.split(".");
  const idb = pb.split(".");
  for (let i = 0; i < Math.max(ida.length, idb.length); i++) {
    const x = ida[i];
    const y = idb[i];
    if (x === undefined) return -1;
    if (y === undefined) return 1;
    const xn = /^\d+$/.test(x);
    const yn = /^\d+$/.test(y);
    if (xn && yn) {
      const d = Number.parseInt(x, 10) - Number.parseInt(y, 10);
      if (d !== 0) return d;
    } else if (xn !== yn) {
      return xn ? -1 : 1; // 数字标识符 < 字母数字标识符
    } else if (x !== y) {
      return x < y ? -1 : 1;
    }
  }
  return 0;
}

/**
 * 从候选池挑选完整包含 `component` 的 release。
 * stable 保留 GitHub 列表顺序（created_at 倒序）的首个匹配；frontier 取 SemVer 最大，
 * 避免旧版本线上发布时间更晚的 hotfix 盖过更高的预发布。
 */
export function pickComponentRelease(
  pool: GitHubRelease[],
  component: ReleaseComponent,
  frontier: boolean,
): GitHubRelease | undefined {
  const re = componentTagRe(component, frontier);
  const matches = pool.filter(
    (r) => re.test(r.tag_name) && hasComponent(r, component),
  );
  if (!frontier) return matches[0];
  return matches.reduce<GitHubRelease | undefined>(
    (best, r) =>
      best && cmpReleaseTag(best.tag_name, r.tag_name) >= 0 ? best : r,
    undefined,
  );
}

/**
 * 桌面端资产过滤：统一 release 里服务器与 CLI 的 `-linux-x64.tar.gz` / `-macos-*.tar.gz`
 * 与桌面便携包后缀相同，按后缀匹配桌面资产前必须先排除它们，否则桌面自动更新会拿到
 * 服务器/CLI 压缩包。
 */
export function desktopAssets(release: GitHubRelease): GitHubAsset[] {
  return release.assets.filter(
    (a) =>
      !a.name.startsWith("FluxDown-Server-") &&
      !a.name.startsWith("FluxDown-CLI-"),
  );
}
