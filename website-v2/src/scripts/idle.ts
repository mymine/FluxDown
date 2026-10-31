/**
 * 装饰动效(canvas 粒子、演示动画)的统一延后入口。
 *
 * 首屏 UI 由 SSR 直出,不依赖任何脚本即可完整显示;动效脚本经 `whenIdle(() => import(...))`
 * 推迟到首帧绘制之后、主线程空闲时才下载并挂载,不与首屏渲染争抢网络与主线程。
 * 后台标签页里 rAF 不触发,动效会在标签页首次可见时再挂载(它们本就只在可见时播放)。
 */
const ready = new Promise<void>((resolve) => {
  const idle = () => {
    if (typeof window.requestIdleCallback === "function") window.requestIdleCallback(() => resolve(), { timeout: 1500 });
    else window.setTimeout(resolve, 200);
  };
  // 双 rAF:第二帧回调时首帧已提交绘制
  requestAnimationFrame(() => requestAnimationFrame(idle));
});

/** 首帧绘制后的空闲时段执行 `task`;失败只记日志,不影响页面其余部分。 */
export function whenIdle(task: () => Promise<void>): void {
  ready.then(task).catch((error: unknown) => console.error("[effects]", error));
}
