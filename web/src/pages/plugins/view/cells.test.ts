import { describe, expect, test } from 'bun:test'
import { formatCell } from './cells'

describe('formatCell', () => {
  test('bytes / speed', () => {
    expect(formatCell('bytes', 1536)).toEqual({ text: '1.5 KB' })
    expect(formatCell('speed', 1048576)).toEqual({ text: '1.0 MB/s' })
    expect(formatCell('bytes', null)).toEqual({ text: '—' })
  })
  test('percent clamps 0..1', () => {
    expect(formatCell('percent', 0.256).text).toBe('26%')
    expect(formatCell('percent', 3).text).toBe('100%')
    expect(formatCell('percent', -1).text).toBe('0%')
  })
  test('progress carries a fraction', () => {
    expect(formatCell('progress', 0.5)).toEqual({ text: '50%', fraction: 0.5 })
    expect(formatCell('progress', 'x')).toEqual({ text: '—' })
  })
  test('time takes unix ms', () => {
    expect(formatCell('time', 0).text).toBe('—')
    expect(/^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}$/.test(formatCell('time', 1_700_000_000_000).text)).toBe(true)
  })
  test('badge accepts string or {text,tone}', () => {
    expect(formatCell('badge', 'ok')).toEqual({ text: 'ok', tone: 'neutral' })
    expect(formatCell('badge', { text: 'bad', tone: 'danger' })).toEqual({ text: 'bad', tone: 'danger' })
    expect(formatCell('badge', { text: 'x', tone: 'nope' })).toEqual({ text: 'x', tone: 'neutral' })
  })
  test('text stringifies', () => {
    expect(formatCell('text', 12)).toEqual({ text: '12' })
    expect(formatCell('text', null)).toEqual({ text: '' })
  })
})
