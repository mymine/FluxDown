// 「诊断此会话」请求：分享面板的接收方行跳到设置 › FluxMesh › 网络诊断并聚焦该会话（GPUI `open_diagnostics`）。
// 设置页挂载时取走并清空（与 `account/planRequest.ts` 同一模式）；`sessionId` 为 `null` 只切到网络诊断页签。

export interface DiagnosticsRequest {
  sessionId: string | null
}

let pending: DiagnosticsRequest | null = null

export function requestDiagnostics(sessionId: string | null): void {
  pending = { sessionId }
}

/** 取走并清空待处理请求。 */
export function takeDiagnosticsRequest(): DiagnosticsRequest | null {
  const request = pending
  pending = null
  return request
}
