import { describe, expect, test } from 'bun:test'
import type { CloudConnectionDto } from './rpc'
import {
  canDisconnectDevice,
  cloudConnectionLabelKey,
  cloudPresenceKnown,
  devicePresenceKey,
  maxOnlineDevices,
  onlineDeviceCount,
  onlineLimitWaiting,
} from './cloud-presence'

function connection(state: CloudConnectionDto['state']): CloudConnectionDto {
  return { state }
}

describe('cloud presence freshness', () => {
  test('only a live cloud connection through a ready agent makes presence trustworthy', () => {
    for (const state of ['disconnected', 'connecting', 'reconnecting'] as const) {
      expect(cloudPresenceKnown(connection(state), true)).toBe(false)
    }
    expect(cloudPresenceKnown(undefined, true)).toBe(false)
    expect(cloudPresenceKnown(connection('connected'), false)).toBe(false)
    expect(cloudPresenceKnown(connection('connected'), true)).toBe(true)
  })

  test('cached online and offline records are both unknown after disconnect', () => {
    expect(devicePresenceKey({ isOnline: true }, false)).toBe('devicePresenceUnknown')
    expect(devicePresenceKey({ isOnline: false }, false)).toBe('devicePresenceUnknown')
    expect(devicePresenceKey({ isOnline: true }, true)).toBe('deviceOnline')
    expect(devicePresenceKey({ isOnline: false }, true)).toBe('deviceOffline')
  })

  test('online count includes the current device', () => {
    expect(onlineDeviceCount([{ isOnline: true }, { isOnline: true }, { isOnline: false }])).toBe(2)
  })

  test('maxOnlineDevices accepts only positive integers', () => {
    expect(maxOnlineDevices({ maxOnlineDevices: 2 })).toBe(2)
    expect(maxOnlineDevices({ maxOnlineDevices: 0 })).toBeNull()
    expect(maxOnlineDevices({ maxOnlineDevices: '2' })).toBeNull()
    expect(maxOnlineDevices({})).toBeNull()
    expect(maxOnlineDevices(undefined)).toBeNull()
  })

  test('waiting state and disconnect eligibility', () => {
    expect(onlineLimitWaiting({ state: 'reconnecting', lastErrorReason: 'onlineDeviceLimit' }, true)).toBe(true)
    expect(onlineLimitWaiting({ state: 'connected', lastErrorReason: 'onlineDeviceLimit' }, true)).toBe(false)
    expect(onlineLimitWaiting({ state: 'reconnecting', lastErrorReason: 'onlineDeviceLimit' }, false)).toBe(false)
    expect(canDisconnectDevice({ isOnline: true, isCurrent: false }, false, true)).toBe(true)
    expect(canDisconnectDevice({ isOnline: true, isCurrent: false }, false, false)).toBe(false)
    expect(canDisconnectDevice({ isOnline: true, isCurrent: true }, true, false)).toBe(false)
    expect(canDisconnectDevice({ isOnline: false, isCurrent: false }, true, false)).toBe(false)
  })

  test('local disconnection takes precedence over a cached connected snapshot', () => {
    expect(cloudConnectionLabelKey(connection('connected'), false)).toBe('localServiceDisconnected')
    expect(cloudConnectionLabelKey(connection('connected'), true)).toBe('cloudConnectionConnected')
    expect(cloudConnectionLabelKey(connection('connecting'), true)).toBe('cloudConnectionConnecting')
    expect(cloudConnectionLabelKey(connection('reconnecting'), true)).toBe('cloudConnectionReconnecting')
    expect(cloudConnectionLabelKey(undefined, true)).toBe('cloudConnectionDisconnected')
  })
})
