// 接收任务的「停止共享」：任务右键菜单与任务详情「传输诊断」页共用；记录状态经 `shareChanged` 回到快照。

import type { TFunction } from '../../i18n'
import { rpc } from '../../lib/rpc'
import { toast } from '../../ui'
import { accountErrorKey } from '../settings/sections/account/errorText'

/** 停止该次接收的共享（幂等）；失败 toast 原因。 */
export function stopProviding(recordId: string, t: TFunction): Promise<void> {
  return rpc.agent.share.stopProviding({ recordId }).then(
    () => undefined,
    (error: unknown) => toast.error(t(accountErrorKey(error))),
  )
}
