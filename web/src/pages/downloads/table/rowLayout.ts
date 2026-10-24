// 虚拟列表的行访问器：随 `rows` 数组身份换新。
//
// 行引擎的 `rows` 只在顺序 / 成员 / 分组布局变化时换数组（进度帧里行对象原位更新、数组不动），所以
// 访问器只在布局变化时更换，虚拟化器不会在每个进度帧重算全部行的位置（O(行数)）。访问器直接按下标读
// `rows`，不拷贝键序列。

import { useMemo } from 'react'

interface LayoutRow {
  key: string
  type: string
}

export interface RowAccessors {
  getItemKey: (index: number) => string | number
  estimateSize: (index: number) => number
}

export function useRowAccessors(rows: readonly LayoutRow[], bucketHeight: number, taskHeight: number): RowAccessors {
  return useMemo<RowAccessors>(
    () => ({
      getItemKey: (index) => rows[index]?.key ?? index,
      estimateSize: (index) => (rows[index]?.type === 'bucket' ? bucketHeight : taskHeight),
    }),
    [rows, bucketHeight, taskHeight],
  )
}
