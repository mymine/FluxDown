/** 非数组的普通对象（用于收窄 JSON.parse / storage 读出的 unknown）。 */
export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
