/**
 * 官网浏览器下载前的 ESA AI 验证码（下载页、更新日志页；服务端见 src/pages/api/download/pass.ts）。
 *
 * 页面加载时问 `GET /api/download/pass`：只有服务端说需要（启用了通行证、大陆访客经 ESA、还没有有效通行证）
 * 才加载验证码 JS——文档要求加载与验证间隔 > 2s，所以不等到点击再加载。点击本站 `/api/download/*`
 * （非 `source=github`）链接时先跑无痕验证（可疑时自动换滑块），通过后 `POST /api/download/pass`
 * 换通行证 Cookie，再在本页跳转下载（异步回调里 window.open 会被拦截；附件下载不会离开本页）。
 * 验证码加载失败、用户关掉验证弹窗、换证失败都照常跳转：没有通行证时服务端回落 GitHub。
 *
 * 身份标与场景 ID 来自 ESA 控制台「AI 验证码 → 配置」规则 `download-pass`（公开值，会出现在页面里）。
 */
import { withBase } from "@/lib/base";

const CAPTCHA_PREFIX = "esa-ssgyk4wq8p";
const CAPTCHA_SCENE_ID = "7dazxm3c";
const CAPTCHA_SCRIPT = "https://o.alicdn.com/captcha-frontend/aliyunCaptcha/AliyunCaptcha.js";
const CAPTCHA_SERVERS = ["captcha-esa-open.aliyuncs.com", "captcha-esa-open-b.aliyuncs.com"];
const PASS_PATH = "/api/download/pass";
const ELEMENT_ID = "fd-dl-captcha";
const TRIGGER_ID = "fd-dl-captcha-trigger";
/** 通行证到期前这么久就重新验证，避免点下去时刚好过期。 */
const EXPIRY_MARGIN_MS = 60_000;
/** 验证码 JS 还没就绪时点了下载：最多等这么久，超时直接跳转。 */
const LOAD_WAIT_MS = 5_000;
/** 关闭弹窗后等成功回调的宽限：SDK 可能先关弹窗、延迟约 1 秒才回调 success。 */
const CLOSE_GRACE_MS = 1_500;

interface CaptchaInstance {
  refresh?: () => void;
}

declare global {
  interface Window {
    AliyunCaptchaConfig?: { region: string; prefix: string };
    initAliyunCaptcha?: (options: Record<string, unknown>) => void;
  }
}

let installed = false;

/** 在当前页拦截镜像下载链接；同一页只装一次。 */
export function installDownloadCaptcha(lang: "zh" | "en"): void {
  if (installed) return;
  installed = true;

  /** off：不需要或不可用，链接照常；loading / ready：点击先验证。 */
  let state: "off" | "loading" | "ready" = "off";
  let passUntil = 0;
  let pending: string | null = null;
  let redeeming = false;
  let captcha: CaptchaInstance | null = null;

  const passUrl = withBase(PASS_PATH);

  function proceed() {
    const href = pending;
    pending = null;
    if (href) window.location.assign(href);
  }

  function disable() {
    state = "off";
    proceed();
  }

  function trigger() {
    document.getElementById(TRIGGER_ID)?.click();
  }

  async function redeem(param: string) {
    redeeming = true;
    const response = await fetch(passUrl, {
      method: "POST",
      headers: { "captcha-verify-param": param },
      credentials: "same-origin",
    }).catch(() => null);
    const data = response?.ok
      ? ((await response.json().catch(() => null)) as { expires_in?: number } | null)
      : null;
    if (typeof data?.expires_in === "number") passUntil = Date.now() + data.expires_in * 1000 - EXPIRY_MARGIN_MS;
    redeeming = false;
    captcha?.refresh?.();
    proceed();
  }

  function load() {
    state = "loading";
    const element = document.createElement("div");
    element.id = ELEMENT_ID;
    const button = document.createElement("button");
    button.type = "button";
    button.id = TRIGGER_ID;
    button.hidden = true;
    button.tabIndex = -1;
    document.body.append(element, button);

    window.AliyunCaptchaConfig = { region: "cn", prefix: CAPTCHA_PREFIX };
    const script = document.createElement("script");
    script.src = CAPTCHA_SCRIPT;
    script.async = true;
    script.onerror = disable;
    script.onload = () => {
      if (!window.initAliyunCaptcha) return disable();
      window.initAliyunCaptcha({
        SceneId: CAPTCHA_SCENE_ID,
        mode: "popup",
        element: `#${ELEMENT_ID}`,
        button: `#${TRIGGER_ID}`,
        server: CAPTCHA_SERVERS,
        language: lang === "zh" ? "cn" : "en",
        success: (param: string) => void redeem(param),
        // 立即回调 success（默认延迟 1 秒），尽量赶在 onClose 宽限之前开始换证。
        delayBeforeSuccess: false,
        // 无痕未通过时 SDK 自动切换其他验证方式并刷新，等用户完成或关闭弹窗。
        fail: () => undefined,
        getInstance: (instance: CaptchaInstance) => {
          captcha = instance;
          state = "ready";
          if (pending) trigger();
        },
        onError: disable,
        onClose: () => {
          window.setTimeout(() => {
            if (!redeeming) proceed();
          }, CLOSE_GRACE_MS);
        },
      });
    };
    document.head.append(script);
  }

  /** 本站下载入口（不含 GitHub 备用直链）：带通行证才可能拿到大陆镜像。 */
  function mirrorLink(target: EventTarget | null): HTMLAnchorElement | null {
    const link = target instanceof Element ? target.closest<HTMLAnchorElement>("a[href]") : null;
    if (!link) return null;
    const url = new URL(link.href, window.location.href);
    if (url.origin !== window.location.origin || url.searchParams.get("source") === "github") return null;
    return url.pathname.startsWith(withBase("/api/download/")) && !url.pathname.endsWith(PASS_PATH) ? link : null;
  }

  document.addEventListener(
    "click",
    (event) => {
      if (state === "off" || Date.now() < passUntil) return;
      if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
      const link = mirrorLink(event.target);
      if (!link) return;
      event.preventDefault();
      const busy = pending !== null;
      pending = link.href;
      if (busy) return;
      if (state === "ready") return trigger();
      window.setTimeout(() => {
        if (state !== "ready") proceed();
      }, LOAD_WAIT_MS);
    },
    { capture: true },
  );

  void fetch(passUrl, { credentials: "same-origin", cache: "no-store" })
    .then((response) => (response.ok ? (response.json() as Promise<{ required?: unknown }>) : null))
    .then((data) => {
      if (data?.required === true) load();
    })
    .catch(() => undefined);
}
