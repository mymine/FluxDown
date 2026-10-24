import { describe, expect, test } from 'bun:test'
import { formatRatio, isFieldValid, loadSeedLimit, parseMinutes, parseRatio, seedLimitToSend } from './seedLimits'

describe('seed limit form', () => {
  test('分享率按两位小数显示与解析', () => {
    expect(formatRatio(1500)).toBe('1.50')
    expect(formatRatio(1000)).toBe('1.00')
    expect(parseRatio('1.50')).toBe(1500)
    expect(parseRatio(' 2 ')).toBe(2000)
    expect(parseRatio('0.0005')).toBe(1)
    expect(parseRatio('0')).toBeNull()
    expect(parseRatio('-1')).toBeNull()
    expect(parseRatio('abc')).toBeNull()
    expect(parseRatio('')).toBeNull()
  })

  test('分钟只接受正整数', () => {
    expect(parseMinutes('90')).toBe(90)
    expect(parseMinutes('0')).toBeNull()
    expect(parseMinutes('1.5')).toBeNull()
    expect(parseMinutes('')).toBeNull()
  })

  test('加载：缺省 / -2 跟随全局，-1 与 0 不限制，正数自定义；非自定义显示 qBit 默认值', () => {
    expect(loadSeedLimit('ratio', undefined)).toEqual({ mode: 'global', text: '1.00' })
    expect(loadSeedLimit('ratio', -2)).toEqual({ mode: 'global', text: '1.00' })
    expect(loadSeedLimit('ratio', -1)).toEqual({ mode: 'unlimited', text: '1.00' })
    expect(loadSeedLimit('ratio', 0)).toEqual({ mode: 'unlimited', text: '1.00' })
    expect(loadSeedLimit('ratio', 1500)).toEqual({ mode: 'custom', text: '1.50' })
    expect(loadSeedLimit('minutes', -2)).toEqual({ mode: 'global', text: '1440' })
    expect(loadSeedLimit('minutes', 90)).toEqual({ mode: 'custom', text: '90' })
  })

  test('保存：模式映射哨兵，自定义换算千分比', () => {
    const initial = loadSeedLimit('ratio', -2)
    expect(seedLimitToSend('ratio', { mode: 'unlimited', text: '1.00' }, initial, -2)).toBe(-1)
    expect(seedLimitToSend('ratio', { mode: 'custom', text: '1.50' }, initial, -2)).toBe(1500)
    expect(seedLimitToSend('ratio', { mode: 'custom', text: 'x' }, initial, -2)).toBeNull()
    expect(seedLimitToSend('minutes', { mode: 'global', text: '1440' }, loadSeedLimit('minutes', 90), 90)).toBe(-2)
    expect(seedLimitToSend('minutes', { mode: 'custom', text: '120' }, loadSeedLimit('minutes', -1), -1)).toBe(120)
  })

  test('未改动原样回传基线（含 0 与取整前的千分比）', () => {
    for (const [kind, raw] of [
      ['ratio', 0],
      ['ratio', 1234],
      ['ratio', -1],
      ['ratio', -2],
      ['minutes', 0],
      ['minutes', 90],
    ] as const) {
      const initial = loadSeedLimit(kind, raw)
      expect(seedLimitToSend(kind, { ...initial }, initial, raw)).toBe(raw)
    }
    const initial = loadSeedLimit('ratio', undefined)
    expect(seedLimitToSend('ratio', { ...initial }, initial, undefined)).toBe(-2)
  })

  test('只有自定义模式校验文本', () => {
    expect(isFieldValid('ratio', { mode: 'global', text: 'zzz' })).toBe(true)
    expect(isFieldValid('ratio', { mode: 'custom', text: 'zzz' })).toBe(false)
    expect(isFieldValid('minutes', { mode: 'custom', text: '30' })).toBe(true)
  })
})
