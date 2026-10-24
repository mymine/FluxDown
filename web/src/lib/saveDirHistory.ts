// 新建下载「保存目录」历史（镜像 GPUI crates/downloads/src/model/save_dir_history.rs）：
// 最近使用的本机目录，最新在前、去重、封顶；设备本地偏好，不进云同步。
// 两端共用用例 crates/downloads/tests/fixtures/save_dir_history_cases.json（saveDirHistory.test.ts）。

/** 设备本地偏好：历史目录（字符串数组）。 */
export const SAVE_DIR_HISTORY_PREF = 'download.save_dir_history'
/** 设备本地偏好：是否记录并列出历史（缺省开启）。 */
export const SAVE_DIR_HISTORY_ENABLED_PREF = 'download.save_dir_history_enabled'
/** 最多保留的历史条数。 */
export const SAVE_DIR_HISTORY_LIMIT = 10

/** 解析偏好值：非数组为空；逐项 trim，丢弃空串 / 非字符串，保留首次出现，封顶。 */
export function readSaveDirHistory(value: unknown): string[] {
  if (!Array.isArray(value)) return []
  const history: string[] = []
  for (const item of value) {
    if (history.length === SAVE_DIR_HISTORY_LIMIT) break
    if (typeof item !== 'string') continue
    const dir = item.trim()
    if (dir !== '' && !history.includes(dir)) history.push(dir)
  }
  return history
}

/** 记录一次使用：目录移到最前（已存在则前移），超出上限丢弃最旧的；空目录或已在最前返回 null。 */
export function recordSaveDir(history: readonly string[], dir: string): string[] | null {
  const value = dir.trim()
  if (value === '' || history[0] === value) return null
  return [value, ...history.filter((known) => known !== value).slice(0, SAVE_DIR_HISTORY_LIMIT - 1)]
}

/** 下拉候选：历史中除当前输入以外的目录（保持最新在前）。 */
export function saveDirChoices(history: readonly string[], current: string): string[] {
  const value = current.trim()
  return history.filter((dir) => dir !== value)
}
