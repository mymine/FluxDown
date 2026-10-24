import { describe, expect, test } from 'bun:test'
import { judgeFrame } from './cursor'

const cursor = { epoch: 'e', sequence: 5 }
const at = (sequence: number, previous?: number, epoch = 'e') => ({ epoch, sequence, previous })

// 与 Rust `native/protocol/src/coalesce.rs` 的 `judge_frame_rules` 逐条对齐。
describe('judgeFrame（对齐 Rust judge_frame_rules）', () => {
  test('逐帧连续接受', () => {
    expect(judgeFrame(cursor, at(6))).toBe('accept')
  })
  test('快照之前的旧帧/重复帧跳过', () => {
    expect(judgeFrame(cursor, at(5))).toBe('skip')
    expect(judgeFrame(cursor, at(1))).toBe('skip')
    // sequence <= 游标优先于覆盖区间判定。
    expect(judgeFrame(cursor, at(5, 2))).toBe('skip')
  })
  test('无 previous 的断档要求重新 snapshot', () => {
    expect(judgeFrame(cursor, at(7))).toBe('resync')
  })
  test('合并帧：覆盖起点与游标相接或越过游标即接受', () => {
    expect(judgeFrame(cursor, at(9, 5))).toBe('accept')
    expect(judgeFrame(cursor, at(9, 2))).toBe('accept')
  })
  test('合并帧：覆盖起点在游标之后仍是断档', () => {
    expect(judgeFrame(cursor, at(9, 6))).toBe('resync')
  })
  test('epoch 变更要求重新 snapshot', () => {
    expect(judgeFrame(cursor, at(6, undefined, 'f'))).toBe('resync')
    expect(judgeFrame(cursor, at(5, undefined, 'f'))).toBe('resync')
  })
})
