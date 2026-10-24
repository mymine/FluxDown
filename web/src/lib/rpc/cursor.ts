// 事件序号连续性（对应 Rust `fluxdown_protocol::judge_frame`，agent `daemon_client::forward_event` 同规则）。
//
// 帧覆盖序号区间 `(previous, sequence]`，`previous` 缺省 = `sequence - 1`：
//   - epoch 变化 → 必须重新 snapshot；
//   - sequence <= 游标 → 快照或已应用的帧已包含它，丢弃；
//   - 覆盖起点（previous ?? sequence - 1）<= 游标 → 接受并前移到 sequence；
//   - 其他（断档）→ 必须重新 snapshot。
//
// 覆盖区间可以越过游标（合并帧与快照重叠的部分）：合并只发生在「后帧完整取代前帧」的事件之间，
// 重放一遍结果不变，所以不能再要求「必须等于游标 + 1」。

export interface EventCursor {
  epoch: string
  sequence: number
}

/** 判定所需的帧头（`EventFrame` 的子集）。 */
export interface FrameCoverage extends EventCursor {
  previous?: number
}

export type CursorVerdict = 'accept' | 'skip' | 'resync'

export function judgeFrame(cursor: EventCursor, frame: FrameCoverage): CursorVerdict {
  if (frame.epoch !== cursor.epoch) return 'resync'
  if (frame.sequence <= cursor.sequence) return 'skip'
  const coversFrom = frame.previous ?? frame.sequence - 1
  return coversFrom <= cursor.sequence ? 'accept' : 'resync'
}
