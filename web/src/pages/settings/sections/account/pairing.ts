// 局域网配对的纯规则：发现结果 → 地址、配对码归一化、已配对过滤。

import type { LinkDeviceInfo, LinkDiscoveredPeer } from '../../../../lib/rpc'

export const PAIRING_CODE_LENGTH = 6

/** 发现的对端 → `agent.link.pairBegin` 接受的 `host:port`（IPv6 加方括号）。 */
export function peerAddress(peer: Pick<LinkDiscoveredPeer, 'host' | 'port'>): string {
  const host = peer.host.includes(':') && !peer.host.startsWith('[') ? `[${peer.host}]` : peer.host
  return `${host}:${peer.port}`
}

/** 配对码只保留数字并截断到 6 位。 */
export function normalizePairingCode(raw: string): string {
  return raw.replace(/\D/g, '').slice(0, PAIRING_CODE_LENGTH)
}

export function isPairingCodeComplete(code: string): boolean {
  return code.length === PAIRING_CODE_LENGTH
}

/** 手动地址只做非空检查：`host`、`host:port`、`http(s)://…` 的解析与端口校验由 agent 负责（缺失端口有默认值）。 */
export function isManualAddressValid(address: string): boolean {
  return address.trim() !== '' && !/\s/.test(address.trim())
}

/** 发现列表里去掉已配对的设备（按指纹）。已被对端解除（revoked）的不算已配对，保留以便重新配对；指纹未知的 mDNS 条目保留。 */
export function unpairedPeers(
  peers: readonly LinkDiscoveredPeer[],
  paired: readonly Pick<LinkDeviceInfo, 'fingerprint' | 'revoked'>[],
): LinkDiscoveredPeer[] {
  const known = new Set(paired.filter((device) => !device.revoked).map((device) => device.fingerprint))
  return peers.filter((peer) => !peer.fingerprint || !known.has(peer.fingerprint))
}

/** 本机码续期的最小间隔（与移动端 `MIN_REFRESH_DELAY_MS` 一致）：时钟偏差时避免续码风暴。 */
export const MIN_RENEW_DELAY_MS = 3000

/**
 * 配对码 / 入站请求的本机截止时刻：以收到 DTO 的本机时刻 + `ttlMs` 计算（免疫主机与客户端的时钟偏差）；
 * 旧主机没有 `ttlMs` 时才回退主机绝对时间 `expiresAtUnixMs`。
 */
export function localDeadlineMs(dto: { ttlMs?: number; expiresAtUnixMs: number }, receivedAtMs: number): number {
  return dto.ttlMs === undefined ? dto.expiresAtUnixMs : receivedAtMs + dto.ttlMs
}

/** 距截止的剩余秒数（向上取整，不为负）。 */
export function secondsUntil(deadlineMs: number, nowMs: number): number {
  return Math.max(0, Math.ceil((deadlineMs - nowMs) / 1000))
}

/** 续码定时器延迟：至少 `MIN_RENEW_DELAY_MS`。 */
export function renewDelayMs(deadlineMs: number, nowMs: number): number {
  return Math.max(MIN_RENEW_DELAY_MS, deadlineMs - nowMs)
}
