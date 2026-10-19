/**
 * Firefox 用的 MAIN world 钩子：unlisted script，由 sensor.content.ts 以 injectScript 注入页面环境
 * （Firefox MV2 的 content_scripts 不支持 world: MAIN）。
 */
import { defineUnlistedScript } from "wxt/utils/define-unlisted-script";
import { start } from "@/page/hooks";

export default defineUnlistedScript(() => {
  start();
});
