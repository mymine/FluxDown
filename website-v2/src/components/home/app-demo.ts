/**
 * GPUI 客户端复刻界面的脚本化演示。
 *
 * 时间线(秒,循环 LOOP):光标移到「新建下载」→ 打开子窗口 → 输入链接 → 解析出文件名与大小
 * → 开始下载 → 新任务从 1 条连接动态拆分到 16 条;列表中已有任务按各自的分段模拟推进,
 * 完成时弹出系统风格通知。
 *
 * 模拟以固定步长推进(与帧率无关);默认仅在可见且标签页激活时自动播放。
 * `external: true` 时不自动播放,由调用方(/film/ 短片舞台)用自己的时钟调用 `advance()`。
 * 容器缩放不在这里:调用方须先同步调用 app-fit.ts 的 `fitApp()`(首屏布局不等演示挂载)。
 */
import { DEMO_NEW, DEMO_TASKS, DEMO_URL, type DemoTask } from "./demo-data";
import { DESIGN_WIDTH } from "./app-fit";
import { SegmentSim, formatBytes, formatEta } from "./segment-sim";

const LOOP = 17;
const STEP = 1 / 60;

const T = {
  cursorIn: 1.0,
  moveToNew: [1.1, 2.3] as const,
  clickNew: 2.4,
  typing: [2.9, 4.5] as const,
  resolve: 4.8,
  moveToStart: [4.95, 5.85] as const,
  clickStart: 5.95,
  insert: 6.15,
  moveAway: [6.4, 7.4] as const,
  cursorOut: 7.9,
  fadeOut: 16.5,
};

interface RowRef {
  task: DemoTask;
  sim: SegmentSim | null;
  status: DemoTask["status"];
  row: HTMLElement;
  track: HTMLElement | null;
  segs: HTMLElement[];
  speed: HTMLElement;
  eta: HTMLElement;
  statusEl: HTMLElement;
  newUntil: number;
}

interface Labels {
  status: Record<DemoTask["status"], string>;
  done: string;
}

const ease = (x: number) => (x < 0.5 ? 4 * x * x * x : 1 - Math.pow(-2 * x + 2, 3) / 2);
const clamp01 = (x: number) => Math.min(1, Math.max(0, x));

export interface DemoController {
  reset(): void;
  /** 以固定步长推进指定秒数(外部时钟驱动,如 /film/ 录制舞台) */
  advance(seconds: number): void;
  readonly time: number;
}

export const DEMO_LOOP = LOOP;

export function mountAppDemo(root: HTMLElement, options: { external?: boolean } = {}): DemoController | null {
  const rowsEl = root.querySelector<HTMLElement>("[data-rows]");
  const cursor = root.querySelector<SVGElement>("[data-cursor]");
  const click = root.querySelector<HTMLElement>("[data-click]");
  const dialog = root.querySelector<HTMLElement>("[data-dialog]");
  const typed = root.querySelector<HTMLElement>("[data-typed]");
  const resolvedName = root.querySelector<HTMLElement>("[data-resolved-name]");
  const resolvedSize = root.querySelector<HTMLElement>("[data-resolved-size]");
  const newButton = root.querySelector<HTMLElement>("[data-new-button]");
  const startButton = root.querySelector<HTMLElement>("[data-start-button]");
  const toast = root.querySelector<HTMLElement>("[data-toast]");
  const toastBody = root.querySelector<HTMLElement>("[data-toast-body]");
  const totalSpeed = root.querySelector<HTMLElement>("[data-total-speed]");
  const spark = root.querySelector<SVGPathElement>("[data-spark]");
  const queueCount = root.querySelector<HTMLElement>("[data-queue-count]");
  if (!rowsEl || !cursor || !click || !dialog || !typed || !newButton || !startButton) return null;
  const labels = JSON.parse(root.dataset.labels ?? "{}") as Labels;

  const icon = (name: string) =>
    (root.querySelector<HTMLTemplateElement>(`template[data-icon="${name}"]`)?.content.cloneNode(true) ??
      document.createTextNode("")) as Node;

  /* ── State ── */
  let time = 0;
  let rows: RowRef[] = [];
  let sparkSamples: number[] = [];
  let sampleClock = 0;
  let textClock = 0;
  let toastUntil = 0;
  let toastAt = -1;
  let toastEnd = -1;
  let inserted = false;

  const makeSim = (task: DemoTask, seed: number) =>
    task.status === "completed"
      ? null
      : new SegmentSim({
          size: task.size,
          maxConn: task.maxConn,
          perConn: task.perConn,
          startAt: task.progress,
          seed,
        });

  const buildRow = (task: DemoTask, sim: SegmentSim | null): RowRef => {
    const row = document.createElement("div");
    row.className = "app-row";
    row.dataset.status = task.status;
    const cells = Array.from({ length: 7 }, () => document.createElement("span"));
    const [iconCell, name, size, progress, speed, eta, statusEl] = cells as [
      HTMLElement, HTMLElement, HTMLElement, HTMLElement, HTMLElement, HTMLElement, HTMLElement,
    ];
    iconCell.className = "app-ficon";
    iconCell.append(icon(task.icon));
    name.className = "truncate text-[var(--app-fg)]";
    name.textContent = task.name;
    size.className = "text-right";
    size.textContent = formatBytes(task.size);
    progress.className = "app-progress";
    let track: HTMLElement | null = null;
    if (task.status !== "completed") {
      track = document.createElement("span");
      track.className = "app-track";
      progress.append(track);
    }
    speed.className = "text-right";
    eta.className = "text-right";
    statusEl.className = "app-status";
    statusEl.textContent = labels.status[task.status];
    speed.textContent = "—";
    eta.textContent = "—";
    row.append(...cells);
    return { task, sim, status: task.status, row, track, segs: [], speed, eta, statusEl, newUntil: 0 };
  };

  const reset = () => {
    time = 0;
    inserted = false;
    toastUntil = 0;
    toastAt = -1;
    toastEnd = -1;
    sparkSamples = [];
    sampleClock = 0;
    textClock = 0;
    rows = DEMO_TASKS.map((task, i) => buildRow(task, makeSim(task, 11 + i * 7)));
    rowsEl.replaceChildren(...rows.map((r) => r.row));
    rowsEl.style.opacity = "1";
    dialog.classList.remove("is-open");
    toast?.classList.remove("is-visible");
    typed.textContent = "";
    if (resolvedName) resolvedName.textContent = "";
    if (resolvedSize) resolvedSize.textContent = "";
    updateCounts();
  };

  const updateCounts = () => {
    const tasks = rows.map((r) => ({ ...r.task, status: r.status }));
    const counts: Record<string, number> = {
      all: tasks.length,
      downloading: tasks.filter((x) => x.status === "downloading").length,
      completed: tasks.filter((x) => x.status === "completed").length,
      paused: tasks.filter((x) => x.status === "paused").length,
      failed: 0,
      image: 0,
    };
    for (const cat of ["video", "audio", "document", "program", "archive"]) {
      counts[cat] = tasks.filter((x) => x.category === cat).length;
    }
    root.querySelectorAll<HTMLElement>("[data-count-key]").forEach((el) => {
      const n = counts[el.dataset.countKey ?? ""] ?? 0;
      const target = el.querySelector<HTMLElement>("[data-n]");
      if (target) target.textContent = n > 0 ? String(n) : "";
    });
    if (queueCount) queueCount.textContent = String(tasks.length);
  };

  /** 目标元素中心在设计坐标系中的位置(按实际渲染宽度换算,兼容外层镜头缩放/透视) */
  const centerOf = (el: HTMLElement) => {
    const r = el.getBoundingClientRect();
    const base = root.getBoundingClientRect();
    const s = base.width / DESIGN_WIDTH || 1;
    return { x: (r.left - base.left + r.width / 2) / s, y: (r.top - base.top + r.height / 2) / s };
  };

  const cursorPath = () => {
    const rest = { x: 760, y: 520 };
    const newPos = centerOf(newButton);
    const startPos = centerOf(startButton);
    const away = { x: 640, y: 600 };
    const seg = (a: { x: number; y: number }, b: { x: number; y: number }, [t0, t1]: readonly [number, number]) => {
      const k = ease(clamp01((time - t0) / (t1 - t0)));
      return { x: a.x + (b.x - a.x) * k, y: a.y + (b.y - a.y) * k };
    };
    if (time < T.moveToNew[1]) return seg(rest, newPos, T.moveToNew);
    if (time < T.moveToStart[0]) return newPos;
    if (time < T.moveToStart[1] + 0.2) return seg(newPos, startPos, T.moveToStart);
    if (time < T.moveAway[0]) return startPos;
    return seg(startPos, away, T.moveAway);
  };

  let pressed: { el: HTMLElement; until: number } | null = null;
  const fireClick = (target: HTMLElement) => {
    const p = centerOf(target);
    click.style.translate = `${p.x}px ${p.y}px`;
    click.classList.remove("is-firing");
    void click.offsetWidth;
    click.classList.add("is-firing");
    target.classList.add("is-pressed");
    pressed = { el: target, until: time + 0.14 };
  };

  const crossed = (prev: number, at: number) => prev < at && time >= at;

  const renderRow = (ref: RowRef, updateText: boolean) => {
    const { sim, track } = ref;
    if (sim && track) {
      const needed = sim.segments.length;
      while (ref.segs.length < needed) {
        const seg = document.createElement("span");
        seg.className = "app-seg";
        track.append(seg);
        ref.segs.push(seg);
      }
      sim.segments.forEach((s, i) => {
        const el = ref.segs[i];
        if (!el) return;
        el.style.left = `${(s.start / sim.size) * 100}%`;
        el.style.width = `${((s.pos - s.start) / sim.size) * 100}%`;
      });
    }
    if (updateText && sim && ref.status === "downloading") {
      const speed = sim.speed;
      ref.speed.textContent = `${formatBytes(speed)}/s`;
      ref.eta.textContent = formatEta((sim.size - sim.downloaded) / Math.max(1, speed));
    }
    if (ref.newUntil && time > ref.newUntil) {
      ref.row.classList.remove("is-new");
      ref.newUntil = 0;
    }
  };

  const complete = (ref: RowRef) => {
    ref.status = "completed";
    ref.row.dataset.status = "completed";
    ref.statusEl.textContent = labels.status.completed;
    ref.speed.textContent = "—";
    ref.eta.textContent = "—";
    ref.track?.remove();
    ref.track = null;
    ref.sim = null;
    if (toast && toastBody) {
      toastBody.textContent = ref.task.name;
      toast.classList.add("is-visible");
      toastUntil = time + 3.2;
      toastAt = time;
      toastEnd = toastUntil;
    }
    updateCounts();
  };

  /**
   * 外部时钟模式下(逐帧录制),CSS 过渡被舞台禁用,改由时间线直接计算这些过渡的中间态,
   * 保证任意帧都可精确复现。
   */
  const outCubic = (x: number) => 1 - Math.pow(1 - clamp01(x), 3);
  const applyTimedVisuals = () => {
    const dk =
      time < T.clickNew ? 0 : time < T.clickStart ? outCubic((time - T.clickNew) / 0.32) : 1 - clamp01((time - T.clickStart) / 0.22);
    dialog.style.opacity = String(dk);
    dialog.style.transform = `translateY(${(1 - dk) * 12}px) scale(${0.97 + 0.03 * dk})`;
    if (toast) {
      const tk = toastAt < 0 ? 0 : time < toastEnd ? outCubic((time - toastAt) / 0.36) : 1 - clamp01((time - toastEnd) / 0.26);
      toast.style.opacity = String(tk);
      toast.style.transform = `translateY(${(1 - tk) * 10}px) scale(${0.98 + 0.02 * tk})`;
    }
    const lastClick = time >= T.clickStart ? T.clickStart : time >= T.clickNew ? T.clickNew : -1;
    const cp = lastClick < 0 ? 1 : (time - lastClick) / 0.52;
    click.style.opacity = cp >= 1 ? "0" : String(0.9 * (1 - cp));
    click.style.scale = String(0.3 + 1.1 * outCubic(cp));
    cursor.style.opacity = String(clamp01((time - T.cursorIn) / 0.3) * (1 - clamp01((time - T.cursorOut) / 0.3)));
    rowsEl.style.opacity = String(1 - clamp01((time - T.fadeOut) / 0.4));
  };

  const advance = (dt: number) => {
    const prev = time;
    time += dt;
    if (pressed && time > pressed.until) {
      pressed.el.classList.remove("is-pressed");
      pressed = null;
    }

    // 光标
    const visible = time >= T.cursorIn && time < T.cursorOut;
    const pos = cursorPath();
    cursor.style.opacity = visible ? "1" : "0";
    cursor.style.transition = "opacity 300ms";
    cursor.style.transform = `translate(${pos.x - 2}px, ${pos.y - 2}px)`;

    if (crossed(prev, T.clickNew)) {
      fireClick(newButton);
      dialog.classList.add("is-open");
    }
    if (time >= T.typing[0] && time <= T.typing[1] + STEP) {
      const k = clamp01((time - T.typing[0]) / (T.typing[1] - T.typing[0]));
      typed.textContent = DEMO_URL.slice(0, Math.round(DEMO_URL.length * k));
    }
    if (crossed(prev, T.resolve)) {
      if (resolvedName) resolvedName.textContent = DEMO_NEW.name;
      if (resolvedSize) resolvedSize.textContent = formatBytes(DEMO_NEW.size);
    }
    if (crossed(prev, T.clickStart)) {
      fireClick(startButton);
      dialog.classList.remove("is-open");
    }
    if (!inserted && time >= T.insert) {
      inserted = true;
      const ref = buildRow(DEMO_NEW, makeSim(DEMO_NEW, 3));
      ref.row.classList.add("is-new");
      ref.newUntil = time + 1.4;
      rows.unshift(ref);
      rowsEl.prepend(ref.row);
      updateCounts();
    }

    // 下载推进
    for (const ref of rows) {
      if (ref.sim && ref.status === "downloading") {
        ref.sim.step(dt);
        if (ref.sim.done) complete(ref);
      }
    }
    if (toastUntil && time > toastUntil) {
      toast?.classList.remove("is-visible");
      toastUntil = 0;
    }

    textClock += dt;
    const updateText = textClock >= 0.125;
    if (updateText) textClock = 0;
    for (const ref of rows) renderRow(ref, updateText);

    sampleClock += dt;
    if (sampleClock >= 0.2) {
      sampleClock = 0;
      const total = rows.reduce((sum, r) => sum + (r.sim && r.status === "downloading" ? r.sim.speed : 0), 0);
      if (totalSpeed) totalSpeed.textContent = `${formatBytes(total)}/s`;
      sparkSamples.push(total);
      if (sparkSamples.length > 40) sparkSamples.shift();
      if (spark) {
        const max = Math.max(...sparkSamples, 1);
        spark.setAttribute(
          "d",
          sparkSamples
            .map((v, i) => `${i === 0 ? "M" : "L"}${(i / 39) * 120} ${15 - (v / max) * 14}`)
            .join(" "),
        );
      }
    }

    if (options.external) applyTimedVisuals();

    if (crossed(prev, T.fadeOut)) {
      rowsEl.style.transition = "opacity 400ms";
      rowsEl.style.opacity = "0";
    }
    if (time >= LOOP) {
      reset();
    }
  };

  reset();

  const controller: DemoController = {
    reset,
    advance: (seconds: number) => {
      for (let i = 0; i < Math.round(seconds / STEP); i++) advance(STEP);
    },
    get time() {
      return time;
    },
  };
  if (options.external) return controller;

  if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
    // 静态:直接展示演示中段(新任务已分段、有完成通知)
    controller.advance(9);
    return controller;
  }

  let visible = false;
  let last = 0;
  let acc = 0;
  let raf = 0;
  const frame = (now: number) => {
    raf = 0;
    if (!visible || document.hidden) return;
    const dt = Math.min(0.1, last ? (now - last) / 1000 : STEP);
    last = now;
    acc += dt;
    while (acc >= STEP) {
      advance(STEP);
      acc -= STEP;
    }
    raf = requestAnimationFrame(frame);
  };
  const play = () => {
    if (!raf && visible && !document.hidden) {
      last = 0;
      raf = requestAnimationFrame(frame);
    }
  };
  new IntersectionObserver(
    ([entry]) => {
      visible = Boolean(entry?.isIntersecting);
      play();
    },
    { threshold: 0.15 },
  ).observe(root);
  document.addEventListener("visibilitychange", play);
  return controller;
}
