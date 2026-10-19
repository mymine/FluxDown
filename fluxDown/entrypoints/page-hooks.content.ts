/**
 * MAIN world 钩子入口（Chrome / Edge）：由浏览器在 document_start 直接注入页面环境，
 * 保证赶在页面脚本之前装好 fetch / XHR / MSE 钩子。Firefox 走 page-hooks-main.ts（unlisted，由 sensor 注入）。
 */
import { defineContentScript } from "wxt/utils/define-content-script";
import { start } from "@/page/hooks";

export default defineContentScript({
  matches: ["<all_urls>"],
  world: "MAIN",
  runAt: "document_start",
  allFrames: true,
  matchAboutBlank: true,
  exclude: ["firefox"],
  main() {
    start();
  },
});
