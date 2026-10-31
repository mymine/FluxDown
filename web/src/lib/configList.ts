// daemon 配置键里「JSON 数组字符串」的读-改-写（webhook 端点、站点代理规则共用）。

import { RpcError, rpc, rpcStore } from './rpc'

/** 与 GPUI `MAX_CONFLICT_RETRIES` 一致：修订冲突时的自动重试上限。 */
const MAX_CONFLICT_RETRIES = 3

/**
 * 创建某个配置键的列表写入器。`mutate` 返回 `null` 表示无需写入。
 * 每次都基于最新快照重新计算，`expectedRevision` 冲突时重新 `config.get` 后再算一遍
 * （最多 3 次），避免覆盖别处（另一浏览器 / GPUI 客户端）的并发修改。
 * 写入串行化：连续操作不会互相制造冲突。
 */
export function createConfigListPatcher<T>(
  key: string,
  parse: (raw: string | undefined) => T[],
  encode: (list: T[]) => string,
): (mutate: (list: T[]) => T[] | null) => Promise<void> {
  let writeChain: Promise<unknown> = Promise.resolve()
  return (mutate) => {
    const run = async (): Promise<void> => {
      let config = rpcStore.peek().snapshot?.daemon.config
      for (let attempt = 0; ; attempt += 1) {
        if (!config) config = await rpc.daemon.config.get()
        const next = mutate(parse(config.values[key]))
        if (next === null) return
        try {
          await rpc.daemon.config.patch({
            expectedRevision: config.revision,
            values: { [key]: encode(next) },
          })
          return
        } catch (error) {
          if (error instanceof RpcError && error.is('conflict') && attempt < MAX_CONFLICT_RETRIES) {
            config = await rpc.daemon.config.get()
            continue
          }
          throw error
        }
      }
    }
    const result = writeChain.then(run, run)
    writeChain = result.catch(() => undefined)
    return result
  }
}
