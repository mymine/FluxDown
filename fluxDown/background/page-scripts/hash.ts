/** 源码 UTF-8 字节的 SHA-256 小写十六进制（契约 §1：与桌面端索引里的 sha256 同口径）。 */
export async function sha256Hex(text: string): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
  return Array.from(new Uint8Array(digest), (b) => b.toString(16).padStart(2, "0")).join("");
}
