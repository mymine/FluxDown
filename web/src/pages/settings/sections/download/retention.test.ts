import { describe, expect, test } from 'bun:test'
import {
  RETENTION_CUSTOM_START,
  RETENTION_PRESETS,
  retentionCustomTarget,
  retentionModeId,
} from './retention'

interface Fixture {
  presets: number[]
  customStart: number
  mode: { days: number; customFlag: boolean; expect: string }[]
  customTarget: { current: number; max: number; expect: number }[]
}

// 与 crates/settings/tests/fixtures 共用同一份用例（运行期读取：Docker 构建上下文不含 crates/）。
declare const Bun: { file(path: URL): { json(): Promise<unknown> } }
const fixture = (await Bun.file(new URL('../../../../../../crates/settings/tests/fixtures/retention_cases.json', import.meta.url)).json()) as Fixture

describe('retention mode (shared fixtures)', () => {
  test('constants', () => {
    expect([...RETENTION_PRESETS]).toEqual(fixture.presets)
    expect(RETENTION_CUSTOM_START).toBe(fixture.customStart)
  })
  for (const item of fixture.mode) {
    test(`mode days=${item.days} flag=${item.customFlag}`, () => {
      expect(retentionModeId(item.days, item.customFlag)).toBe(item.expect)
    })
  }
  for (const item of fixture.customTarget) {
    test(`custom target current=${item.current}`, () => {
      expect(retentionCustomTarget(item.current, item.max)).toBe(item.expect)
    })
  }
})
