/**
 * 内容脚本 Shadow DOM 的样式表。
 *
 * `cssInjectionMode: "manual"` 的内容脚本在构建期把依赖图里每个 `import "x.css"` 内联成字符串，
 * 模块求值时按导入顺序登记到这里（wxt.config.ts 的 `inlineContentCss` 插件）；UI 以
 * `createShadowRootUi(ctx, { css: contentCss() })` 注入。
 *
 * 不用 WXT 的 `"ui"` 模式：它在页面里 `fetch(runtime.getURL(css))`，请求走页面的 fetch 上下文，
 * 受站点策略约束（YouTube 的 Connection-Allowlist 报「Unsafe attempt to load URL …」并可能拦下样式），
 * 还要把 CSS 暴露为 web_accessible_resources。
 */
const sheets: string[] = [];
let joined: string | null = null;

/** 构建期生成的模块调用；业务代码不要直接调用。 */
export function registerContentCss(css: string): void {
  sheets.push(css);
  joined = null;
}

/** 本内容脚本的全部样式；`:root` 换成 `:host`，主题 token 落在 Shadow 宿主上（与 WXT `"ui"` 模式一致）。 */
export function contentCss(): string {
  joined ??= sheets.join("\n").replaceAll(":root", ":host");
  return joined;
}
