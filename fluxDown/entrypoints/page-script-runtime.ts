import { defineUnlistedScript } from "wxt/utils/define-unlisted-script";
import { installRuntime } from "@/background/page-scripts/runtime-core";

/**
 * 页面脚本桥运行时：由 background 以 `userScripts.register` 的 `{ file: "page-script-runtime.js" }` 注入
 * USER_SCRIPT world（先于插件源码执行）。逻辑见 background/page-scripts/runtime-core.ts。
 */
export default defineUnlistedScript(() => {
  installRuntime(globalThis as unknown as Record<string, unknown>);
});
