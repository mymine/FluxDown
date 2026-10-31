// 发送入口的可用性：入口全部由 `snapshot.share` 门控（未编入 / 服务不可用 → 隐藏；未登录且云端不允许游客发起 → 置灰）。
// 返回基础类型，既可用于 hooks（`useRpcSelector` 需引用稳定），也可在菜单构建时读一次最新状态。

import { CAPABILITY_AGENT_SHARE, rpcStore, useRpcSelector } from '../../lib/rpc'
import type { RpcState } from '../../lib/rpc'
import { shareDisabledKey } from './model'

type SendView = Parameters<typeof shareDisabledKey>[0][number]

/** `hidden` = 不显示入口；`ok` = 可点；其余为置灰原因的文案键。 */
export type SendState = 'hidden' | 'ok' | (string & {})

export function sendState(state: RpcState, views: readonly SendView[]): SendState {
  const capability = state.hello?.capabilities.includes(CAPABILITY_AGENT_SHARE) ?? false
  const share = state.snapshot?.share
  if (!capability || share?.available !== true) return 'hidden'
  if (!share.canSend) return 'shareDisabledSignedOut'
  return shareDisabledKey(views) ?? 'ok'
}

/** 菜单在打开时构建，读最新状态即可。 */
export function peekSendState(views: readonly SendView[]): SendState {
  return sendState(rpcStore.peek(), views)
}

export function useSendState(views: readonly SendView[]): SendState {
  return useRpcSelector((state) => sendState(state, views))
}
