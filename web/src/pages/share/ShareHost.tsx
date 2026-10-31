// FluxMesh对话框宿主：由 AppShell 挂一次，任意路由都能弹出分享面板 / 接收确认框。
// 接收确认有两个来源：本页发起（`openShareReceive`，先 resolve）与 agent 推来的 `pendingReceive`
// （系统打开 / 剪贴板 / NMH 到达的链接，agent 已解析，只等用户确认；永不自动建任务）。

import { ReceiveByInput, ReceivePending } from './ReceiveDialog'
import { SharePanel } from './SharePanel'
import { usePendingReceive, useShareAvailable } from './state'
import { closeSharePanel, closeShareReceive, useShareDialogs } from './store'

export function ShareHost() {
  const { panel, receive } = useShareDialogs()
  const available = useShareAvailable()
  const pending = usePendingReceive()

  return (
    <>
      {available && panel ? <SharePanel key={panel.nonce} request={panel} onClose={closeSharePanel} /> : null}
      {available && receive ? (
        <ReceiveByInput key={receive.nonce} codeOrLink={receive.codeOrLink} onClose={closeShareReceive} />
      ) : available && pending ? (
        <ReceivePending preview={pending} />
      ) : null}
    </>
  )
}
