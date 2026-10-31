import { describe, expect, test } from 'bun:test'
import { flagKey, flagTitle, normalizeCountry, regionName, resolveFlagUrl, UNKNOWN_FLAG } from './flags'

const urls = { cn: '/f/cn.svg', us: '/f/us.svg', [UNKNOWN_FLAG]: '/f/xx.svg' }

describe('国家码规整', () => {
  test('大小写 / 空白归一为大写两位', () => {
    expect(normalizeCountry('cn')).toBe('CN')
    expect(normalizeCountry(' Us ')).toBe('US')
  })
  test('缺省与非法 = 空', () => {
    for (const bad of [undefined, null, '', ' ', 'C', 'CHN', 'C1', '中国', 'gb-eng', '../cn']) {
      expect(normalizeCountry(bad)).toBe('')
      expect(flagKey(bad)).toBeNull()
    }
  })
  test('资源主名为小写', () => {
    expect(flagKey('CN')).toBe('cn')
  })
})

describe('国旗 URL 解析', () => {
  test('命中', () => {
    expect(resolveFlagUrl(urls, 'CN')).toBe('/f/cn.svg')
    expect(resolveFlagUrl(urls, 'us')).toBe('/f/us.svg')
  })
  test('码合法但缺文件 → 未知旗', () => {
    expect(resolveFlagUrl(urls, 'ZZ')).toBe('/f/xx.svg')
  })
  test('连未知旗也缺 → null，不抛错', () => {
    expect(resolveFlagUrl({ cn: '/f/cn.svg' }, 'ZZ')).toBeNull()
    expect(resolveFlagUrl({}, 'CN')).toBeNull()
  })
  test('非法 / 缺省 → null（留空位）', () => {
    expect(resolveFlagUrl(urls, undefined)).toBeNull()
    expect(resolveFlagUrl(urls, '')).toBeNull()
    expect(resolveFlagUrl(urls, 'toString')).toBeNull()
  })
  test('原型链键不被当作国旗', () => {
    expect(resolveFlagUrl({}, 'TO')).toBeNull()
  })
})

describe('地区名与提示', () => {
  test('按界面语言本地化', () => {
    expect(regionName('US', 'en')).toBe('United States')
    expect(regionName('DE', 'de')).toBe('Deutschland')
  })
  test('提示 = 地区名 (CC)', () => {
    expect(flagTitle('US', 'en')).toBe('United States (US)')
  })
  test('无法识别的 locale 退回国家码', () => {
    expect(regionName('US', 'not a locale!!')).toBe('US')
    expect(flagTitle('US', 'not a locale!!')).toBe('US')
  })
})
