import { describe, expect, test } from 'bun:test'
import { buildSeedLocalParams, seedLocalFeedback } from './seedLocal'

const base = { path: '', torrentPath: '', trackers: '', unlimited: false }

describe('buildSeedLocalParams', () => {
  test('空路径（含纯空白）报 pathRequired', () => {
    expect(buildSeedLocalParams(base)).toEqual({ ok: false, error: 'pathRequired' })
    expect(buildSeedLocalParams({ ...base, path: '  \t' })).toEqual({ ok: false, error: 'pathRequired' })
  })

  test('路径去空白，省略空的可选字段', () => {
    expect(buildSeedLocalParams({ ...base, path: ' /data/a ' })).toEqual({ ok: true, params: { path: '/data/a', unlimited: false } })
  })

  test('tracker 按行拆分、去空白与空行', () => {
    const result = buildSeedLocalParams({ ...base, path: '/d', trackers: ' udp://a:1/announce \r\n\n  \nhttp://b/announce\n', unlimited: true })
    expect(result).toEqual({ ok: true, params: { path: '/d', unlimited: true, trackers: ['udp://a:1/announce', 'http://b/announce'] } })
  })

  test('指定 torrentPath 时忽略 trackers', () => {
    const result = buildSeedLocalParams({ ...base, path: '/d', torrentPath: ' /t.torrent ', trackers: 'udp://a' })
    expect(result).toEqual({ ok: true, params: { path: '/d', torrentPath: '/t.torrent', unlimited: false } })
  })
})

describe('seedLocalFeedback', () => {
  test('非重复 → 成功', () => {
    expect(seedLocalFeedback({})).toEqual({ key: 'seedLocalStarted', level: 'success' })
  })
  test('重复带名称 → 警告带 name', () => {
    expect(seedLocalFeedback({ duplicate: true, existingName: 'a' })).toEqual({ key: 'duplicateTorrentToast', level: 'warning', params: { name: 'a' } })
  })
  test('重复无名称 → Unnamed', () => {
    expect(seedLocalFeedback({ duplicate: true })).toEqual({ key: 'duplicateTorrentToastUnnamed', level: 'warning' })
    expect(seedLocalFeedback({ duplicate: true, existingName: '' })).toEqual({ key: 'duplicateTorrentToastUnnamed', level: 'warning' })
  })
})
