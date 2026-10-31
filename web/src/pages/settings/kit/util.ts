// 设置控件的纯工具与样式常量（与组件分文件，避免破坏 Fast Refresh）。

import type { TFunction } from '../../../i18n'

/** 行内控件宽度档位（桌面），移动端一律撑满。 */
export const WIDTH_INPUT = 'mobile:w-full desktop:w-[240px]'
export const WIDTH_INPUT_WIDE = 'mobile:w-full desktop:w-[420px]'
/** 数字框宽度档（GPUI `NUMBER_WIDTH`）：只放得下 5~6 位数字，比下拉窄；单位紧随其后。 */
export const WIDTH_NUMBER = 'mobile:w-full desktop:w-[104px]'
export const WIDTH_DROPDOWN = 'mobile:w-full desktop:min-w-[160px]'

/** 翻译键存在才返回文案（GPUI `item()`：描述键不存在则不显示描述）。 */
export function optionalText(t: TFunction, key: string | undefined): string | undefined {
  if (!key) return undefined
  const text = t(key)
  return text === key ? undefined : text
}

/** camelCase 枚举文案键：`delete_files` → `DeleteFiles`。 */
export function camel(value: string): string {
  return value
    .split('_')
    .map((part) => (part ? part[0]!.toUpperCase() + part.slice(1) : ''))
    .join('')
}

/** 数字回显：整数不带小数，浮点噪声最多 9 位（同 GPUI `format_number`）。 */
export function formatNumber(value: number): string {
  const text = value.toFixed(9).replace(/0+$/, '').replace(/\.$/, '')
  return text === '-0' ? '0' : text
}

