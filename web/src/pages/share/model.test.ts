import { describe, expect, test } from 'bun:test'
import type { ShareUsageDto, ShareDto, ShareNetStatusDto, ShareTaskFactsDto } from '../../lib/rpc'
import {
  appealState,
  shareActionApplies,
  boostLabel,
  formatQuotaBytes,
  grantSourceKey,
  levelLabel,
  usageMeters,
  CLOUD_STATE_KEYS,
  canStartShare,
  canStopShare,
  isSourceMissing,
  planShareBatch,
  pruneSelection,
  selectAllState,
  bytesSplit,
  canDeleteCloudCopy,
  canSwitchBridge,
  cloudConvertAvailability,
  cloudRetryTierId,
  claimOptions,
  currentPathId,
  extractShareItems,
  findExistingShare,
  formatPickupCode,
  isPickupCode,
  isPickupCodeItem,
  netStatusLine,
  netSummary,
  normalizePickupDigits,
  parseShareLink,
  pickupCodeInLine,
  pickupCodeLength,
  providingRecordId,
  reasonTextKey,
  retentionTiers,
  progressPercent,
  rateLabel,
  removeShareItem,
  shareDisabledKey,
  shareDisplayStatus,
  shareLinkInLine,
  shareMetaLine,
  shareNoticeKey,
  shareSourceBadge,
  sortShares,
  usageSummaryLine,
  sourceBytesRows,
  SESSION_ROLE_KEYS,
  servedFileLabel,
  shortId,
  swarmToggle,
  tierLabel,
  uploadPercent,
  uploadProgressText,
} from './model'
import { parseInput } from '../downloads/dialogs/model'

const dict: Record<string, string> = {
  shareDurationHours: '{count} h',
  shareDurationDays: '{count} d',
  shareCloudUploadProgress: '{done} / {total} ({percent}%) · {rate}',
  shareCloudUploadPaused: '{done} / {total} ({percent}%) paused',
  shareCloudUploadFailed: '{done} / {total} ({percent}%) failed',
  shareNetHealthIdle: 'idle',
  shareNetHealthOffline: 'offline',
  shareNatCone: 'cone',
  shareNatSymmetric: 'symmetric',
  shareNatUnknown: 'unknown',
  shareIpv6On: 'IPv6',
  shareIpv6Off: 'no IPv6',
  shareRelayLatency: 'relay {ms} ms',
  shareFilesCount: '{count} files',
  shareExpiresAt: '{time} expires',
}
const t = (key: string, params?: Record<string, string | number>): string => {
  let text = dict[key] ?? key
  for (const [name, value] of Object.entries(params ?? {})) text = text.replace(`{${name}}`, String(value))
  return text
}

const SLUG = 'AbC123_-xYz9'

function share(patch: Partial<ShareDto> = {}): ShareDto {
  return {
    id: 's1',
    slug: SLUG,
    url: `https://fluxdown.com/s/${SLUG}`,
    pickupCode: '123456',
    title: 'a.zip',
    files: [{ name: 'a.zip', size: 10 }],
    totalBytes: 10,
    mode: 'live',
    tierId: '24h',
    expiresAt: '2026-10-27T12:00:00Z',
    maxClaims: 0,
    claimCount: 0,
    notifyOnClaim: true,
    status: 'active',
    lastAccessedAt: null,
    taskIds: ['t1'],
    activeReceivers: [],
    cloudState: 'none',
    cloudBytes: 0,
    ...patch,
  }
}

function net(patch: Partial<ShareNetStatusDto> = {}): ShareNetStatusDto {
  return {
    bound: true,
    online: true,
    endpointId: 'ep',
    homeRelay: { url: 'https://r1', connected: true },
    natType: 'cone',
    isp: '联通',
    province: '北京',
    ipv6: true,
    relayLatencyMs: { 'https://r1': 18.4, 'https://r2': 40 },
    ...patch,
  }
}

describe('pickup code', () => {
  test('normalize drops separators and non-digits, caps at 6', () => {
    expect(normalizePickupDigits('12 34-56')).toBe('123456')
    expect(normalizePickupDigits('1234567890')).toBe('123456')
    expect(normalizePickupDigits('ab1c2')).toBe('12')
    expect(normalizePickupDigits('')).toBe('')
  })

  test('format groups by three', () => {
    expect(formatPickupCode('123456')).toBe('123 456')
    expect(formatPickupCode('1234')).toBe('123 4')
    expect(formatPickupCode('')).toBe('')
  })

  test('validation requires exactly six digits', () => {
    expect(isPickupCode('123456')).toBe(true)
    expect(isPickupCode(' 123 456 ')).toBe(true)
    expect(isPickupCode('123-456')).toBe(true)
    expect(isPickupCode('12345')).toBe(false)
    expect(isPickupCode('1234567')).toBe(false)
    expect(isPickupCode('12a456')).toBe(false)
    expect(isPickupCode('')).toBe(false)
  })

  test('pickup code length falls back to 6 when the cloud sends 0', () => {
    expect(pickupCodeLength({ pickupCodeLength: 0 })).toBe(6)
    expect(pickupCodeLength({ pickupCodeLength: 8 })).toBe(8)
  })

  test('pickup code line: separators stripped, exact ASCII digit count, wire value is the digits', () => {
    for (const text of ['123456', '123 456', '123-456', '  123456  ', '1-2 3\t4-5 6']) {
      expect(pickupCodeInLine(text, 6)).toBe('123456')
      expect(extractShareItems(text, 6)).toEqual(['123456'])
    }
    // 长度 0（云端未下发）按默认 6 位；云端配置其他位数时按该位数识别。
    expect(pickupCodeInLine('123456', pickupCodeLength({ pickupCodeLength: 0 }))).toBe('123456')
    expect(extractShareItems('1234 5678', 8)).toEqual(['12345678'])
    expect(extractShareItems('123456', 8)).toEqual([])
  })

  test('wrong length or decorated numbers are not pickup codes', () => {
    for (const text of ['12345', '1234567', '12 34 5', '123456 note', 'code 123456', '12345a', '１２３４５６', '123.456', '-', '']) {
      expect(pickupCodeInLine(text)).toBeNull()
      expect(extractShareItems(text)).toEqual([])
    }
  })

  test('pending items tell codes from links', () => {
    expect(isPickupCodeItem('123456')).toBe(true)
    expect(isPickupCodeItem('')).toBe(false)
    expect(isPickupCodeItem(`fluxdown://share/${SLUG}`)).toBe(false)
    expect(isPickupCodeItem(`https://fluxdown.com/s/${SLUG}`)).toBe(false)
  })
})

describe('share link detection', () => {
  test('deep link and https link', () => {
    expect(parseShareLink(`fluxdown://share/${SLUG}`)).toEqual({ link: `fluxdown://share/${SLUG}`, slug: SLUG, scheme: 'deeplink' })
    expect(parseShareLink(`FluxDown://share/${SLUG}/`)?.scheme).toBe('deeplink')
    expect(parseShareLink(`  https://fluxdown.com/s/${SLUG}\n`)).toEqual({ link: `https://fluxdown.com/s/${SLUG}`, slug: SLUG, scheme: 'https' })
    expect(parseShareLink(`https://fluxdown.zerx.dev/s/${SLUG}?from=qr`)?.slug).toBe(SLUG)
    expect(parseShareLink(`https://share.example.net/s/${SLUG}`)).toBeNull()
  })

  test('rejects lookalikes', () => {
    expect(parseShareLink('')).toBeNull()
    expect(parseShareLink('https://fluxdown.com/s/short')).toBeNull()
    expect(parseShareLink(`https://fluxdown.com/s/${SLUG}extra`)).toBeNull()
    expect(parseShareLink(`https://fluxdown.com/x/${SLUG}`)).toBeNull()
    expect(parseShareLink(`http://fluxdown.com/s/${SLUG}`)).toBeNull()
    expect(parseShareLink(`fluxdown://download?url=${SLUG}`)).toBeNull()
    expect(parseShareLink(`https://fluxdown.com/s/${SLUG} https://a.b/c`)).toBeNull()
  })

  test('a share link followed by a note is still a pending item (leading word, like GPUI)', () => {
    const deep = `fluxdown://share/${SLUG}`
    const https = `https://fluxdown.com/s/${SLUG}`
    expect(shareLinkInLine(`  ${deep}  来自小明的电脑`)).toBe(deep)
    expect(shareLinkInLine(`${https}\t备注`)).toBe(https)
    expect(shareLinkInLine(`备注 ${deep}`)).toBeNull()
    expect(shareLinkInLine(`# ${deep}`)).toBeNull()
    expect(extractShareItems(`${deep} 第一份\n${deep}\n${https} note`)).toEqual([deep, https])
    expect(removeShareItem(`a\n${deep} 第一份\nb`, deep)).toBe('a\nb')
  })
})

describe('status derivation', () => {
  const now = Date.parse('2026-10-27T00:00:00Z')

  test('active share past its expiry shows as expired', () => {
    expect(shareDisplayStatus(share(), now)).toBe('active')
    expect(shareDisplayStatus(share({ expiresAt: '2026-10-26T23:59:59Z' }), now)).toBe('expired')
    expect(shareDisplayStatus(share({ status: 'expired' }), now)).toBe('expired')
    expect(shareDisplayStatus(share({ status: 'stopped' }), now)).toBe('stopped')
    expect(shareDisplayStatus(share({ status: 'unknown' }), now)).toBe('unknown')
  })

  test('tier labels', () => {
    expect(tierLabel(t, { hours: 24 })).toBe('24 h')
    expect(tierLabel(t, { hours: 168 })).toBe('7 d')
    expect(tierLabel(t, { hours: 50 })).toBe('50 h')
  })

  test('existing share is reused only for the same task set while active', () => {
    const shares = [share({ id: 'a', taskIds: ['t1', 't2'] }), share({ id: 'b', taskIds: ['t3'], status: 'stopped' })]
    expect(findExistingShare(shares, ['t2', 't1'], now)?.id).toBe('a')
    expect(findExistingShare(shares, ['t1'], now)).toBeUndefined()
    expect(findExistingShare(shares, ['t3'], now)).toBeUndefined()
    expect(findExistingShare(shares, [], now)).toBeUndefined()
  })
})

describe('claims options', () => {
  test('presets with unlimited last; current value merged in order', () => {
    expect(claimOptions(0)).toEqual([1, 3, 5, 10, 20, 50, 0])
    expect(claimOptions(7)).toEqual([1, 3, 5, 7, 10, 20, 50, 0])
    expect(claimOptions(10)).toEqual([1, 3, 5, 10, 20, 50, 0])
  })
})

describe('source badge', () => {
  const facts = (patch: Partial<ShareTaskFactsDto>): ShareTaskFactsDto => ({
    isShareReceive: true,
    pathKind: 'directLan',
    peerNatType: 'cone',
    peerIsp: null,
    shareId: 's1',
    sessionId: 'x',
    relayStuck: false,
    ...patch,
  })

  test('only share-receive tasks get a badge', () => {
    expect(shareSourceBadge(undefined, 'downloading')).toBeNull()
    expect(shareSourceBadge(facts({ isShareReceive: false }), 'downloading')).toBeNull()
  })

  test('badge label and tone follow the path', () => {
    expect(shareSourceBadge(facts({}), 'downloading')).toEqual({ pathKind: 'directLan', labelKey: 'sharePathDirectLan', tone: 'success', peers: 0 })
    expect(shareSourceBadge(facts({ pathKind: 'directWan' }), 'pending')?.tone).toBe('success')
    expect(shareSourceBadge(facts({ pathKind: 'relay' }), 'downloading')).toEqual({ pathKind: 'relay', labelKey: 'sharePathRelay', tone: 'warning', peers: 0 })
    expect(shareSourceBadge(facts({ pathKind: 'cdn' }), 'downloading')).toEqual({ pathKind: 'cdn', labelKey: 'shareBadgeCloud', tone: 'success', peers: 0 })
    expect(shareSourceBadge(facts({ pathKind: 'bridge' }), 'downloading')).toEqual({ pathKind: 'bridge', labelKey: 'shareBadgeBridge', tone: 'neutral', peers: 0 })
    expect(shareSourceBadge(facts({ pathKind: 'unknown' }), 'downloading')?.tone).toBe('neutral')
    expect(shareSourceBadge(facts({ pathKind: 'swarm', swarmPeers: 3 }), 'downloading')).toEqual({ pathKind: 'swarm', labelKey: 'sharePathSwarm', tone: 'success', peers: 3 })
    expect(shareSourceBadge(facts({ swarmBlock: 'quotaExhausted' }), 'downloading')?.swarmBlockKey).toBe('shareSwarmQuotaChip')
    expect(shareSourceBadge(facts({ swarmBlock: 'loginRequired' }), 'downloading')?.swarmBlockKey).toBe('shareSwarmLoginChip')
    expect(shareSourceBadge(facts({ swarmBlock: 'unknown' }), 'downloading')?.swarmBlockKey).toBeUndefined()
    expect(sourceBytesRows({ direct: 0, relay: 0, cdn: 0, bridge: 0, peer: 9 }).map((row) => [row.kind, row.bytes]).at(-1)).toEqual(['peer', 9])
    expect(bytesSplit({ direct: 1, relay: 0, cdn: 0, bridge: 0, peer: 3 }).percent).toBe(25)
  })

  test('a completed task only shows the sharing badge while its record is still providing', () => {
    const providing = facts({ pathKind: 'unknown', sessionId: null, provideState: 'providing', recordId: 'r1' })
    expect(shareSourceBadge(providing, 'completed')).toEqual({ pathKind: 'unknown', labelKey: 'shareProvidingBadge', tone: 'success', peers: 0 })
    expect(shareSourceBadge(facts({ provideState: 'ended' }), 'completed')).toBeNull()
    expect(shareSourceBadge(facts({ provideState: 'stopped' }), 'completed')).toBeNull()
    expect(shareSourceBadge(facts({}), 'completed')).toBeNull()
    expect(shareSourceBadge(facts({ isShareReceive: false, provideState: 'providing' }), 'completed')).toBeNull()
    // 进行中的任务照常按路径显示，不受共享状态影响。
    expect(shareSourceBadge(facts({ provideState: 'providing' }), 'downloading')?.labelKey).toBe('sharePathDirectLan')
  })

  test('stop-sharing needs a providing record with an id', () => {
    expect(providingRecordId(facts({ provideState: 'providing', recordId: 'r1' }))).toBe('r1')
    expect(providingRecordId(facts({ provideState: 'providing', recordId: null }))).toBeNull()
    expect(providingRecordId(facts({ provideState: 'providing' }))).toBeNull()
    expect(providingRecordId(facts({ provideState: 'ended', recordId: 'r1' }))).toBeNull()
    expect(providingRecordId(facts({ provideState: 'off', recordId: 'r1' }))).toBeNull()
    expect(providingRecordId(facts({ isShareReceive: false, provideState: 'providing', recordId: 'r1' }))).toBeNull()
    expect(providingRecordId(undefined)).toBeNull()
  })

  test('bridge switch is offered on active share-receive tasks not already on the bridge', () => {
    expect(canSwitchBridge(facts({ pathKind: 'relay' }), 'downloading')).toBe(true)
    expect(canSwitchBridge(facts({ pathKind: 'directWan' }), 'downloading')).toBe(true)
    expect(canSwitchBridge(facts({ pathKind: 'cdn' }), 'pending')).toBe(true)
    expect(canSwitchBridge(facts({ pathKind: 'bridge' }), 'downloading')).toBe(false)
    expect(canSwitchBridge(facts({ pathKind: 'relay' }), 'paused')).toBe(false)
    expect(canSwitchBridge(facts({ pathKind: 'relay' }), 'completed')).toBe(false)
    expect(canSwitchBridge(facts({ pathKind: 'relay', isShareReceive: false }), 'downloading')).toBe(false)
    expect(canSwitchBridge(undefined, 'downloading')).toBe(false)
  })
})

describe('network status', () => {
  test('summary picks the home relay latency, else the lowest', () => {
    expect(netSummary(net()).relayMs).toBe(18.4)
    expect(netSummary(net({ homeRelay: null })).relayMs).toBe(18.4)
    expect(netSummary(net({ homeRelay: { url: 'https://missing', connected: false } })).relayMs).toBe(18.4)
    expect(netSummary(net({ relayLatencyMs: {} })).relayMs).toBeNull()
  })

  test('state derives from bound / online', () => {
    expect(netSummary(net({ bound: false })).state).toBe('unbound')
    expect(netSummary(net({ online: false })).state).toBe('offline')
    expect(netSummary(net()).state).toBe('online')
  })

  test('status line', () => {
    expect(netStatusLine(t, net())).toBe('cone · 北京 联通 · IPv6 · relay 18 ms')
    expect(netStatusLine(t, net({ isp: null, province: null, ipv6: false, relayLatencyMs: {} }))).toBe('cone · no IPv6')
    expect(netStatusLine(t, net({ bound: false }))).toBe('idle')
    expect(netStatusLine(t, net({ online: false }))).toBe('offline')
  })
})

describe('sendability', () => {
  const view = { source: 'local' as const, state: 'completed', fileMissing: false }

  test('single task gets a specific reason; a mixed selection a general one', () => {
    expect(shareDisabledKey([])).toBe('shareDisabledSelection')
    expect(shareDisabledKey([view])).toBeNull()
    expect(shareDisabledKey([view, view])).toBeNull()
    expect(shareDisabledKey([{ ...view, source: 'remote' }])).toBe('shareDisabledRemote')
    expect(shareDisabledKey([{ ...view, state: 'downloading' }])).toBe('shareDisabledIncomplete')
    expect(shareDisabledKey([{ ...view, fileMissing: true }])).toBe('shareDisabledMissing')
    expect(shareDisabledKey([view, { ...view, source: 'remote' }])).toBe('shareDisabledSelection')
    expect(shareDisabledKey([view, { ...view, fileMissing: true }])).toBe('shareDisabledSelection')
  })
})

describe('numbers', () => {
  test('rate label and percent', () => {
    expect(rateLabel(1536)).toBe('1.5 KB/s')
    expect(rateLabel(-1)).toBe('0 B/s')
    expect(progressPercent(0.456)).toBe(46)
    expect(progressPercent(2)).toBe(100)
    expect(progressPercent(Number.NaN)).toBe(0)
  })
})

describe('lists', () => {
  const now = Date.parse('2026-10-27T00:00:00Z')

  test('shares: active first, then latest expiry first', () => {
    const list = [
      share({ id: 'stopped', status: 'stopped', expiresAt: '2026-12-01T00:00:00Z' }),
      share({ id: 'soon', expiresAt: '2026-10-28T00:00:00Z' }),
      share({ id: 'later', expiresAt: '2026-11-05T00:00:00Z' }),
      share({ id: 'lapsed', expiresAt: '2026-10-01T00:00:00Z' }),
    ]
    expect(sortShares(list, now).map((item) => item.id)).toEqual(['later', 'soon', 'stopped', 'lapsed'])
    expect(list[0]?.id).toBe('stopped')
  })

  test('share meta line: files · size · expiry, expiry omitted when the time is invalid', () => {
    const base = { files: [{ name: 'a', size: 1 }, { name: 'b', size: 2 }], totalBytes: 1536 }
    expect(shareMetaLine(t, { ...base, expiresAt: 'not a time' })).toBe('2 files · 1.5 KB')
    const withExpiry = shareMetaLine(t, { ...base, expiresAt: '2026-10-27T12:00:00Z' })
    expect(withExpiry.startsWith('2 files · 1.5 KB · ')).toBe(true)
    expect(withExpiry.endsWith(' expires')).toBe(true)
    expect(shareMetaLine(t, { files: [], totalBytes: 0, expiresAt: '' })).toBe('0 files · 0 B')
  })
})

describe('new download paste', () => {
  const deep = `fluxdown://share/${SLUG}`
  const https = `https://fluxdown.com/s/${SLUG}`

  test('share items are found line by line; one is removed without touching the rest', () => {
    const text = `https://example.com/a.zip\n${deep}\n  out=a.zip\n${https}\r\n# note`
    expect(extractShareItems(text)).toEqual([deep, https])
    expect(removeShareItem(text, deep)).toBe(`https://example.com/a.zip\n  out=a.zip\n${https}\n# note`)
    expect(removeShareItem(`${deep}\n${deep}`, deep)).toBe(deep)
    expect(removeShareItem('https://example.com/a.zip', deep)).toBe('https://example.com/a.zip')
    expect(extractShareItems('https://example.com/a.zip')).toEqual([])
    // 取件码按纯数字串匹配，分组写法的行同样被拿走。
    expect(removeShareItem('https://a.example/x.zip\n123 456\n654321', '123456')).toBe('https://a.example/x.zip\n654321')
  })

  test('codes mix with links and ordinary entries, deduped by value in order of appearance', () => {
    const text = `https://a.example/one.zip\n123 456\n${deep}\n123-456\n  654321\nmagnet:?xt=urn:btih:xyz`
    expect(parseInput(text, false).entries.map((entry) => entry.url)).toEqual(['https://a.example/one.zip', 'magnet:?xt=urn:btih:xyz'])
    expect(extractShareItems(text, 6)).toEqual(['123456', deep, '654321'])
    expect(parseInput(text, false).entries[0]?.fileName).toBe('')
  })

  test('a bare code line never becomes a download entry', () => {
    for (const text of ['123456', '123 456', '123-456', '  123456  ', '1-2 3\t4-5 6']) expect(parseInput(text, false).entries).toEqual([])
  })

  test('notice follows pending share items (same table as GPUI share_note) plus the unavailable case', () => {
    // (entries, pending, pendingCodes)
    expect(shareNoticeKey(3, 0, 0, true)).toBeNull()
    expect(shareNoticeKey(0, 0, 0, true)).toBeNull()
    expect(shareNoticeKey(0, 1, 0, true)).toBe('shareNewDownloadLink')
    expect(shareNoticeKey(0, 1, 1, true)).toBe('shareNewDownloadCode')
    expect(shareNoticeKey(2, 1, 0, true)).toBe('shareNewDownloadMixed')
    expect(shareNoticeKey(2, 1, 1, true)).toBe('shareNewDownloadMixed')
    expect(shareNoticeKey(0, 3, 0, true)).toBe('shareNewDownloadMultiple')
    expect(shareNoticeKey(0, 2, 1, true)).toBe('shareNewDownloadMultiple')
    expect(shareNoticeKey(5, 2, 2, true)).toBe('shareNewDownloadMultiple')
    // FluxMesh 不可用：取件码与链接同样说明无法接收，不静默忽略。
    expect(shareNoticeKey(0, 1, 1, false)).toBe('shareNewDownloadUnavailable')
    expect(shareNoticeKey(0, 1, 0, false)).toBe('shareNewDownloadUnavailable')
    expect(shareNoticeKey(2, 3, 1, false)).toBe('shareNewDownloadUnavailable')
    expect(shareNoticeKey(2, 0, 0, false)).toBeNull()
  })
})

describe('path diagnostics', () => {
  test('current path follows the latest selected event until it closes', () => {
    expect(currentPathId([])).toBeNull()
    expect(currentPathId([{ kind: 'opened', path: 'a' }, { kind: 'selected', path: 'a' }])).toBe('a')
    expect(currentPathId([{ kind: 'selected', path: 'a' }, { kind: 'selected', path: 'b' }, { kind: 'closed', path: 'a' }])).toBe('b')
    expect(currentPathId([{ kind: 'selected', path: 'a' }, { kind: 'closed', path: 'a' }])).toBeNull()
  })

  test('direct share of bytes', () => {
    expect(bytesSplit({ direct: 0, relay: 0, cdn: 0, bridge: 0 }).percent).toBeNull()
    expect(bytesSplit({ direct: 3, relay: 1, cdn: 0, bridge: 0 }).percent).toBe(75)
    expect(bytesSplit({ direct: 0, relay: 5, cdn: 0, bridge: 0 }).percent).toBe(0)
    expect(bytesSplit({ direct: 1, relay: 0, cdn: 2, bridge: 1 }).percent).toBe(25)
  })

  test('swarm provider session: role label, served file name or short root hash', () => {
    expect(SESSION_ROLE_KEYS.swarm).toBe('shareNetRoleSwarm')
    expect(SESSION_ROLE_KEYS.sender).toBe('shareNetRoleSender')
    const root = 'ab'.repeat(32)
    expect(servedFileLabel({ rootHash: root, fileName: '电影 2024 第一季.mkv' })).toBe('电影 2024 第一季.mkv')
    for (const fileName of [undefined, null, '', '  \t']) {
      expect(servedFileLabel({ rootHash: root, fileName })).toBe(`${root.slice(0, 6)}…${root.slice(-4)}`)
    }
    expect(shortId('peer')).toBe('peer')
    expect(shortId('日本語のエンドポイント識別子です')).toBe('日本語のエン…別子です')
  })

  test('swarm toggle is off and disabled once an administrator locked it', () => {
    expect(swarmToggle({ swarm: true }, true, false)).toEqual({ checked: true, disabled: false, labelKey: 'shareSwarmOn' })
    expect(swarmToggle({ swarm: false }, true, false)).toEqual({ checked: false, disabled: false, labelKey: 'shareSwarmOff' })
    expect(swarmToggle({}, true, false).checked).toBe(false)
    expect(swarmToggle({ swarm: true }, false, false).disabled).toBe(true)
    expect(swarmToggle({ swarm: true }, true, true).disabled).toBe(true)
    // 锁定时即使本地缓存里还带着 swarm=true，也显示为关，并给出提示文案。
    expect(swarmToggle({ swarm: true, swarmLocked: true }, true, false)).toEqual({ checked: false, disabled: true, labelKey: 'shareSwarmLockedHint' })
  })
})

describe('source bytes', () => {
  test('direct and relay always listed; cloud and bridge only when used', () => {
    expect(sourceBytesRows({ direct: 1, relay: 2, cdn: 0, bridge: 0 }).map((row) => row.kind)).toEqual(['direct', 'relay'])
    expect(sourceBytesRows({ direct: 0, relay: 0, cdn: 5, bridge: 7 }).map((row) => [row.kind, row.bytes])).toEqual([
      ['direct', 0],
      ['relay', 0],
      ['cdn', 5],
      ['bridge', 7],
    ])
  })
})

describe('cloud storage', () => {
  const policy = { cloudEnabled: true, retentionTiers: [{ id: 'cloud-7d', hours: 168 }, { id: 'cloud-24h', hours: 24 }] }
  const convert = (patch: Partial<Parameters<typeof cloudConvertAvailability>[0]> = {}) =>
    cloudConvertAvailability({ share: { cloudState: 'none' }, displayStatus: 'active', policy, signedIn: true, ...patch })

  test('retention tiers sort by duration', () => {
    expect(retentionTiers(policy).map((tier) => tier.id)).toEqual(['cloud-24h', 'cloud-7d'])
    expect(retentionTiers({ retentionTiers: [] })).toEqual([])
  })

  test('convert is hidden without storage, disabled with a reason otherwise', () => {
    expect(convert({ policy: { ...policy, cloudEnabled: false } })).toEqual({ kind: 'hidden' })
    expect(convert({ signedIn: false })).toEqual({ kind: 'disabled', reasonKey: 'shareCloudNeedLogin' })
    expect(convert({ displayStatus: 'expired' })).toEqual({ kind: 'disabled', reasonKey: 'shareCloudNotActive' })
    expect(convert({ policy: { ...policy, retentionTiers: [] } })).toEqual({ kind: 'disabled', reasonKey: 'shareCloudNoTiers' })
    expect(convert()).toEqual({ kind: 'enabled' })
    expect(convert({ share: { cloudState: 'deleted' } })).toEqual({ kind: 'enabled' })
  })

  test('convert is not offered once uploading or ready; deleting is', () => {
    expect(convert({ share: { cloudState: 'uploading' } })).toEqual({ kind: 'hidden' })
    expect(convert({ share: { cloudState: 'ready' } })).toEqual({ kind: 'hidden' })
    expect(canDeleteCloudCopy({ cloudState: 'uploading' })).toBe(true)
    expect(canDeleteCloudCopy({ cloudState: 'ready' })).toBe(true)
    expect(canDeleteCloudCopy({ cloudState: 'none' })).toBe(false)
    expect(canDeleteCloudCopy({ cloudState: 'deleted' })).toBe(false)
  })

  test('state keys', () => {
    expect(CLOUD_STATE_KEYS.none).toBeNull()
    expect(CLOUD_STATE_KEYS.ready).toBe('shareCloudStateReady')
  })

  test('upload percent is clamped and safe for unknown totals', () => {
    expect(uploadPercent({ doneBytes: 50, totalBytes: 200 })).toBe(25)
    expect(uploadPercent({ doneBytes: 5, totalBytes: 0 })).toBe(0)
    expect(uploadPercent({ doneBytes: 300, totalBytes: 200 })).toBe(100)
    expect(uploadPercent({ doneBytes: 199, totalBytes: 200 })).toBe(99)
  })

  test('upload text follows the status', () => {
    const base = { doneBytes: 512, totalBytes: 1024, rateBps: 2048 }
    expect(uploadProgressText(t, { ...base, status: 'uploading' })).toBe('512 B / 1.0 KB (50%) · 2.0 KB/s')
    expect(uploadProgressText(t, { ...base, status: 'paused' })).toBe('512 B / 1.0 KB (50%) paused')
    expect(uploadProgressText(t, { ...base, status: 'failed' })).toBe('512 B / 1.0 KB (50%) failed')
  })

  test('reason text key resolves typed reasons only', () => {
    expect(reasonTextKey('shareTrafficExhausted')).toBe('errReasonShareTrafficExhausted')
    expect(reasonTextKey('shareQuotaExceeded')).toBe('errReasonShareQuotaExceeded')
    expect(reasonTextKey(undefined)).toBeNull()
    expect(reasonTextKey(null)).toBeNull()
  })

  test('retry reuses the share tier only when it is a cloud tier', () => {
    expect(cloudRetryTierId({ tierId: 'cloud-7d' }, policy.retentionTiers)).toBe('cloud-7d')
    expect(cloudRetryTierId({ tierId: '24h' }, policy.retentionTiers)).toBeUndefined()
  })
})

describe('batch share actions', () => {
  const stopped = share({ id: 'stopped', status: 'stopped' })
  const expired = share({ id: 'expired', status: 'expired' })
  const active = share({ id: 'active', status: 'active' })
  const missing = share({ id: 'missing', status: 'stopped', sourceMissing: true })
  const missingCloud = share({ id: 'missing-cloud', status: 'stopped', sourceMissing: true, cloudState: 'ready' })
  const activeMissing = share({ id: 'active-missing', status: 'active', sourceMissing: true })
  const activeMissingCloud = share({ id: 'active-missing-cloud', status: 'active', sourceMissing: true, cloudState: 'ready' })
  const unknown = share({ id: 'unknown', status: 'unknown' })

  test('start needs a stopped / expired share whose source is still there', () => {
    expect(canStartShare(stopped)).toBe(true)
    expect(canStartShare(expired)).toBe(true)
    expect(canStartShare(active)).toBe(false)
    expect(canStartShare(unknown)).toBe(false)
    expect(canStartShare(missing)).toBe(false)
  })

  test('missing badge: ended shares always, active ones only without a ready cloud copy', () => {
    expect(isSourceMissing(stopped)).toBe(false)
    expect(isSourceMissing(missing)).toBe(true)
    expect(isSourceMissing(missingCloud)).toBe(true)
    expect(isSourceMissing(activeMissing)).toBe(true)
    expect(isSourceMissing(activeMissingCloud)).toBe(false)
  })

  test('a stopped share with a missing source cannot start even with a cloud copy', () => {
    expect(canStartShare(missingCloud)).toBe(false)
  })

  test('stop only applies to active shares', () => {
    expect(canStopShare(active)).toBe(true)
    expect(canStopShare(stopped)).toBe(false)
    expect(canStopShare(expired)).toBe(false)
  })

  test('plan picks applicable subsets and delete covers everything', () => {
    const plan = planShareBatch([stopped, active, missing, expired, unknown])
    expect(plan.start.map((s) => s.id)).toEqual(['stopped', 'expired'])
    expect(plan.stop.map((s) => s.id)).toEqual(['active'])
    expect(plan.remove.map((s) => s.id)).toEqual(['stopped', 'active', 'missing', 'expired', 'unknown'])
    expect(planShareBatch([missing])).toEqual({ start: [], stop: [], remove: [missing] })
  })

  test('select-all state', () => {
    expect(selectAllState(0, 3)).toBe(false)
    expect(selectAllState(2, 3)).toBe('indeterminate')
    expect(selectAllState(3, 3)).toBe(true)
    expect(selectAllState(0, 0)).toBe(false)
  })

  test('selection drops shares that are gone', () => {
    const kept = pruneSelection(new Set(['a', 'b', 'c']), [{ id: 'a' }, { id: 'c' }])
    expect([...kept]).toEqual(['a', 'c'])
  })
})

describe('usage helpers', () => {
  test('formatQuotaBytes caps at 1 PB', () => {
    expect(formatQuotaBytes(2 ** 50)).toBe('≥ 1 PB')
    expect(formatQuotaBytes(2 ** 50 - 1).startsWith('≥')).toBe(false)
    expect(formatQuotaBytes(1024)).toBe('1.0 KB')
  })
  test('boostLabel / levelLabel / grantSourceKey', () => {
    expect(boostLabel(1000)).toBeNull()
    expect(boostLabel(1500)).toBe('×1.5')
    expect(boostLabel(2000)).toBe('×2')
    expect(boostLabel(1250)).toBe('×1.25')
    expect(levelLabel(3, 10)).toBe('Lv.3')
    expect(grantSourceKey('admin')).toBe('shareUsageGrantGranted')
    expect(grantSourceKey('free')).toBe('shareUsageGrantGranted')
    expect(grantSourceKey('unknown')).toBe('shareUsageGrantGranted')
    expect(grantSourceKey('pack')).toBe('shareUsageGrantPack')
  })
  test('usageMeters flags unlimited / exhausted and skips unknown kinds', () => {
    const k = (kind: string, o: object) => ({ kind, unlimited: false, monthlyAllowance: 100, usedThisMonth: 50, remaining: 50, reserved: 0, ...o })
    const usage = {
      kinds: [k('swarm', { remaining: 0, usedThisMonth: 100 }), k('download', { unlimited: true }), k('upload', {}), k('weird', {})],
    } as unknown as ShareUsageDto
    const meters = usageMeters(usage)
    expect(meters.map((m) => m.kind)).toEqual(['swarm', 'download', 'upload'])
    expect(meters[0]).toMatchObject({ exhausted: true, ratio: 1 })
    expect(meters[1]).toMatchObject({ unlimited: true, exhausted: false, ratio: 0 })
    expect(meters[2].ratio).toBe(0.5)
  })
  test('summary line: exhausted first, else tightest remaining, else unlimited', () => {
    const k = (kind: string, o: object) => ({ kind, unlimited: false, monthlyAllowance: 100, usedThisMonth: 0, remaining: 100, reserved: 0, ...o })
    const line = (kinds: object[]) => usageSummaryLine(usageMeters({ kinds } as unknown as ShareUsageDto))
    expect(line([])).toBeNull()
    expect(line([k('weird', {})])).toBeNull()
    // 已用完优先于剩余比例更低的项；多个已用完取云端顺序里的第一个。
    const exhausted = line([k('swarm', { remaining: 5 }), k('download', { remaining: 0 }), k('upload', { remaining: 0 })])
    expect(exhausted).toMatchObject({ kind: 'exhausted', meter: { kind: 'download' } })
    // 否则剩余比例最低的限额项；不限项不参与。
    const tight = line([k('swarm', { unlimited: true }), k('download', { remaining: 80 }), k('upload', { remaining: 20 })])
    expect(tight).toMatchObject({ kind: 'remaining', meter: { kind: 'upload' } })
    // 比例按剩余 / 月赠送，而不是绝对字节。
    const ratio = line([k('swarm', { monthlyAllowance: 1000, remaining: 300 }), k('download', { monthlyAllowance: 10, remaining: 2 })])
    expect(ratio).toMatchObject({ kind: 'remaining', meter: { kind: 'download' } })
    // 平手保持云端顺序；月赠送为 0 但仍有剩余（流量包）按 100% 计，不抢占。
    expect(line([k('swarm', { remaining: 50 }), k('download', { remaining: 50 })])).toMatchObject({ meter: { kind: 'swarm' } })
    expect(line([k('swarm', { monthlyAllowance: 0, remaining: 7 }), k('download', { remaining: 90 })])).toMatchObject({ meter: { kind: 'download' } })
    // 剩余超过月赠送（结转 / 流量包）夹到 100%。
    expect(line([k('swarm', { remaining: 500 }), k('download', { remaining: 99 })])).toMatchObject({ meter: { kind: 'download' } })
    // 全部不限。
    expect(line([k('swarm', { unlimited: true }), k('download', { unlimited: true }), k('upload', { unlimited: true })])).toEqual({ kind: 'unlimited' })
  })
})

describe('takedown', () => {
  const taken = share({ id: 'taken', status: 'takedown', takedownReason: '侵权' })

  test('display status and actions: only delete applies', () => {
    expect(shareDisplayStatus(taken, Date.now())).toBe('takedown')
    expect(shareActionApplies('start', taken)).toBe(false)
    expect(shareActionApplies('stop', taken)).toBe(false)
    expect(shareActionApplies('delete', taken)).toBe(true)
    expect(shareActionApplies('stop', share({ id: 'a', status: 'active' }))).toBe(true)
    expect(planShareBatch([taken])).toEqual({ start: [], stop: [], remove: [taken] })
  })

  test('appeal state', () => {
    expect(appealState(taken)).toBe('canAppeal')
    expect(appealState({ ...taken, appeal: { status: 'pending', createdAt: '2026-01-01T00:00:00Z' } })).toBe('pending')
    expect(appealState({ ...taken, appeal: { status: 'rejected', createdAt: '2026-01-01T00:00:00Z' } })).toBe('rejected')
    expect(appealState({ ...taken, appeal: { status: 'approved', createdAt: '2026-01-01T00:00:00Z' } })).toBe('none')
    expect(appealState(share({ id: 'a', status: 'active' }))).toBe('none')
  })
})
