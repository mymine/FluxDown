// 分类目录路由（移植 native/protocol/src/agent.rs `CategoryRouteDto`）：
// from_categories / follows / base_dir。分类列表解析复用下载页的 `categoriesFromPreference`。

import { categoriesFromPreference } from '../pages/downloads/model/categories'
import { compileEngineRegex } from '../pages/rss/filter'
import type { CategoryRouteDto, CustomCategoryDto } from './rpc'

/** 由分类列表（已按 position 排序）构建路由；没有任何分类配置目录时为 null。 */
export function routeFromCategories(categories: readonly CustomCategoryDto[]): CategoryRouteDto | null {
  const visible = categories.filter((category) => category.visible)
  const otherDir = visible.find((category) => category.builtinType === 'other')?.saveDir.trim() ?? ''
  const rules = visible
    .filter((category) => category.builtinType !== 'all' && category.builtinType !== 'other' && (otherDir !== '' || category.saveDir.trim() !== ''))
    .map((category) => {
      const rule: NonNullable<CategoryRouteDto['rules']>[number] = { matchMode: category.matchMode }
      if (category.extensions.length > 0) rule.extensions = category.extensions
      if (category.regexPattern !== '') rule.regexPattern = category.regexPattern
      const dir = category.saveDir.trim()
      if (dir !== '') rule.saveDir = dir
      return rule
    })
  const routes = otherDir !== '' || rules.some((rule) => (rule.saveDir ?? '') !== '')
  if (!routes) return null
  return otherDir !== '' ? { rules, otherDir } : { rules }
}

/** 由 `custom_categories` 偏好值构建路由。 */
export function routeFromPreference(value: unknown): CategoryRouteDto | null {
  return routeFromCategories(categoriesFromPreference(value))
}

/** 提交目录为空、等于提供目录，或是某个分类目录时，认为用户没有手选目录。 */
export function routeFollows(route: CategoryRouteDto, submitted: string, offered: string): boolean {
  const dir = submitted.trim()
  if (dir === '') return true
  const base = offered.trim()
  if (base !== '' && dir === base) return true
  if (route.otherDir && dir === route.otherDir) return true
  return route.rules.some((rule) => !!rule.saveDir && rule.saveDir === dir)
}

/** following 时作为 `saveDir` 下发的基准目录（无分类命中时使用）。 */
export function routeBaseDir(route: CategoryRouteDto, submitted: string, offered: string): string {
  const dir = submitted.trim()
  if (dir === '') return offered.trim()
  const isCategoryDir = (!!route.otherDir && dir === route.otherDir) || route.rules.some((rule) => !!rule.saveDir && rule.saveDir === dir)
  return isCategoryDir ? offered.trim() : dir
}

/** 把 `%XX` 按字节解码（非法序列原样保留，非法 UTF-8 用替换符），对齐 agent `percent_decode`。 */
function percentDecode(input: string): string {
  const bytes: number[] = []
  const encoder = new TextEncoder()
  for (let index = 0; index < input.length; ) {
    const hex = input[index] === '%' ? input.slice(index + 1, index + 3) : ''
    if (/^[0-9a-fA-F]{2}$/.test(hex)) {
      bytes.push(Number.parseInt(hex, 16))
      index += 3
    } else {
      const code = input.codePointAt(index) ?? 0
      bytes.push(...encoder.encode(String.fromCodePoint(code)))
      index += code > 0xffff ? 2 : 1
    }
  }
  return new TextDecoder().decode(new Uint8Array(bytes))
}

/** URL 路径末段（百分号解码）且含 `.` 时作为文件名。 */
function fileNameFromUrl(url: string): string | null {
  let parsed: URL
  try {
    parsed = new URL(url.trim())
  } catch {
    return null
  }
  const last = parsed.pathname.split('/').pop() ?? ''
  const decoded = percentDecode(last)
  return decoded.includes('.') ? decoded : null
}

function ruleMatches(rule: NonNullable<CategoryRouteDto['rules']>[number], name: string): boolean {
  if (rule.matchMode === 'regex') {
    const pattern = rule.regexPattern ?? ''
    if (pattern === '') return false
    const regex = compileEngineRegex(pattern)
    return regex !== null && new RegExp(regex.source, regex.flags.replace(/[gy]/g, '')).test(name)
  }
  const dot = name.lastIndexOf('.')
  if (dot < 0) return false
  const extension = name.slice(dot + 1).toLowerCase()
  if (extension === '') return false
  return (rule.extensions ?? []).some((candidate) => candidate.replace(/^\.+/, '').toLowerCase() === extension)
}

/**
 * 命中的分类保存目录；无命中 / 命中分类未配置目录时为 null。
 * 语义同 agent `category_dir.rs::route_save_dir`（夹具 `native/engine/src/fixtures/category_route_cases.json`）。
 */
export function routeSaveDir(route: CategoryRouteDto, fileName: string, url: string): string | null {
  const name = fileName.includes('.') ? fileName : (fileNameFromUrl(url) ?? fileName)
  if (name === '') return null
  const hit = route.rules.find((rule) => !!rule.saveDir && ruleMatches(rule, name))
  if (hit?.saveDir) return hit.saveDir
  if (route.otherDir && !route.rules.some((rule) => ruleMatches(rule, name))) return route.otherDir
  return null
}

/** 分类下拉的模式：自动 / 不分类 / 指定分类 id。 */
export type CategoryPickMode = { kind: 'auto' } | { kind: 'none' } | { kind: 'category'; id: string }

/** 可选分类：visible、非 all、目录非空，按 position 排序（`categories` 已排序）。 */
export function pickableCategories(categories: readonly CustomCategoryDto[]): CustomCategoryDto[] {
  return categories.filter((category) => category.visible && category.builtinType !== 'all' && category.saveDir.trim() !== '')
}

/** 预测目录对应的分类：可选列表中第一个目录等于预测目录的项。 */
export function matchedCategory(options: readonly CustomCategoryDto[], predicted: string | null): CustomCategoryDto | null {
  if (predicted === null) return null
  return options.find((category) => category.saveDir.trim() === predicted) ?? null
}

/**
 * Auto 模式下预测输入变化时目录框应改写成的值；不应覆盖（用户手改过）返回 null。
 * 目录框为空、等于 offered、或等于上一次自动写入值才跟随。
 */
export function autoFollowDir(current: string, offered: string, lastAuto: string | null, predicted: string | null): string | null {
  const dir = current.trim()
  if (dir !== '' && dir !== offered.trim() && dir !== lastAuto) return null
  return predicted ?? offered
}

/** 模式对应的目录框值（Auto 取预测或 offered）。 */
export function modeDir(mode: CategoryPickMode, categories: readonly CustomCategoryDto[], offered: string, predicted: string | null): string {
  if (mode.kind === 'auto') return predicted ?? offered
  if (mode.kind === 'none') return offered
  const category = categories.find((item) => item.id === mode.id)
  return category ? category.saveDir.trim() : offered
}
