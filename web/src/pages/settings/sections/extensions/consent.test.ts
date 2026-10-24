import { describe, expect, test } from 'bun:test'
import type { MarketEntryDto, PluginDto } from '../../../../lib/rpc/protocol'
import { consentMatches, hostsToConfirm, permissionKeys, postInstallConsent, riskOf } from './logic'

interface Fixture {
  risk: { name: string; permissions: string[]; risk: string }[]
  matches: {
    name: string
    confirmed: { permissions: string[]; networkHosts: string[] }
    pending: { permissions: string[]; networkHosts: string[] }
    matches: boolean
  }[]
}

// 与 crates/extensions/tests/fixtures 共用同一份用例（运行期读取：Docker 构建上下文不含 crates/）。
declare const Bun: { file(path: URL): { json(): Promise<unknown> } }
const fixture = (await Bun.file(new URL('../../../../../../crates/extensions/tests/fixtures/consent_cases.json', import.meta.url)).json()) as Fixture

function plugin(patch: Partial<PluginDto>): PluginDto {
  return { identity: 'a@b', consentPending: [], consentPendingHosts: [], permissions: [], networkHosts: [], ...patch } as PluginDto
}

describe('riskOf (shared fixtures)', () => {
  for (const item of fixture.risk) {
    test(item.name, () => expect(riskOf(item.permissions)).toBe(item.risk))
  }
})

describe('consentMatches (shared fixtures)', () => {
  for (const item of fixture.matches) {
    test(item.name, () => expect(consentMatches(item.confirmed, item.pending)).toBe(item.matches))
  }
})

describe('postInstallConsent', () => {
  test('无待授权 → none', () => {
    expect(postInstallConsent(plugin({}), null)).toBe('none')
  })
  test('无预确认 → dialog（zip / dev）', () => {
    expect(postInstallConsent(plugin({ consentPending: ['tasks'] }), null)).toBe('dialog')
  })
  test('市场预确认与 pending 吻合 → grant', () => {
    const p = plugin({ consentPending: ['tasks', 'desktop'], consentPendingHosts: ['a.com'] })
    expect(postInstallConsent(p, { permissions: ['desktop', 'tasks'], networkHosts: ['a.com'] })).toBe('grant')
  })
  test('预确认不全 → dialog', () => {
    const p = plugin({ consentPending: ['tasks'], consentPendingHosts: ['a.com'] })
    expect(postInstallConsent(p, { permissions: ['tasks'], networkHosts: [] })).toBe('dialog')
  })
})

describe('hostsToConfirm / permissionKeys', () => {
  const entry = { networkHosts: ['a.com', 'b.com'] } as MarketEntryDto
  test('新装取全部主机', () => expect(hostsToConfirm(entry, undefined)).toEqual(['a.com', 'b.com']))
  test('更新排除已授权主机', () => {
    expect(hostsToConfirm(entry, plugin({ networkHosts: ['a.com'] }))).toEqual(['b.com'])
    expect(hostsToConfirm(entry, plugin({ networkHosts: ['a.com'], consentPendingHosts: ['a.com'] }))).toEqual(['a.com', 'b.com'])
  })
  test('九个权限都有文案键', () => {
    for (const name of ['ffmpeg', 'ytdlp', 'auth', 'tasks', 'browserRules', 'desktop', 'background', 'intercept', 'pageScripts']) {
      expect(permissionKeys(name)).not.toBeNull()
    }
    expect(permissionKeys('nope')).toBeNull()
  })
})
