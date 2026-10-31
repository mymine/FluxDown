import { logWarn } from "./error-log";
/**
 * 右键「用 FluxDown 下载链接」的浏览器捕获事务。
 *
 * 链接交给浏览器发起一次下载，由下载拦截层（onDeterminingFilename）在浏览器解析完响应头后
 * 取消并发送——与直接点击下载同一条路径：文件名 / 大小 / 重定向后的地址 / Cookie 都来自浏览器
 * 自己的网络栈（代理、登录态与点击一致），不再依赖桌面端事后的 HEAD 探测。
 *
 * 本模块只记账：哪些浏览器下载属于右键事务、事务是否已被拦截层认领、超时 / 失败后交还调用方
 * 直接发送。浏览器 API 经构造参数注入，便于测试。
 */

/** 一次右键发送请求。 */
export interface LinkCaptureRequest<Channel = string> {
  url: string;
  referrer: string;
  channel?: Channel;
}

/** 注入的浏览器下载能力。 */
export interface LinkCaptureDeps {
  /** 发起浏览器下载，返回下载 id。 */
  download(url: string): Promise<number>;
  cancel(downloadId: number): Promise<void>;
  erase(downloadId: number): Promise<unknown>;
}

export interface LinkCaptureOptions {
  /** 浏览器迟迟拿不到响应头时放弃浏览器捕获、交还调用方直接发送的时限。 */
  startTimeoutMs: number;
  /** 已放弃的事务保留多久：期间迟到的浏览器下载仍会被识别并静默取消。 */
  abandonedTtlMs: number;
}

/** 浏览器下载在拦截层看到的身份（`DownloadItem` 的子集）。 */
export interface LinkCaptureItem {
  id: number;
  url: string;
}

/**
 * 拦截层认领结果：`claimed` = 由拦截层取消并发送；`declined` = 浏览器拿到的不是文件
 * （网页），已交还调用方直接发送，只需取消；`abandoned` = 已超时交还直接发送，只需取消。
 */
export type LinkCaptureClaim<Channel = string> =
  | { kind: "claimed"; request: LinkCaptureRequest<Channel> }
  | { kind: "declined" }
  | { kind: "abandoned" };

interface Entry<Channel> {
  key: string;
  request: LinkCaptureRequest<Channel>;
  downloadId?: number;
  abandoned: boolean;
  /** 取消启动超时计时器。 */
  cancelTimer?: () => void;
  /** 未决时存在；调用一次后清空。 */
  settle?: (claimed: boolean) => void;
}

const DEFAULT_OPTIONS: LinkCaptureOptions = {
  startTimeoutMs: 20_000,
  abandonedTtlMs: 60_000,
};

/** 浏览器下载项的 url 是规范化后的形式；两边按 WHATWG href 比较。 */
function normalizeUrl(url: string): string {
  try {
    return new URL(url).href;
  } catch {
    return url;
  }
}

export class LinkCaptureRegistry<Channel = string> {
  private readonly entries = new Set<Entry<Channel>>();
  private readonly options: LinkCaptureOptions;

  constructor(
    private readonly deps: LinkCaptureDeps,
    options: Partial<LinkCaptureOptions> = {},
  ) {
    this.options = { ...DEFAULT_OPTIONS, ...options };
  }

  /**
   * 经浏览器捕获发送链接。resolve `true` = 拦截层已认领（发送与失败回退由拦截层负责）；
   * `false` = 浏览器捕获没有成立（发起失败、响应前中断、超时、同一链接已有未决事务），
   * 调用方应直接发送原链接。
   */
  capture(request: LinkCaptureRequest<Channel>): Promise<boolean> {
    const key = normalizeUrl(request.url);
    for (const entry of this.entries) {
      if (entry.key === key && entry.settle) return Promise.resolve(false);
    }
    // 未用 Promise.withResolvers：扩展未声明 Chrome 最低版本，119 以下没有该 API。
    return new Promise((resolve) => {
      const entry: Entry<Channel> = { key, request, abandoned: false, settle: resolve };
      this.entries.add(entry);
      const timer = setTimeout(() => this.abandon(entry), this.options.startTimeoutMs);
      entry.cancelTimer = () => clearTimeout(timer);
      this.deps.download(request.url).then(
        (downloadId) => {
          if (!this.entries.has(entry)) return;
          entry.downloadId = downloadId;
          // 超时后才拿到 id：浏览器下载仍在进行，取消掉（直接发送已经接手）。
          if (entry.abandoned) this.discard(downloadId);
        },
        (error: unknown) => {
          logWarn("[FluxDown] browser capture of link failed to start:", request.url, error);
          this.entries.delete(entry);
          this.settle(entry, false);
        },
      );
    });
  }

  /** 该浏览器下载是否属于右键事务（含已放弃的）：兜底拦截层据此让路。 */
  owns(item: LinkCaptureItem): boolean {
    return this.find(item) !== undefined;
  }

  /**
   * onDeterminingFilename：认领属于右键事务的下载；不属于时返回 `null`。`accept` 为 false
   * （响应不是文件）时交还调用方直接发送原链接。
   */
  claim(item: LinkCaptureItem, accept: boolean): LinkCaptureClaim<Channel> | null {
    const entry = this.find(item);
    if (!entry) return null;
    this.entries.delete(entry);
    if (entry.abandoned) return { kind: "abandoned" };
    this.settle(entry, accept);
    return accept ? { kind: "claimed", request: entry.request } : { kind: "declined" };
  }

  /** 浏览器下载在拦截层认领前中断（403 / 网络错误）：交还调用方直接发送。 */
  interrupted(item: LinkCaptureItem): void {
    const entry = this.find(item);
    if (!entry) return;
    this.entries.delete(entry);
    this.settle(entry, false);
    this.deps.erase(item.id).catch((error: unknown) => {
      console.debug("[FluxDown] failed to erase interrupted link capture:", item.id, error);
    });
  }

  private find(item: LinkCaptureItem): Entry<Channel> | undefined {
    const key = normalizeUrl(item.url);
    for (const entry of this.entries) {
      if (entry.downloadId !== undefined ? entry.downloadId === item.id : entry.key === key) return entry;
    }
    return undefined;
  }

  private settle(entry: Entry<Channel>, claimed: boolean): void {
    entry.cancelTimer?.();
    entry.cancelTimer = undefined;
    const settle = entry.settle;
    entry.settle = undefined;
    settle?.(claimed);
  }

  private abandon(entry: Entry<Channel>): void {
    if (!this.entries.has(entry) || !entry.settle) return;
    entry.abandoned = true;
    this.settle(entry, false);
    if (entry.downloadId !== undefined) this.discard(entry.downloadId);
    setTimeout(() => this.entries.delete(entry), this.options.abandonedTtlMs);
  }

  private discard(downloadId: number): void {
    this.deps
      .cancel(downloadId)
      .then(() => this.deps.erase(downloadId))
      .catch((error: unknown) => {
        console.debug("[FluxDown] failed to discard abandoned link capture:", downloadId, error);
      });
  }
}
