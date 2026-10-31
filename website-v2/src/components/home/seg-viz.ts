/**
 * 首页「引擎」分区的分段下载可视化:SegmentSim 驱动的分段条、拆分标记、连接工人列表与火花粒子层。
 * 由 Engine.astro 在首屏绘制后的空闲时段动态加载并挂载;挂载前显示 SSR 的静态初始态。
 */
import { mountStage } from "../../scripts/particle-stage";
import { SegSparks } from "./seg-sparks";
import { SegmentSim, formatBytes } from "./segment-sim";

const MB = 1024 * 1024;
const SIZE = 1.2 * 1024 * MB;
const STEP = 1 / 60;

export function mountSegViz(root: HTMLElement): void {
  const max = Number(root.dataset.max ?? 16);
  const layer = root.querySelector<HTMLElement>("[data-seg-layer]");
  const marks = root.querySelector<HTMLElement>("[data-seg-marks]");
  const conn = root.querySelector<HTMLElement>("[data-seg-conn]");
  const speed = root.querySelector<HTMLElement>("[data-seg-speed]");
  const bytes = root.querySelector<HTMLElement>("[data-seg-bytes]");
  const pct = root.querySelector<HTMLElement>("[data-seg-pct]");
  const workers = [...root.querySelectorAll<HTMLElement>(".seg-worker")];
  if (!layer || !marks) return;

  let sim = new SegmentSim({ size: SIZE, maxConn: max, perConn: 5.2 * MB, seed: 42, rampEvery: 0.8 });
  let pieces: { fill: HTMLElement; head: HTMLElement }[] = [];
  let seenSplits = 0;
  let doneAt = 0;
  let textClock = 0;

  const restart = () => {
    sim = new SegmentSim({ size: SIZE, maxConn: max, perConn: 5.2 * MB, seed: 42 + Math.floor(performance.now()) % 97, rampEvery: 0.8 });
    layer.replaceChildren();
    marks.replaceChildren();
    pieces = [];
    seenSplits = 0;
    doneAt = 0;
  };

  const render = (updateText: boolean) => {
    while (pieces.length < sim.segments.length) {
      const fill = document.createElement("span");
      fill.className = "seg-fill";
      const head = document.createElement("span");
      head.className = "seg-head";
      layer.append(fill, head);
      pieces.push({ fill, head });
    }
    sim.segments.forEach((s, i) => {
      const p = pieces[i];
      if (!p) return;
      const left = (s.start / SIZE) * 100;
      const width = ((s.pos - s.start) / SIZE) * 100;
      p.fill.style.left = `${left}%`;
      p.fill.style.width = `${width}%`;
      p.head.style.left = `${(s.pos / SIZE) * 100}%`;
      p.head.classList.toggle("is-active", s.pos < s.end);
      p.head.classList.toggle("is-slow", s.base < 2 * MB);
    });
    for (; seenSplits < sim.splits.length; seenSplits++) {
      const split = sim.splits[seenSplits];
      if (!split) continue;
      const mark = document.createElement("span");
      mark.className = "seg-mark";
      mark.style.left = `${(split.offset / SIZE) * 100}%`;
      marks.append(mark);
    }
    if (updateText) {
      const active = sim.active;
      if (conn) conn.textContent = String(active.length);
      if (speed) speed.textContent = `${formatBytes(sim.speed)}/s`;
      if (bytes) bytes.textContent = `${formatBytes(sim.downloaded)} / ${formatBytes(SIZE)}`;
      if (pct) pct.textContent = `${(sim.progress * 100).toFixed(1)}%`;
      workers.forEach((w, i) => {
        const seg = active.find((s) => s.worker === i);
        w.dataset.state = seg ? (seg.base < 2 * MB ? "slow" : "active") : "idle";
        w.style.setProperty("--level", seg ? String(Math.min(1, seg.speed / (7 * MB))) : "0");
      });
    }
  };

  let visible = false;
  let raf = 0;
  let last = 0;
  let acc = 0;
  const frame = (now: number) => {
    raf = 0;
    if (!visible || document.hidden) return;
    acc += Math.min(0.1, last ? (now - last) / 1000 : STEP);
    last = now;
    while (acc >= STEP) {
      acc -= STEP;
      if (!sim.done) sim.step(STEP);
      else if (!doneAt) doneAt = now;
    }
    textClock += 1;
    render(textClock % 6 === 0);
    if (doneAt && now - doneAt > 2600) restart();
    raf = requestAnimationFrame(frame);
  };
  const play = () => {
    if (!raf && visible && !document.hidden) {
      last = 0;
      raf = requestAnimationFrame(frame);
    }
  };
  root.querySelector("[data-seg-replay]")?.addEventListener("click", () => {
    restart();
    play();
  });
  const bar = root.querySelector<HTMLElement>("[data-seg-bar]");
  const sparks = root.querySelector<HTMLCanvasElement>("[data-seg-sparks]");
  if (bar && sparks) mountStage(sparks, root, new SegSparks(() => sim, bar, sparks));
  if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
    for (let i = 0; i < 420; i++) sim.step(STEP);
    render(true);
    return;
  }
  new IntersectionObserver(
    ([entry]) => {
      visible = Boolean(entry?.isIntersecting);
      play();
    },
    { threshold: 0.25 },
  ).observe(root);
  document.addEventListener("visibilitychange", play);
}
