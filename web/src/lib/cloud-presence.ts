import type { CloudConnectionDto, CloudDevice, Entitlements } from './rpc'

export function cloudPresenceKnown(connection: CloudConnectionDto | undefined, localReady: boolean): boolean {
  return localReady && connection?.state === 'connected'
}

export function cloudConnectionLabelKey(connection: CloudConnectionDto | undefined, localReady: boolean): string {
  if (!localReady) return 'localServiceDisconnected'
  switch (connection?.state) {
    case 'connected': return 'cloudConnectionConnected'
    case 'connecting': return 'cloudConnectionConnecting'
    case 'reconnecting': return 'cloudConnectionReconnecting'
    default: return 'cloudConnectionDisconnected'
  }
}

export function devicePresenceKey(device: Pick<CloudDevice, 'isOnline'>, presenceKnown: boolean): string {
  return presenceKnown ? (device.isOnline ? 'deviceOnline' : 'deviceOffline') : 'devicePresenceUnknown'
}

/** 在线设备数（含本机）。 */
export function onlineDeviceCount(devices: readonly Pick<CloudDevice, 'isOnline'>[]): number {
  return devices.filter((device) => device.isOnline).length
}

/** 套餐的同时在线设备上限（`maxOnlineDevices`）；缺失、非正整数或 0（不限）为 null。 */
export function maxOnlineDevices(entitlements: Entitlements | undefined): number | null {
  const value = entitlements?.maxOnlineDevices
  return typeof value === 'number' && Number.isInteger(value) && value > 0 ? value : null
}

/** 因同时在线设备数已达上限而在等待名额（连接未建立，原因为 onlineDeviceLimit）。 */
export function onlineLimitWaiting(connection: CloudConnectionDto | undefined, localReady: boolean): boolean {
  return localReady && connection?.state !== 'connected' && connection?.lastErrorReason === 'onlineDeviceLimit'
}

/** 能否让该设备下线：非本机、列表标记在线，且在线状态可信（已连接）或正在等待名额。 */
export function canDisconnectDevice(
  device: Pick<CloudDevice, 'isOnline' | 'isCurrent'>,
  presenceKnown: boolean,
  waiting: boolean,
): boolean {
  return !device.isCurrent && device.isOnline && (presenceKnown || waiting)
}
