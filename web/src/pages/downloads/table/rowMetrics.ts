// 桌面任务表的行度量与单元格外壳：任务行、组行、目录行共用（列宽 / 内边距 / 行高 / 树轨缩进）。

import { MIN_WIDTH } from '../model/viewPrefs'
import type { ResolvedColumn, ViewDensity } from '../model/viewPrefs'

/** 选中底色左右内缩（`left-1` / `right-1`）。 */
const SELECTED_INSET_X = 4
/** 选择列图标与选中底色左右边缘之间的最小留白。 */
const FILE_ICON_GAP_X = 4
/** 宽松密度的文件图标底块边长（`KindGlyph` 的 `size-8`）。 */
const RELAXED_TILE_SIZE = 32

/**
 * 固定左侧选择列宽（含左侧留白），与 GPUI `selection_column_width_for` 同一规则：
 * 图标居中后两侧仍留出 选中内缩 + 留白，且不小于 36。
 */
export function selectionColumnWidth(density: ViewDensity): number {
  const min = 36
  return density === 'relaxed' ? Math.max(min, RELAXED_TILE_SIZE + 2 * (SELECTED_INSET_X + FILE_ICON_GAP_X)) : min
}

export const TRAILING_GUTTER = 8
export const HEADER_HEIGHT = 28
export const CELL_PADDING_X = 8
export const PROGRESS_LABEL_WIDTH = 32
export const PROGRESS_GAP = 8
/** 分桶标题槽位：28 高内容 + 上方 8px 留白。 */
export const BUCKET_SLOT_HEIGHT = HEADER_HEIGHT + 8

/** 树形缩进：chevron 宽（16）+ 间距（4）。成员行每级缩进一格，树轨画在每级 chevron 中线。 */
export const TREE_CHEVRON = 16
export const TREE_INDENT = TREE_CHEVRON + 4

/** 目录行的文件夹图标宽（icon.md）与 spacing.sm。 */
export const DIR_ICON_WIDTH = 16
export const SPACING_SM = 8
/** 目录之下成员的缩进槽宽 = 展开箭头槽 + 目录图标 + 间距；槽内竖线在「箭头槽 + 目录图标 / 2」处（对准目录行文件夹图标中线）。 */
export const DIR_INDENT_SLOT = TREE_CHEVRON + DIR_ICON_WIDTH + SPACING_SM
export const DIR_RAIL_OFFSET = TREE_CHEVRON + DIR_ICON_WIDTH / 2

export interface LayoutColumn extends ResolvedColumn {
  /** 文件名列吸收剩余宽度（未固定）。 */
  fluid: boolean
}

export function cellStyle(column: LayoutColumn) {
  return column.fluid
    ? { flex: '1 1 0%', minWidth: MIN_WIDTH.file_name }
    : { flex: '0 0 auto', width: column.width }
}

/** 行高类（任务行 / 组行 / 目录行等高）。 */
export function rowHeightClass(density: ViewDensity): string {
  return density === 'relaxed' ? 'h-task-row-relaxed' : density === 'comfortable' ? 'h-task-row' : 'h-task-row-compact'
}
