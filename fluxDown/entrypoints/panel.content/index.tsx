/**
 * 页内悬浮球 + 资源面板（仅顶层帧，document_idle）。
 * 视频浮钮在 sensor.content（每帧一个实例），本入口不处理。
 */
import { render } from "preact";
import { defineContentScript } from "wxt/utils/define-content-script";
import { createShadowRootUi } from "wxt/utils/content-script-ui/shadow-root";
import { initI18n } from "@/utils/i18n";
import { PanelApp } from "./PanelApp";
import "@/ui/theme/tokens.css";
import "@/ui/theme/base.css";
import "./panel.css";

export default defineContentScript({
  matches: ["<all_urls>"],
  runAt: "document_idle",
  cssInjectionMode: "ui",

  async main(ctx) {
    // 只在顶层帧加载：iframe 内的媒体由各帧 sensor 的浮钮处理
    if (window.self !== window.top) return;
    await initI18n();

    const ui = await createShadowRootUi(ctx, {
      name: "fluxdown-panel",
      position: "overlay",
      anchor: "body",
      onMount(container, _shadow, shadowHost) {
        render(<PanelApp host={shadowHost} />, container);
        return container;
      },
      onRemove(container) {
        if (container) render(null, container);
      },
    });
    ui.mount();
  },
});
