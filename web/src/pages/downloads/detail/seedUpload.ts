// 做种上传限速：输入框以 KB/s 为单位，线上是字节/秒；未改动的输入保持原 bps（不因取整丢精度）。

/** 任务现值（字节/秒）→ 输入框文本（KB/s）；0 / 缺省 = 任务不限速，显示 "0"。 */
export function uploadLimitText(bps: number | undefined): string {
  return bps === undefined || bps <= 0 ? '0' : String(Math.round(bps / 1024))
}

/** 文本是否是合法的非负整数 KB/s。 */
export function isUploadValid(text: string): boolean {
  return /^\d+$/.test(text.trim())
}

/** 保存时要发送的上传限速：文本未改动 → 原值；否则按 KB/s 解析，0 = 任务不限速；非法按 0。 */
export function uploadLimitToSend(text: string, initialText: string, originalBps: number | undefined): number {
  if (text === initialText) return Math.max(0, originalBps ?? 0)
  const kbps = Number.parseInt(text.trim(), 10)
  return Number.isFinite(kbps) && kbps > 0 ? kbps * 1024 : 0
}
