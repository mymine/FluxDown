import { describe, expect, test } from 'bun:test'
import { isUploadValid, uploadLimitText, uploadLimitToSend } from './seedUpload'

describe('seed upload limit', () => {
  test('现值按 KB/s 预填，未设置显示 0', () => {
    expect(uploadLimitText(512 * 1024)).toBe('512')
    expect(uploadLimitText(0)).toBe('0')
    expect(uploadLimitText(undefined)).toBe('0')
  })

  test('未改动时保持原字节数，不因取整丢精度', () => {
    const original = 1_500_000
    const initial = uploadLimitText(original)
    expect(uploadLimitToSend(initial, initial, original)).toBe(original)
  })

  test('改动后按 KB/s 换算；0 与非法回到不限速', () => {
    expect(uploadLimitToSend('256', '512', 512 * 1024)).toBe(256 * 1024)
    expect(uploadLimitToSend('0', '512', 512 * 1024)).toBe(0)
    expect(uploadLimitToSend('abc', '512', 512 * 1024)).toBe(0)
  })

  test('原本未设置且未改动时发送 0', () => {
    expect(uploadLimitToSend('0', '0', undefined)).toBe(0)
  })

  test('校验非负整数', () => {
    expect(isUploadValid('0')).toBe(true)
    expect(isUploadValid('128')).toBe(true)
    expect(isUploadValid('')).toBe(false)
    expect(isUploadValid('1.5')).toBe(false)
  })
})
