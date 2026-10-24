/** 镜像 `native/engine/src/downloader.rs` 的 `POST_ONLY_ENDPOINT_MESSAGE`；改动需两边同步。 */
export const POST_ONLY_ENDPOINT_MESSAGE = 'This link only accepts POST requests (HTTP 405); start the download from the browser'

/** 镜像 `native/engine/src/downloader.rs` 的 `POST_ONLY_RESUME_MESSAGE`；改动需两边同步。 */
export const POST_ONLY_RESUME_MESSAGE = 'This task was started by a browser POST request and cannot be resumed; start it again from the browser'

/** 引擎稳定错误消息（整串 trim 后精确匹配）→ i18n key；其余返回 null。 */
export function taskErrorKey(message: string): string | null {
  switch (message.trim()) {
    case POST_ONLY_ENDPOINT_MESSAGE:
      return 'taskErrorPostOnlyEndpoint'
    case POST_ONLY_RESUME_MESSAGE:
      return 'taskErrorPostOnlyResume'
    default:
      return null
  }
}

/** 展示用：已知引擎消息本地化，其余原样返回。 */
export function localizeTaskError(t: (key: string) => string, message: string): string {
  const key = taskErrorKey(message)
  return key === null ? message : t(key)
}
