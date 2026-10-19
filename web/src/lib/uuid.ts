// 随机 UUID v4。不要直接用 `crypto.randomUUID()`：浏览器只在安全上下文（HTTPS / localhost）
// 暴露它，NAS / Docker 部署经 `http://<局域网 IP>:17800` 访问时缺失，调用即抛错。
// `crypto.getRandomValues()` 没有安全上下文限制。

/** RFC 4122 UUID v4（小写、带连字符），任何上下文可用。 */
export function randomUuid(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(16))
  bytes[6] = (bytes[6] & 0x0f) | 0x40 // version 4
  bytes[8] = (bytes[8] & 0x3f) | 0x80 // variant 10
  const hex = Array.from(bytes, (byte) => byte.toString(16).padStart(2, '0')).join('')
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`
}
