import { useCallback, useSyncExternalStore } from 'react'

/** 与 index.css 的 `mobile` / `narrow` 自定义变体同一断点。 */
export const MOBILE_QUERY = '(max-width: 820px)'
export const NARROW_QUERY = '(max-width: 480px)'
export const COARSE_QUERY = '(pointer: coarse)'

export function useMediaQuery(query: string): boolean {
  const subscribe = useCallback(
    (onChange: () => void) => {
      const list = window.matchMedia(query)
      list.addEventListener('change', onChange)
      return () => list.removeEventListener('change', onChange)
    },
    [query],
  )
  const get = useCallback(() => window.matchMedia(query).matches, [query])
  return useSyncExternalStore(subscribe, get, () => false)
}

/** 宽度 <= 820px：活动栏变底部标签栏、对话框全屏、菜单变底部面板。 */
export function useIsMobile(): boolean {
  return useMediaQuery(MOBILE_QUERY)
}

/** 主要指针为触屏（无 hover）。 */
export function useIsCoarsePointer(): boolean {
  return useMediaQuery(COARSE_QUERY)
}
