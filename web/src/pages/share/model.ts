// FluxMesh页面的纯展示逻辑：取件码 / 分享链接识别、状态与来源派生、网络状态摘要。
// 与 `crates/share`（GPUI）的同名规则对齐；DTO 见 `lib/rpc/protocol/share.ts`。

import type { TFunction } from '../../i18n'
import { formatBytes } from '../downloads/dialogs/utils'
import type {
  ShareCloudState,
  ShareDto,
  ShareNatType,
  ShareNetStatusDto,
  SharePathEventDto,
  SharePathKind,
  SharePolicySummaryDto,
  ShareProvideState,
  ShareSessionBytesDto,
  ShareSessionRole,
  ShareSwarmServedFileDto,
  ShareStatus,
  ShareTaskFactsDto,
  ShareTierDto,
  ShareUploadDto,
} from '../../lib/rpc'
import { DISPLAY_BYTES_CAP } from '../../lib/rpc'
import type { ErrorReason, ShareGrantSource, ShareTrafficKind, ShareUsageDto } from '../../lib/rpc'
import { REASON_KEYS } from '../settings/sections/account/errorText'

/** 取件码默认位数；云端 `policySummary.pickupCodeLength` 为 0（未下发）时沿用。 */
export const PICKUP_CODE_LENGTH = 6

/** 云端下发的取件码位数；0 视为默认 6 位（与 GPUI `pickup_code_len` 一致）。 */
export function pickupCodeLength(policy: Pick<SharePolicySummaryDto, 'pickupCodeLength'>): number {
  return policy.pickupCodeLength > 0 ? policy.pickupCodeLength : PICKUP_CODE_LENGTH
}

/** 取件码输入的有效字符：数字；空格 / 连字符只是分组分隔，忽略。 */
const CODE_SEPARATORS = /[\s-]/g

/** 输入框内容 → 纯数字取件码草稿（丢弃分隔符与非数字，封顶 `length` 位）。 */
export function normalizePickupDigits(input: string, length = PICKUP_CODE_LENGTH): string {
  return input.replace(/\D/g, '').slice(0, length)
}

/** 6 位取件码展示：`123456` → `123 456`（不足位数按已有位分组）。 */
export function formatPickupCode(code: string): string {
  const digits = code.replace(/\D/g, '')
  const groups: string[] = []
  for (let index = 0; index < digits.length; index += 3) groups.push(digits.slice(index, index + 3))
  return groups.join(' ')
}

/** 输入是否恰好是一个完整取件码（允许空格 / 连字符分组）。 */
export function isPickupCode(input: string, length = PICKUP_CODE_LENGTH): boolean {
  const compact = input.trim().replace(CODE_SEPARATORS, '')
  return compact.length === length && /^\d+$/.test(compact)
}

/** 分享 slug：10–12 位 base64url 字符（FluxCloud `codes::is_valid_slug`）。 */
const SLUG = '[A-Za-z0-9_-]{10,12}'
const DEEP_LINK = new RegExp(`^fluxdown://share/(${SLUG})/?$`, 'i')
const HTTPS_LINK = new RegExp(`^https://(?:fluxdown\\.com|fluxdown\\.zerx\\.dev)/s/(${SLUG})/?(?:[?#]\\S*)?$`, 'i')

export interface ShareLinkRef {
  /** 原样（trim 后）的链接。 */
  link: string
  slug: string
  /** `fluxdown://share/<slug>` 深链接，或官方分享域名（与 GPUI `parse_input` 同一组域名）下的 `https://<host>/s/<slug>`。 */
  scheme: 'deeplink' | 'https'
}

/** 整段文本恰好是一条分享链接（`fluxdown://share/<slug>` 或 `https://<host>/s/<slug>`）时解析之；否则 `null`。 */
export function parseShareLink(text: string): ShareLinkRef | null {
  const link = text.trim()
  if (link === '' || /\s/.test(link)) return null
  // slug 区分大小写；深链接的 scheme 大小写不敏感，故只对 scheme 部分放宽。
  const deep = DEEP_LINK.exec(link)
  if (deep?.[1]) return { link, slug: deep[1], scheme: 'deeplink' }
  const https = HTTPS_LINK.exec(link)
  if (https?.[1]) return { link, slug: https[1], scheme: 'https' }
  return null
}

/** 取件码行：整行去掉所有空白与 `-` 后恰为 `length` 位 ASCII 数字，返回纯数字串（wire 值）；夹杂其他字符（含全角数字、附注）的行不是。 */
export function pickupCodeInLine(line: string, length = PICKUP_CODE_LENGTH): string | null {
  return isPickupCode(line, length) ? normalizePickupDigits(line, length) : null
}

/**
 * 行里的分享链接（同 GPUI `share_link_in_line` 的手动输入口径）：只认行首的词，后面的内容是附注，与普通
 * 下载链接一致；返回 trim 后的链接本身。
 */
export function shareLinkInLine(line: string): string | null {
  const first = line.trim().split(/\s+/, 1)[0] ?? ''
  return parseShareLink(first)?.link ?? null
}

/** 待接收项是取件码（非空且全为 ASCII 数字）而不是分享链接。 */
export function isPickupCodeItem(item: string): boolean {
  return /^\d+$/.test(item)
}

/**
 * 新建下载框输入里的待接收项（与 GPUI `parse_input` 的 `share_links` 同语义）：逐行识别分享链接（行首的词，
 * 允许后跟附注）与取件码，按出现顺序、按值去重；链接取链接本身，取件码取纯数字串。它们不是下载链接。
 */
export function extractShareItems(text: string, codeLength = PICKUP_CODE_LENGTH): string[] {
  const items: string[] = []
  for (const line of text.split(/\r?\n/)) {
    const item = shareLinkInLine(line) ?? pickupCodeInLine(line, codeLength)
    if (item !== null && !items.includes(item)) items.push(item)
  }
  return items
}

/** 去掉第一处等于 `item` 的行（已交给接收确认框；取件码按纯数字串匹配），其余行原样保留（含注释 / 选项行）。 */
export function removeShareItem(text: string, item: string, codeLength = PICKUP_CODE_LENGTH): string {
  const lines = text.split(/\r?\n/)
  const index = lines.findIndex((line) => (shareLinkInLine(line) ?? pickupCodeInLine(line, codeLength)) === item)
  return index < 0 ? text : lines.filter((_, position) => position !== index).join('\n')
}

/**
 * 新建下载框的分享提示（文案键，同 GPUI `share_note` + 不可用说明）：没有待接收项返回 `null`；
 * FluxMesh 不可用时取件码与分享链接同样说明无法接收；没有普通条目且只有 1 项 → 取件码 / 链接各自的提示；
 * 有普通条目且只有 1 项 → 混合；多于 1 项 → 逐条接收。
 */
export function shareNoticeKey(entryCount: number, pending: number, pendingCodes: number, available: boolean): string | null {
  if (pending === 0) return null
  if (!available) return 'shareNewDownloadUnavailable'
  if (pending > 1) return 'shareNewDownloadMultiple'
  if (entryCount > 0) return 'shareNewDownloadMixed'
  return pendingCodes > 0 ? 'shareNewDownloadCode' : 'shareNewDownloadLink'
}

/** 有效期档位文案：小时数能被 24 整除且 ≥ 48 用天，其余用小时。 */
export function tierLabel(t: TFunction, tier: Pick<ShareTierDto, 'hours'>): string {
  const { hours } = tier
  if (hours >= 48 && hours % 24 === 0) return t('shareDurationDays', { count: hours / 24 })
  return t('shareDurationHours', { count: hours })
}

/** 领取次数候选（0 = 不限）；当前值不在预设里时并入，保持升序且不限排最后。 */
export const CLAIM_PRESETS: readonly number[] = [1, 3, 5, 10, 20, 50]

export function claimOptions(current: number): number[] {
  const values = new Set<number>(CLAIM_PRESETS)
  if (current > 0) values.add(current)
  return [...[...values].sort((a, b) => a - b), 0]
}

/** 分享的展示状态：服务端 `active` 但已过期（清理任务尚未标记）按已过期呈现。 */
export type ShareDisplayStatus = 'active' | 'expired' | 'stopped' | 'takedown' | 'unknown'

export function shareDisplayStatus(share: Pick<ShareDto, 'status' | 'expiresAt'>, nowMs: number): ShareDisplayStatus {
  const status: ShareStatus = share.status
  if (status === 'stopped') return 'stopped'
  if (status === 'expired') return 'expired'
  if (status === 'takedown') return 'takedown'
  if (status !== 'active') return 'unknown'
  const expires = Date.parse(share.expiresAt)
  return Number.isFinite(expires) && expires <= nowMs ? 'expired' : 'active'
}

/** 分享展示状态 → 文案键。 */
export const SHARE_STATUS_KEYS: Readonly<Record<ShareDisplayStatus, string>> = {
  active: 'shareStatusActive',
  expired: 'shareStatusExpired',
  stopped: 'shareStatusStopped',
  takedown: 'shareStatusTakedown',
  unknown: 'shareStatusUnknown',
}

/** 单项操作是否适用：已下架的分享只允许删除（与 GPUI `ShareAction::applies` 一致）。 */
export function shareActionApplies(action: 'start' | 'stop' | 'delete', share: Pick<ShareDto, 'status' | 'sourceMissing'>): boolean {
  if (action === 'delete') return true
  if (share.status === 'takedown') return false
  return action === 'start' ? canStartShare(share) : canStopShare(share)
}

/** 下架申诉的界面状态：可申诉 / 处理中 / 未通过 / 无（与 GPUI `appeal_state` 一致）。 */
export type AppealUi = 'canAppeal' | 'pending' | 'rejected' | 'none'

export function appealState(share: Pick<ShareDto, 'status' | 'appeal'>): AppealUi {
  if (share.status !== 'takedown') return 'none'
  const appeal = share.appeal
  if (appeal === undefined || appeal === null) return 'canAppeal'
  if (appeal.status === 'pending') return 'pending'
  if (appeal.status === 'rejected') return 'rejected'
  return 'none'
}

/** 可以「开始分享」：已停止 / 已过期（以服务端状态为准，不含仅显示过期的有效分享）且源文件未缺失（恢复会清掉云端副本，回到实时分享）。 */
export function canStartShare(share: Pick<ShareDto, 'status' | 'sourceMissing'>): boolean {
  return (share.status === 'stopped' || share.status === 'expired') && share.sourceMissing !== true
}

/** 可以「停止分享」：正在分享（含显示已过期但服务端尚未标记的有效分享）。 */
export function canStopShare(share: Pick<ShareDto, 'status'>): boolean {
  return share.status === 'active'
}

/**
 * 「文件缺失」徽标：已停止 / 已过期的分享只要 `sourceMissing` 就显示；
 * 有效分享还在云端副本就绪时仍能提供下载，不显示。
 */
export function isSourceMissing(share: Pick<ShareDto, 'status' | 'sourceMissing' | 'cloudState'>): boolean {
  if (share.sourceMissing !== true) return false
  return share.status !== 'active' || share.cloudState !== 'ready'
}

/** 批量操作条的按钮可用性与各操作的适用子集（只对适用的项逐个调用单项 RPC）。 */
export interface ShareBatchPlan {
  start: ShareDto[]
  stop: ShareDto[]
  /** 删除对所有选中项适用。 */
  remove: ShareDto[]
}

export function planShareBatch(selected: readonly ShareDto[]): ShareBatchPlan {
  return {
    start: selected.filter(canStartShare),
    stop: selected.filter(canStopShare),
    remove: [...selected],
  }
}

/** 全选框状态：全部选中为 `true`，部分为 `'indeterminate'`，其余 `false`。 */
export function selectAllState(selectedCount: number, total: number): boolean | 'indeterminate' {
  if (total > 0 && selectedCount >= total) return true
  return selectedCount > 0 ? 'indeterminate' : false
}

/** 选中集合只保留仍在列表里的分享（列表经 `shareChanged` 刷新后，已删除的不再占着选中状态）。 */
export function pruneSelection(selected: ReadonlySet<string>, shares: readonly Pick<ShareDto, 'id'>[]): Set<string> {
  const alive = new Set(shares.map((share) => share.id))
  return new Set([...selected].filter((id) => alive.has(id)))
}

/** 「我的分享」副行：`文件数 · 大小 · 到期`；到期时间非法则省略。 */
export function shareMetaLine(t: TFunction, share: Pick<ShareDto, 'files' | 'totalBytes' | 'expiresAt'>): string {
  const parts = [t('shareFilesCount', { count: share.files.length }), formatBytes(share.totalBytes)]
  const expires = dateTimeLabel(share.expiresAt)
  if (expires !== '') parts.push(t('shareExpiresAt', { time: expires }))
  return parts.join(' · ')
}

/** `YYYY-MM-DD HH:mm`（本地时区）；非法时间返回空串。 */
export function dateTimeLabel(iso: string): string {
  const at = new Date(iso)
  if (Number.isNaN(at.getTime())) return ''
  const pad = (value: number) => String(value).padStart(2, '0')
  return `${at.getFullYear()}-${pad(at.getMonth() + 1)}-${pad(at.getDate())} ${pad(at.getHours())}:${pad(at.getMinutes())}`
}

/** 路径类型 → 文案键。 */
export const PATH_KIND_KEYS: Readonly<Record<SharePathKind, string>> = {
  directLan: 'sharePathDirectLan',
  directWan: 'sharePathDirectWan',
  relay: 'sharePathRelay',
  cdn: 'shareBadgeCloud',
  bridge: 'shareBadgeBridge',
  swarm: 'sharePathSwarm',
  unknown: 'sharePathUnknown',
}

/** 接收记录共享状态 → 文案键；`off` 不显示。 */
export const PROVIDE_STATE_KEYS: Readonly<Record<ShareProvideState, string | null>> = {
  off: null,
  providing: 'shareProvideActive',
  ended: 'shareProvideEnded',
  stopped: 'shareProvideStopped',
}

/** 路径事件 `kind`（wire 为 `opened` / `selected` / `closed` / `lagged`）→ 文案键；未知种类显示原文。 */
export const PATH_EVENT_KEYS: Readonly<Record<string, string>> = {
  opened: 'shareDiagEventOpened',
  selected: 'shareDiagEventSelected',
  closed: 'shareDiagEventClosed',
  lagged: 'shareDiagEventLagged',
}

/** 当前在用的路径：最近一次 `selected` 的路径；它之后被 `closed` 则没有。 */
export function currentPathId(events: readonly Pick<SharePathEventDto, 'kind' | 'path'>[]): string | null {
  let current: string | null = null
  for (const event of events) {
    if (event.kind === 'selected') current = event.path
    else if (event.kind === 'closed' && event.path === current) current = null
  }
  return current
}

/** 一次会话各来源的字节（直连 / 中继 / 云端暂存 CDN / 桥接）。 */
export type SourceBytes = Pick<ShareSessionBytesDto, 'direct' | 'relay' | 'cdn' | 'bridge' | 'peer'>

/** 直连字节占总传输（四种来源之和）的整数百分比；尚无传输为 `null`。 */
export function bytesSplit(bytes: SourceBytes): { percent: number | null } {
  const total = bytes.direct + bytes.relay + bytes.cdn + bytes.bridge + (bytes.peer ?? 0)
  return { percent: total > 0 ? Math.round((bytes.direct / total) * 100) : null }
}

/** 来源字节明细：直连 / 中继恒有；云端 / 桥接仅在有字节时出现。 */
export function sourceBytesRows(bytes: SourceBytes): { kind: 'direct' | 'relay' | 'cdn' | 'bridge' | 'peer'; labelKey: string; bytes: number }[] {
  const rows: { kind: 'direct' | 'relay' | 'cdn' | 'bridge' | 'peer'; labelKey: string; bytes: number }[] = [
    { kind: 'direct', labelKey: 'shareDiagBytesDirect', bytes: bytes.direct },
    { kind: 'relay', labelKey: 'shareDiagBytesRelay', bytes: bytes.relay },
  ]
  if (bytes.cdn > 0) rows.push({ kind: 'cdn', labelKey: 'shareBadgeCloud', bytes: bytes.cdn })
  if (bytes.bridge > 0) rows.push({ kind: 'bridge', labelKey: 'shareBadgeBridge', bytes: bytes.bridge })
  if ((bytes.peer ?? 0) > 0) rows.push({ kind: 'peer', labelKey: 'sharePathSwarm', bytes: bytes.peer ?? 0 })
  return rows
}

/** 任务行来源徽标：仅FluxMesh接收任务有；`pathKind` 决定徽标文案与色调。 */
export interface ShareSourceBadge {
  pathKind: SharePathKind
  labelKey: string
  tone: 'success' | 'warning' | 'neutral'
  /** 共享对端数；0 不显示。 */
  peers: number
  /** 共享加速被云端拒绝时的 chip 文案键（点击进入设置 › 账户的额度区）；无则 null。 */
  swarmBlockKey?: string
}

const PATH_TONES: Readonly<Record<SharePathKind, ShareSourceBadge['tone']>> = {
  directLan: 'success',
  directWan: 'success',
  relay: 'warning',
  cdn: 'success',
  bridge: 'neutral',
  swarm: 'success',
  unknown: 'neutral',
}

/**
 * 任务行来源徽标。进行中的接收任务按路径显示；已完成的任务只在其接收记录仍在共享时显示「共享中」
 * （此时路径已无意义），其余已完成任务不显示。
 */
export function shareSourceBadge(facts: ShareTaskFactsDto | undefined, taskState: string): ShareSourceBadge | null {
  if (!facts || !facts.isShareReceive) return null
  if (taskState === 'completed') {
    return facts.provideState === 'providing' ? { pathKind: facts.pathKind, labelKey: 'shareProvidingBadge', tone: 'success', peers: 0 } : null
  }
  const swarmBlockKey = swarmBlockChipKey(facts.swarmBlock)
  return {
    pathKind: facts.pathKind,
    labelKey: PATH_KIND_KEYS[facts.pathKind],
    tone: PATH_TONES[facts.pathKind],
    peers: facts.swarmPeers ?? 0,
    ...(swarmBlockKey ? { swarmBlockKey } : {}),
  }
}

/** 可「停止共享」的接收记录 ID：任务的接收记录仍在共享（`providing`）且 agent 给出了记录 ID；否则 `null`。 */
export function providingRecordId(facts: ShareTaskFactsDto | undefined): string | null {
  return facts?.isShareReceive === true && facts.provideState === 'providing' ? (facts.recordId ?? null) : null
}

/** 共享加速被拒时的弱化 chip 文案键；`unknown` / 缺省不显示。 */
export function swarmBlockChipKey(block: ShareTaskFactsDto['swarmBlock']): string | null {
  return block === 'quotaExhausted' ? 'shareSwarmQuotaChip' : block === 'loginRequired' ? 'shareSwarmLoginChip' : null
}

/** 「切换桥接」：进行中（等待 / 下载中）的FluxMesh接收任务、路径尚不是桥接时提供；`relayStuck` 时强调。 */
export function canSwitchBridge(facts: ShareTaskFactsDto | undefined, taskState: string): boolean {
  return facts !== undefined && facts.isShareReceive && facts.pathKind !== 'bridge' && (taskState === 'downloading' || taskState === 'pending')
}

/** 云端暂存状态 → 文案键；没有云端副本为 `null`。 */
export const CLOUD_STATE_KEYS: Readonly<Record<ShareCloudState, string | null>> = {
  none: null,
  uploading: 'shareCloudStateUploading',
  ready: 'shareCloudStateReady',
  deleted: 'shareCloudStateDeleted',
  unknown: null,
}

/** 云端暂存上传的整数百分比（夹到 0..100）；总量未知为 0。 */
export function uploadPercent(upload: Pick<ShareUploadDto, 'doneBytes' | 'totalBytes'>): number {
  if (!(upload.totalBytes > 0)) return 0
  return Math.min(100, Math.max(0, Math.floor((upload.doneBytes / upload.totalBytes) * 100)))
}

/** 上传进度文案：上传中带速率，暂停 / 失败带状态文案（失败原因另见 {@link reasonTextKey}）。 */
export function uploadProgressText(t: TFunction, upload: ShareUploadDto): string {
  const params = { done: formatBytes(upload.doneBytes), total: formatBytes(upload.totalBytes), percent: uploadPercent(upload) }
  if (upload.status === 'paused') return t('shareCloudUploadPaused', params)
  if (upload.status === 'failed') return t('shareCloudUploadFailed', params)
  return t('shareCloudUploadProgress', { ...params, rate: rateLabel(upload.rateBps) })
}

/** 错误 reason → 文案键；没有 reason 或未登记返回 `null`。 */
export function reasonTextKey(reason: ErrorReason | null | undefined): string | null {
  return reason ? (REASON_KEYS[reason] ?? null) : null
}

/** 「转为云端暂存」的保留期档位（`policySummary.retentionTiers`，按时长升序）。 */
export function retentionTiers(policy: Pick<SharePolicySummaryDto, 'retentionTiers'>): ShareTierDto[] {
  return [...policy.retentionTiers].sort((a, b) => a.hours - b.hours)
}

/** 重试上传沿用分享当前的档位（仅当它是云端保留期档位）；否则交给云端默认档。 */
export function cloudRetryTierId(share: Pick<ShareDto, 'tierId'>, tiers: readonly Pick<ShareTierDto, 'id'>[]): string | undefined {
  return tiers.some((tier) => tier.id === share.tierId) ? share.tierId : undefined
}

/** 「转为云端暂存」入口：`hidden` = 云端存储未配置；`disabled` = 给出原因键；`enabled` 可点。 */
export type CloudConvertAvailability = { kind: 'hidden' } | { kind: 'disabled'; reasonKey: string } | { kind: 'enabled' }

/** 仅实时、有效、本机已登录且有可选档位的分享可转换；上传 / 就绪时不再提供转换。 */
export function cloudConvertAvailability(input: {
  share: Pick<ShareDto, 'cloudState'>
  displayStatus: ShareDisplayStatus
  policy: Pick<SharePolicySummaryDto, 'cloudEnabled' | 'retentionTiers'>
  signedIn: boolean
}): CloudConvertAvailability {
  if (!input.policy.cloudEnabled) return { kind: 'hidden' }
  if (!input.signedIn) return { kind: 'disabled', reasonKey: 'shareCloudNeedLogin' }
  if (input.displayStatus !== 'active') return { kind: 'disabled', reasonKey: 'shareCloudNotActive' }
  if (input.policy.retentionTiers.length === 0) return { kind: 'disabled', reasonKey: 'shareCloudNoTiers' }
  if (input.share.cloudState === 'uploading' || input.share.cloudState === 'ready') return { kind: 'hidden' }
  return { kind: 'enabled' }
}

/** 「删除云端副本」可用：正在上传（兼作取消）或已就绪。 */
export function canDeleteCloudCopy(share: Pick<ShareDto, 'cloudState'>): boolean {
  return share.cloudState === 'uploading' || share.cloudState === 'ready'
}

export const NAT_KEYS: Readonly<Record<ShareNatType, string>> = {
  cone: 'shareNatCone',
  symmetric: 'shareNatSymmetric',
  unknown: 'shareNatUnknown',
}

/** 会话角色的文案键（与 GPUI `shareNetRole*` 同源）。 */
export const SESSION_ROLE_KEYS: Readonly<Record<ShareSessionRole, string>> = {
  sender: 'shareNetRoleSender',
  receiver: 'shareNetRoleReceiver',
  swarm: 'shareNetRoleSwarm',
}

/** 端点 / 根哈希的短形式（保留头尾，按字符边界截取）；很短时原样返回（同 GPUI `short_id`）。 */
export function shortId(id: string): string {
  const chars = Array.from(id)
  if (chars.length <= 11) return id
  return `${chars.slice(0, 6).join('')}…${chars.slice(-4).join('')}`
}

/** 共享提供会话里一个内容的显示名：本机已知的文件名，否则回退到短根哈希。 */
export function servedFileLabel(file: Pick<ShareSwarmServedFileDto, 'rootHash' | 'fileName'>): string {
  const name = file.fileName?.trim() ?? ''
  return name === '' ? shortId(file.rootHash) : name
}

/** 共享开关：管理员锁定后恒为关且不可再开；其余按分享是否仍有效。 */
export function swarmToggle(share: Pick<ShareDto, 'swarm' | 'swarmLocked'>, live: boolean, pending: boolean): { checked: boolean; disabled: boolean; labelKey: string } {
  const locked = share.swarmLocked ?? false
  return {
    checked: (share.swarm ?? false) && !locked,
    disabled: !live || pending || locked,
    labelKey: locked ? 'shareSwarmLockedHint' : share.swarm ? 'shareSwarmOn' : 'shareSwarmOff',
  }
}

/** 本机网络状态摘要（分享面板状态行 / 网络诊断页用）。 */
export interface NetSummary {
  state: 'unbound' | 'offline' | 'online'
  natType: ShareNatType
  isp: string | null
  province: string | null
  ipv6: boolean
  /** home relay 延迟（毫秒）；无 home relay 数据时取全部中继里最低值，都没有为 `null`。 */
  relayMs: number | null
}

export function netSummary(net: ShareNetStatusDto): NetSummary {
  const latencies = net.relayLatencyMs
  const home = net.homeRelay ? latencies[net.homeRelay.url] : undefined
  const values = Object.values(latencies).filter((value) => Number.isFinite(value))
  const relayMs = home !== undefined && Number.isFinite(home) ? home : values.length > 0 ? Math.min(...values) : null
  return {
    state: !net.bound ? 'unbound' : net.online ? 'online' : 'offline',
    natType: net.natType,
    isp: net.isp,
    province: net.province,
    ipv6: net.ipv6,
    relayMs,
  }
}

/** 本机网络状态行：`NAT 类型 · 运营商 · IPv6 · 中继 18 ms`；端点未运行 / 离线给出单段状态。 */
export function netStatusLine(t: TFunction, net: ShareNetStatusDto): string {
  const summary = netSummary(net)
  if (summary.state === 'unbound') return t('shareNetHealthIdle')
  if (summary.state === 'offline') return t('shareNetHealthOffline')
  const parts = [t(NAT_KEYS[summary.natType])]
  const carrier = [summary.province, summary.isp].filter((part): part is string => part !== null && part !== '').join(' ')
  if (carrier !== '') parts.push(carrier)
  parts.push(summary.ipv6 ? t('shareIpv6On') : t('shareIpv6Off'))
  if (summary.relayMs !== null) parts.push(t('shareRelayLatency', { ms: Math.round(summary.relayMs) }))
  return parts.join(' · ')
}

/**
 * 任务分享的禁用原因（文案键）；可分享返回 `null`。远程 / 未完成 / 文件缺失一律置灰并以 tooltip 说明：
 * 单个任务给出具体原因，多选里有不可分享的任务统一说明「所选任务有不可分享的」。
 */
export function shareDisabledKey(views: readonly { source: 'local' | 'remote'; state: string; fileMissing: boolean }[]): string | null {
  const reason = (view: { source: 'local' | 'remote'; state: string; fileMissing: boolean }): string | null =>
    view.source !== 'local' ? 'shareDisabledRemote' : view.state !== 'completed' ? 'shareDisabledIncomplete' : view.fileMissing ? 'shareDisabledMissing' : null
  const [only] = views
  if (views.length === 0) return 'shareDisabledSelection'
  if (views.length === 1 && only) return reason(only)
  return views.some((view) => reason(view) !== null) ? 'shareDisabledSelection' : null
}

/** 同一批任务已有有效分享时复用（一个任务一条规范链接，改设置不新建）。 */
export function findExistingShare(shares: readonly ShareDto[], taskIds: readonly string[], nowMs: number): ShareDto | undefined {
  if (taskIds.length === 0) return undefined
  const wanted = new Set(taskIds)
  return shares.find(
    (share) =>
      shareDisplayStatus(share, nowMs) === 'active' &&
      share.taskIds.length === wanted.size &&
      share.taskIds.every((id) => wanted.has(id)),
  )
}

/** 字节 / 秒 → `1.5 MB/s`；未知 / 非正数为 `0 B/s`。 */
export function rateLabel(bytesPerSec: number): string {
  return `${formatBytes(Number.isFinite(bytesPerSec) && bytesPerSec > 0 ? bytesPerSec : 0)}/s`
}

/** 进度 0..1 → 整数百分比（夹到 0..100）。 */
export function progressPercent(progress: number): number {
  if (!Number.isFinite(progress)) return 0
  return Math.round(Math.min(1, Math.max(0, progress)) * 100)
}

/** 分享标题：云端生成的标题为主；为空时回退首个文件名（多文件附带数量）。 */
export function shareTitleText(t: TFunction, share: Pick<ShareDto, 'title' | 'files'>): string {
  if (share.title.trim() !== '') return share.title
  const first = share.files[0]
  if (!first) return t('shareFilesCount', { count: 0 })
  return share.files.length > 1 ? `${first.name} · ${t('shareFilesCount', { count: share.files.length })}` : first.name
}

/** 我的分享排序：有效的在前；同类按过期时间倒序（最晚到期在前），非法时间排最后。 */
export function sortShares(shares: readonly ShareDto[], nowMs: number): ShareDto[] {
  const time = (share: ShareDto) => {
    const value = Date.parse(share.expiresAt)
    return Number.isFinite(value) ? value : Number.NEGATIVE_INFINITY
  }
  return [...shares].sort((a, b) => {
    const live = Number(shareDisplayStatus(b, nowMs) === 'active') - Number(shareDisplayStatus(a, nowMs) === 'active')
    return live !== 0 ? live : time(b) - time(a)
  })
}

// ───────────────────────── 额度与贡献（与 GPUI `crates/share/src/model/usage.rs` 同语义） ─────────────────────────

/** 额度字节：≥ 展示封顶（1 PiB）显示 `≥ 1 PB`，否则按 `formatBytes`。 */
export function formatQuotaBytes(bytes: number): string {
  return bytes >= DISPLAY_BYTES_CAP ? '≥ 1 PB' : formatBytes(bytes)
}

export interface MeterView {
  kind: ShareTrafficKind
  labelKey: string
  remaining: number
  allowance: number
  used: number
  unlimited: boolean
  exhausted: boolean
  /** 已用占比 0..1。 */
  ratio: number
}

const KIND_LABEL_KEYS: Partial<Record<ShareTrafficKind, string>> = {
  swarm: 'shareUsageKindSwarm',
  download: 'shareUsageKindDownload',
  upload: 'shareUsageKindUpload',
}

/** 流量种类 → 名称键；unknown → null。 */
export function trafficKindKey(kind: ShareTrafficKind): string | null {
  return KIND_LABEL_KEYS[kind] ?? null
}

/** 三条额度条（保持云端顺序，跳过未知种类）。 */
export function usageMeters(usage: ShareUsageDto): MeterView[] {
  const meters: MeterView[] = []
  for (const item of usage.kinds) {
    const labelKey = trafficKindKey(item.kind)
    if (!labelKey) continue
    const exhausted = !item.unlimited && item.remaining <= 0
    const ratio = item.unlimited ? 0 : exhausted ? 1 : item.monthlyAllowance > 0 ? Math.min(1, Math.max(0, item.usedThisMonth / item.monthlyAllowance)) : 0
    meters.push({
      kind: item.kind,
      labelKey,
      remaining: item.remaining,
      allowance: item.monthlyAllowance,
      used: item.usedThisMonth,
      unlimited: item.unlimited,
      exhausted,
      ratio,
    })
  }
  return meters
}

/** 额度来源 → 文案键；Admin / Free / Unknown 都是「系统赠送」。 */
export function grantSourceKey(source: ShareGrantSource): string {
  switch (source) {
    case 'pack':
      return 'shareUsageGrantPack'
    case 'rollover':
      return 'shareUsageGrantRollover'
    case 'contribution':
      return 'shareUsageGrantContribution'
    default:
      return 'shareUsageGrantGranted'
  }
}

/** `Lv.N`；`max` 只用于判断满级（本函数不区分，保持签名与 GPUI 一致）。 */
export function levelLabel(level: number, _max: number): string {
  return `Lv.${level}`
}

/** 贡献加成：1000‰ → null；否则 `×1.5`（最多两位小数、去尾随 0）。 */
export function boostLabel(permille: number): string | null {
  if (permille === 1000) return null
  const value = Math.round(permille / 10) / 100
  return `×${value}`
}

/** 摘要行的额度项：已用完 / 最受限的剩余 / 全部不限。 */
export type UsageSummaryLine = { kind: 'exhausted' | 'remaining'; meter: MeterView } | { kind: 'unlimited' }

/**
 * 分享页摘要行的额度（与 GPUI 摘要同语义）：有已用完的项取第一个已用完项；否则取剩余比例最低的限额项
 * （总额为 0 的限额项按 100% 计，平手保持云端顺序）；全部不限为 `unlimited`；没有可识别的额度项为 `null`。
 */
export function usageSummaryLine(meters: readonly MeterView[]): UsageSummaryLine | null {
  if (meters.length === 0) return null
  const exhausted = meters.find((meter) => meter.exhausted)
  if (exhausted) return { kind: 'exhausted', meter: exhausted }
  let tightest: MeterView | null = null
  let tightestFraction = Number.POSITIVE_INFINITY
  for (const meter of meters) {
    if (meter.unlimited) continue
    const fraction = meter.allowance > 0 ? Math.min(1, Math.max(0, meter.remaining / meter.allowance)) : 1
    if (fraction < tightestFraction) {
      tightest = meter
      tightestFraction = fraction
    }
  }
  return tightest ? { kind: 'remaining', meter: tightest } : { kind: 'unlimited' }
}
