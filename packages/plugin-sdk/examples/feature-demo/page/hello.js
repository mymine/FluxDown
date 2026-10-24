// 页面脚本演示（manifest contributes.pageScripts，world: "isolated"）。
// 运行在浏览器扩展的 USER_SCRIPT world，全局 `fluxdown` 桥由扩展注入；类型见 fluxdown-plugin-sdk/types/page。
(async () => {
  try {
    // 调用 main.js 中以 { page: true } 注册的处理器。
    const reply = await fluxdown.invoke("pageHello", { href: location.href });
    // 把页面上的链接数写入插件隔离存储（每个插件独立，值须可 JSON 序列化）。
    await fluxdown.storage.set("lastLinkCount", document.links.length);
    console.info("[feature-demo]", fluxdown.info.scriptId, reply);
  } catch (error) {
    console.warn("[feature-demo] bridge call failed:", error);
  }
})();
