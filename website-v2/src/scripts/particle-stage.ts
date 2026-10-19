/**
 * 粒子舞台:canvas 生命周期的唯一实现,场景(Scene)只负责物理与绘制。
 *
 * - 尺寸:ResizeObserver 跟随 canvas 盒,DPR 上限 2。
 * - 调度:宿主离屏或页面隐藏即停帧;`prefers-reduced-motion` 下只绘制一帧静态画面。
 * - 颜色:取自设计 token(`--accent` 等 OKLCH),经 1px 探针画布转成 sRGB;
 *   主题切换(motion.ts 派发 `theme-change`)时重新解析。
 * - 指针:监听宿主元素(canvas 自身 `pointer-events: none`,不遮挡上层内容);
 *   在交互控件上按下不触发 `press`,避免点按钮时误触发场景的按下效果。
 */

export type Rgb = readonly [number, number, number];

export interface Palette {
  accent: Rgb;
  cyan: Rgb;
  fg: Rgb;
  subtle: Rgb;
  /** 页面底色:场景需要遮挡(实心平面)时使用,不参与 StrokeBatch 合批 */
  bg: Rgb;
  dark: boolean;
}

export interface Pointer {
  /** canvas 局部坐标(CSS px) */
  x: number;
  y: number;
  /** 指针速度(px/s),每帧衰减 */
  vx: number;
  vy: number;
  /** 0 → 1 的进出淡变,场景用它给所有指针效果做包络 */
  presence: number;
}

export interface Scene {
  resize(width: number, height: number): void;
  /** `dt` 为秒(已钳制);静态绘制时为 0,场景只需绘制当前状态。 */
  frame(ctx: CanvasRenderingContext2D, dt: number, pointer: Pointer, palette: Palette): void;
  press?(x: number, y: number): void;
}

export const rgba = (c: Rgb, a: number) => `rgba(${c[0]},${c[1]},${c[2]},${a.toFixed(3)})`;

export const clamp01 = (v: number) => (v < 0 ? 0 : v > 1 ? 1 : v);

export const smoothstep = (a: number, b: number, v: number) => {
  const t = clamp01((v - a) / (b - a));
  return t * t * (3 - 2 * t);
};

type ColorKey = "accent" | "cyan" | "fg" | "subtle";
const COLOR_KEYS: readonly ColorKey[] = ["accent", "cyan", "fg", "subtle"];

/**
 * 按「颜色 × 透明度档位」合批的线段绘制:每帧成百上千条线段只产生十几次 stroke,
 * 也避免逐粒子拼 rgba 字符串。样式表随调色板(主题)重建。
 * 圆头线帽下极短线段即圆点,`dot` 用它画节点。
 */
export class StrokeBatch {
  private readonly coords: number[][] = [];
  private styles: string[] = [];
  private palette: Palette | null = null;

  constructor(
    private readonly width: number,
    private readonly levels = 12,
  ) {
    for (let i = 0; i < COLOR_KEYS.length * (levels + 1); i++) this.coords.push([]);
  }

  line(palette: Palette, color: ColorKey, alpha: number, x1: number, y1: number, x2: number, y2: number) {
    const level = Math.round(clamp01(alpha) * this.levels);
    if (!level) return;
    if (palette !== this.palette) {
      this.palette = palette;
      this.styles = COLOR_KEYS.flatMap((key) =>
        Array.from({ length: this.levels + 1 }, (_, i) => rgba(palette[key], i / this.levels)),
      );
    }
    this.coords[COLOR_KEYS.indexOf(color) * (this.levels + 1) + level]?.push(x1, y1, x2, y2);
  }

  dot(palette: Palette, color: ColorKey, alpha: number, x: number, y: number) {
    this.line(palette, color, alpha, x, y, x + 0.01, y);
  }

  flush(ctx: CanvasRenderingContext2D) {
    ctx.lineWidth = this.width;
    ctx.lineCap = "round";
    this.coords.forEach((c, i) => {
      if (!c.length) return;
      ctx.strokeStyle = this.styles[i] ?? "transparent";
      ctx.beginPath();
      for (let k = 0; k < c.length; k += 4) {
        ctx.moveTo(c[k], c[k + 1]);
        ctx.lineTo(c[k + 2], c[k + 3]);
      }
      ctx.stroke();
      c.length = 0;
    });
  }
}

const probe = document.createElement("canvas");
probe.width = probe.height = 1;
const probeCtx = probe.getContext("2d", { willReadFrequently: true });

function resolveColor(token: string, fallback: Rgb): Rgb {
  const value = getComputedStyle(document.documentElement).getPropertyValue(token).trim();
  if (!probeCtx || !value) return fallback;
  probeCtx.clearRect(0, 0, 1, 1);
  // 不支持的颜色语法赋值会被静默忽略:以哨兵色判定解析失败
  probeCtx.fillStyle = "#010203";
  probeCtx.fillStyle = value;
  probeCtx.fillRect(0, 0, 1, 1);
  const [r = 1, g = 2, b = 3] = probeCtx.getImageData(0, 0, 1, 1).data;
  return r === 1 && g === 2 && b === 3 ? fallback : [r, g, b];
}

function resolvePalette(): Palette {
  const dark = document.documentElement.dataset.theme !== "light";
  return {
    accent: resolveColor("--accent", dark ? [72, 139, 247] : [37, 111, 238]),
    cyan: resolveColor("--cyan", [48, 184, 212]),
    fg: resolveColor("--fg", dark ? [242, 243, 246] : [24, 26, 32]),
    subtle: resolveColor("--fg-subtle", dark ? [118, 122, 133] : [136, 139, 148]),
    bg: resolveColor("--bg", dark ? [12, 13, 16] : [250, 250, 251]),
    dark,
  };
}

const INTERACTIVE = "a, button, input, textarea, select, label, [role='button'], [role='tab']";

export function mountStage(canvas: HTMLCanvasElement, host: HTMLElement, scene: Scene) {
  const ctx = canvas.getContext("2d");
  if (!ctx) return;
  const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  let palette = resolvePalette();
  let width = 0;
  let height = 0;
  let visible = false;
  let raf = 0;
  let last = 0;

  const pointer: Pointer = { x: 0, y: 0, vx: 0, vy: 0, presence: 0 };
  let inside = false;
  let clientX = 0;
  let clientY = 0;
  let lastMoveAt = 0;

  const draw = (dt: number) => {
    if (!width || !height) return;
    ctx.clearRect(0, 0, width, height);
    scene.frame(ctx, dt, pointer, palette);
  };

  const tick = (now: number) => {
    raf = 0;
    if (!visible || document.hidden) return;
    const dt = last ? Math.min(0.05, (now - last) / 1000) : 1 / 60;
    last = now;
    // 按帧用最新 clientX/Y 重算局部坐标:页面滚动而指针不动时效果仍贴合指针
    const rect = canvas.getBoundingClientRect();
    const x = clientX - rect.left;
    const y = clientY - rect.top;
    if (inside) {
      const k = dt > 0 ? 1 / dt : 0;
      const idle = now - lastMoveAt > 80;
      pointer.vx = idle ? pointer.vx * 0.85 : (x - pointer.x) * k * 0.35 + pointer.vx * 0.65;
      pointer.vy = idle ? pointer.vy * 0.85 : (y - pointer.y) * k * 0.35 + pointer.vy * 0.65;
      pointer.x = x;
      pointer.y = y;
    } else {
      pointer.vx *= 0.85;
      pointer.vy *= 0.85;
    }
    pointer.presence += ((inside ? 1 : 0) - pointer.presence) * Math.min(1, dt * 6);
    draw(dt);
    raf = requestAnimationFrame(tick);
  };

  const play = () => {
    if (reduced || raf || !visible || document.hidden) return;
    last = 0;
    raf = requestAnimationFrame(tick);
  };

  new ResizeObserver(([entry]) => {
    if (!entry) return;
    width = entry.contentRect.width;
    height = entry.contentRect.height;
    const dpr = Math.min(2, window.devicePixelRatio || 1);
    canvas.width = Math.max(1, Math.round(width * dpr));
    canvas.height = Math.max(1, Math.round(height * dpr));
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    scene.resize(width, height);
    if (!raf) draw(0);
  }).observe(canvas);

  new IntersectionObserver(
    ([entry]) => {
      visible = Boolean(entry?.isIntersecting);
      play();
    },
    { rootMargin: "80px" },
  ).observe(host);
  document.addEventListener("visibilitychange", play);

  window.addEventListener("theme-change", () => {
    palette = resolvePalette();
    if (!raf) draw(0);
  });

  if (reduced) return;

  host.addEventListener(
    "pointermove",
    (event) => {
      if (event.pointerType === "touch") return;
      if (!inside) {
        // 首次进入:直接落位,避免从上一个位置「飞」过来产生巨大速度
        const rect = canvas.getBoundingClientRect();
        pointer.x = event.clientX - rect.left;
        pointer.y = event.clientY - rect.top;
        pointer.vx = pointer.vy = 0;
      }
      inside = true;
      clientX = event.clientX;
      clientY = event.clientY;
      lastMoveAt = performance.now();
    },
    { passive: true },
  );
  host.addEventListener("pointerleave", () => {
    inside = false;
  });
  host.addEventListener("pointerdown", (event) => {
    if (!scene.press || event.button !== 0) return;
    if ((event.target as Element | null)?.closest(INTERACTIVE)) return;
    const rect = canvas.getBoundingClientRect();
    scene.press(event.clientX - rect.left, event.clientY - rect.top);
  });
}
