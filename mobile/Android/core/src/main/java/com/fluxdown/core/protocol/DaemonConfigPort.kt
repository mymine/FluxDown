package com.fluxdown.core.protocol

import com.fluxdown.core.host.HostErrorCode
import com.fluxdown.core.host.HostException
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

/* 整键 JSON 数组配置（`webhook.endpoints`、`proxy_rules`）的读-改-写：同 GPUI `SettingsStore::mutate_daemon`、Web `createConfigListPatcher`、iOS `DaemonConfigListWriter`。 */

/** `daemon.config.get` 结果（`DaemonConfigSnapshot`）。 */
data class DaemonConfigSnapshot(val revision: Long, val values: Map<String, String>) {
    companion object {
        fun fromJson(v: JsonValue?): DaemonConfigSnapshot = DaemonConfigSnapshot(v.long("revision"), v.stringMap("values"))
    }
}

/** 列表写入依赖的主机端口（生产实现读 `HostStore` + `HostSession`；测试用假实现）。 */
interface DaemonConfigPort {
    /** 快照里的最新配置；未连接为 null。 */
    val cachedConfig: DaemonConfigSnapshot?

    /** `daemon.config.get`。 */
    suspend fun fetchConfig(): DaemonConfigSnapshot

    /** `daemon.config.patch`：revision 不符抛 [HostErrorCode.Conflict]。 */
    suspend fun patchConfig(expectedRevision: Long, values: Map<String, String>)
}

/**
 * 读-改-写某个整键 JSON 数组配置（AGENTS.md §5 镜像契约：写入始终基于最新配置重算，冲突时重读并重放）。
 *
 * 这是唯一允许绕过 `ConfigEditor` 的设置写入：整键覆盖会把别的客户端刚加的条目冲掉，所以每次写入都要
 * 「取最新值 → 应用幂等变更 → `expectedRevision` 补丁 → Conflict 重取重放」。
 *
 * - 冲突时重新 `daemon.config.get` 后再算一遍（最多 [MAX_CONFLICT_RETRIES] 次）。
 * - 写入串行化（FIFO 互斥）。
 */
class DaemonConfigListWriter<T>(
    private val port: DaemonConfigPort,
    private val key: String,
    private val parse: (String?) -> List<T>,
    private val encode: (List<T>) -> String,
) {
    private val lock = Mutex()

    /** [transform] 返回 null 表示无需写入。失败抛 [HostException]。 */
    suspend fun mutate(transform: (List<T>) -> List<T>?) {
        lock.withLock { perform(transform) }
    }

    private suspend fun perform(transform: (List<T>) -> List<T>?) {
        var config = port.cachedConfig
        var attempt = 0
        while (true) {
            val current = config ?: port.fetchConfig()
            val next = transform(parse(current.values[key])) ?: return
            val encoded = encode(next)
            try {
                port.patchConfig(current.revision, mapOf(key to encoded))
                return
            } catch (e: HostException) {
                if (e.code != HostErrorCode.Conflict || attempt >= MAX_CONFLICT_RETRIES) throw e
                attempt += 1
                config = port.fetchConfig()
            }
        }
    }

    companion object {
        /** 与 GPUI `MAX_CONFLICT_RETRIES` 一致。 */
        const val MAX_CONFLICT_RETRIES = 3
    }
}
