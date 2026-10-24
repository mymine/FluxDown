// 状态栏限速「最近自定义值」（镜像 GPUI crates/downloads/src/model/speed_limit_recent.rs）：
// 单位 KB/s，最新在前、去重、封顶；设备本地偏好，不进云同步。
// 两端共用用例 crates/downloads/tests/fixtures/speed_limit_recent_cases.json（speedLimitRecent.test.ts）。

/** 设备本地偏好：下载限速最近自定义值（KB/s 整数数组）。 */
export const SPEED_LIMIT_RECENT_PREF = 'download.speed_limit_recent_kbps'
/** 设备本地偏好：上传限速最近自定义值（KB/s 整数数组）。 */
export const UPLOAD_LIMIT_RECENT_PREF = 'download.upload_limit_recent_kbps'
/** 最多保留的条数。 */
export const SPEED_LIMIT_RECENT_LIMIT = 3

/** 内置预设（1 / 5 / 10 / 50 MB/s）折算成 KB/s；预设本身不记入最近列表。 */
const PRESET_KBPS = [1024, 5120, 10240, 51200]

function isRecordable(kbps: number): boolean {
  return kbps > 0 && !PRESET_KBPS.includes(kbps)
}

/** 解析偏好值：非数组为空；丢弃非整数、非正数、预设值与重复项（保留首次出现），封顶。 */
export function readSpeedLimitRecent(value: unknown): number[] {
  if (!Array.isArray(value)) return []
  const recent: number[] = []
  for (const item of value) {
    if (recent.length === SPEED_LIMIT_RECENT_LIMIT) break
    if (typeof item !== 'number' || !Number.isSafeInteger(item)) continue
    if (isRecordable(item) && !recent.includes(item)) recent.push(item)
  }
  return recent
}

/** 记录一次自定义值：移到最前（已存在则前移），超出上限丢弃最旧的；不可记录或已在最前返回 null。 */
export function pushRecent(recent: readonly number[], kbps: number): number[] | null {
  if (!Number.isSafeInteger(kbps) || !isRecordable(kbps) || recent[0] === kbps) return null
  return [kbps, ...recent.filter((known) => known !== kbps).slice(0, SPEED_LIMIT_RECENT_LIMIT - 1)]
}
