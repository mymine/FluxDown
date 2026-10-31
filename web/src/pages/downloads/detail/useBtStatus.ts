import { useEffect, useState } from 'react'
import { rpc } from '../../../lib/rpc'
import type { BtStatusDto } from '../../../lib/rpc'
import { errorMessage } from '../../../lib/rpc/error'
import { BT_STATUS_POLL_MS } from './btStatus'

export interface BtStatusState {
  /** 最近一次成功的结果；首次响应前为 null。切换任务即清空。 */
  data: BtStatusDto | null
  /** 最近一次刷新失败的原因（数据保持上次成功值）；成功后清除。 */
  error: string | null
}

const EMPTY: BtStatusState = { data: null, error: null }

/**
 * BT 实况轮询：挂载（标签可见）即请求，之后在上一次请求结束 2 s 后再请求（天然不重叠）；
 * 卸载（标签隐藏 / 面板关闭）或 taskId 变化即停止并丢弃迟到的响应。失败保留旧数据并带 `error`。
 * 浏览器页面在后台时跳过本轮请求。
 */
export function useBtStatus(taskId: string): BtStatusState {
  const [state, setState] = useState<{ taskId: string; value: BtStatusState }>({ taskId, value: EMPTY })

  useEffect(() => {
    let alive = true
    let timer: number | undefined
    const schedule = () => {
      if (alive) timer = window.setTimeout(poll, BT_STATUS_POLL_MS)
    }
    const poll = () => {
      if (document.hidden) {
        schedule()
        return
      }
      rpc.daemon.task
        .btStatus({ taskId })
        .then(
          (data) => {
            if (alive) setState({ taskId, value: { data, error: null } })
          },
          (error: unknown) => {
            if (!alive) return
            const message = errorMessage(error)
            setState((prev) => ({ taskId, value: { data: prev.taskId === taskId ? prev.value.data : null, error: message } }))
          },
        )
        .finally(schedule)
    }
    poll()
    return () => {
      alive = false
      clearTimeout(timer)
    }
  }, [taskId])

  return state.taskId === taskId ? state.value : EMPTY
}
