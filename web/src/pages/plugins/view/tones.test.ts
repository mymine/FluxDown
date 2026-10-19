import { describe, expect, test } from 'bun:test'
import { TONE_TOKENS, toneBackground, toneColor } from './tones'

// 与 native/protocol/src/plugin_view_tones.json 共用（运行期读取）。
declare const Bun: { file(path: URL): { json(): Promise<unknown> } }
const fixture = (await Bun.file(new URL('../../../../../native/protocol/src/plugin_view_tones.json', import.meta.url)).json()) as Record<string, string>

describe('plugin view tones', () => {
  test('table matches the shared fixture', () => {
    expect({ ...TONE_TOKENS }).toEqual(fixture)
  })
  test('maps to --fx-colors-* variables', () => {
    expect(toneColor('muted')).toBe('var(--fx-colors-muted-foreground)')
    expect(toneColor('danger')).toBe('var(--fx-colors-destructive)')
  })
  test('unknown tone falls back to neutral', () => {
    expect(toneColor('bogus')).toBe('var(--fx-colors-foreground)')
    expect(toneBackground(undefined).includes('--fx-colors-foreground')).toBe(true)
  })
})
