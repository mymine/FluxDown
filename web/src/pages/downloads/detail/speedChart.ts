/** 速度曲线的纯计算：横轴下载用时标签、纵轴刻度（对齐 GPUI `speed_chart_points` / 纵轴三刻度）。 */

/** `m:ss`，满一小时为 `h:mm:ss`。 */
export function clockText(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds))
  const h = Math.floor(s / 3600)
  const m = Math.floor(s / 60) % 60
  const sec = String(s % 60).padStart(2, '0')
  return h > 0 ? `${h}:${String(m).padStart(2, '0')}:${sec}` : `${m}:${sec}`
}

/**
 * 每个点结束时的下载用时（秒）：第 i 点 = min((i+1)·bucketSecs, activeSecs)；
 * 末点可能是未满桶，按累计用时截断，且保证严格递增。
 */
export function pointElapsedSecs(bucketSecs: number, activeSecs: number, count: number): number[] {
  const bucket = Math.max(1, bucketSecs)
  const out: number[] = []
  let previous = 0
  for (let i = 0; i < count; i++) {
    const elapsed = Math.max(Math.min((i + 1) * bucket, activeSecs), previous + 1)
    out.push(elapsed)
    previous = elapsed
  }
  return out
}

/** 纵轴上限取 ≥ peak 的「整」值（1 / 2 / 2.5 / 5 × 10^k），刻度为 0、一半、上限。 */
export function niceCeil(peak: number): number {
  if (!(peak > 0)) return 1
  const magnitude = 10 ** Math.floor(Math.log10(peak))
  for (const step of [1, 2, 2.5, 5, 10]) {
    if (step * magnitude >= peak) return step * magnitude
  }
  return 10 * magnitude
}
