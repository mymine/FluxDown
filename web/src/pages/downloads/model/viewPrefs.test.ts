import { describe, expect, test } from 'bun:test'
import { parseTimestampSecs } from './task'
import type { DownloadTaskView, TaskState } from './task'
import { compareNatural, compareViews, defaultViewPrefs, nextHeaderSort, parseViewPrefs, smartTier } from './viewPrefs'
import type { ViewPrefs, ViewSortKey } from './viewPrefs'

interface Spec {
  key: string
  state?: TaskState
  createdAtSecs?: number
  queueOrder?: number
  queueKey?: number | null
  boosted?: boolean
  preparing?: boolean
  name?: string
  sizeBytes?: number
  progress?: number
  speed?: number | null
}

const view = (spec: Spec): DownloadTaskView => {
  const name = spec.name ?? spec.key
  return {
    state: 'completed',
    createdAtSecs: 0,
    queueOrder: 0,
    queueKey: null,
    boosted: false,
    preparing: false,
    sizeBytes: 0,
    progress: 0,
    speed: null,
    ...spec,
    name,
    nameFold: name.toLowerCase(),
  } as DownloadTaskView
}

const order = (prefs: Partial<ViewPrefs>, specs: Spec[]) =>
  specs
    .map(view)
    .sort((a, b) => compareViews({ ...defaultViewPrefs(), ...prefs }, a, b))
    .map((v) => v.key)

const smart = (specs: Spec[]) => order({ sort_key: 'smart' }, specs)

describe('智能排序档位', () => {
  test('优先下载 > 下载中/准备中 > 排队 > 失败 > 暂停 > 已完成', () => {
    const specs: Spec[] = [
      { key: 'done', state: 'completed' },
      { key: 'paused', state: 'paused' },
      { key: 'failed', state: 'failed' },
      { key: 'queued', state: 'pending' },
      { key: 'downloading', state: 'downloading' },
      { key: 'boosted', state: 'pending', boosted: true },
    ]
    expect(smart(specs)).toEqual(['boosted', 'downloading', 'queued', 'failed', 'paused', 'done'])
  })

  test('准备中的排队任务与下载中同档，按添加顺序正序', () => {
    const specs: Spec[] = [
      { key: 'queued', state: 'pending', createdAtSecs: 1 },
      { key: 'preparing', state: 'pending', preparing: true, createdAtSecs: 5 },
      { key: 'downloading', state: 'downloading', createdAtSecs: 3 },
    ]
    expect(smart(specs)).toEqual(['downloading', 'preparing', 'queued'])
    expect(smartTier(view({ key: 'x', state: 'pending', preparing: true }))).toBe(1)
  })

  test('boosted 只对下载中/排队生效，暂停的优先任务仍在暂停档', () => {
    const specs: Spec[] = [
      { key: 'boostedPaused', state: 'paused', boosted: true },
      { key: 'downloading', state: 'downloading' },
    ]
    expect(smart(specs)).toEqual(['downloading', 'boostedPaused'])
  })

  test('活跃档：先加的在上，新任务接在末尾', () => {
    const specs: Spec[] = [
      { key: 'new', state: 'downloading', createdAtSecs: 30 },
      { key: 'old', state: 'downloading', createdAtSecs: 10 },
      { key: 'mid', state: 'downloading', createdAtSecs: 20 },
    ]
    expect(smart(specs)).toEqual(['old', 'mid', 'new'])
  })

  test('同一秒批量添加按 queueOrder 打破平局', () => {
    const active: Spec[] = [
      { key: 'c', state: 'downloading', createdAtSecs: 7, queueOrder: 3 },
      { key: 'a', state: 'downloading', createdAtSecs: 7, queueOrder: 1 },
      { key: 'b', state: 'downloading', createdAtSecs: 7, queueOrder: 2 },
    ]
    expect(smart(active)).toEqual(['a', 'b', 'c'])
    const history = active.map((spec) => ({ ...spec, state: 'completed' as const }))
    expect(smart(history)).toEqual(['c', 'b', 'a'])
  })

  test('排队档按引擎排序键升序（键可为 0 / 负数），不在队列(null)的在所有已知之后再按添加顺序', () => {
    const specs: Spec[] = [
      { key: 'unknownNew', state: 'pending', queueKey: null, createdAtSecs: 9 },
      { key: 'key300', state: 'pending', queueKey: 300, createdAtSecs: 1 },
      { key: 'unknownOld', state: 'pending', queueKey: null, createdAtSecs: 2 },
      { key: 'key100', state: 'pending', queueKey: 100, createdAtSecs: 8 },
      { key: 'key0', state: 'pending', queueKey: 0, createdAtSecs: 7 },
      { key: 'keyNegative', state: 'pending', queueKey: -5, createdAtSecs: 6 },
    ]
    expect(smart(specs)).toEqual(['keyNegative', 'key0', 'key100', 'key300', 'unknownOld', 'unknownNew'])
  })

  test('历史档（失败/暂停/已完成）最新在前', () => {
    const specs: Spec[] = [
      { key: 'oldFailed', state: 'failed', createdAtSecs: 1 },
      { key: 'newFailed', state: 'failed', createdAtSecs: 5 },
      { key: 'oldDone', state: 'completed', createdAtSecs: 2 },
      { key: 'newDone', state: 'completed', createdAtSecs: 6 },
      { key: 'newPaused', state: 'paused', createdAtSecs: 7 },
      { key: 'oldPaused', state: 'paused', createdAtSecs: 3 },
    ]
    expect(smart(specs)).toEqual(['newFailed', 'oldFailed', 'newPaused', 'oldPaused', 'newDone', 'oldDone'])
  })

  test('智能排序忽略 sort_dir', () => {
    const specs: Spec[] = [
      { key: 'a', state: 'downloading', createdAtSecs: 1 },
      { key: 'b', state: 'completed', createdAtSecs: 2 },
    ]
    expect(order({ sort_key: 'smart', sort_dir: 'asc' }, specs)).toEqual(order({ sort_key: 'smart', sort_dir: 'desc' }, specs))
  })
})

describe('非智能排序键', () => {
  test('status 按 STATE_RANK（失败在暂停前），随 sort_dir 反转', () => {
    const specs: Spec[] = [
      { key: 'done', state: 'completed' },
      { key: 'paused', state: 'paused' },
      { key: 'failed', state: 'failed' },
      { key: 'queued', state: 'pending' },
      { key: 'downloading', state: 'downloading' },
    ]
    expect(order({ sort_key: 'status', sort_dir: 'asc' }, specs)).toEqual(['downloading', 'queued', 'failed', 'paused', 'done'])
    expect(order({ sort_key: 'status', sort_dir: 'desc' }, specs)).toEqual(['done', 'paused', 'failed', 'queued', 'downloading'])
  })

  test('主键相等回落到添加顺序倒序，且不随方向翻转', () => {
    const items = (): Spec[] => [
      { key: 'old', createdAtSecs: 1, sizeBytes: 5 },
      { key: 'new', createdAtSecs: 9, sizeBytes: 5 },
      { key: 'newSameSecond', createdAtSecs: 9, queueOrder: 2, sizeBytes: 5 },
    ]
    for (const sort_dir of ['asc', 'desc'] as const) {
      expect(order({ sort_key: 'size', sort_dir }, items())).toEqual(['newSameSecond', 'new', 'old'])
    }
  })

  test('size / progress / speed / created 按方向排序', () => {
    const specs: Spec[] = [
      { key: 'a', sizeBytes: 1, progress: 0.9, speed: 30, createdAtSecs: 3 },
      { key: 'b', sizeBytes: 3, progress: 0.1, speed: null, createdAtSecs: 1 },
      { key: 'c', sizeBytes: 2, progress: 0.5, speed: 10, createdAtSecs: 2 },
    ]
    const keys: [ViewSortKey, string[]][] = [
      ['size', ['a', 'c', 'b']],
      ['progress', ['b', 'c', 'a']],
      ['speed', ['b', 'c', 'a']],
      ['created', ['b', 'c', 'a']],
    ]
    for (const [sort_key, ascending] of keys) {
      expect(order({ sort_key, sort_dir: 'asc' }, specs)).toEqual(ascending)
      expect(order({ sort_key, sort_dir: 'desc' }, specs)).toEqual([...ascending].reverse())
    }
  })
})

describe('名称自然序', () => {
  const natural = (names: string[], sort_dir: 'asc' | 'desc' = 'asc') =>
    order({ sort_key: 'name', sort_dir }, names.map((name) => ({ key: name, name })))

  test('数字段按数值比较，忽略大小写', () => {
    expect(natural(['ep10', 'EP2', 'ep1', 'ep20', 'ep3'])).toEqual(['ep1', 'EP2', 'ep3', 'ep10', 'ep20'])
    expect(natural(['ep10', 'EP2', 'ep1'], 'desc')).toEqual(['ep10', 'EP2', 'ep1'])
  })

  test('前导 0 数值相等时由原始串定序', () => {
    expect(compareNatural('a01', 'a1')).toBe(0)
    expect(natural(['a1', 'a01', 'a2'])).toEqual(['a01', 'a1', 'a2'])
  })

  test('一边先耗尽则短者在前，数字段与文本段按普通字符串比较', () => {
    expect(natural(['file2.zip', 'file2', 'file'])).toEqual(['file', 'file2', 'file2.zip'])
    expect(compareNatural('a1', 'ab') < 0).toBe(true)
    expect(compareNatural('a10b', 'a9c') > 0).toBe(true)
    expect(compareNatural('same', 'same')).toBe(0)
  })
})

describe('表头三档循环', () => {
  const start = (sort_key: ViewSortKey, sort_dir: 'asc' | 'desc') => ({ ...defaultViewPrefs(), sort_key, sort_dir })

  test('数值列：desc → asc → 智能排序（方向复位 desc）', () => {
    const first = nextHeaderSort(start('smart', 'desc'), 'size')
    expect([first.sort_key, first.sort_dir]).toEqual(['size', 'desc'])
    const second = nextHeaderSort(first, 'size')
    expect([second.sort_key, second.sort_dir]).toEqual(['size', 'asc'])
    const third = nextHeaderSort(second, 'size')
    expect([third.sort_key, third.sort_dir]).toEqual(['smart', 'desc'])
  })

  test('名称列默认 asc，反向为 desc', () => {
    const first = nextHeaderSort(start('smart', 'desc'), 'name')
    expect([first.sort_key, first.sort_dir]).toEqual(['name', 'asc'])
    const second = nextHeaderSort(first, 'name')
    expect([second.sort_key, second.sort_dir]).toEqual(['name', 'desc'])
    expect(nextHeaderSort(second, 'name').sort_key).toBe('smart')
  })

  test('切到别的列从该列默认方向开始', () => {
    const next = nextHeaderSort(start('size', 'asc'), 'name')
    expect([next.sort_key, next.sort_dir]).toEqual(['name', 'asc'])
    const status = nextHeaderSort(start('name', 'desc'), 'status')
    expect([status.sort_key, status.sort_dir]).toEqual(['status', 'desc'])
  })
})

describe('偏好解析', () => {
  test('三档密度保存后可恢复，未知密度只回退该字段', () => {
    for (const density of ['compact', 'comfortable', 'relaxed'] as const) {
      const prefs = { ...defaultViewPrefs(), density, group_by: 'status' as const }
      expect(parseViewPrefs(JSON.stringify(prefs))).toEqual(prefs)
    }
    const future = parseViewPrefs({ density: 'future', group_by: 'status' })
    expect(future.density).toBe(defaultViewPrefs().density)
    expect(future.group_by).toBe('status')
  })

  test('已移除的「按任务组分组」读成默认「无」', () => {
    expect(parseViewPrefs({ group_by: 'group' }).group_by).toBe('none')
  })

  test('status 是合法排序键，非法值回落默认', () => {
    expect(parseViewPrefs({ sort_key: 'status' }).sort_key).toBe('status')
    expect(parseViewPrefs({ sort_key: 'bogus' }).sort_key).toBe('smart')
  })
})

describe('时间戳解析', () => {
  test('本地任务：Unix 秒字符串', () => {
    expect(parseTimestampSecs('1780000000')).toBe(1780000000)
  })

  test('远程任务：RFC3339 按日期解析，而不是取开头的年份', () => {
    expect(parseTimestampSecs('2026-09-28T12:34:56Z')).toBe(Date.UTC(2026, 8, 28, 12, 34, 56) / 1000)
    expect(parseTimestampSecs('2026-09-28T12:34:56.789+00:00')).toBe(Date.UTC(2026, 8, 28, 12, 34, 56) / 1000)
  })

  test('空串与无法解析都是 0', () => {
    expect(parseTimestampSecs('')).toBe(0)
    expect(parseTimestampSecs('not a date')).toBe(0)
  })

  test('远程任务与本地任务能按时间比较', () => {
    const remote = parseTimestampSecs('2026-09-28T00:00:00Z')
    const local = parseTimestampSecs('1700000000')
    expect(remote > local).toBe(true)
  })
})
