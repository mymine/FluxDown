// 诊断数据拉取：只在使用它的视图挂载（且页面可见）时按间隔轮询 `agent.share.diagnostics`；
// 视图卸载即停，空闲时零开销（快照只带摘要，会话路径 / 采样仅在这里按需取）。

import { useEffect, useState } from 'react'
import { rpc } from '../../lib/rpc'
import type { ShareDiagnosticsDto } from '../../lib/rpc'

export interface DiagnosticsState {
  data: ShareDiagnosticsDto | null
  error: unknown
}

export function useShareDiagnostics(options: { sessionId?: string; enabled?: boolean; intervalMs?: number }): DiagnosticsState {
  const { sessionId, enabled = true, intervalMs = 3000 } = options
  const [state, setState] = useState<DiagnosticsState>({ data: null, error: null })

  useEffect(() => {
    if (!enabled) return
    let cancelled = false
    let timer: ReturnType<typeof setTimeout> | undefined
    const run = async () => {
      if (!document.hidden) {
        try {
          const data = await rpc.agent.share.diagnostics(sessionId === undefined ? undefined : { sessionId })
          if (!cancelled) setState({ data, error: null })
        } catch (error) {
          if (!cancelled) setState((previous) => ({ data: previous.data, error }))
        }
      }
      if (!cancelled) timer = setTimeout(() => void run(), intervalMs)
    }
    void run()
    return () => {
      cancelled = true
      clearTimeout(timer)
    }
  }, [sessionId, enabled, intervalMs])

  return state
}
