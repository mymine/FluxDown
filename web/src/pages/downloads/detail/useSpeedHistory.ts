import { useEffect, useRef, useState } from 'react'
import { rpc } from '../../../lib/rpc'
import type { TaskSpeedHistoryDto } from '../../../lib/rpc'

const REFRESH_MS = 2000

/**
 * 任务全生命周期速度曲线（daemon 持久化）。「速度」标签可见时拉取；下载中每 2 s 刷新，
 * 离开下载态再补拉一次拿到终态曲线。请求不重叠、丢弃非当前任务的响应、失败保留上次数据
 * （旧 daemon 无此方法同样落到这里，表现为空态）。随 TaskDetail 以 taskId 为 key 挂载。
 */
export function useSpeedHistory(taskId: string, downloading: boolean, visible: boolean): TaskSpeedHistoryDto | null {
  const [data, setData] = useState<TaskSpeedHistoryDto | null>(null)
  const taskRef = useRef(taskId)
  taskRef.current = taskId

  useEffect(() => {
    if (!visible) return
    let alive = true
    let inflight = false
    const fetchOnce = () => {
      if (inflight) return
      inflight = true
      const id = taskId
      rpc.daemon.task
        .speedHistory({ taskId: id })
        .then(
          (dto) => {
            if (alive && taskRef.current === id && (dto === null || dto.taskId === id)) setData(dto)
          },
          () => undefined,
        )
        .finally(() => {
          inflight = false
        })
    }
    fetchOnce()
    if (!downloading) return () => {
      alive = false
    }
    const timer = setInterval(fetchOnce, REFRESH_MS)
    return () => {
      alive = false
      clearInterval(timer)
    }
  }, [taskId, downloading, visible])

  return data
}
