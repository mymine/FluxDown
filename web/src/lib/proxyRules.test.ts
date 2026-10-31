import { describe, expect, test } from 'bun:test'
import {
  displayEndpoint,
  enabledRuleCount,
  encodeProxyRules,
  issueText,
  manualEndpoint,
  moved,
  parseProxyRules,
  routeLines,
  upserted,
} from './proxyRules'
import type { ProxyRule } from './proxyRules'

const t = (key: string, params?: Record<string, unknown>) =>
  params ? `${key}(${Object.entries(params).map(([k, v]) => `${k}=${String(v)}`).join(',')})` : key

const rule = (id: string): ProxyRule => ({ id, enabled: true, name: '', match: 'domain', pattern: 'a.com', action: 'direct', proxy: '' })

describe('parseProxyRules', () => {
  test('invalid input is empty', () => {
    for (const raw of ['', '  ', 'nope', '{}', '42', '"[]"', undefined]) expect(parseProxyRules(raw)).toEqual([])
  })
  test('non-object elements are skipped', () => {
    expect(parseProxyRules('[1,null,[],"x",{"id":"a"}]').map((r) => r.id)).toEqual(['a'])
  })
  test('defaults for missing, wrong-typed and unknown values', () => {
    expect(parseProxyRules('[{}]')[0]).toEqual({ id: '', enabled: true, name: '', match: 'domain', pattern: '', action: 'direct', proxy: '' })
    expect(parseProxyRules('[{"id":1,"enabled":"no","name":2,"match":"glob","pattern":3,"action":"tor","proxy":4}]')[0]).toEqual({
      id: '', enabled: true, name: '', match: 'domain', pattern: '', action: 'direct', proxy: '',
    })
    const full = parseProxyRules('[{"id":"x","enabled":false,"match":"regex","action":"custom","proxy":"http://h:1"}]')[0]
    expect(full).toMatchObject({ enabled: false, match: 'regex', action: 'custom', proxy: 'http://h:1' })
  })
  test('round trip keeps parsed values', () => {
    const list = parseProxyRules('[{"id":"a","enabled":false,"name":"N","match":"regex","pattern":"x\\\\.y","action":"manual","proxy":""},{"id":"b"}]')
    expect(parseProxyRules(encodeProxyRules(list))).toEqual(list)
    expect(Object.keys(JSON.parse(encodeProxyRules(list))[0]).length).toBe(7)
  })
  test('enabled count', () => {
    expect(enabledRuleCount('[{"id":"a"},{"id":"b","enabled":false},5]')).toBe(1)
    expect(enabledRuleCount('')).toBe(0)
  })
})

describe('list helpers', () => {
  const list = [rule('a'), rule('b'), rule('c')]
  test('moved boundaries', () => {
    expect(moved(list, 'a', -1)).toBeNull()
    expect(moved(list, 'c', 1)).toBeNull()
    expect(moved(list, 'zz', 1)).toBeNull()
    expect(moved(list, 'b', -1)?.map((r) => r.id)).toEqual(['b', 'a', 'c'])
    expect(moved(list, 'b', 1)?.map((r) => r.id)).toEqual(['a', 'c', 'b'])
  })
  test('upserted overwrites or appends', () => {
    expect(upserted(list, { ...rule('b'), name: 'X' })[1]?.name).toBe('X')
    expect(upserted(list, rule('d')).map((r) => r.id)).toEqual(['a', 'b', 'c', 'd'])
  })
})

describe('endpoints', () => {
  test('displayEndpoint strips credentials', () => {
    expect(displayEndpoint('socks5://user:p@ss@127.0.0.1:7891/')).toBe('socks5://127.0.0.1:7891')
    expect(displayEndpoint('http://h.example:8080')).toBe('http://h.example:8080')
    expect(displayEndpoint('u@h:1')).toBe('h:1')
  })
  test('manualEndpoint', () => {
    expect(manualEndpoint('http', '127.0.0.1', '7890')).toBe('http://127.0.0.1:7890')
    expect(manualEndpoint('http', '::1', '1')).toBe('http://[::1]:1')
    expect(manualEndpoint('http', '', '1')).toBe('')
    expect(manualEndpoint('http', 'h', '0')).toBe('')
    expect(manualEndpoint('http', 'h', '70000')).toBe('')
  })
})

describe('routeLines', () => {
  const base = { issues: [], endpoint: '' }
  test('empty reason is null', () => {
    expect(routeLines(t, { ...base, reason: '' })).toBeNull()
  })
  test('matched rule with and without name', () => {
    expect(routeLines(t, { ...base, reason: 'direct', matched: { index: 1, id: 'a', name: '' } })).toEqual({
      title: { text: 'proxyRouteRuleHit(index=2)', tone: 'accent' },
      detail: { text: 'proxyRouteDirect', tone: 'normal' },
    })
    expect(routeLines(t, { ...base, reason: 'proxy', endpoint: 'http://h:1', matched: { index: 0, id: 'a', name: 'GH' } })).toEqual({
      title: { text: 'proxyRouteRuleHitNamed(index=1,name=GH)', tone: 'accent' },
      detail: { text: 'proxyRouteProxy(endpoint=http://h:1)', tone: 'normal' },
    })
  })
  test('every reason', () => {
    const expected: Record<string, [string, 'warning' | 'normal', boolean]> = {
      direct: ['proxyRouteDirect', 'normal', true],
      systemUndetected: ['proxyRouteSystemUndetected', 'warning', true],
      manualIncomplete: ['proxyRouteManualIncomplete', 'warning', true],
      modeNone: ['proxyRouteModeNone', 'normal', true],
      bypass: ['proxyRouteBypass', 'normal', true],
      pac: ['proxyRouteModePac', 'normal', true],
      auto: ['proxyRouteModeAuto', 'normal', true],
      loopback: ['proxyRouteLoopback', 'normal', false],
      unsupported: ['proxyRouteUnsupported', 'normal', false],
      invalidUrl: ['proxyRouteInvalidUrl', 'warning', false],
    }
    for (const [reason, [text, tone, hasTitle]] of Object.entries(expected)) {
      const lines = routeLines(t, { ...base, reason, matched: null })
      expect(lines?.detail).toEqual({ text, tone })
      expect(lines?.title).toEqual(hasTitle ? { text: 'proxyRouteNoRule', tone: 'normal' } : undefined)
    }
  })
})

describe('issueText', () => {
  const issue = (code: string, detail = '') => ({ index: 0, field: 'pattern', code, detail })
  test('mapping', () => {
    expect(issueText(t, issue('emptyPattern'))).toBe('proxyRuleErrEmpty')
    expect(issueText(t, issue('invalidDomain', 'a:b'))).toBe('proxyRuleErrDomain(detail=a:b)')
    expect(issueText(t, issue('invalidRegex', 'bad'))).toBe('proxyRuleErrRegex(detail=bad)')
    expect(issueText(t, issue('regexTooLarge'))).toBe('proxyRuleErrRegex(detail=1 MiB)')
    expect(issueText(t, issue('tooLong', '64'))).toBe('proxyRuleErrTooLong(max=64)')
    expect(issueText(t, issue('invalidProxy', 'missing port'))).toBe('proxyRuleErrProxy(detail=missing port)')
    expect(issueText(t, issue('tooManyRules', '200'))).toBe('proxyRuleErrGeneric(detail=tooManyRules)')
  })
})
