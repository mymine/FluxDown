import { fileURLToPath, URL } from 'node:url'
import { defineConfig, type Plugin } from 'vite'
import react, { reactCompilerPreset } from '@vitejs/plugin-react'
import babel from '@rolldown/plugin-babel'
import tailwindcss from '@tailwindcss/vite'
import { I18N_TABLES_ID, i18nTablesSource } from './i18n-tables.ts'

const repoRoot = fileURLToPath(new URL('..', import.meta.url))
const i18nDir = fileURLToPath(new URL('../assets/i18n', import.meta.url))
const AGENT = 'http://127.0.0.1:17800'

/** `virtual:i18n-tables`：构建期列出 assets/i18n/*.json，新增语言无需改代码（dev 下新增文件需重启）。 */
function i18nTables(): Plugin {
  const resolved = `\0${I18N_TABLES_ID}`
  return {
    name: 'fluxdown-i18n-tables',
    resolveId: (id) => (id === I18N_TABLES_ID ? resolved : undefined),
    load: (id) => (id === resolved ? i18nTablesSource(i18nDir, (file) => `@i18n-assets/${file}`) : undefined),
  }
}

// https://vite.dev/config/
export default defineConfig({
  plugins: [i18nTables(), react(), babel({ presets: [reactCompilerPreset()] }), tailwindcss()],
  resolve: {
    alias: {
      // 与 GPUI 共享的资源与主题实现：直接从仓库根引用，不复制。
      '@i18n-assets': fileURLToPath(new URL('../assets/i18n', import.meta.url)),
      '@gpui-theme': fileURLToPath(new URL('../website-v2/src/lib/gpui-theme', import.meta.url)),
      // 内置文件图标包与扩展名 → 大类表（GPUI `crates/icon_pack` 编译期嵌入同一批文件）。
      '@icon-packs': fileURLToPath(new URL('../assets/icon-packs', import.meta.url)),
      // 国旗 SVG（flag-icons，MIT）：GPUI 编译期嵌入同一目录。
      '@flags': fileURLToPath(new URL('../assets/flags', import.meta.url)),
    },
  },
  server: {
    fs: { allow: [repoRoot] },
    // dev 同源代理到 fluxdown-agent（server 模式，默认 17800）。
    proxy: {
      '/rpc': { target: AGENT, changeOrigin: false, ws: true },
      '/api': { target: AGENT, changeOrigin: false },
      '/ping': { target: AGENT, changeOrigin: false },
      '/demo': { target: AGENT, changeOrigin: false },
    },
  },
})
