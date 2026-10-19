/**
 * 本帧媒体元素跟踪（ISOLATED world，sensor 使用）。
 *
 * 维护每个 video/audio 元素的稳定 key、播放 / 可见 / 交互状态与 MSE 绑定，
 * 500 ms 节流后以 `mediaElementState` 交给 background；同时向浮钮提供矩形命中、
 * 当前播放元素等查询。元素发现来源：初始扫描、MutationObserver、EV_MEDIA 事件目标。
 */
import { browser } from "wxt/browser";
import type { ElementState } from "@/background/protocol";
import type { MediaDetail } from "./bridge";

export type ElementPayload = Omit<ElementState, "frameId">;

/** 浮钮 / 命中测试要求的最小可操作面积。 */
export const MIN_TARGET_W = 120;
export const MIN_TARGET_H = 80;

const SEND_THROTTLE_MS = 500;
const TIMEUPDATE_TICK_MS = 1000;
/** 播放中的 lastPlayedAt 仅在距上次上报超过该间隔时才触发上报，避免每秒刷新 background 视图。 */
const LAST_PLAYED_REPORT_MS = 15_000;

const MEDIA_EVENTS = [
  "play",
  "playing",
  "pause",
  "ended",
  "emptied",
  "loadedmetadata",
  "resize",
  "timeupdate",
  // EME：媒体数据带加密初始化信息（DRM 内容），下载得到的是密文
  "encrypted",
] as const;

export interface TrackedMedia {
  readonly key: string;
  readonly el: HTMLMediaElement;
  readonly tag: "video" | "audio";
  playing: boolean;
  visibleRatio: number;
  lastPlayedAt: number;
  lastInteractionAt: number;
}

interface Item extends TrackedMedia {
  bindings: MediaDetail | null;
  /** 收到过 `encrypted` 事件（当前来源受 DRM 保护）。 */
  encrypted: boolean;
  lastTick: number;
  lastReportedPlayedAt: number;
  lastSentJson: string;
  handler: (e: Event) => void;
}

export interface MediaTrackerOptions {
  /** `full`：本 sid 的全量快照（background 据此删除快照外的本 sid 元素）。 */
  send(payload: { elements: ElementPayload[]; removed: string[]; full: boolean }): void;
}

function makeFrameUid(): string {
  try {
    return crypto.randomUUID().slice(0, 8);
  } catch {
    // 非安全上下文（http 页面）没有 randomUUID
    return Math.random().toString(36).slice(2, 10).padEnd(8, "0");
  }
}

export function isMediaTag(node: unknown): node is HTMLMediaElement {
  if (!node || typeof node !== "object") return false;
  const tag = (node as Element).tagName;
  return tag === "VIDEO" || tag === "AUDIO";
}

/** 媒体元素可作为浮钮目标的最小尺寸：视频按画面，音频按控件条（通常 300×54 左右）。 */
export function minTargetSize(tag: "video" | "audio"): { w: number; h: number } {
  return tag === "video" ? { w: MIN_TARGET_W, h: MIN_TARGET_H } : { w: MIN_TARGET_W, h: 24 };
}

/**
 * 元素的 shadow root（含 closed）：内容脚本可用 `chrome.dom.openOrClosedShadowRoot`（Chrome）或
 * `openOrClosedShadowRoot`（Firefox）读取页面自身拿不到的 closed root。
 */
function shadowRootOf(el: Element): ShadowRoot | null {
  const dom = browser.dom;
  if (dom && typeof dom.openOrClosedShadowRoot === "function") {
    try {
      const root = dom.openOrClosedShadowRoot(el as HTMLElement);
      if (root instanceof ShadowRoot) return root;
      return null;
    } catch {
      return el.shadowRoot;
    }
  }
  if ("openOrClosedShadowRoot" in el && el.openOrClosedShadowRoot instanceof ShadowRoot) return el.openOrClosedShadowRoot;
  return el.shadowRoot;
}

/** 播放元素标题的最大长度（候选名 / 文件名用，过长的描述截断）。 */
const MAX_TITLE_LEN = 200;

function cleanText(text: string | null | undefined): string {
  return (text ?? "").replace(/\s+/g, " ").trim().slice(0, MAX_TITLE_LEN);
}

/** `a` 与 `b` 最近公共祖先的深度（越大越近）；跨 shadow 边界按宿主继续向上。 */
function commonDepth(ancestorsOfA: Map<Node, number>, b: Node): number {
  let node: Node | null = b;
  while (node) {
    const depth = ancestorsOfA.get(node);
    if (depth !== undefined) return depth;
    node = node.parentNode ?? (node instanceof ShadowRoot ? node.host : null);
  }
  return -1;
}

function ancestorDepths(el: Node): Map<Node, number> {
  const chain: Node[] = [];
  let node: Node | null = el;
  while (node) {
    chain.push(node);
    node = node.parentNode ?? (node instanceof ShadowRoot ? node.host : null);
  }
  const out = new Map<Node, number>();
  chain.reverse().forEach((n, depth) => out.set(n, depth));
  return out;
}

export class MediaTracker {
  /** sensor 会话 id：元素 key 的前缀，background 据此识别同一帧换了新的 sensor 实例。 */
  readonly sid = makeFrameUid();
  /** 下一次 flush 发送全部元素的全量快照（resync / 重注入 / bfcache 恢复）。 */
  private fullPending = false;
  private seq = 0;
  private readonly keys = new WeakMap<Element, string>();
  private readonly items = new Map<string, Item>();
  private readonly dirty = new Set<string>();
  private readonly removedKeys = new Set<string>();
  private readonly listeners = new Set<() => void>();
  private sendTimer: number | undefined;
  private mutationTimer: number | undefined;
  private readonly addedNodes = new Set<Element>();
  private sawRemoval = false;
  private io: IntersectionObserver | null = null;
  private readonly blobSources = new Map<string, string>();
  private ro: ResizeObserver | null = null;
  private mo: MutationObserver | null = null;
  private started = false;
  private readonly shadowRoots = new WeakSet<ShadowRoot>();
  private titleRules: { selector: string; attr?: string }[] = [];
  private floatExclude: string[] = [];

  constructor(private readonly opts: MediaTrackerOptions) {}

  // ===== 生命周期 =====

  start(): void {
    if (this.started) return;
    this.started = true;
    this.scan(document);
    this.scanShadowHosts(document);
    document.addEventListener("pointerdown", this.onPointerDown, { capture: true, passive: true });
    this.mo = new MutationObserver((records) => {
      for (const r of records) {
        for (const n of r.addedNodes) if (n.nodeType === 1) this.addedNodes.add(n as Element);
        if (r.removedNodes.length > 0) this.sawRemoval = true;
      }
      this.scheduleMutationFlush();
    });
    this.mo.observe(document.documentElement, { childList: true, subtree: true });
    // 启动前已挂上的 shadow root 在 scanShadowHosts 里补观察（它在 mo 创建前执行，这里重扫一次）
    this.scanShadowHosts(document);
  }

  stop(): void {
    if (!this.started) return;
    this.started = false;
    document.removeEventListener("pointerdown", this.onPointerDown, { capture: true });
    this.mo?.disconnect();
    this.mo = null;
    for (const item of [...this.items.values()]) this.release(item, false);
    window.clearTimeout(this.sendTimer);
    window.clearTimeout(this.mutationTimer);
    this.sendTimer = undefined;
    this.mutationTimer = undefined;
  }

  subscribe(cb: () => void): () => void {
    this.listeners.add(cb);
    return () => this.listeners.delete(cb);
  }

  // ===== 元素发现 =====

  private scan(root: ParentNode): void {
    for (const el of root.querySelectorAll("video,audio")) this.adopt(el as HTMLMediaElement);
  }

  /** 遍历 `root` 下已有的 shadow 宿主（启动时一次；之后由 attachShadow 通知增量发现）。 */
  private scanShadowHosts(root: ParentNode): void {
    for (const el of root.querySelectorAll("*")) {
      const shadow = shadowRootOf(el);
      if (shadow) this.observeShadowRoot(shadow);
    }
  }

  /** 页面在 `host` 上创建了 shadow root（MAIN world 的 attachShadow 钩子通知）。 */
  observeShadowHost(host: Element): void {
    const shadow = shadowRootOf(host);
    if (shadow) this.observeShadowRoot(shadow);
  }

  private observeShadowRoot(shadow: ShadowRoot): void {
    if (!this.mo || this.shadowRoots.has(shadow)) return;
    this.shadowRoots.add(shadow);
    this.mo.observe(shadow, { childList: true, subtree: true });
    this.scan(shadow);
    this.scanShadowHosts(shadow);
  }

  /** page 规则：标题来源与浮钮排除。 */
  setHints(titles: { selector: string; attr?: string }[], floatExclude: string[]): void {
    this.titleRules = titles;
    this.floatExclude = floatExclude;
    for (const item of this.items.values()) this.markDirty(item);
  }

  /** 元素是否位于 page 规则声明的浮钮排除容器内（背景 / 广告 / 预览视频）。 */
  isFloatExcluded(el: Element): boolean {
    return this.floatExclude.some((selector) => {
      try {
        return el.closest(selector) !== null;
      } catch {
        return false;
      }
    });
  }

  /**
   * 播放元素的标题：page 规则选择器（取离元素最近的匹配节点）> 正在播放时的 Media Session 标题 >
   * 元素自身 title / aria-label。都没有返回空串（候选沿用页面标题）。
   */
  private titleOf(item: Item): string {
    if (this.titleRules.length > 0) {
      const ancestors = ancestorDepths(item.el);
      let best = "";
      let bestDepth = -1;
      for (const rule of this.titleRules) {
        let nodes: NodeListOf<Element>;
        try {
          nodes = document.querySelectorAll(rule.selector);
        } catch {
          continue;
        }
        for (const node of nodes) {
          const text = cleanText(rule.attr ? node.getAttribute(rule.attr) : node.textContent);
          const depth = commonDepth(ancestors, node);
          if (text && depth > bestDepth) {
            best = text;
            bestDepth = depth;
          }
        }
      }
      if (best) return best;
    }
    if (item.playing) {
      const session = cleanText(navigator.mediaSession?.metadata?.title);
      if (session) return session;
    }
    return cleanText(item.el.getAttribute("title") || item.el.getAttribute("aria-label"));
  }

  private scheduleMutationFlush(): void {
    if (this.mutationTimer !== undefined) return;
    this.mutationTimer = window.setTimeout(() => {
      this.mutationTimer = undefined;
      const added = [...this.addedNodes];
      this.addedNodes.clear();
      for (const node of added) {
        if (isMediaTag(node)) this.adopt(node);
        else if (node.childElementCount > 0) this.scan(node);
        // 构造时挂的 shadow root（宿主当时未连接，MAIN 不会派发通知）在插入文档时补观察
        const own = shadowRootOf(node);
        if (own) this.observeShadowRoot(own);
        if (node.childElementCount > 0) this.scanShadowHosts(node);
      }
      if (this.sawRemoval) {
        this.sawRemoval = false;
        for (const item of [...this.items.values()]) {
          if (!item.el.isConnected) this.release(item, true);
        }
      }
    }, 100);
  }

  /** 开始跟踪元素；已跟踪时直接返回。 */
  adopt(el: HTMLMediaElement): TrackedMedia {
    let key = this.keys.get(el);
    if (!key) {
      key = `${this.sid}:${++this.seq}`;
      this.keys.set(el, key);
    }
    const existing = this.items.get(key);
    if (existing) return existing;

    const handler = (e: Event): void => this.onMediaEvent(item, e.type);
    const item: Item = {
      key,
      el,
      tag: el.tagName === "AUDIO" ? "audio" : "video",
      playing: !el.paused && !el.ended,
      visibleRatio: 0,
      lastPlayedAt: !el.paused && !el.ended ? Date.now() : 0,
      lastInteractionAt: 0,
      bindings: null,
      lastTick: 0,
      lastReportedPlayedAt: 0,
      lastSentJson: "",
      encrypted: false,
      handler,
    };
    this.items.set(key, item);
    this.removedKeys.delete(key);
    for (const name of MEDIA_EVENTS) el.addEventListener(name, handler, { passive: true });
    this.ensureObservers();
    this.io?.observe(el);
    this.ro?.observe(el);
    this.markDirty(item);
    return item;
  }

  private ensureObservers(): void {
    if (!this.io && typeof IntersectionObserver !== "undefined") {
      this.io = new IntersectionObserver(
        (entries) => {
          for (const entry of entries) {
            const item = this.itemOf(entry.target);
            if (!item) continue;
            item.visibleRatio = entry.isIntersecting ? entry.intersectionRatio : 0;
            this.markDirty(item);
          }
        },
        { threshold: [0, 0.5, 1] },
      );
    }
    if (!this.ro && typeof ResizeObserver !== "undefined") {
      this.ro = new ResizeObserver((entries) => {
        for (const entry of entries) {
          const item = this.itemOf(entry.target);
          if (item) this.markDirty(item);
        }
      });
    }
  }

  private itemOf(el: Element): Item | undefined {
    const key = this.keys.get(el);
    return key ? this.items.get(key) : undefined;
  }

  private release(item: Item, report: boolean): void {
    for (const name of MEDIA_EVENTS) item.el.removeEventListener(name, item.handler);
    this.io?.unobserve(item.el);
    this.ro?.unobserve(item.el);
    this.items.delete(item.key);
    this.dirty.delete(item.key);
    if (report) {
      this.removedKeys.add(item.key);
      this.scheduleSend();
      this.notify();
    }
  }

  // ===== 状态来源 =====

  private onMediaEvent(item: Item, type: string): void {
    const el = item.el;
    const playing = !el.paused && !el.ended;
    const now = Date.now();
    if (type === "timeupdate") {
      if (now - item.lastTick < TIMEUPDATE_TICK_MS) return;
      item.lastTick = now;
      if (playing) {
        item.lastPlayedAt = now;
        if (now - item.lastReportedPlayedAt < LAST_PLAYED_REPORT_MS && item.playing === playing) return;
      }
    }
    if (playing) item.lastPlayedAt = now;
    item.playing = playing;
    if (type === "emptied") {
      item.bindings = null;
      item.encrypted = false;
    }
    if (type === "encrypted") item.encrypted = true;
    this.markDirty(item);
  }

  private readonly onPointerDown = (e: PointerEvent): void => {
    const hit = this.fromEvent(e);
    if (!hit) return;
    const item = this.items.get(hit.key);
    if (!item) return;
    item.lastInteractionAt = Date.now();
    this.markDirty(item);
  };

  /**
   * 绑定 EV_MEDIA 的 MSE 归因。target 为 null（无元素）时仅当本帧恰有一个播放中的元素才归属；
   * target 是 closed shadow 宿主（事件被重定向到宿主）时，在其 shadow 树里找唯一的媒体元素。
   */
  applyBinding(target: unknown, detail: MediaDetail): void {
    let item: Item | undefined;
    if (isMediaTag(target)) {
      this.adopt(target);
      item = this.itemOf(target);
    } else {
      const shadow = target instanceof Element ? shadowRootOf(target) : null;
      const inside = shadow ? [...shadow.querySelectorAll("video,audio")] : [];
      if (inside.length === 1 && isMediaTag(inside[0])) {
        this.adopt(inside[0]);
        item = this.itemOf(inside[0]);
      } else {
        const playing = [...this.items.values()].filter((i) => i.playing);
        // 无元素的会话只能归给唯一的播放元素，且不得覆盖它自己已关联的另一个会话
        const only = playing.length === 1 ? playing[0] : undefined;
        const own = only?.bindings;
        if (only && (!own || (own.msId === detail.msId && own.hid === detail.hid))) item = only;
      }
    }
    if (!item) return;
    item.bindings = detail;
    this.markDirty(item);
  }

  private markDirty(item: Item): void {
    this.dirty.add(item.key);
    this.scheduleSend();
    this.notify();
  }

  private notify(): void {
    for (const cb of this.listeners) {
      try {
        cb();
      } catch {
        /* 订阅方异常不影响跟踪 */
      }
    }
  }

  private scheduleSend(): void {
    if (this.sendTimer !== undefined) return;
    this.sendTimer = window.setTimeout(() => this.flush(), SEND_THROTTLE_MS);
  }

  /** blob URL 对应的原始响应地址（hooks 在 createObjectURL 时还原）。 */
  setBlobSource(blobUrl: string, url: string): void {
    this.blobSources.set(blobUrl, url);
    if (this.blobSources.size > 256) {
      const oldest = this.blobSources.keys().next().value;
      if (oldest !== undefined) this.blobSources.delete(oldest);
    }
    for (const item of this.items.values()) {
      if (item.el.currentSrc === blobUrl) this.markDirty(item);
    }
  }

  private snapshot(item: Item): ElementPayload {
    const el = item.el;
    const rect = el.getBoundingClientRect();
    // blob: 来源若是整段下载的响应（hooks 还原过），按原始地址上报
    const rawSrc = el.currentSrc || "";
    const src = this.blobSources.get(rawSrc) ?? rawSrc;
    const video = item.tag === "video" ? (el as HTMLVideoElement) : null;
    return {
      key: item.key,
      tag: item.tag,
      playing: item.playing,
      visibleRatio: Math.round(item.visibleRatio * 100) / 100,
      rect: { w: Math.round(rect.width), h: Math.round(rect.height) },
      videoWidth: video?.videoWidth ?? 0,
      videoHeight: video?.videoHeight ?? 0,
      currentSrc: /^https?:/i.test(src) ? src : "",
      lastPlayedAt: item.lastPlayedAt,
      lastInteractionAt: item.lastInteractionAt,
      bindings: item.bindings?.sourceBuffers ?? [],
      msId: item.bindings?.msId,
      hid: item.bindings?.hid,
      encrypted: item.encrypted || el.mediaKeys !== null,
      title: this.titleOf(item),
      tracks: [...el.querySelectorAll("track")].map((t) => ({
        kind: t.kind,
        srclang: t.srclang,
        label: t.label,
        src: t.src,
      })),
    };
  }

  private flush(): void {
    this.sendTimer = undefined;
    const full = this.fullPending;
    this.fullPending = false;
    const elements: ElementPayload[] = [];
    for (const key of full ? this.items.keys() : this.dirty) {
      const item = this.items.get(key);
      if (!item) continue;
      const payload = this.snapshot(item);
      const json = JSON.stringify(payload);
      if (json === item.lastSentJson && !full) continue;
      item.lastSentJson = json;
      item.lastReportedPlayedAt = item.lastPlayedAt;
      elements.push(payload);
    }
    this.dirty.clear();
    const removed = [...this.removedKeys];
    this.removedKeys.clear();
    if (!full && elements.length === 0 && removed.length === 0) return;
    this.opts.send({ elements, removed, full });
  }

  /**
   * 下一次发送改为全量快照：background 不认识本 sid（SW 重启丢了登记）、刚被重注入，
   * 或页面从 bfcache 恢复时调用。没有任何元素也要发（让 background 清掉本 sid 的残留）。
   */
  resendAll(): void {
    this.fullPending = true;
    this.scheduleSend();
  }

  /**
   * SPA 换页后 background 已清空元素状态：丢弃上一页的 MSE 归因证据（新页的追加会重新带来），
   * 清掉发送去重并让所有现存元素重新上报。
   */
  resetForNavigation(): void {
    for (const [key, item] of this.items) {
      item.bindings = null;
      item.lastSentJson = "";
      this.dirty.add(key);
    }
    if (this.dirty.size > 0) this.markDirty(this.items.values().next().value as Item);
  }

  // ===== 查询（浮钮 / 右键） =====

  list(): TrackedMedia[] {
    return [...this.items.values()];
  }

  get(key: string): TrackedMedia | undefined {
    return this.items.get(key);
  }

  keyOf(el: Element): string | undefined {
    return this.keys.get(el);
  }

  /** 指针所在位置的媒体元素（视频画面或音频控件条）：矩形命中（覆盖层是兄弟节点也命中），取面积最小者。 */
  elementAtPoint(x: number, y: number): TrackedMedia | null {
    let best: TrackedMedia | null = null;
    let bestArea = Infinity;
    for (const item of this.items.values()) {
      if (!item.el.isConnected) continue;
      const r = item.el.getBoundingClientRect();
      const min = minTargetSize(item.tag);
      if (r.width < min.w || r.height < min.h) continue;
      if (x < r.left || x > r.right || y < r.top || y > r.bottom) continue;
      const area = r.width * r.height;
      if (area < bestArea) {
        best = item;
        bestArea = area;
      }
    }
    return best;
  }

  /** 事件命中的媒体元素：先看事件路径，再退化为指针矩形命中。 */
  fromEvent(e: MouseEvent): TrackedMedia | null {
    const path = typeof e.composedPath === "function" ? e.composedPath() : [];
    for (const node of path) {
      if (isMediaTag(node)) {
        const key = this.keys.get(node);
        const item = key ? this.items.get(key) : undefined;
        if (item) return item;
      }
    }
    return this.elementAtPoint(e.clientX, e.clientY);
  }

  /** 本帧播放中的元素（rank ≥ 2）：可见比例高者优先，其次最近播放、面积。 */
  activePlaying(): TrackedMedia | null {
    let best: Item | null = null;
    let bestScore: [number, number, number] = [-1, -1, -1];
    for (const item of this.items.values()) {
      if (!item.playing || !item.el.isConnected) continue;
      const r = item.el.getBoundingClientRect();
      const score: [number, number, number] = [item.visibleRatio >= 0.5 ? 1 : 0, item.lastPlayedAt, r.width * r.height];
      if (
        score[0] > bestScore[0] ||
        (score[0] === bestScore[0] && (score[1] > bestScore[1] || (score[1] === bestScore[1] && score[2] > bestScore[2])))
      ) {
        best = item;
        bestScore = score;
      }
    }
    return best;
  }
}
