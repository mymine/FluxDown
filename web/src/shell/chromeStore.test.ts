import { describe, expect, test } from 'bun:test'
import { CHROME_LAYOUT_KEY, getChromeLayout, moveChromeItem, resetChromeLayout } from './chromeStore'

class MemoryStorage {
  private readonly data = new Map<string, string>()
  getItem(key: string): string | null {
    return this.data.get(key) ?? null
  }
  setItem(key: string, value: string): void {
    this.data.set(key, value)
  }
  removeItem(key: string): void {
    this.data.delete(key)
  }
}

// 存储在首次读取 / 写入时才访问，桩只需在首个用例前装好。
const local = new MemoryStorage()
Object.assign(globalThis, { localStorage: local })

const stored = () => JSON.parse(local.getItem(CHROME_LAYOUT_KEY) ?? 'null') as Record<string, string[]>

describe('chromeStore', () => {
  test('默认布局复现改造前的结构', () => {
    const layout = getChromeLayout()
    expect(layout.railTop).toEqual(['downloads', 'rss', 'webhooks'])
    expect(layout.railBottom).toEqual(['account', 'settings'])
    expect(layout.statusLeading).toEqual(['downloadSpeed', 'uploadSpeed', 'pauseAll', 'resumeAll'])
    expect(layout.statusTrailing).toEqual(['downloadLimit', 'uploadLimit', 'proxy', 'diskFree'])
    expect(layout.titleLeading).toEqual([])
    expect(layout.titleTrailing).toEqual([])
  })

  test('成功的移动立即写入 localStorage，且写全六个区域', () => {
    expect(moveChromeItem('proxy', 'titleTrailing', null)).toBe(true)
    expect(Object.keys(stored())).toHaveLength(6)
    expect(stored().titleTrailing).toEqual(['proxy'])
    expect(stored().statusTrailing).toEqual(['downloadLimit', 'uploadLimit', 'diskFree'])
  })

  test('被拒绝的移动不写入；重置写入默认布局', () => {
    expect(moveChromeItem('downloadSpeed', 'railTop', null)).toBe(false)
    resetChromeLayout()
    expect(stored().titleTrailing).toEqual([])
    expect(stored().statusTrailing).toEqual(['downloadLimit', 'uploadLimit', 'proxy', 'diskFree'])
  })
})
