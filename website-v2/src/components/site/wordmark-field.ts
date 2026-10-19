/**
 * 页脚「FluxDown」巨型字标的粒子版:把字标按页面实际排版栅格化后采样成粒子。
 *
 * - 首次进入视口:粒子从下方散落处汇流成字。
 * - 轮廓粒子较亮(延续原描边字标的观感),字腔内部是淡淡的强调色填充;自下而上渐隐,同原遮罩。
 * - 指针:像一枚放大镜/聚光灯——光标附近的粒子被点亮成强调色 / cyan,并向外轻微鼓起几像素
 *   (按归位坐标计算,字形始终可读,不会被搅散);移开后余辉慢慢冷却,留下一道淡淡的光痕。
 * - 按下:从按下点扩散出一圈涟漪,波前经过处粒子短暂发亮并沿径向轻轻起伏,随距离与时间衰减。
 * - 空闲时随机零星闪烁。
 */
import { StrokeBatch, smoothstep, type Palette, type Pointer, type Scene } from "../../scripts/particle-stage";

interface Grain {
  hx: number;
  hy: number;
  x: number;
  y: number;
  vx: number;
  vy: number;
  heat: number;
  edge: boolean;
  cyan: boolean;
  /** 自下而上渐隐系数(按归位坐标,0..1) */
  fade: number;
}

interface Ripple {
  x: number;
  y: number;
  /** 已扩散时长(秒) */
  age: number;
}

const SPRING = 60;
/** 透镜半径与最大鼓起位移(px) */
const LENS_R = 140;
const LENS_PUSH = 7;
/** 涟漪波前速度(px/s)、波带半宽(px)、最大径向起伏(px)、寿命(s) */
const RIPPLE_SPEED = 560;
const RIPPLE_BAND = 46;
const RIPPLE_PUSH = 5;
const RIPPLE_LIFE = 1.8;
const MAX_RIPPLES = 4;

export class WordmarkField implements Scene {
  private grains: Grain[] = [];
  private ripples: Ripple[] = [];
  private width = 0;
  private height = 0;
  private introPending = true;

  private readonly edgeBatch = new StrokeBatch(1.8);
  private readonly fillBatch = new StrokeBatch(1.5);
  private readonly hotBatch = new StrokeBatch(2.6);

  constructor(
    private readonly text: HTMLElement,
    private readonly canvas: HTMLCanvasElement,
  ) {}

  resize(width: number, height: number) {
    this.width = width;
    this.height = height;
    this.sample();
  }

  /** 按字标 span 的实际字体与位置栅格化并采样(字体加载完成后需重采)。 */
  sample() {
    const { width, height } = this;
    const node = this.text.firstChild;
    if (!width || !height || !node) return;
    const style = getComputedStyle(this.text);
    const size = Number.parseFloat(style.fontSize) || 200;
    const range = document.createRange();
    range.selectNodeContents(node);
    const box = range.getBoundingClientRect();
    const origin = this.canvas.getBoundingClientRect();

    const off = document.createElement("canvas");
    off.width = Math.ceil(width);
    off.height = Math.ceil(height);
    const ctx = off.getContext("2d", { willReadFrequently: true });
    if (!ctx) return;
    ctx.font = `${style.fontWeight} ${size}px ${style.fontFamily}`;
    if ("letterSpacing" in ctx) ctx.letterSpacing = style.letterSpacing === "normal" ? "0px" : style.letterSpacing;
    ctx.textAlign = "center";
    ctx.textBaseline = "middle";
    ctx.fillStyle = "#000";
    ctx.fillText(this.text.textContent ?? "", box.left - origin.left + box.width / 2, box.top - origin.top + box.height / 2);
    const data = ctx.getImageData(0, 0, off.width, off.height).data;
    const inside = (x: number, y: number) => {
      const ix = Math.round(x);
      const iy = Math.round(y);
      if (ix < 0 || iy < 0 || ix >= off.width || iy >= off.height) return false;
      return (data[(iy * off.width + ix) * 4 + 3] ?? 0) > 110;
    };

    const step = Math.max(4, Math.round(size / 42));
    const probe = step * 0.75;
    const grains: Grain[] = [];
    for (let y = step / 2; y < height; y += step) {
      for (let x = step / 2; x < width; x += step) {
        if (!inside(x, y)) continue;
        const edge = !inside(x - probe, y) || !inside(x + probe, y) || !inside(x, y - probe) || !inside(x, y + probe);
        grains.push({
          hx: x,
          hy: y,
          x,
          y,
          vx: 0,
          vy: 0,
          heat: 0,
          edge,
          cyan: Math.random() < 0.15 + 0.6 * (x / width),
          fade: 1 - smoothstep(0.3 * height, height, y),
        });
      }
    }
    this.grains = grains;
  }

  press(x: number, y: number) {
    if (this.ripples.length >= MAX_RIPPLES) this.ripples.shift();
    this.ripples.push({ x, y, age: 0 });
  }

  frame(ctx: CanvasRenderingContext2D, dt: number, p: Pointer, pal: Palette) {
    if (!this.grains.length) return;
    if (dt > 0 && this.introPending) {
      // 汇流入场:从下方散落处飞回字形
      this.introPending = false;
      for (const g of this.grains) {
        g.x = g.hx + (Math.random() - 0.5) * this.width * 0.5;
        g.y = this.height + 20 + Math.random() * 160;
        g.vx = g.vy = 0;
      }
    }

    const damp = Math.exp(-7 * dt);
    const cool = Math.exp(-1.4 * dt);
    if (dt > 0) {
      // 空闲闪烁
      for (let i = 0; i < 3; i++) {
        const g = this.grains[Math.floor(Math.random() * this.grains.length)];
        if (g) g.heat = Math.max(g.heat, 0.55);
      }
      let alive = 0;
      for (const r of this.ripples) {
        r.age += dt;
        if (r.age < RIPPLE_LIFE) this.ripples[alive++] = r;
      }
      this.ripples.length = alive;

      for (const g of this.grains) {
        // 目标位置 = 归位点 + 透镜鼓起 + 涟漪起伏;位移只按归位坐标算,避免与当前位置互相反馈抖动
        let tx = g.hx;
        let ty = g.hy;
        if (p.presence > 0.01) {
          const dx = g.hx - p.x;
          const dy = g.hy - p.y;
          const d2 = dx * dx + dy * dy;
          if (d2 < LENS_R * LENS_R) {
            const d = Math.sqrt(d2) || 1;
            const t = d / LENS_R;
            const push = LENS_PUSH * Math.sin(Math.PI * t) * p.presence;
            tx += (dx / d) * push;
            ty += (dy / d) * push;
            g.heat = Math.max(g.heat, smoothstep(1, 0, t) * 0.9 * p.presence);
          }
        }
        for (const r of this.ripples) {
          const dx = g.hx - r.x;
          const dy = g.hy - r.y;
          const d = Math.hypot(dx, dy) || 1;
          const band = 1 - Math.abs(d - r.age * RIPPLE_SPEED) / RIPPLE_BAND;
          if (band <= 0) continue;
          const k = band * band * (1 - r.age / RIPPLE_LIFE) ** 2;
          tx += (dx / d) * RIPPLE_PUSH * k;
          ty += (dy / d) * RIPPLE_PUSH * k;
          g.heat = Math.max(g.heat, k);
        }
        g.vx = (g.vx + (tx - g.x) * SPRING * dt) * damp;
        g.vy = (g.vy + (ty - g.y) * SPRING * dt) * damp;
        g.x += g.vx * dt;
        g.y += g.vy * dt;
        g.heat *= cool;
      }
    }

    const tone = pal.dark ? 1 : 0.9;
    for (const g of this.grains) {
      const moving = Math.abs(g.vx) + Math.abs(g.vy) > 30;
      const x0 = moving ? g.x - g.vx * 0.016 : g.x;
      const y0 = moving ? g.y - g.vy * 0.016 : g.y;
      const x1 = moving ? g.x : g.x + 0.01;
      // 运动中的粒子(入场汇流)不受渐隐影响太多,保证入场可见
      const fade = Math.max(g.fade, moving ? 0.5 : 0);
      if (g.heat > 0.35) {
        this.hotBatch.line(pal, g.cyan ? "cyan" : "accent", (0.45 + 0.55 * g.heat) * fade * tone, x0, y0, x1, g.y);
      } else if (g.edge) {
        this.edgeBatch.line(pal, g.heat > 0.12 ? "accent" : "subtle", (0.6 + 0.4 * g.heat) * fade * tone, x0, y0, x1, g.y);
      } else {
        this.fillBatch.line(pal, g.cyan ? "cyan" : "accent", (0.14 + 0.6 * g.heat) * fade * tone, x0, y0, x1, g.y);
      }
    }
    this.fillBatch.flush(ctx);
    this.edgeBatch.flush(ctx);
    if (pal.dark) ctx.globalCompositeOperation = "lighter";
    this.hotBatch.flush(ctx);
    ctx.globalCompositeOperation = "source-over";
  }
}
