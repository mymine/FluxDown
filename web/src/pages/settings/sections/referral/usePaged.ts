// 分页列表加载：页码 / 重载版本 / 过滤键变化即重新拉取；卸载或过期结果丢弃。
// 删除后当前页为空且仍有数据时自动回退到最后一页。

import { useEffect, useRef, useState } from 'react'
import { pageCount, REFERRAL_PAGE_SIZE } from '../account/billing'
import { accountErrorKey } from '../account/errorText'

export interface PagedState<T> {
  status: 'loading' | 'error' | 'ready'
  total: number
  items: readonly T[]
  errorKey: string | null
}

export function usePaged<T>(fetchPage: (page: number) => Promise<{ total: number; items: T[] }>, filterKey: string) {
  const [page, setPage] = useState(1)
  const [version, setVersion] = useState(0)
  const [state, setState] = useState<PagedState<T>>({ status: 'loading', total: 0, items: [], errorKey: null })
  const fetchRef = useRef(fetchPage)
  fetchRef.current = fetchPage

  useEffect(() => {
    let alive = true
    setState((previous) => ({ ...previous, status: 'loading', errorKey: null }))
    fetchRef
      .current(page)
      .then((result) => {
        if (!alive) return
        if (result.items.length === 0 && page > 1 && result.total > 0) {
          setPage(pageCount(result.total, REFERRAL_PAGE_SIZE))
          return
        }
        setState({ status: 'ready', total: result.total, items: result.items, errorKey: null })
      })
      .catch((error: unknown) => {
        if (alive) setState((previous) => ({ ...previous, status: 'error', errorKey: accountErrorKey(error) }))
      })
    return () => {
      alive = false
    }
  }, [page, version, filterKey])

  return { state, page, setPage, reload: () => setVersion((value) => value + 1) }
}
