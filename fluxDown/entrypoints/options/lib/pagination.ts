/** options 列表分页：页码钳制与页码条窗口（纯函数，便于测试）。 */

export const PAGE_SIZE_OPTIONS = [20, 50, 100] as const;
export type PageSize = (typeof PAGE_SIZE_OPTIONS)[number];
export const DEFAULT_PAGE_SIZE: PageSize = 20;

export interface PageSlice {
  /** 钳制后的当前页（从 1 开始；空列表为 1）。 */
  page: number;
  pageCount: number;
  /** 当前页在列表中的半开区间 [start, end)。 */
  start: number;
  end: number;
}

/** 列表变短（筛选 / 删除）后页码可能越界，这里统一钳到最后一页。 */
export function paginate(total: number, page: number, pageSize: number): PageSlice {
  const pageCount = Math.max(1, Math.ceil(total / pageSize));
  const current = Math.min(Math.max(1, Math.floor(page)), pageCount);
  const start = (current - 1) * pageSize;
  return { page: current, pageCount, start, end: Math.min(start + pageSize, total) };
}

export type PageItem = number | "gap";

/**
 * 页码条：总是含首页、末页与当前页前后各一页，其余折叠为 "gap"。
 * 只省略一个页码时直接显示该页（省略号不比数字短）。7 页以内全部列出。
 */
export function pageWindow(page: number, pageCount: number): PageItem[] {
  if (pageCount <= 7) return Array.from({ length: pageCount }, (_, i) => i + 1);
  const lo = Math.max(2, Math.min(page - 1, pageCount - 4));
  const hi = Math.min(pageCount - 1, Math.max(page + 1, 5));
  const items: PageItem[] = [1];
  if (lo === 3) items.push(2);
  else if (lo > 3) items.push("gap");
  for (let p = lo; p <= hi; p++) items.push(p);
  if (hi === pageCount - 2) items.push(pageCount - 1);
  else if (hi < pageCount - 2) items.push("gap");
  items.push(pageCount);
  return items;
}
