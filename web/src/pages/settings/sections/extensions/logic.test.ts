import { describe, expect, test } from 'bun:test'
import type { MarketEntryDto, PluginDto, SettingFieldDto } from '../../../../lib/rpc/protocol'
import {
  CHALLENGE_TEXT_LIMIT,
  dataImageChallengeSrc,
  filterMarket,
  formatBytes,
  liveDialogPlugins,
  safeHttpUrl,
  truncateChallengeText,
  validateField,
} from './logic'

function field(type: string, widget: string, patch: Partial<SettingFieldDto> = {}): SettingFieldDto {
  return {
    key: 'k',
    title: 'K',
    description: '',
    type,
    widget,
    options: [],
    default: null,
    required: false,
    min: null,
    max: null,
    pattern: null,
    helperScript: null,
    helperLabel: null,
    ...patch,
  }
}

function entry(id: string, name: string, description: string, author: string, tags: string[]): MarketEntryDto {
  return {
    pluginId: id,
    version: '1.0.0',
    sequence: 1,
    contentHash: '',
    minAppVersion: '',
    name,
    description,
    author,
    homepage: '',
    mirrors: [],
    publishTime: '',
    yanked: '',
    tags,
    permissions: [],
    networkHosts: [],
  }
}

describe('validateField', () => {
  test('required 只拒绝空白', () => {
    const required = field('string', 'text', { required: true })
    expect(validateField(required, '  ')).toEqual({ kind: 'required' })
    expect(validateField(required, 'x')).toBeNull()
    expect(validateField(field('string', 'text'), '')).toBeNull()
  })

  test('number 范围与格式', () => {
    const number = field('number', 'number', { min: 1, max: 10.5 })
    expect(validateField(number, 'abc')).toEqual({ kind: 'number' })
    expect(validateField(number, '0x10')).toEqual({ kind: 'number' })
    expect(validateField(number, 'Infinity')).toEqual({ kind: 'number' })
    expect(validateField(number, '0')).toEqual({ kind: 'min', min: '1' })
    expect(validateField(number, '11')).toEqual({ kind: 'max', max: '10.5' })
    expect(validateField(number, ' 5 ')).toBeNull()
    expect(validateField(number, '1e1')).toBeNull()
  })

  test('select 必须是已知选项', () => {
    const select = field('string', 'select', { options: [{ value: 'a', label: 'A' }] })
    expect(validateField(select, 'b')).toEqual({ kind: 'select' })
    expect(validateField(select, 'a')).toBeNull()
  })
})

describe('filterMarket', () => {
  const entries = [
    entry('video.dl', 'Video', 'grabs videos', 'Ann', ['media']),
    entry('other', 'Other', 'misc', 'Bob', ['tools']),
  ]
  const ids = (query: string) => filterMarket(entries, query).map((item) => item.pluginId)

  test('空查询保留全部', () => {
    expect(ids('   ')).toEqual(['video.dl', 'other'])
  })

  test('名称/id/描述/作者/标签任一命中且大小写不敏感', () => {
    expect(ids('VIDEO')).toEqual(['video.dl'])
    expect(ids('bob')).toEqual(['other'])
    expect(ids('media')).toEqual(['video.dl'])
    expect(ids('grabs')).toEqual(['video.dl'])
    expect(ids('nothing')).toEqual([])
  })
})

describe('dataImageChallengeSrc', () => {
  test('接受已知 mime 的 base64 data URL', () => {
    expect(dataImageChallengeSrc('data:image/png;base64,aGVsbG8=')).toBe('data:image/png;base64,aGVsbG8=')
    expect(dataImageChallengeSrc('data:IMAGE/PNG;charset=utf-8;base64,aGVs bG8=')).toBe('data:image/png;base64,aGVsbG8=')
  })

  test('拒绝非图片、未知格式、非 base64、非法载荷与超限', () => {
    expect(dataImageChallengeSrc('plain-text-challenge')).toBeNull()
    expect(dataImageChallengeSrc('https://example.com/login')).toBeNull()
    expect(dataImageChallengeSrc('data:text/html;base64,aGVsbG8=')).toBeNull()
    expect(dataImageChallengeSrc('data:image/unknown;base64,aGVsbG8=')).toBeNull()
    expect(dataImageChallengeSrc('data:image/png,aGVsbG8=')).toBeNull()
    expect(dataImageChallengeSrc('data:image/png;base64,not base64!')).toBeNull()
    expect(dataImageChallengeSrc(`data:image/png;base64,${'A'.repeat(256 * 1024)}`)).toBeNull()
  })
})

describe('challenge text', () => {
  test('短文本原样，长文本按码点截断并加省略号', () => {
    expect(truncateChallengeText('short')).toBe('short')
    const truncated = truncateChallengeText('码'.repeat(600))
    expect(Array.from(truncated).length).toBe(CHALLENGE_TEXT_LIMIT + 1)
    expect(truncated.endsWith('…')).toBe(true)
  })

  test('只放行 http(s) 链接', () => {
    expect(safeHttpUrl('https://example.com/a')).toBe('https://example.com/a')
    expect(safeHttpUrl('javascript:alert(1)')).toBeNull()
    expect(safeHttpUrl('not a url')).toBeNull()
  })
})

describe('formatBytes', () => {
  test('按 1024 进制换算', () => {
    expect(formatBytes(0)).toBe('0 B')
    expect(formatBytes(-5)).toBe('0 B')
    expect(formatBytes(1023)).toBe('1023 B')
    expect(formatBytes(1536)).toBe('1.5 KB')
    expect(formatBytes(12 * 1024 * 1024 + 300 * 1024)).toBe('12.3 MB')
    expect(formatBytes(1024 ** 3)).toBe('1.0 GB')
  })
})

describe('liveDialogPlugins', () => {
  const plugin = (identity: string, pending: string[] = []) => ({ identity, consentPending: pending, consentPendingHosts: [] }) as unknown as PluginDto
  const none = { detail: null, settings: null, auth: null, consent: null }

  test('对话框内容取最新快照，卸载后为 null', () => {
    const v1 = plugin('a@x')
    const open = { ...none, detail: 'a@x', settings: 'a@x', auth: 'b@x' }
    expect(liveDialogPlugins([v1, plugin('b@x')], open)).toMatchObject({ detail: v1, settings: v1 })
    const v2 = plugin('a@x')
    const next = liveDialogPlugins([v2], open)
    expect(next.detail).toBe(v2)
    expect(next.settings).toBe(v2)
    expect(next.auth).toBeNull()
  })

  test('授权框在待授权集合清空后为 null，仍有待授权则取最新快照', () => {
    const open = { ...none, consent: 'a@x' }
    const pending = plugin('a@x', ['ffmpeg'])
    expect(liveDialogPlugins([pending], open).consent).toBe(pending)
    expect(liveDialogPlugins([plugin('a@x')], open).consent).toBeNull()
    expect(liveDialogPlugins([], open).consent).toBeNull()
  })

  test('未打开的对话框恒为 null', () => {
    expect(liveDialogPlugins([plugin('a@x')], none)).toEqual(none)
  })
})
