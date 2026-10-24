import { describe, expect, test } from 'bun:test'
import { filterMatches, STATUS_FILTERS } from './filters'
import { CategoryIndex } from './categories'
import { canResumeSeeding, isSeeding } from './task'
import type { DownloadTaskView, TaskState } from './task'

const view = (patch: Partial<DownloadTaskView>): DownloadTaskView =>
  ({ source: 'local', state: 'completed', seedingStatus: 0, fileMissing: false, ...patch }) as unknown as DownloadTaskView

const categories = new CategoryIndex([])
const inFolder = (folder: 'seeding' | 'completed', item: DownloadTaskView) =>
  filterMatches({ status: folder, category: null }, item, categories)

describe('做种判定', () => {
  test('「做种中」文件夹紧随「已完成」', () => {
    expect(STATUS_FILTERS.indexOf('seeding')).toBe(STATUS_FILTERS.indexOf('completed') + 1)
  })

  test('文件夹成员：1 与 8 属于，2..=7 / 0 不属于，且仍属于已完成', () => {
    for (const seedingStatus of [1, 8]) {
      expect(isSeeding(view({ seedingStatus }))).toBe(true)
      expect(inFolder('seeding', view({ seedingStatus }))).toBe(true)
      expect(inFolder('completed', view({ seedingStatus }))).toBe(true)
    }
    for (const seedingStatus of [0, 2, 3, 4, 5, 6, 7]) {
      expect(inFolder('seeding', view({ seedingStatus }))).toBe(false)
      expect(inFolder('completed', view({ seedingStatus }))).toBe(true)
    }
  })

  test('非完成态与远程任务不属于做种中', () => {
    for (const state of ['pending', 'downloading', 'paused', 'failed'] as TaskState[]) {
      expect(isSeeding(view({ state, seedingStatus: 1 }))).toBe(false)
    }
    expect(isSeeding(view({ source: 'remote', seedingStatus: 1 }))).toBe(false)
  })

  test('可恢复做种：完成态 + seeding_status 2..=7 + 文件在；0 / 1 / 8 / 文件缺失 / 远程不可', () => {
    for (const seedingStatus of [2, 3, 4, 5, 6, 7]) {
      expect(canResumeSeeding(view({ seedingStatus }))).toBe(true)
      expect(canResumeSeeding(view({ seedingStatus, fileMissing: true }))).toBe(false)
    }
    for (const seedingStatus of [0, 1, 8]) expect(canResumeSeeding(view({ seedingStatus }))).toBe(false)
    expect(canResumeSeeding(view({ state: 'paused', seedingStatus: 4 }))).toBe(false)
    expect(canResumeSeeding(view({ source: 'remote', seedingStatus: 4 }))).toBe(false)
  })
})
