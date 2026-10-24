import { describe, expect, test } from 'bun:test'
import { splitPathTail } from './pathDisplay'

describe('splitPathTail', () => {
  test('POSIX 与 Windows 路径保留末段及其分隔符', () => {
    expect(splitPathTail('/Users/zero/Downloads/Programs')).toEqual({ head: '/Users/zero/Downloads', tail: '/Programs' })
    expect(splitPathTail('C:\\Users\\张 三\\Downloads\\My  Apps')).toEqual({ head: 'C:\\Users\\张 三\\Downloads', tail: '\\My  Apps' })
    expect(splitPathTail('\\\\nas\\share\\下载')).toEqual({ head: '\\\\nas\\share', tail: '\\下载' })
  })

  test('结尾分隔符被去掉，根目录与盘符根保持原样', () => {
    expect(splitPathTail('/storage/emulated/0/Download/')).toEqual({ head: '/storage/emulated/0', tail: '/Download' })
    expect(splitPathTail('D:\\Video\\\\')).toEqual({ head: 'D:', tail: '\\Video' })
    expect(splitPathTail('/')).toEqual({ head: '', tail: '/' })
    expect(splitPathTail('D:\\')).toEqual({ head: '', tail: 'D:\\' })
    expect(splitPathTail('/Downloads')).toEqual({ head: '', tail: '/Downloads' })
    expect(splitPathTail('Downloads')).toEqual({ head: '', tail: 'Downloads' })
  })
})
