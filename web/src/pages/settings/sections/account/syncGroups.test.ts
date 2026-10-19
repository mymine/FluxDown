import { describe, expect, test } from 'bun:test'
import { SYNC_GROUPS, syncGroupState, syncPhase, toggleGroupParams } from './syncGroups'

// tsconfig 未引入 Bun 类型，仅声明测试用到的最小接口。
declare const Bun: { file(path: URL): { text(): Promise<string> } }

const group = (id: string) => {
  const found = SYNC_GROUPS.find((item) => item.id === id)
  if (!found) throw new Error(`missing group ${id}`)
  return found
}

describe('同步分组', () => {
  test('键表与 Rust SYNC_SETTING_SPECS 一致（无遗漏、无多余、无重复）', async () => {
    const source = await Bun.file(new URL('../../../../../../native/protocol/src/settings.rs', import.meta.url)).text()
    const rust = [...source.matchAll(/spec!\(\s*"([^"]+)"/g)].map((match) => match[1] as string)
    const web = SYNC_GROUPS.flatMap((item) => item.keys)
    expect(new Set(web).size).toBe(web.length)
    expect([...web].sort()).toEqual([...rust].sort())
    for (const item of SYNC_GROUPS) {
      if (item.id === 'categories') continue
      expect(item.keys.every((key) => key.startsWith(`${item.id}.`))).toBe(true)
    }
  })

  test('分组状态：全同步 / 全本机 / 混合；点击仅在全同步时转本机', () => {
    const bt = group('bt')
    expect(syncGroupState(bt, [])).toBe('sync')
    expect(syncGroupState(bt, [...bt.keys])).toBe('local')
    expect(syncGroupState(bt, [bt.keys[0] as string])).toBe('mixed')
    expect(syncGroupState(bt, ['download.keep_awake'])).toBe('sync')
    expect(toggleGroupParams(bt, 'sync').localOnly).toBe(true)
    expect(toggleGroupParams(bt, 'local').localOnly).toBe(false)
    expect(toggleGroupParams(bt, 'mixed').localOnly).toBe(false)
    expect(toggleGroupParams(bt, 'sync').keys).toEqual([...bt.keys])
  })

  test('同步状态优先级：halted 高于错误，错误高于连接中', () => {
    const base = { enabled: true, lastError: null, dirtyKeys: [] as string[] }
    expect(syncPhase({ ...base, enabled: false, halted: true })).toBe('off')
    expect(syncPhase({ ...base, halted: true, lastError: 'x' })).toBe('halted')
    expect(syncPhase({ ...base, lastErrorReason: 'onlineDeviceLimit' })).toBe('error')
    expect(syncPhase({ ...base, connected: false })).toBe('connecting')
    expect(syncPhase({ ...base, connected: true, dirtyKeys: ['a'] })).toBe('syncing')
    expect(syncPhase({ ...base, connected: true })).toBe('synced')
    // 旧 agent 不带 connected 字段：不误报连接中
    expect(syncPhase(base)).toBe('synced')
  })
})
