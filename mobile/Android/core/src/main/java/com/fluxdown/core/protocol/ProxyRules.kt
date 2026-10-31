package com.fluxdown.core.protocol

/*
 * 站点代理规则（配置键 `proxy_rules` + `daemon.config.proxyRoute`）：
 * 宽松解析对齐 `native/protocol/src/proxy_rules.rs`、Web `lib/proxyRules.ts`、iOS `ProxyRules.swift`；
 * 匹配只在 engine 实现，客户端只调用 RPC，不在本地匹配。
 */

/** 一条规则；数组顺序即优先级。 */
data class ProxyRule(
    val id: String,
    val enabled: Boolean = true,
    val name: String = "",
    /** `domain` / `regex`。 */
    val match: String = MATCH_DOMAIN,
    val pattern: String = "",
    /** `direct` / `system` / `manual` / `custom`。 */
    val action: String = ACTION_DIRECT,
    /** 仅 `custom` 使用。 */
    val proxy: String = "",
) {
    fun toJson(): JsonValue = jsonObject(
        "id" to id,
        "enabled" to enabled,
        "name" to name,
        "match" to match,
        "pattern" to pattern,
        "action" to action,
        "proxy" to proxy,
    )

    companion object {
        const val CONFIG_KEY = "proxy_rules"
        const val MATCH_DOMAIN = "domain"
        const val MATCH_REGEX = "regex"
        const val ACTION_DIRECT = "direct"
        const val ACTION_SYSTEM = "system"
        const val ACTION_MANUAL = "manual"
        const val ACTION_CUSTOM = "custom"

        val actions: List<String> = listOf(ACTION_DIRECT, ACTION_SYSTEM, ACTION_MANUAL, ACTION_CUSTOM)

        /** 非对象为 null；缺失 / 类型错误 / 未知取值回退默认（`match`→domain，`action`→direct，`enabled`→true）。 */
        fun fromJsonLenient(v: JsonValue?): ProxyRule? {
            if (v !is JsonValue.Obj) return null
            val match = v.str("match")
            val action = v.str("action")
            return ProxyRule(
                id = v.str("id"),
                enabled = v.bool("enabled", true),
                name = v.str("name"),
                match = if (match == MATCH_REGEX) MATCH_REGEX else MATCH_DOMAIN,
                pattern = v.str("pattern"),
                action = if (action in actions) action else ACTION_DIRECT,
                proxy = v.str("proxy"),
            )
        }

        /** 配置串 → 规则列表：空串 / 非法 JSON / 非数组为空列表；非对象元素跳过。 */
        fun parseList(raw: String?): List<ProxyRule> {
            if (raw == null || raw.isBlank()) return emptyList()
            val items = Json.parseOrNull(raw).arrayOrNull ?: return emptyList()
            return items.mapNotNull { fromJsonLenient(it) }
        }

        fun encodeList(list: List<ProxyRule>): String = JsonValue.Arr(list.map { it.toJson() }).toJson()

        /** 已启用的规则条数。 */
        fun enabledCount(raw: String?): Int = parseList(raw).count { it.enabled }

        /** 展示用代理端点：去掉凭据与末尾 `/`（同 `proxy_display_endpoint`）。 */
        fun displayEndpoint(url: String): String {
            val trimmed = url.trim()
            val sep = trimmed.indexOf("://")
            val scheme = if (sep >= 0) trimmed.substring(0, sep) else null
            val rest = if (sep >= 0) trimmed.substring(sep + 3) else trimmed
            val host = rest.substringAfterLast('@').trimEnd('/')
            return if (scheme != null) "$scheme://$host" else host
        }

        /** `manual` 出口需要手动字段完整（host 非空且 port 在 1..65535）。 */
        fun manualConfigured(host: String, port: String): Boolean =
            host.isNotBlank() && (port.trim().toIntOrNull() ?: 0) in 1..65535
    }
}

/** 规则列表的写入（读-改-写，冲突重放）。 */
class ProxyRuleWriter(port: DaemonConfigPort) {
    private val writer = DaemonConfigListWriter<ProxyRule>(port, ProxyRule.CONFIG_KEY, { ProxyRule.parseList(it) }, { ProxyRule.encodeList(it) })

    suspend fun mutate(transform: (List<ProxyRule>) -> List<ProxyRule>?) = writer.mutate(transform)

    suspend fun upsert(draft: ProxyRule) = mutate { upsertRule(it, draft) }

    suspend fun setEnabled(id: String, enabled: Boolean) = mutate { list ->
        if (list.none { it.id == id }) null else list.map { if (it.id == id) it.copy(enabled = enabled) else it }
    }

    suspend fun remove(id: String) = mutate { list -> list.filter { it.id != id } }

    /** 按 [delta] 移动；id 缺失或越界不写入。 */
    suspend fun move(id: String, delta: Int) = mutate { list ->
        val from = list.indexOfFirst { it.id == id }
        val to = from + delta
        if (from < 0 || delta == 0 || to !in list.indices) {
            null
        } else {
            list.toMutableList().also { it.add(to, it.removeAt(from)) }
        }
    }
}

/** 按 id 覆盖或追加（保存前检测与写入共用，保证两者的列表一致）。 */
fun upsertRule(list: List<ProxyRule>, draft: ProxyRule): List<ProxyRule> {
    val index = list.indexOfFirst { it.id == draft.id }
    return if (index < 0) list + draft else list.toMutableList().also { it[index] = draft }
}

/** `daemon.config.proxyRoute` 请求；[rules] 为 null 时检查已保存的规则。 */
data class ProxyRouteRequest(val url: String, val rules: String? = null) {
    fun toJson(): JsonValue = jsonObjectOmitNulls("url" to url, "rules" to rules)
}

data class ProxyRuleIssue(val index: Int, val field: String, val code: String, val detail: String) {
    companion object {
        fun fromJson(v: JsonValue?): ProxyRuleIssue =
            ProxyRuleIssue(v.int("index"), v.str("field"), v.str("code"), v.str("detail"))
    }
}

data class ProxyRouteMatch(val index: Int, val id: String, val name: String) {
    companion object {
        fun fromJson(v: JsonValue?): ProxyRouteMatch = ProxyRouteMatch(v.int("index"), v.str("id"), v.str("name"))
    }
}

/** `reason == ""` 表示请求没有给出 url。 */
data class ProxyRouteResponse(
    val issues: List<ProxyRuleIssue> = emptyList(),
    val matched: ProxyRouteMatch? = null,
    val reason: String = "",
    val endpoint: String = "",
) {
    companion object {
        fun fromJson(v: JsonValue?): ProxyRouteResponse = ProxyRouteResponse(
            issues = v.list("issues").map { ProxyRuleIssue.fromJson(it) },
            matched = if (v["matched"] is JsonValue.Obj) ProxyRouteMatch.fromJson(v["matched"]) else null,
            reason = v.str("reason"),
            endpoint = v.str("endpoint"),
        )
    }
}
