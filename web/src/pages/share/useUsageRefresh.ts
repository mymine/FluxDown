// 额度刷新：使用方可见（挂载）且已登录时向 agent 取一次最新额度（agent 侧 30 s 缓存），结果经 `shareChanged`
// 推到快照；`refresh()` 手动再取一次。分享页摘要行与设置 › 账户的额度区共用。

import { useEffect, useState } from 'react'
import { rpc } from '../../lib/rpc'
import { accountErrorKey } from '../settings/sections/account/errorText'
import { useShareAvailable, useShareSignedIn } from './state'

export function useUsageRefresh(): { refreshing: boolean; refresh: () => void } {
  const signedInUser = useShareSignedIn()
  const available = useShareAvailable()
  const signedIn = signedInUser && available
  const [refreshing, setRefreshing] = useState(false)
  const [nonce, setNonce] = useState(0)

  useEffect(() => {
    if (!signedIn) return
    let cancelled = false
    setRefreshing(true)
    rpc.agent.share
      .usageRefresh()
      .catch((error: unknown) => console.warn('[share] usageRefresh failed', accountErrorKey(error), error))
      .finally(() => {
        if (!cancelled) setRefreshing(false)
      })
    return () => {
      cancelled = true
    }
  }, [signedIn, nonce])

  return { refreshing, refresh: () => setNonce((value) => value + 1) }
}
