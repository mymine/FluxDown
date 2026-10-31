/**
 * 复刻界面按 1120px 设计宽度等比缩放到容器宽度。
 *
 * 与演示动画(app-demo.ts)分开:缩放决定首屏布局,须同步执行;演示动画可延后挂载。
 */
export const DESIGN_WIDTH = 1120;

export function fitApp(root: HTMLElement): void {
  const fit = root.closest<HTMLElement>("[data-app-fit]");
  if (!fit) return;
  const resize = () => root.style.setProperty("--app-scale", String(fit.clientWidth / DESIGN_WIDTH));
  resize();
  new ResizeObserver(resize).observe(fit);
}
