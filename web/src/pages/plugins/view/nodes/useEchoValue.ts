// 输入回显规则（与 GPUI 一致）：本地草稿即时显示，300 ms 去抖后发 change；
// 帧里的值等于本地待回显值、或输入框正聚焦 / 仍有未发出的输入时不覆盖草稿，失焦后再同步。

import { useCallback, useEffect, useRef, useState } from 'react'

export const INPUT_DEBOUNCE_MS = 300

export interface EchoValue {
  draft: string
  onChange: (value: string) => void
  onFocus: () => void
  /** 失焦：立即冲刷未发出的 change，并同步帧里的值。 */
  onBlur: () => void
  /** 立即发出当前草稿（用于 submit）。 */
  flush: () => void
}

export function useEchoValue(frameValue: string, send: (value: string) => void): EchoValue {
  const [draft, setDraft] = useState(frameValue)
  const draftRef = useRef(frameValue)
  const frameRef = useRef(frameValue)
  const focused = useRef(false)
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const pending = useRef<string | null>(null)
  const sendRef = useRef(send)
  sendRef.current = send

  const adopt = useCallback((value: string) => {
    draftRef.current = value
    setDraft(value)
  }, [])

  const flush = useCallback(() => {
    if (timer.current === null) return
    clearTimeout(timer.current)
    timer.current = null
    pending.current = draftRef.current
    sendRef.current(draftRef.current)
  }, [])

  useEffect(() => {
    frameRef.current = frameValue
    if (timer.current !== null) return
    if (pending.current !== null && frameValue === pending.current) {
      pending.current = null
      return
    }
    if (!focused.current) {
      pending.current = null
      adopt(frameValue)
    }
  }, [frameValue, adopt])

  useEffect(
    () => () => {
      if (timer.current !== null) clearTimeout(timer.current)
    },
    [],
  )

  const onChange = useCallback(
    (value: string) => {
      adopt(value)
      if (timer.current !== null) clearTimeout(timer.current)
      timer.current = setTimeout(() => {
        timer.current = null
        pending.current = draftRef.current
        sendRef.current(draftRef.current)
      }, INPUT_DEBOUNCE_MS)
    },
    [adopt],
  )

  const onFocus = useCallback(() => {
    focused.current = true
  }, [])

  const onBlur = useCallback(() => {
    focused.current = false
    flush()
    if (pending.current === null && draftRef.current !== frameRef.current) adopt(frameRef.current)
  }, [flush, adopt])

  return { draft, onChange, onFocus, onBlur, flush }
}
