import { describe, expect, test } from 'bun:test'
import { TableLoad, TableSyncError } from './tableSync'
import type { DaemonSnapshot, TablePage, TaskDto } from './protocol'

function task(taskId: string): TaskDto {
  return { taskId } as unknown as TaskDto
}

function taskPage(ids: string[], nextCursor?: string, epoch = 'e'): TablePage {
  return {
    epoch,
    sequence: 1,
    section: 'tasks',
    total: 99,
    tasks: ids.map(task),
    queue: [],
    ...(nextCursor === undefined ? {} : { nextCursor }),
  }
}

function queuePage(entries: [string, number][], nextCursor?: string): TablePage {
  return {
    epoch: 'e',
    sequence: 1,
    section: 'queue',
    total: 99,
    tasks: [],
    queue: entries.map(([taskId, order]) => ({ taskId, order })),
    ...(nextCursor === undefined ? {} : { nextCursor }),
  }
}

function failure(run: () => void): string | undefined {
  try {
    run()
  } catch (error) {
    return error instanceof TableSyncError ? error.failure : 'other'
  }
  return undefined
}

describe('TableLoad', () => {
  test('无待载入标记：不建立载入', () => {
    expect(TableLoad.forSnapshot('e', { tasks: [], queuePositions: [] } as unknown as DaemonSnapshot)).toBeNull()
    expect(TableLoad.forSnapshot('e', { taskTotal: 0 } as unknown as DaemonSnapshot)).not.toBeNull()
  })

  test('先任务后队列，各自翻到末页；游标 = 上一页 nextCursor；只请求有标志的表', () => {
    const load = new TableLoad('e', 5, 3)
    expect(load.nextRequest()).toEqual({ section: 'tasks' })
    load.accept(taskPage(['a', 'b'], 'b'))
    expect(load.loadedTasks).toBe(2)
    expect(load.nextRequest()).toEqual({ section: 'tasks', cursor: 'b' })
    load.accept(taskPage(['c']))
    expect(load.nextRequest()).toEqual({ section: 'queue' })
    load.accept(queuePage([['a', 10], ['b', 20]], '20'))
    expect(load.nextRequest()).toEqual({ section: 'queue', cursor: '20' })
    load.accept(queuePage([['c', 30]]))
    expect(load.nextRequest()).toBeNull()
    const parts = load.parts()
    expect(parts.tasks?.map((item) => item.taskId)).toEqual(['a', 'b', 'c'])
    expect(parts.queue).toEqual([
      { taskId: 'a', order: 10 },
      { taskId: 'b', order: 20 },
      { taskId: 'c', order: 30 },
    ])

    const queueOnly = new TableLoad('e', undefined, 2)
    expect(queueOnly.nextRequest()).toEqual({ section: 'queue' })
    queueOnly.accept(queuePage([['a', 1]]))
    expect(queueOnly.nextRequest()).toBeNull()
    expect(queueOnly.parts()).toEqual({ tasks: null, queue: [{ taskId: 'a', order: 1 }] })
  })

  test('空表（total = 0）也要请求一页确认；空页即末页', () => {
    const load = new TableLoad('e', 0, undefined)
    expect(load.nextRequest()).toEqual({ section: 'tasks' })
    load.accept(taskPage([]))
    expect(load.nextRequest()).toBeNull()
    expect(load.parts().tasks).toEqual([])
  })

  test('epoch 与基准不同：epochChanged', () => {
    const load = new TableLoad('e', 1, undefined)
    load.nextRequest()
    expect(failure(() => load.accept(taskPage(['a'], undefined, 'other')))).toBe('epochChanged')
  })

  test('nextCursor 与请求游标相同（不前进）：stalled，含首页游标缺省后返回同值的情形', () => {
    const load = new TableLoad('e', 1, undefined)
    load.nextRequest()
    load.accept(taskPage(['a'], 'a'))
    load.nextRequest()
    expect(failure(() => load.accept(taskPage(['a'], 'a')))).toBe('stalled')
  })

  test('页的分区与请求不符 / 没有在途请求：unexpectedSection', () => {
    const load = new TableLoad('e', 1, 1)
    load.nextRequest()
    expect(failure(() => load.accept(queuePage([['a', 1]])))).toBe('unexpectedSection')
    const idle = new TableLoad('e', 1, undefined)
    expect(failure(() => idle.accept(taskPage(['a'])))).toBe('unexpectedSection')
  })
})
