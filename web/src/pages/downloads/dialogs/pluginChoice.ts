// 「多插件择一」对话框的纯逻辑：默认候选 / 回答构造 / 兜底文案。

import type { PluginCandidateDto, SelectionOutcome, SelectionRequestDto } from '../../../lib/rpc'

/** 服务端 `defaultChoice` 指向的候选 identity；不在候选里（或不是 plugin 种类）时回退到第一个候选。 */
export function defaultPluginIdentity(request: Pick<SelectionRequestDto, 'defaultChoice'>, candidates: readonly PluginCandidateDto[]): string {
  const choice = request.defaultChoice
  if (choice.kind === 'plugin' && candidates.some((candidate) => candidate.identity === choice.identity)) return choice.identity
  return candidates[0]?.identity ?? ''
}

/** 确认回答；未选中任何候选时返回 null（不可确认）。 */
export function buildPluginOutcome(identity: string, remember: boolean, candidates: readonly PluginCandidateDto[]): SelectionOutcome | null {
  if (!candidates.some((candidate) => candidate.identity === identity)) return null
  return { kind: 'plugin', identity, remember }
}

/** 候选显示名：缺名字时回退 identity。 */
export function candidateTitle(candidate: PluginCandidateDto): string {
  return candidate.name.trim() === '' ? candidate.identity : candidate.name
}
