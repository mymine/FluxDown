// `RowEngine` 的 React 绑定：每次输入变化把差分交给引擎、保持期内顺序过期时在保持期结束后强制重排一次。

import { useEffect, useMemo, useRef, useState } from 'react'
import { RowEngine } from './rowEngine'
import type { RowEngineInput, RowSnapshot } from './rowEngine'
import { holdExpiresAt, isDynamicSortKey, isMenuOpen, pointerActivity } from './rowOrder'

/** 菜单打开时的复查间隔。 */
const MENU_RECHECK_MS = 300

export interface RowEngineState {
  /** 稳定的引擎实例（按键查视图 / 命中标记 / 任务组汇总 / 全部视图）。 */
  engine: RowEngine
  snapshot: RowSnapshot
}

export function useRowEngine(input: RowEngineInput): RowEngineState {
  const [engine] = useState(() => new RowEngine())
  const [tick, setTick] = useState(0)
  const forced = useRef(0)
  const { tasks, speeds, uploads, boostedId, queuePositions, runtimes, connected, remote, selection, query, groupMatches, categories, sortKey, sortDir } = input
  const dynamicKey = isDynamicSortKey(sortKey)
  const dynamicRef = useRef(dynamicKey)
  dynamicRef.current = dynamicKey

  const snapshot = useMemo(() => {
    const force = tick !== forced.current
    forced.current = tick
    return engine.update(
      { tasks, speeds, uploads, boostedId, queuePositions, runtimes, connected, remote, selection, query, groupMatches, categories, sortKey, sortDir },
      { now: Date.now(), lastPointerAt: pointerActivity.lastAt, menuOpen: isMenuOpen(), force },
    )
  }, [engine, tick, tasks, speeds, uploads, boostedId, queuePositions, runtimes, connected, remote, selection, query, groupMatches, categories, sortKey, sortDir])

  useEffect(() => {
    if (!snapshot.stale) return
    let timer: ReturnType<typeof setTimeout> | undefined
    const arm = () => {
      if (isMenuOpen()) {
        timer = setTimeout(arm, MENU_RECHECK_MS)
        return
      }
      const wait =
        holdExpiresAt({
          lastPointerAt: pointerActivity.lastAt,
          dynamicKey: dynamicRef.current,
          lastReorderAt: snapshot.lastReorderAt,
        }) - Date.now()
      if (wait <= 0) setTick((value) => value + 1)
      else timer = setTimeout(arm, wait)
    }
    arm()
    return () => clearTimeout(timer)
  }, [snapshot])

  return { engine, snapshot }
}
