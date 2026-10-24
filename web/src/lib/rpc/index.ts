// RPC 层对外入口：页面 agent 只需从这里导入。
export { rpc } from './methods'
export { call, retryConnection, startConnection, stopConnection, subscribeServiceEvents } from './client'
export type { CallOptions } from './client'
export { RpcError, errorMessage } from './error'
export { tablesLoading } from './apply'
export {
  shallowEqual,
  useAgent,
  useAgentSnapshot,
  useConfigValue,
  useConfigValues,
  useConnection,
  useDaemon,
  usePref,
  usePrefBool,
  usePreferences,
  useRpcSelector,
  useServiceEvents,
  useTask,
  useTaskRuntime,
  useTaskTableStatus,
  useTasks,
  useTasksLoading,
} from './hooks'
export type { TaskTableStatus } from './hooks'
export { UploadError, downloadLogExport, downloadTaskFile, exportLogs, logExportUrl, taskFileUrl, triggerDownload, uploadBlob } from './http'
export type { BlobKind, UploadOptions } from './http'
export { rpcStore } from './store'
export type { ConnectionPhase, ConnectionState, RpcState } from './store'
export * from './protocol'
