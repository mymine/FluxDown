package com.fluxdown.core.store

import com.fluxdown.core.model.HostRef
import com.fluxdown.core.protocol.Json
import com.fluxdown.core.protocol.JsonValue
import com.fluxdown.core.protocol.arrayOrNull
import com.fluxdown.core.protocol.jsonObjectOmitNulls
import com.fluxdown.core.protocol.str
import com.fluxdown.core.protocol.strOrNull

/** 已保存远端主机的一条持久化记录：[sealedKey] 是凭据（访问密钥 / 设备控制凭据）的密文，明文从不在这里出现。 */
data class RemoteHostRecord(val ref: HostRef.Remote, val sealedKey: String)

/**
 * 远端主机列表的 JSON 编解码：`[{id, name, endpoint, key, linkFingerprint?}]`。
 *
 * `linkFingerprint` 是可选字段（仅局域网配对加入的主机有）；旧数据缺省为 null，访问密钥主机不写出该字段。
 * 解码宽松：整体 / 单条损坏一律丢弃（返回已能解析的部分），不让坏数据拖垮启动。
 * 自带 [Json]，因为 `org.json` 在 JVM 单测里是桩。
 */
object RemoteHostCodec {
    fun encode(records: List<RemoteHostRecord>): String =
        JsonValue.Arr(
            records.map { r ->
                jsonObjectOmitNulls(
                    "id" to r.ref.id,
                    "name" to r.ref.displayName,
                    "endpoint" to r.ref.endpoint,
                    "key" to r.sealedKey,
                    "linkFingerprint" to r.ref.linkFingerprint,
                )
            },
        ).toJson()

    fun decode(raw: String?): List<RemoteHostRecord> {
        if (raw.isNullOrEmpty()) return emptyList()
        val items = Json.parseOrNull(raw).arrayOrNull ?: return emptyList()
        return items.mapNotNull { o ->
            val id = o.str("id")
            val endpoint = o.str("endpoint")
            val key = o.str("key")
            if (id.isEmpty() || endpoint.isEmpty() || key.isEmpty()) return@mapNotNull null
            val fingerprint = o.strOrNull("linkFingerprint")?.takeIf { it.isNotEmpty() }
            RemoteHostRecord(HostRef.Remote(id, o.str("name").ifEmpty { endpoint }, endpoint, fingerprint), key)
        }
    }

    /** 已保存的、指纹相同的那条主机 id（同一台主机重新配对时原地更新，不新增一条）；没有 → null。 */
    fun idForFingerprint(records: List<RemoteHostRecord>, fingerprint: String): String? =
        records.firstOrNull { it.ref.linkFingerprint == fingerprint }?.ref?.id
}
