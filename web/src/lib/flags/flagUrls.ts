// 国旗资源表（仅 Vite 构建 / dev 可用）：`assets/flags/*.svg` 文件主名（小写码）→ 产物 URL。
// `?no-inline` 禁止把小 SVG 以 data URI 塞进 JS 包，保持每个国旗一个独立缓存文件。

const modules = import.meta.glob<string>('@flags/*.svg', { query: '?no-inline', import: 'default', eager: true })

export const FLAG_URLS: Readonly<Record<string, string>> = Object.fromEntries(
  Object.entries(modules).map(([path, url]) => [path.slice(path.lastIndexOf('/') + 1, -'.svg'.length), url]),
)
