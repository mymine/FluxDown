import { describe, expect, test } from 'bun:test'
import type { DownloadTaskView } from '../model/task'
import { relaxedMeta } from './text'

const t = (key: string) => key
const view = (patch: Partial<DownloadTaskView>) =>
  ({ state: 'completed', kind: 'video', sizeBytes: 0, downloadedBytes: 0, speed: null, url: '', referrer: '', site: '', ...patch }) as DownloadTaskView

describe('relaxedMeta（宽松密度副标题）', () => {
  test('已完成：大小 · 类别 · 站点，未知字段省略', () => {
    expect(relaxedMeta(t, view({ sizeBytes: 2048, site: 'example.com' }))).toBe('2.0 KB · categoryVideo · example.com')
    expect(relaxedMeta(t, view({}))).toBe('categoryVideo')
  })

  test('未完成：已下 / 总量，仅下载中且速度 > 0 才带速度，不含百分比与剩余时间', () => {
    const base = { state: 'downloading' as const, sizeBytes: 2048, downloadedBytes: 1024, site: 'example.com' }
    expect(relaxedMeta(t, view({ ...base, speed: 512 }))).toBe('1.0 KB / 2.0 KB · 512 B/s · example.com')
    expect(relaxedMeta(t, view({ ...base, speed: 0 }))).toBe('1.0 KB / 2.0 KB · example.com')
    expect(relaxedMeta(t, view({ ...base, state: 'paused', speed: 512 }))).toBe('1.0 KB / 2.0 KB · example.com')
  })

  test('总量未知只给已下', () => {
    expect(relaxedMeta(t, view({ state: 'paused', downloadedBytes: 1024 }))).toBe('1.0 KB')
  })
})
