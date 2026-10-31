// 语言表虚拟模块 `virtual:i18n-tables` 的源码生成：assets/i18n 下每个 *.json 即一种语言，
// 代码 = 文件名小写（与 GPUI `crates/i18n/build.rs` 一致）。Vite（vite.config.ts）与 bun test（bun-preload.ts）共用。
import { readdirSync } from 'node:fs'
import { join } from 'node:path'

export const I18N_TABLES_ID = 'virtual:i18n-tables'

/** `specifier` 决定生成的 import 路径：Vite 走 `@i18n-assets` 别名，Bun 用绝对路径。 */
export function i18nTablesSource(dir: string, specifier: (file: string) => string = (f) => join(dir, f)): string {
  const files = readdirSync(dir)
    .filter((file) => file.endsWith('.json'))
    .sort()
  const imports = files.map((file, i) => `import t${i} from ${JSON.stringify(specifier(file))}`)
  const entries = files.map((file, i) => `  ${JSON.stringify(file.slice(0, -'.json'.length).toLowerCase())}: t${i},`)
  return `${imports.join('\n')}\nexport default {\n${entries.join('\n')}\n}\n`
}
