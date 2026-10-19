import { describe, expect, test } from 'bun:test'
import type { PluginDto } from '../../lib/rpc'
import {
  deriveActivityEntries,
  deriveChromeSpecs,
  deriveDetailTabs,
  derivePaletteCommands,
  deriveTaskCommands,
  hasPluginSettings,
  isPluginEntryActive,
  parsePluginRouteId,
  pluginRouteId,
  settingsViewOf,
} from './contributions'

function plugin(over: Partial<PluginDto> & { identity: string }): PluginDto {
  return {
    name: over.identity,
    version: '1.0.0',
    description: '',
    homepage: '',
    enabled: true,
    devMode: false,
    disabledReason: 'None',
    settings: [],
    settingsValues: {},
    permissions: [],
    authSupported: false,
    subscriptionProviderIds: [],
    resolverUrls: [],
    loadStatus: 'Loaded',
    loadError: '',
    contributions: { views: [], commands: [], sniffRules: false },
    risk: 'standard',
    consentPending: [],
    consentPendingHosts: [],
    networkHosts: [],
    ...over,
  }
}

const view = (id: string, location: string, portable = true) => ({ id, title: `T-${id}`, location, portable })

const good = plugin({
  identity: 'a@x',
  name: 'Alpha',
  contributions: {
    views: [view('dash', 'activity'), view('detail', 'taskDetail'), view('cfg', 'settings'), view('win', 'window', false)],
    commands: [
      { id: 'refresh', title: 'Refresh', palette: true, taskMenu: false },
      { id: 'send', title: 'Send', palette: false, taskMenu: true },
    ],
    sniffRules: false,
  },
})
const disabled = plugin({ ...good, identity: 'b@x', enabled: false })
const failed = plugin({ ...good, identity: 'c@x', loadStatus: 'Failed' })
const pending = plugin({ ...good, identity: 'd@x', consentPending: ['tasks'] })
const pendingHosts = plugin({ ...good, identity: 'e@x', consentPendingHosts: ['api.x.com'] })
const all = [good, disabled, failed, pending, pendingHosts]

describe('plugin contributions', () => {
  test('activity entries only include usable plugins and activity views', () => {
    const entries = deriveActivityEntries(all)
    expect(entries.map((entry) => entry.id)).toEqual(['plugin:a@x:dash'])
    expect(entries[0]?.label).toBe('T-dash')
  })

  test('chrome specs default to railTop with rail/title families', () => {
    expect(deriveChromeSpecs(deriveActivityEntries(all))).toEqual([{ id: 'plugin:a@x:dash', defaultZone: 'railTop', families: ['rail', 'title'] }])
  })

  test('route id round trip', () => {
    expect(parsePluginRouteId(pluginRouteId('a@x', 'dash'))).toEqual({ identity: 'a@x', viewId: 'dash' })
    expect(parsePluginRouteId('downloads')).toBeNull()
    expect(parsePluginRouteId('plugin:bad')).toBeNull()
  })

  test('active detection decodes path segments', () => {
    const entry = deriveActivityEntries([good])[0]
    if (!entry) throw new Error('entry missing')
    expect(isPluginEntryActive(entry, `/plugins/${encodeURIComponent('a@x')}/dash`)).toBe(true)
    expect(isPluginEntryActive(entry, '/plugins/a@x/other')).toBe(false)
    expect(isPluginEntryActive(entry, '/rss')).toBe(false)
  })

  test('detail tabs, settings view and commands', () => {
    expect(deriveDetailTabs(all).map((tab) => tab.value)).toEqual(['plugin:a@x:detail'])
    expect(settingsViewOf(good)?.id).toBe('cfg')
    expect(settingsViewOf(plugin({ identity: 'z@x' }))).toBeUndefined()
    expect(deriveTaskCommands(all).map((ref) => `${ref.identity}/${ref.command.id}`)).toEqual(['a@x/send'])
    expect(derivePaletteCommands(good).map((command) => command.id)).toEqual(['refresh'])
    expect(derivePaletteCommands(pending)).toEqual([])
  })

  test('desktop-only views are hidden from every entry', () => {
    const desktopOnly = plugin({
      identity: 'f@x',
      contributions: {
        views: [view('panel', 'activity', false), view('info', 'taskDetail', false), view('cfg', 'settings', false)],
        commands: [],
        sniffRules: false,
      },
    })
    expect(deriveActivityEntries([desktopOnly])).toEqual([])
    expect(deriveDetailTabs([desktopOnly])).toEqual([])
    expect(settingsViewOf(desktopOnly)).toBeUndefined()
  })

  test('settings button: fields or a usable settings view', () => {
    const field = { key: 'k' } as unknown as PluginDto['settings'][number]
    expect(hasPluginSettings(good)).toBe(true) // 仅视图
    expect(hasPluginSettings(plugin({ identity: 'n@x' }))).toBe(false)
    expect(hasPluginSettings(plugin({ identity: 'f@x', settings: [field] }))).toBe(true)
    expect(hasPluginSettings(disabled)).toBe(false) // 视图不可运行且无字段
    expect(hasPluginSettings(plugin({ ...disabled, settings: [field] }))).toBe(true)
  })
})
