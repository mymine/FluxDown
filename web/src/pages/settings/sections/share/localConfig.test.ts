import { describe, expect, test } from 'bun:test'
import { formatAddrList, formatDailyCap, parseAddrList, parseDailyCap, parsePort, portmapperChoice, portmapperWire, rateFromWire, rateToWire } from './localConfig'

describe('port', () => {
  test('empty is automatic; valid range accepted; garbage rejected', () => {
    expect(parsePort('')).toBeNull()
    expect(parsePort('  ')).toBeNull()
    expect(parsePort('443')).toBe(443)
    expect(parsePort(' 65535 ')).toBe(65535)
    expect(parsePort('0')).toBeUndefined()
    expect(parsePort('65536')).toBeUndefined()
    expect(parsePort('12a')).toBeUndefined()
    expect(parsePort('-1')).toBeUndefined()
    expect(parsePort('123456')).toBeUndefined()
  })
})

describe('extra addresses', () => {
  test('split on commas / whitespace / semicolons, dedupe, keep order', () => {
    expect(parseAddrList('1.2.3.4:5, [::1]:6\n1.2.3.4:5;host:7')).toEqual(['1.2.3.4:5', '[::1]:6', 'host:7'])
    expect(parseAddrList('  ')).toEqual([])
    expect(formatAddrList(['a:1', 'b:2'])).toBe('a:1, b:2')
  })
})

describe('rate limit', () => {
  test('MiB/s ↔ bytes/s; zero and null mean unlimited', () => {
    expect(rateToWire(0)).toBeNull()
    expect(rateToWire(-3)).toBeNull()
    expect(rateToWire(Number.NaN)).toBeNull()
    expect(rateToWire(2)).toBe(2 * 1024 * 1024)
    expect(rateToWire(0.5)).toBe(524_288)
    expect(rateFromWire(null)).toBe(0)
    expect(rateFromWire(0)).toBe(0)
    expect(rateFromWire(3 * 1024 * 1024)).toBe(3)
  })
})

describe('portmapper', () => {
  test('tri-state round trip', () => {
    expect(portmapperChoice(null)).toBe('auto')
    expect(portmapperChoice(true)).toBe('on')
    expect(portmapperChoice(false)).toBe('off')
    expect(portmapperWire('auto')).toBeNull()
    expect(portmapperWire('on')).toBe(true)
    expect(portmapperWire('off')).toBe(false)
  })
})

describe('daily cap', () => {
  test('empty follows cloud, zero is unlimited, GB converts to bytes', () => {
    expect(parseDailyCap('')).toBeNull()
    expect(parseDailyCap(' 0 ')).toBe(0)
    expect(parseDailyCap('2')).toBe(2 * 1024 ** 3)
    expect(parseDailyCap('0.5')).toBe(512 * 1024 ** 2)
    expect(parseDailyCap('-1')).toBeUndefined()
    expect(parseDailyCap('abc')).toBeUndefined()
    expect(formatDailyCap(null)).toBe('')
    expect(formatDailyCap(undefined)).toBe('')
    expect(formatDailyCap(0)).toBe('0')
    expect(formatDailyCap(1.5 * 1024 ** 3)).toBe('1.5')
  })
})
