import { describe, expect, test } from 'bun:test'
import { clockText, niceCeil, pointElapsedSecs } from './speedChart'

describe('speedChart', () => {
  test('point elapsed time caps the partial last bucket and stays increasing', () => {
    expect(pointElapsedSecs(2, 5, 3)).toEqual([2, 4, 5])
    // 统计秒数落后于点数（过期快照）时仍严格递增，横坐标不重叠。
    expect(pointElapsedSecs(1, 1, 3)).toEqual([1, 2, 3])
  })

  test('clock text switches to hours past one hour', () => {
    expect(clockText(5)).toBe('0:05')
    expect(clockText(125)).toBe('2:05')
    expect(clockText(3725)).toBe('1:02:05')
  })

  test('axis max rounds the peak up to a nice value', () => {
    expect(niceCeil(7_500_000)).toBe(10_000_000)
    expect(niceCeil(5_189_661)).toBe(10_000_000)
    expect(niceCeil(2_100)).toBe(2_500)
    expect(niceCeil(2_000)).toBe(2_000)
    expect(niceCeil(0)).toBe(1)
  })
})
