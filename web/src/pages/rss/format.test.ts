import { describe, expect, test } from 'bun:test'
import type { RssItemDto } from '../../lib/rpc'
import { TASK_STATE_UNKNOWN, encodeTaskState, itemStatusKey } from './format'

const downloaded = { status: 1, taskId: 't1' } as unknown as RssItemDto

describe('itemStatusKey：关联任务', () => {
  test('任务存在：取真实状态', () => {
    expect(itemStatusKey(downloaded, { t1: encodeTaskState(3, false) })).toBe('statusCompleted')
    expect(itemStatusKey(downloaded, { t1: encodeTaskState(3, true) })).toBe('statusIncomplete')
  })

  test('任务表完整且找不到任务：已删除', () => {
    expect(itemStatusKey(downloaded, {})).toBe('rssTaskMissing')
  })

  test('任务表载入期间（状态未知）：不能判定为已删除', () => {
    expect(itemStatusKey(downloaded, { t1: TASK_STATE_UNKNOWN })).toBe('rssTaskCreated')
  })
})
