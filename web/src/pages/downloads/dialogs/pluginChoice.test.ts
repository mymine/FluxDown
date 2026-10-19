import { describe, expect, test } from 'bun:test'
import type { PluginCandidateDto, SelectionOutcome } from '../../../lib/rpc'
import { buildPluginOutcome, candidateTitle, defaultPluginIdentity } from './pluginChoice'

const candidates: PluginCandidateDto[] = [
  { identity: 'a.plugin', name: 'A', version: '1.0.0', description: '' },
  { identity: 'b.plugin', name: '', version: '2.0.0', description: 'desc' },
]
const request = (defaultChoice: SelectionOutcome) => ({ defaultChoice })

describe('pluginChoice', () => {
  test('默认候选取 defaultChoice', () => {
    expect(defaultPluginIdentity(request({ kind: 'plugin', identity: 'b.plugin', remember: false }), candidates)).toBe('b.plugin')
  })
  test('defaultChoice 无效时回退第一个候选', () => {
    expect(defaultPluginIdentity(request({ kind: 'plugin', identity: 'x', remember: false }), candidates)).toBe('a.plugin')
    expect(defaultPluginIdentity(request({ kind: 'cancelled' }), candidates)).toBe('a.plugin')
    expect(defaultPluginIdentity(request({ kind: 'cancelled' }), [])).toBe('')
  })
  test('构造回答', () => {
    expect(buildPluginOutcome('a.plugin', true, candidates)).toEqual({ kind: 'plugin', identity: 'a.plugin', remember: true })
    expect(buildPluginOutcome('zzz', false, candidates)).toBeNull()
  })
  test('显示名回退 identity', () => {
    expect(candidateTitle(candidates[0]!)).toBe('A')
    expect(candidateTitle(candidates[1]!)).toBe('b.plugin')
  })
})
