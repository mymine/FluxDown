import { describe, expect, test } from 'bun:test'
import type { DownloadTaskView } from '../model/task'
import { buildTaskMenu } from './menus'

const view = { key: 'l:t1', source: 'local', taskId: 't1', state: 'completed', protocol: 'http', fileMissing: false, kind: 'other', url: 'https://e.com/a.zip' } as unknown as DownloadTaskView
const t = (key: string) => key

describe('buildTaskMenu plugin commands', () => {
  const base = { t, views: [view], queues: [], queueName: (id: string) => id, showDetail: () => undefined }
  const command = { identity: 'a@x', pluginName: 'Alpha', command: { id: 'send', title: 'Send', palette: false, taskMenu: true } }

  test('plugin group sits between the selection group and the delete group', () => {
    const keys = buildTaskMenu({
      ...base,
      pluginCommands: [command],
      selection: { multiSelect: false, enterMultiSelect: () => undefined, selectAll: () => undefined },
    }).map((entry) => entry.key)
    const select = keys.indexOf('select-all')
    const sep = keys.indexOf('sep-plugin')
    expect(select > -1).toBe(true)
    expect(sep).toBe(select + 1)
    expect(keys[sep + 1]).toBe('plugin:a@x:send')
    expect(keys[sep + 2]).toBe('sep-delete')
  })

  test('no plugin commands leaves the menu unchanged', () => {
    const keys = buildTaskMenu(base).map((entry) => entry.key)
    expect(keys.includes('sep-plugin')).toBe(false)
    expect(keys.includes('sep-delete')).toBe(true)
  })
})

describe('buildTaskMenu ignore plugin retry', () => {
  const failed = { ...view, state: 'failed', errorMessage: '[插件] yt: boom', groupId: '' } as unknown as DownloadTaskView
  const build = (v: DownloadTaskView) =>
    buildTaskMenu({ t, views: [v], queues: [], queueName: (id: string) => id, showDetail: () => undefined }).map((entry) => entry.key)

  test('standalone failed plugin task offers ignore-retry', () => {
    expect(build(failed).includes('ignore-plugin-retry')).toBe(true)
  })

  test('task group member with plugin error does not', () => {
    const member = { ...failed, groupId: 'g1' } as unknown as DownloadTaskView
    expect(build(member).includes('ignore-plugin-retry')).toBe(false)
  })
})
