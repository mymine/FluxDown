import { describe, expect, test } from 'bun:test'
import type { TaskDto } from '../../../lib/rpc'
import { aggregateOf, anyActive, anyResumable, compressPath, displayName, flatten, groupSortView, memberDir, sparklineHeight, sparklineSample } from './groupTree'
import type { TreeRow } from './groupTree'
import { buildLocalView, stateOfStatus } from './task'
import type { DownloadTaskView } from './task'

interface FixtureMember {
  id: string
  name: string
  status?: number
  totalBytes: number
  downloadedBytes?: number
  speed?: number
  saveDir?: string
}

interface Fixture {
  memberDir: { groupDir: string; taskDir: string; expect: string }[]
  displayName: { name: string; saveDir: string; id: string; expect: string }[]
  compressPath: { path: string; expect: string }[]
  sparklineSample: { len: number; max: number; expect: number[] }[]
  sparklineHeight: { progress: number; status: number; expect: number }[]
  aggregate: {
    title: string
    members: FixtureMember[]
    expect: {
      state: string
      total: number
      done: number
      downloading: number
      pending: number
      paused: number
      failed: number
      sizeBytes: number
      downloadedBytes: number
      speed: number
      progress: number
      etaSeconds: number | null
    }
  }[]
  flatten: {
    title: string
    groupDir: string
    collapsedDirs: string[]
    members: FixtureMember[]
    expect: ({ member: string; nested: boolean } | { dir: string; count: number; sizeBytes: number; collapsed: boolean })[]
  }[]
}

// 与 crates/downloads/tests/fixtures 共用同一份用例（运行期读取：Docker 构建上下文不含 crates/）。
// tsconfig 未引入 Bun 类型，仅声明测试用到的最小接口。
declare const Bun: { file(path: URL): { json(): Promise<unknown> } }
const fixture = (await Bun.file(new URL('../../../../../crates/downloads/tests/fixtures/group_tree_cases.json', import.meta.url)).json()) as Fixture

/** 与 Rust 测试的 `Member::view` 同构：本地任务视图（引擎 status、字节数、实时速度）。 */
function viewOf(member: FixtureMember): DownloadTaskView {
  const dto = {
    taskId: member.id,
    url: 'https://example.com/file',
    fileName: member.name,
    saveDir: member.saveDir ?? '',
    status: member.status ?? 0,
    downloadedBytes: member.downloadedBytes ?? 0,
    totalBytes: member.totalBytes,
    errorMessage: '',
    createdAt: '1',
    completedAt: '',
    proxyUrl: '',
    queueId: 'main',
    checksum: '',
    referrer: '',
    groupId: '',
    originUrl: '',
    fileMissing: false,
    seedingStatus: 0,
    uploadedBytes: 0,
    queueOrder: 0,
  } as unknown as TaskDto
  return buildLocalView(dto, member.speed ?? 0, false, null, undefined, true)
}

describe('group tree (shared fixtures)', () => {
  for (const item of fixture.memberDir) {
    test(`memberDir: ${item.groupDir} / ${item.taskDir}`, () => {
      expect(memberDir(item.groupDir, item.taskDir)).toBe(item.expect)
    })
  }
  for (const item of fixture.displayName) {
    test(`displayName: ${JSON.stringify(item.name)} / ${item.saveDir}`, () => {
      expect(displayName(item.name, item.saveDir, item.id)).toBe(item.expect)
    })
  }
  for (const item of fixture.compressPath) {
    test(`compressPath: ${JSON.stringify(item.path)}`, () => {
      expect(compressPath(item.path)).toBe(item.expect)
    })
  }
  for (const item of fixture.sparklineSample) {
    test(`sparklineSample: ${item.len} / ${item.max}`, () => {
      expect(sparklineSample(item.len, item.max)).toEqual(item.expect)
    })
  }
  for (const item of fixture.sparklineHeight) {
    test(`sparklineHeight: ${item.progress} / status ${item.status}`, () => {
      expect(Math.abs(sparklineHeight(item.progress, stateOfStatus(item.status)) - item.expect) < 1e-3).toBe(true)
    })
  }
  for (const item of fixture.aggregate) {
    test(`aggregate: ${item.title}`, () => {
      const aggregate = aggregateOf(item.members.map(viewOf))
      expect({
        state: aggregate.state,
        total: aggregate.counts.total,
        done: aggregate.counts.done,
        downloading: aggregate.counts.downloading,
        pending: aggregate.counts.pending,
        paused: aggregate.counts.paused,
        failed: aggregate.counts.failed,
        sizeBytes: aggregate.sizeBytes,
        downloadedBytes: aggregate.downloadedBytes,
        speed: aggregate.speed,
        etaSeconds: aggregate.etaSeconds,
      }).toEqual({
        state: item.expect.state as never,
        total: item.expect.total,
        done: item.expect.done,
        downloading: item.expect.downloading,
        pending: item.expect.pending,
        paused: item.expect.paused,
        failed: item.expect.failed,
        sizeBytes: item.expect.sizeBytes,
        downloadedBytes: item.expect.downloadedBytes,
        speed: item.expect.speed,
        etaSeconds: item.expect.etaSeconds,
      })
      expect(Math.abs(aggregate.progress - item.expect.progress) < 1e-6).toBe(true)
    })
  }
  for (const item of fixture.flatten) {
    test(`flatten: ${item.title}`, () => {
      const members = item.members.map((member) => ({
        dir: memberDir(item.groupDir, member.saveDir ?? ''),
        nameFold: member.name.toLowerCase(),
        sizeBytes: Math.max(0, member.totalBytes),
      }))
      const rows = flatten(members, (dir) => item.collapsedDirs.includes(dir))
      const actual = rows.map((row: TreeRow) =>
        row.type === 'member'
          ? { member: (item.members[row.index] as FixtureMember).id, nested: row.nested }
          : { dir: row.path, count: row.count, sizeBytes: row.sizeBytes, collapsed: row.collapsed },
      )
      expect(actual).toEqual(item.expect)
    })
  }
})

describe('group aggregate helpers', () => {
  test('anyActive / anyResumable', () => {
    const paused = aggregateOf([viewOf({ id: 'a', name: 'a', status: 2, totalBytes: 1 })]).counts
    expect(anyActive(paused)).toBe(false)
    expect(anyResumable(paused)).toBe(true)
    const preparing = aggregateOf([viewOf({ id: 'a', name: 'a', status: 5, totalBytes: 1 })]).counts
    expect(anyActive(preparing)).toBe(true)
  })

  test('sort proxy projects the aggregate; earliest positive member time fills a missing group time', () => {
    const early = { ...viewOf({ id: 'a', name: 'a.bin', status: 1, totalBytes: 100, downloadedBytes: 50, speed: 10 }), createdAtSecs: 50, queueOrder: 7, queueKey: 9 }
    const late = { ...viewOf({ id: 'b', name: 'b.bin', status: 3, totalBytes: 100, downloadedBytes: 100 }), createdAtSecs: 80, queueOrder: 3, queueKey: 4 }
    const members = [early, late]
    const proxy = groupSortView(
      { groupId: 'g1', name: 'Pack', sourceUrl: 'https://host.example/x', createdAtSecs: 0 },
      aggregateOf(members),
      members,
    )
    expect(proxy).toMatchObject({
      name: 'Pack',
      nameFold: 'pack',
      sizeBytes: 200,
      downloadedBytes: 150,
      speed: 10,
      state: 'downloading',
      createdAtSecs: 50,
      queueOrder: 3,
      queueKey: 4,
      queueId: 'main',
      groupId: 'g1',
      url: 'https://host.example/x',
      originUrl: 'https://host.example/x',
      site: 'host.example',
      preparing: false,
    })
    expect(Math.abs(proxy.progress - 0.75) < 1e-6).toBe(true)
    const dated = groupSortView({ groupId: 'g1', name: 'Pack', sourceUrl: '', createdAtSecs: 99 }, aggregateOf(members), members)
    expect(dated.createdAtSecs).toBe(99)
  })
})
