// 多设备协同的纯逻辑：其他设备、远程任务可见性与按目标设备计数（契约 §5）。

import { effectivePathStyle } from '../../../lib/rpc'
import type { AgentSessionDto, CloudDevice, LinkDeviceInfo, PathStyle, RemoteTaskDto } from '../../../lib/rpc'

/** 本机在 FluxCloud 的 deviceId：优先会话，其次设备列表里的 `isCurrent`。 */
export function currentDeviceId(
  session: Pick<AgentSessionDto, 'device'> | null,
  cloudDevices: readonly CloudDevice[],
): string | null {
  return session?.device.deviceId ?? cloudDevices.find((device) => device.isCurrent)?.deviceId ?? null
}

/** 「其他设备」：账号设备去掉本机。 */
export function otherDevices(cloudDevices: readonly CloudDevice[]): CloudDevice[] {
  return cloudDevices.filter((device) => !device.isCurrent)
}

/** 合并后的「其他设备」条目：云条目（可挂接 link）或独立 link 条目。 */
export interface DeviceEntry {
  /** 云条目 = deviceId；独立 link = 指纹。 */
  id: string
  kind: 'cloud' | 'link'
  /** 已消歧的显示名。 */
  name: string
  /** 任一通道在线 → true；否则任一未知 → null；否则 false。 */
  online: boolean | null
  defaultSaveDir: string | null
  pathStyle: PathStyle | null
  /** 挂接的直连指纹（独立 link 条目 = 自身指纹）。 */
  linkFingerprint: string | null
  /** 直连在线状态；未挂接 / 无直连为 null。 */
  linkOnline: boolean | null
}

function shortCode(id: string): string {
  return id.replace(/[^A-Za-z0-9]/g, '').slice(-4)
}

function disambiguate(entries: DeviceEntry[]): void {
  const counts = new Map<string, number>()
  for (const entry of entries) {
    const key = entry.name.toLowerCase()
    counts.set(key, (counts.get(key) ?? 0) + 1)
  }
  for (const entry of entries) {
    if (entry.name === '') entry.name = shortCode(entry.id)
    else if ((counts.get(entry.name.toLowerCase()) ?? 0) > 1) entry.name = `${entry.name} · ${shortCode(entry.id)}`
  }
}

function mergeOnline(cloud: boolean | null, link: boolean | null): boolean | null {
  if (cloud === true || link === true) return true
  if (cloud === null || link === null) return null
  return false
}

/**
 * 合并云设备名册与直连名册（契约「其他设备」合并规则）：
 * 云条目按 id 去重；非 revoked 的 link 按指纹去重，`cloudDeviceId` 命中尚未挂接的云条目则挂接，
 * 否则成为独立 link 条目；同名消歧在合并后的整表上做。
 */
export function mergeDevices(
  cloudDevices: readonly CloudDevice[],
  linkedDevices: readonly LinkDeviceInfo[],
  presence: { cloud: boolean; local: boolean },
): DeviceEntry[] {
  const cloudEntries = new Map<string, DeviceEntry>()
  for (const device of otherDevices(cloudDevices)) {
    if (device.deviceId === '' || cloudEntries.has(device.deviceId)) continue
    cloudEntries.set(device.deviceId, {
      id: device.deviceId,
      kind: 'cloud',
      name: device.name.trim(),
      online: presence.cloud ? device.isOnline : null,
      defaultSaveDir: device.defaultSaveDir?.trim() || null,
      pathStyle: effectivePathStyle(device),
      linkFingerprint: null,
      linkOnline: null,
    })
  }
  const standalone: DeviceEntry[] = []
  const seen = new Set<string>()
  for (const device of linkedDevices) {
    if (device.fingerprint === '' || device.revoked || seen.has(device.fingerprint)) continue
    seen.add(device.fingerprint)
    const linkOnline = presence.local ? device.online : null
    const linkDir = device.defaultSaveDir?.trim() || null
    const linkStyle = effectivePathStyle(device)
    const attach = device.cloudDeviceId ? cloudEntries.get(device.cloudDeviceId) : undefined
    if (attach && attach.linkFingerprint === null) {
      attach.linkFingerprint = device.fingerprint
      attach.linkOnline = linkOnline
      attach.online = mergeOnline(attach.online, linkOnline)
      attach.defaultSaveDir ??= linkDir
      attach.pathStyle ??= linkStyle
      continue
    }
    standalone.push({
      id: device.fingerprint,
      kind: 'link',
      name: device.name.trim(),
      online: linkOnline,
      defaultSaveDir: linkDir,
      pathStyle: linkStyle,
      linkFingerprint: device.fingerprint,
      linkOnline,
    })
  }
  const entries = [...cloudEntries.values(), ...standalone]
  disambiguate(entries)
  return entries
}

/** 目标为本机的远程任务在本地已有真实任务，不再作为镜像行显示。 */
export function visibleRemoteTasks(tasks: readonly RemoteTaskDto[], currentId: string | null): RemoteTaskDto[] {
  return currentId === null ? tasks.slice() : tasks.filter((task) => task.toDevice !== currentId)
}

/** 远程任务按目标设备计数。 */
export function countByTargetDevice(tasks: readonly Pick<RemoteTaskDto, 'toDevice'>[]): Map<string, number> {
  const counts = new Map<string, number>()
  for (const task of tasks) counts.set(task.toDevice, (counts.get(task.toDevice) ?? 0) + 1)
  return counts
}
