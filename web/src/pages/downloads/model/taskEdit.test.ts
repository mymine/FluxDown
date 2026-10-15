import { describe, expect, test } from 'bun:test'
import { canChangeSegments, canChangeUrl, isBtUrl, isReplaceableUrl, parseTaskSegments } from './taskEdit'
import type { TaskState, TaskSource } from './task'

function facts(state: TaskState, over: { source?: TaskSource; url?: string } = {}) {
  return { source: over.source ?? 'local', state, url: over.url ?? 'https://example.com/a.bin' } as const
}

describe('更换链接可用性', () => {
  test('仅暂停 / 失败 / 等待', () => {
    for (const state of ['paused', 'failed', 'pending'] as const) expect(canChangeUrl(facts(state))).toBe(true)
    for (const state of ['downloading', 'completed'] as const) expect(canChangeUrl(facts(state))).toBe(false)
  })

  test('远程任务与非 http(s)/ftp 源不可用', () => {
    expect(canChangeUrl(facts('paused', { source: 'remote' }))).toBe(false)
    for (const url of ['magnet:?xt=urn:btih:abc', 'torrent-file://local', 'ed2k://|file|a|1|h|/', 'sftp://a/b', '']) {
      expect(canChangeUrl(facts('paused', { url }))).toBe(false)
    }
    for (const url of ['http://a/b', 'HTTPS://a/b', 'ftp://a/b']) {
      expect(canChangeUrl(facts('failed', { url }))).toBe(true)
    }
    // 引擎 `task_url_protocol` 不接受 ftps://。
    expect(canChangeUrl(facts('failed', { url: 'ftps://a/b' }))).toBe(false)
  })
})

describe('修改线程数可用性', () => {
  test('下载中也可，已完成不可', () => {
    for (const state of ['downloading', 'paused', 'failed', 'pending'] as const) {
      expect(canChangeSegments(facts(state))).toBe(true)
    }
    expect(canChangeSegments(facts('completed'))).toBe(false)
  })

  test('远程与 BT 任务不可', () => {
    expect(canChangeSegments(facts('paused', { source: 'remote' }))).toBe(false)
    expect(canChangeSegments(facts('paused', { url: 'magnet:?xt=urn:btih:abc' }))).toBe(false)
    expect(canChangeSegments(facts('paused', { url: 'torrent-file://local' }))).toBe(false)
  })
})

describe('URL 分类', () => {
  test('BT 与可换源互斥，ed2k 两者皆非', () => {
    expect(isBtUrl('MAGNET:?x')).toBe(true)
    expect(isReplaceableUrl('MAGNET:?x')).toBe(false)
    expect(isBtUrl('ed2k://x')).toBe(false)
    expect(isReplaceableUrl('ed2k://x')).toBe(false)
  })
})

describe('线程数输入解析', () => {
  test('0..=512 的整数，空串不等同自动', () => {
    expect(parseTaskSegments('0')).toBe(0)
    expect(parseTaskSegments(' 16 ')).toBe(16)
    expect(parseTaskSegments('512')).toBe(512)
    for (const input of ['', '  ', 'abc', '-1', '513', '1.5', '99999999999999999999']) {
      expect(parseTaskSegments(input)).toBeNull()
    }
  })
})
