import { describe, expect, test } from 'bun:test'
import type { MarketEntryDto } from '../../../../lib/rpc/protocol'
import { arrangeMarket } from './logic'
import type { MarketSort } from './logic'

interface Case {
  name: string
  entries: { pluginId: string; publishTime: string }[]
  counts: Record<string, number>
  mine: string[]
  sort: MarketSort
  favoritesOnly: boolean
  expected: string[]
}

// 与 crates/extensions/tests/fixtures 共用同一份用例（运行期读取：Docker 构建上下文不含 crates/）。
declare const Bun: { file(path: URL): { json(): Promise<unknown> } }
const fixture = (await Bun.file(new URL('../../../../../../crates/extensions/tests/fixtures/market_sort_cases.json', import.meta.url)).json()) as { cases: Case[] }

describe('arrangeMarket (shared fixtures)', () => {
  for (const item of fixture.cases) {
    test(item.name, () => {
      const entries = item.entries as unknown as MarketEntryDto[]
      const result = arrangeMarket(entries, item.sort, item.counts, new Set(item.mine), item.favoritesOnly)
      expect(result.map((entry) => entry.pluginId)).toEqual(item.expected)
    })
  }
})
