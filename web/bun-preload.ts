// bun test 预加载：注册与 Vite 插件同源的 `virtual:i18n-tables` 虚拟模块（Bun 不支持 Vite 插件）。
import { fileURLToPath } from 'node:url'
import { I18N_TABLES_ID, i18nTablesSource } from './i18n-tables.ts'

declare const Bun: {
  plugin(plugin: {
    name: string
    setup(build: { module(id: string, load: () => { contents: string; loader: 'js' }): void }): void
  }): void
}

const i18nDir = fileURLToPath(new URL('../assets/i18n', import.meta.url))

Bun.plugin({
  name: 'fluxdown-i18n-tables',
  setup(build) {
    build.module(I18N_TABLES_ID, () => ({ contents: i18nTablesSource(i18nDir), loader: 'js' }))
  },
})
