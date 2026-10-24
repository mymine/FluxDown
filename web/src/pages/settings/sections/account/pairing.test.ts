import { describe, expect, test } from 'bun:test'
import type { LinkDiscoveredPeer } from '../../../../lib/rpc'
import { isManualAddressValid, isPairingCodeComplete, localDeadlineMs, MIN_RENEW_DELAY_MS, normalizePairingCode, peerAddress, renewDelayMs, secondsUntil, unpairedPeers } from './pairing'

const peer = (over: Partial<LinkDiscoveredPeer>): LinkDiscoveredPeer => ({ name: 'p', host: '10.0.0.2', port: 17800, source: 'mdns', ...over })

describe('局域网配对规则', () => {
  test('发现对端地址：IPv6 加方括号，已带括号不重复', () => {
    expect(peerAddress(peer({}))).toBe('10.0.0.2:17800')
    expect(peerAddress(peer({ host: 'fe80::1', port: 9 }))).toBe('[fe80::1]:9')
    expect(peerAddress(peer({ host: '[::1]', port: 9 }))).toBe('[::1]:9')
  })

  test('配对码只留数字并截断 6 位', () => {
    expect(normalizePairingCode('12a3-45 678')).toBe('123456')
    expect(isPairingCodeComplete('12345')).toBe(false)
    expect(isPairingCodeComplete('123456')).toBe(true)
  })

  test('手动地址：https 与带路径的写法放行，空白拒绝', () => {
    expect(isManualAddressValid('https://nas.example.com/fluxdown')).toBe(true)
    expect(isManualAddressValid('192.168.1.5')).toBe(true)
    expect(isManualAddressValid('  ')).toBe(false)
    expect(isManualAddressValid('a b')).toBe(false)
  })

  test('已配对设备不再出现在发现列表；指纹未知保留', () => {
    const list = [peer({ fingerprint: 'a' }), peer({ fingerprint: 'b' }), peer({})]
    expect(unpairedPeers(list, [{ fingerprint: 'a', revoked: false }]).map((item) => item.fingerprint)).toEqual(['b', undefined])
  })

  test('被对端解除（revoked）的设备仍可重新配对', () => {
    const list = [peer({ fingerprint: 'a' }), peer({ fingerprint: 'b' })]
    expect(unpairedPeers(list, [{ fingerprint: 'a', revoked: true }, { fingerprint: 'b', revoked: false }]).map((item) => item.fingerprint)).toEqual(['a'])
  })

  test('截止时刻以本机收到时刻 + ttlMs 为准，缺失才回退主机绝对时间', () => {
    // 主机时钟比本机快 10 分钟：绝对时间已过去很久，ttl 仍给出完整窗口。
    const hostAbsolute = 1_000_000
    expect(localDeadlineMs({ ttlMs: 30_000, expiresAtUnixMs: hostAbsolute }, 5_000_000)).toBe(5_030_000)
    expect(localDeadlineMs({ expiresAtUnixMs: hostAbsolute }, 5_000_000)).toBe(hostAbsolute)
  })

  test('剩余秒数向上取整且不为负；续码延迟有 3s 下限', () => {
    expect(secondsUntil(10_001, 10_000)).toBe(1)
    expect(secondsUntil(9_000, 10_000)).toBe(0)
    expect(renewDelayMs(10_000, 20_000)).toBe(MIN_RENEW_DELAY_MS)
    expect(renewDelayMs(100_000, 20_000)).toBe(80_000)
  })
})
