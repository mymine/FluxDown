import { describe, expect, test } from 'bun:test'
import type { MarketEntryDto, PluginDto } from '../../../../lib/rpc/protocol'
import { MARKET_MAX_AGE_MS, installedVersionYanked, latestPerPlugin, marketAction, marketNeedsLoad, permissionsToConfirm, versionNewer } from './logic'

function entry(pluginId: string, version: string, sequence: number, yanked = 'none', permissions: string[] = []): MarketEntryDto {
  return { pluginId, version, sequence, yanked, permissions, name: pluginId, tags: [] } as unknown as MarketEntryDto
}

function plugin(identity: string, version: string, devMode = false, permissions: string[] = []): PluginDto {
  return { identity, version, devMode, permissions } as unknown as PluginDto
}

describe('market logic', () => {
  test('latestPerPlugin：取最新可安装版本，撤回版本让位，保持首次出现顺序', () => {
    const list = latestPerPlugin([
      entry('a', '1.0.0', 1),
      entry('b', '1.0.0', 1),
      entry('a', '1.1.0', 2),
      entry('a', '1.2.0', 3, 'malicious'),
    ])
    expect(list.map((e) => `${e.pluginId}@${e.version}`)).toEqual(['a@1.1.0', 'b@1.0.0'])
  })

  test('latestPerPlugin：全部撤回（下架）的插件不出现', () => {
    const list = latestPerPlugin([entry('a', '1.0.0', 1, 'deprecated'), entry('a', '1.1.0', 2, 'vulnerable'), entry('b', '1.0.0', 3)])
    expect(list.map((e) => e.pluginId)).toEqual(['b'])
  })

  test('versionNewer：忽略前导 v 与预发布后缀，无法解析视为不更新', () => {
    expect(versionNewer('v1.10.0', '1.9.9')).toBe(true)
    expect(versionNewer('1.0.0-rc.1', '1.0.0')).toBe(false)
    expect(versionNewer('abc', '1.0.0')).toBe(false)
  })

  test('marketAction', () => {
    expect(marketAction(entry('a', '1.0.0', 1), undefined)).toBe('install')
    expect(marketAction(entry('a', '1.0.0', 1, 'deprecated'), undefined)).toBe('unavailable')
    expect(marketAction(entry('a', '1.1.0', 2), plugin('a', '1.0.0'))).toBe('update')
    expect(marketAction(entry('a', '1.1.0', 2), plugin('a', '1.0.0', true))).toBe('installed')
    expect(marketAction(entry('a', '1.1.0', 2, 'malicious'), plugin('a', '1.0.0'))).toBe('installed')
  })

  test('permissionsToConfirm：更新只取新增权限', () => {
    const e = entry('a', '2.0.0', 2, 'none', ['ffmpeg', 'auth'])
    expect(permissionsToConfirm(e, undefined)).toEqual(['ffmpeg', 'auth'])
    expect(permissionsToConfirm(e, plugin('a', '1.0.0', false, ['ffmpeg']))).toEqual(['auth'])
  })

  test('installedVersionYanked', () => {
    const entries = [entry('a', '1.0.0', 1, 'vulnerable'), entry('b', '1.0.0', 1)]
    expect(installedVersionYanked(entries, plugin('a', '1.0.0'))).toBe('vulnerable')
    expect(installedVersionYanked(entries, plugin('a', '1.0.0', true))).toBeNull()
    expect(installedVersionYanked(entries, plugin('b', '1.0.0'))).toBeNull()
  })

  test('marketNeedsLoad：从未请求 / 断连重试 / 过期才加载，失败态不自动重试', () => {
    const loaded = { requested: true, retryOnReconnect: false, failed: false, loadedAt: 1_000 }
    expect(marketNeedsLoad({ requested: false, retryOnReconnect: false, failed: false, loadedAt: null }, 1_000)).toBe(true)
    expect(marketNeedsLoad({ ...loaded, retryOnReconnect: true, failed: true }, 1_000)).toBe(true)
    expect(marketNeedsLoad(loaded, 1_000 + MARKET_MAX_AGE_MS - 1)).toBe(false)
    expect(marketNeedsLoad(loaded, 1_000 + MARKET_MAX_AGE_MS)).toBe(true)
    expect(marketNeedsLoad({ ...loaded, failed: true }, 1_000 + MARKET_MAX_AGE_MS)).toBe(false)
    expect(marketNeedsLoad({ ...loaded, loadedAt: null, failed: true }, 5_000)).toBe(false)
  })
})
