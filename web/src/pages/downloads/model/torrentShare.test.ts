import { describe, expect, test } from 'bun:test'
import { canShareTorrent, torrentFileName } from './actions'
import type { DownloadTaskView } from './task'

const view = (source: string, protocol: string) => ({ source, protocol }) as unknown as DownloadTaskView

describe('torrentFileName', () => {
  test('清理非法字符并补扩展名', () => {
    expect(torrentFileName('a/b:c')).toBe('a_b_c.torrent')
    expect(torrentFileName('x.TORRENT')).toBe('x.TORRENT')
    expect(torrentFileName('  ')).toBe('torrent.torrent')
  })
})

describe('canShareTorrent', () => {
  test('仅单选本地 BT', () => {
    expect(canShareTorrent([view('local', 'bt')])).toBe(true)
    expect(canShareTorrent([view('local', 'http')])).toBe(false)
    expect(canShareTorrent([view('remote', 'bt')])).toBe(false)
    expect(canShareTorrent([view('local', 'bt'), view('local', 'bt')])).toBe(false)
  })
})
