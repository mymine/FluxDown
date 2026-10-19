import { describe, expect, test } from 'bun:test'
import { VIEW_ERROR_PROP, ViewApplyError, applyViewOps } from './pluginView'
import type { ViewNode, ViewOp } from './pluginView'

interface Case {
  name: string
  oldTree: ViewNode
  newTree: ViewNode
  ops: ViewOp[]
}

// 与 native/protocol/tests/fixtures 共用同一份用例（运行期读取：Docker 构建上下文不含 native/）。
declare const Bun: { file(path: URL): { json(): Promise<unknown> } }
const cases = (await Bun.file(new URL('../../../../../native/protocol/tests/fixtures/plugin_view_apply.json', import.meta.url)).json()) as Case[]

describe('applyViewOps (shared fixtures)', () => {
  test('fixture is present', () => {
    expect(cases.length >= 15).toBe(true)
  })
  for (const item of cases) {
    test(item.name, () => {
      const before = JSON.stringify(item.oldTree)
      expect(applyViewOps(item.oldTree, item.ops)).toEqual(item.newTree)
      expect(JSON.stringify(item.oldTree)).toBe(before)
    })
  }
})

describe('applyViewOps edge cases', () => {
  const root: ViewNode = { id: 'root', k: 'stack', p: {}, c: [] }

  test('error op writes root __error and props.unset clears it', () => {
    const failed = applyViewOps(root, [{ op: 'error', message: 'boom' }])
    expect(failed.p[VIEW_ERROR_PROP]).toBe('boom')
    const cleared = applyViewOps(failed, [{ op: 'props', id: 'root', set: {}, unset: [VIEW_ERROR_PROP] }])
    expect(cleared.p[VIEW_ERROR_PROP]).toBeUndefined()
  })

  test('unknown node id throws', () => {
    expect(() => applyViewOps(root, [{ op: 'props', id: 'root/x', set: {}, unset: [] }])).toThrow(ViewApplyError)
  })
})
