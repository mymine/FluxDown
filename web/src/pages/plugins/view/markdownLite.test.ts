import { describe, expect, test } from 'bun:test'
import { splitMarkdown } from './markdownLite'

describe('splitMarkdown', () => {
  test('paragraphs and fenced code', () => {
    expect(splitMarkdown('a\nb\n\n```ts\nlet x\n\nlet y\n```\nc')).toEqual([
      { type: 'paragraph', text: 'a\nb' },
      { type: 'code', text: 'let x\n\nlet y', lang: 'ts' },
      { type: 'paragraph', text: 'c' },
    ])
  })
  test('unterminated fence is still code', () => {
    expect(splitMarkdown('```\nx')).toEqual([{ type: 'code', text: 'x', lang: '' }])
  })
  test('empty input', () => {
    expect(splitMarkdown('')).toEqual([])
  })
})
