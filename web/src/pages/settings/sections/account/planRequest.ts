// 跨页面请求「打开套餐对话框」：额度页点「购买流量包 / 升级会员」后跳转账户设置，由 ProfileCard 挂载时取走。

export type PlanDialogTab = 'membership' | 'packs'

let pending: PlanDialogTab | null = null

export function requestPlanDialog(tab: PlanDialogTab): void {
  pending = tab
}

/** 取走并清空待处理请求。 */
export function takePlanRequest(): PlanDialogTab | null {
  const tab = pending
  pending = null
  return tab
}
