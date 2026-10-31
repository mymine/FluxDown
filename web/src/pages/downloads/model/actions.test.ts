import { afterEach, beforeEach, describe, expect, test } from 'bun:test'
import { canCopyTaskFiles, canOpenTaskFile, canRevealTaskFile, isDownloadable } from './actions'
import type { DownloadTaskView } from './task'
import { CAPABILITY_AGENT_COPY_TASK_FILES, CAPABILITY_AGENT_OPEN_TASK_FILES, rpcStore } from '../../../lib/rpc'
import type { ServiceHello } from '../../../lib/rpc'

function mockHello(capabilities: string[]): ServiceHello {
  return {
    role: 'agent',
    serviceName: 'fluxdown-agent',
    serviceVersion: '0.1.0',
    protocolVersion: 1,
    instanceId: 'test',
    capabilities,
  }
}

function view(overrides: Partial<DownloadTaskView> = {}): DownloadTaskView {
  return {
    taskId: 't1',
    source: 'local',
    state: 'completed',
    fileMissing: false,
    ...overrides,
  } as unknown as DownloadTaskView
}

describe('actions task file permissions', () => {
  afterEach(() => {
    rpcStore.reset()
  })

  describe('无 agent.openTaskFiles 能力时（远程连接或无打开器宿主）', () => {
    beforeEach(() => {
      rpcStore.update((state) => ({ ...state, hello: mockHello([]) }), true)
    })

    test('即便本地已完成且文件存在，canOpenTaskFile 仍返回 false', () => {
      expect(canOpenTaskFile(view({}))).toBe(false)
    })

    test('即便为本地任务，canRevealTaskFile 仍返回 false', () => {
      expect(canRevealTaskFile(view({}))).toBe(false)
      expect(canRevealTaskFile(view({ state: 'downloading' }))).toBe(false)
      expect(canRevealTaskFile(view({ state: 'failed' }))).toBe(false)
    })

    test('未握手/hello 为空时一律返回 false', () => {
      rpcStore.reset()
      expect(canOpenTaskFile(view({}))).toBe(false)
      expect(canRevealTaskFile(view({}))).toBe(false)
    })
  })

  describe('具备 agent.openTaskFiles 能力时（本机来源 / 显式授权）', () => {
    beforeEach(() => {
      rpcStore.update(
        (state) => ({ ...state, hello: mockHello([CAPABILITY_AGENT_OPEN_TASK_FILES]) }),
        true,
      )
    })

    test('canOpenTaskFile：本地已完成且文件存在 → true', () => {
      expect(canOpenTaskFile(view({}))).toBe(true)
    })

    test('canOpenTaskFile：远程任务不可在宿主机打开', () => {
      expect(canOpenTaskFile(view({ source: 'remote' }))).toBe(false)
    })

    test('canOpenTaskFile：下载中 / 失败 / 文件缺失不可打开', () => {
      expect(canOpenTaskFile(view({ state: 'downloading' }))).toBe(false)
      expect(canOpenTaskFile(view({ state: 'failed' }))).toBe(false)
      expect(canOpenTaskFile(view({ fileMissing: true }))).toBe(false)
    })

    test('canRevealTaskFile：本地任务不论状态和文件状态均可定位（对齐 GPUI 定位保存目录/临时文件）', () => {
      expect(canRevealTaskFile(view({}))).toBe(true)
      expect(canRevealTaskFile(view({ state: 'downloading' }))).toBe(true)
      expect(canRevealTaskFile(view({ state: 'failed' }))).toBe(true)
      expect(canRevealTaskFile(view({ fileMissing: true }))).toBe(true)
    })

    test('canRevealTaskFile：远程任务不可定位', () => {
      expect(canRevealTaskFile(view({ source: 'remote' }))).toBe(false)
    })

    test('isDownloadable 独立于连接能力，仅由任务状态决定', () => {
      expect(isDownloadable(view({}))).toBe(true)
      expect(isDownloadable(view({ source: 'remote' }))).toBe(false)
      expect(isDownloadable(view({ fileMissing: true }))).toBe(false)
    })
  })

  describe('复制文件：只看 agent.copyTaskFiles（同机桌面会话），NAS / 远程连接不提供', () => {
    test('只有 openTaskFiles（如本机访问 headless server）时不提供复制文件', () => {
      rpcStore.update((state) => ({ ...state, hello: mockHello([CAPABILITY_AGENT_OPEN_TASK_FILES]) }), true)
      expect(canCopyTaskFiles([view({})])).toBe(false)
    })

    test('具备 copyTaskFiles：多选全部可复制才提供，任一不满足或空选区即隐藏', () => {
      rpcStore.update((state) => ({ ...state, hello: mockHello([CAPABILITY_AGENT_COPY_TASK_FILES]) }), true)
      expect(canCopyTaskFiles([view({}), view({ taskId: 't2' })])).toBe(true)
      expect(canCopyTaskFiles([])).toBe(false)
      expect(canCopyTaskFiles([view({}), view({ taskId: 't2', fileMissing: true })])).toBe(false)
      expect(canCopyTaskFiles([view({}), view({ taskId: 't2', state: 'downloading' })])).toBe(false)
      expect(canCopyTaskFiles([view({}), view({ taskId: 't2', source: 'remote' })])).toBe(false)
    })
  })
})
