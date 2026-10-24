import { describe, expect, test } from 'bun:test'
import { SPEED_LIMIT_RECENT_LIMIT, pushRecent, readSpeedLimitRecent } from './speedLimitRecent'

interface Fixture {
  limit: number
  read: { name: string; value: unknown; expected: number[] }[]
  push: { name: string; recent: number[]; kbps: number; expected: number[] | null }[]
}

// 与 crates/downloads/tests/fixtures 共用同一份用例（运行期读取：Docker 构建上下文不含 crates/）。
// tsconfig 未引入 Bun 类型，仅声明测试用到的最小接口。
declare const Bun: { file(path: URL): { json(): Promise<unknown> } }
const fixture = (await Bun.file(new URL('../../../crates/downloads/tests/fixtures/speed_limit_recent_cases.json', import.meta.url)).json()) as Fixture

describe('speed limit recent (shared fixtures)', () => {
  test('limit', () => {
    expect(SPEED_LIMIT_RECENT_LIMIT).toBe(fixture.limit)
  })
  for (const item of fixture.read) {
    test(`read: ${item.name}`, () => {
      expect(readSpeedLimitRecent(item.value)).toEqual(item.expected)
    })
  }
  for (const item of fixture.push) {
    test(`push: ${item.name}`, () => {
      expect(pushRecent(item.recent, item.kbps)).toEqual(item.expected)
    })
  }
})
