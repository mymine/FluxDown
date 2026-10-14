package com.fluxdown.core.protocol

import java.time.Instant
import java.time.temporal.ChronoUnit

// 环境诊断与日志（`agent.diagnostics.*` / `daemon.diagnostics.describe`）。
// 镜像 `native/protocol/src/agent.rs`（DiagnosticLevel / DiagnosticCheckDto / DiagnosticRepairParams /
// DiagnosticsReportDto / LogExportParams / LogExportResult / LogPathsDto）与 `native/daemon` 的
// `DAEMON_DIAGNOSTICS_DESCRIBE` 结果；同 iOS `FluxDomain/Protocol/Diagnostics.swift`。

/** Doctor 检查项级别（serde camelCase：info / ok / warn / error）；[Unknown] = 主机新增了本端不认识的级别。 */
sealed interface DiagnosticLevel {
    val wire: String

    data object Info : DiagnosticLevel {
        override val wire = "info"
    }

    data object Ok : DiagnosticLevel {
        override val wire = "ok"
    }

    data object Warn : DiagnosticLevel {
        override val wire = "warn"
    }

    data object Error : DiagnosticLevel {
        override val wire = "error"
    }

    data class Unknown(override val wire: String) : DiagnosticLevel

    /** 计入「N 项问题」：warn + error（同 Web / GPUI）。 */
    val isIssue: Boolean get() = this == Warn || this == Error

    /** 报告里的级别标签（`[WARN]`）：wire 名大写（同 Web `check.level.toUpperCase()`）。 */
    val reportTag: String get() = wire.uppercase()

    /** 严重度排序（unknown 按 info 处理）：error 3 > warn 2 > info 1 > ok 0。 */
    val severity: Int
        get() = when (this) {
            Error -> 3
            Warn -> 2
            Ok -> 0
            Info, is Unknown -> 1
        }

    companion object {
        fun fromWire(wire: String): DiagnosticLevel = when (wire) {
            "info" -> Info
            "ok" -> Ok
            "warn" -> Warn
            "error" -> Error
            else -> Unknown(wire)
        }
    }
}

/** `agent.diagnostics.repair` 参数（也是检查项上可用的就地修复）。 */
data class DiagnosticRepairParams(val action: String, val target: String = "") {
    fun toJson(): JsonValue = jsonObject("action" to action, "target" to target)

    companion object {
        /** `action` 缺失无法执行，返回 null。 */
        fun fromJson(v: JsonValue?): DiagnosticRepairParams? {
            val action = v.strOrNull("action") ?: return null
            return DiagnosticRepairParams(action, v.str("target"))
        }
    }
}

/** 单条 Doctor 检查结果。[id] 稳定（据此取 `doctorCheck{Camel}` 文案）；同一 [id] 可有多个 [target]。 */
data class DiagnosticCheckDto(
    val id: String,
    /** 同一 id 下的子目标（浏览器名、scheme、目录路径…）；无则为空。 */
    val target: String = "",
    val level: DiagnosticLevel = DiagnosticLevel.Info,
    val detail: String = "",
    /** 既有 hint code（如 `check_disk`）；无则为空。 */
    val hint: String = "",
    /** 可用的就地修复动作；无则 null。 */
    val repair: DiagnosticRepairParams? = null,
) {
    companion object {
        /** `id` 缺失的项无法归类，返回 null；其余字段缺省宽松。 */
        fun fromJson(v: JsonValue?): DiagnosticCheckDto? {
            val id = v.strOrNull("id") ?: return null
            return DiagnosticCheckDto(
                id = id,
                target = v.str("target"),
                level = v.strOrNull("level")?.let { DiagnosticLevel.fromWire(it) } ?: DiagnosticLevel.Info,
                detail = v.str("detail"),
                hint = v.str("hint"),
                repair = DiagnosticRepairParams.fromJson(v["repair"]),
            )
        }
    }
}

/** `agent.diagnostics.run` 结果。 */
data class DiagnosticsReportDto(
    val generatedAtUnixMs: Long = 0,
    val appVersion: String = "",
    val platform: String = "",
    val agentDataDir: String = "",
    val daemonConnected: Boolean = false,
    val checks: List<DiagnosticCheckDto> = emptyList(),
) {
    companion object {
        fun fromJson(v: JsonValue?): DiagnosticsReportDto = DiagnosticsReportDto(
            generatedAtUnixMs = v.long("generatedAtUnixMs", 0),
            appVersion = v.str("appVersion"),
            platform = v.str("platform"),
            agentDataDir = v.str("agentDataDir"),
            daemonConnected = v.bool("daemonConnected", false),
            checks = v.list("checks").mapNotNull { DiagnosticCheckDto.fromJson(it) },
        )
    }
}

/** `agent.diagnostics.exportLogs` 参数：目标 `.zip` 路径（主机本地路径）。 */
data class LogExportParams(val targetPath: String) {
    fun toJson(): JsonValue = jsonObject("targetPath" to targetPath)
}

/** `agent.diagnostics.exportLogs` 结果。 */
data class LogExportResult(val path: String, val bytes: Long) {
    companion object {
        /** `path` 缺失视为非法响应（null）。 */
        fun fromJson(v: JsonValue?): LogExportResult? {
            val path = v.strOrNull("path") ?: return null
            return LogExportResult(path, v.long("bytes", 0))
        }
    }
}

/** `agent.diagnostics.logPaths`：agent / daemon 日志目录（只有路径，不含大小）。 */
data class LogPathsDto(val agentLogDir: String = "", val daemonLogDir: String = "") {
    companion object {
        fun fromJson(v: JsonValue?): LogPathsDto = LogPathsDto(v.str("agentLogDir"), v.str("daemonLogDir"))
    }
}

/** `daemon.diagnostics.describe` 里的 `service`（`ServiceHello`）。 */
data class DiagnosticsServiceInfo(
    val serviceName: String = "",
    val serviceVersion: String = "",
    val protocolVersion: Long = 0,
    val instanceId: String = "",
    val capabilities: List<String> = emptyList(),
) {
    companion object {
        fun fromJson(v: JsonValue?): DiagnosticsServiceInfo = DiagnosticsServiceInfo(
            serviceName = v.str("serviceName"),
            serviceVersion = v.str("serviceVersion"),
            protocolVersion = v.long("protocolVersion", 0),
            instanceId = v.str("instanceId"),
            capabilities = v.strings("capabilities"),
        )
    }
}

/**
 * `daemon.diagnostics.describe` 结果（任务 / 队列 / 任务组数量、配置版本、daemon 日志目录）。
 * `components`（外部组件状态）这里不用，解析时忽略。
 */
data class DaemonDiagnosticsDescribe(
    val service: DiagnosticsServiceInfo = DiagnosticsServiceInfo(),
    val tasks: Long = 0,
    val queues: Long = 0,
    val groups: Long = 0,
    val configRevision: Long = 0,
    val logDir: String = "",
) {
    companion object {
        fun fromJson(v: JsonValue?): DaemonDiagnosticsDescribe = DaemonDiagnosticsDescribe(
            service = DiagnosticsServiceInfo.fromJson(v["service"]),
            tasks = v.long("tasks", 0),
            queues = v.long("queues", 0),
            groups = v.long("groups", 0),
            configRevision = v.long("configRevision", 0),
            logDir = v.str("logDir"),
        )
    }
}

/** 诊断页的纯逻辑（过滤 / 文案键 / 报告文本）；同 iOS `DiagnosticsLogic`。 */
object DiagnosticsLogic {
    /** 桌面专属检查项（NMH、协议 / 文件关联、自启、系统通知、root 运行）：移动端与 Web 一样隐藏。 */
    val desktopOnlyChecks: Set<String> = setOf(
        "nmh_binary", "nmh_manifest", "nmh_browser", "nmh_relay", "nmh_launch", "nmh_policy", "nmh_ownership",
        "nmh_comspec", "url_protocol", "torrent_association", "autostart", "notifications", "elevated_run",
    )

    /** 需要桌面会话的修复动作：不提供按钮。 */
    val desktopOnlyActions: Set<String> = setOf(
        "reregister", "use_this_install", "register", "open_log_dir", "openLogDir", "test_notification",
        "fix_dir_access", "enable_autostart", "open_settings", "fix_comspec",
    )

    /** `delete_files` → `DeleteFiles`（同 Web `camel`）：拼 `doctorCheck{Camel(id)}` 等文案键。 */
    fun camel(value: String): String =
        value.split('_').joinToString("") { part -> if (part.isEmpty()) "" else part.substring(0, 1).uppercase() + part.substring(1) }

    fun visibleChecks(checks: List<DiagnosticCheckDto>): List<DiagnosticCheckDto> =
        checks.filter { it.id !in desktopOnlyChecks }

    /** 检查项上可用的修复：非桌面专属，且有动作文案（`doctorAction{Camel}`）——没有文案的动作不做成按钮。 */
    fun repair(check: DiagnosticCheckDto, hasLabel: (String) -> Boolean): DiagnosticRepairParams? {
        val repair = check.repair ?: return null
        if (repair.action in desktopOnlyActions || !hasLabel(actionKey(repair.action))) return null
        return repair
    }

    fun checkKey(id: String): String = "doctorCheck" + camel(id)

    fun hintKey(hint: String): String = "doctorHint" + camel(hint)

    fun actionKey(action: String): String = "doctorAction" + camel(action)

    /** 修复失败的可操作说明键（同 GPUI `doctor::repair_outcome` / Web `REPAIR_REASON_KEYS`）；其余回退通用文案。 */
    fun repairErrorKey(reason: String?): String? = when (reason) {
        "elevationCancelled" -> "doctorRepairCancelled"
        "elevationUnavailable" -> "doctorRepairUnavailable"
        "runningElevated" -> "doctorRepairRunningElevated"
        "repairIncomplete" -> "doctorRepairIncomplete"
        "repairNotApplicable" -> "doctorRepairNotApplicable"
        else -> null
    }

    /** warn + error 的条数。 */
    fun issueCount(checks: List<DiagnosticCheckDto>): Int = checks.count { it.level.isIssue }

    /** 一组级别里最严重的一个；空 = ok。 */
    fun worst(levels: List<DiagnosticLevel>): DiagnosticLevel = levels.maxByOrNull { it.severity } ?: DiagnosticLevel.Ok

    /** 正在修复的检查项标识（`index-id-target`：同名队列 / 目录会产生相同的 id·target）。 */
    fun repairTag(index: Int, check: DiagnosticCheckDto): String = "$index-${check.id}-${check.target}"

    // ───────────────────────────── 报告文本 ─────────────────────────────

    data class ReportLine(
        val level: DiagnosticLevel,
        val label: String,
        val target: String = "",
        val detail: String,
        val hint: String = "",
    )

    data class HostPart(
        val name: String,
        val appVersion: String,
        val platform: String,
        val dataDir: String,
        val daemonConnected: Boolean,
        /** `daemon.diagnostics.describe` 的一行摘要（版本 · 任务 / 队列 / 任务组 · 日志目录）；取不到为 null。 */
        val daemonSummary: String? = null,
        val lines: List<ReportLine>,
    )

    data class ReportInput(
        val appVersion: String,
        val deviceDescription: String,
        val generatedAtUnixMs: Long,
        val device: List<ReportLine>,
        val host: HostPart?,
    )

    /** 纯文本报告（复制到反馈）：版本、平台、时间、数据目录、daemon 连接、逐项 `[LEVEL] 标题 (目标): 详情` + 提示。 */
    fun renderReport(input: ReportInput): String = buildString {
        append("FluxDown ${input.appVersion} · ${input.deviceDescription} · ${isoTime(input.generatedAtUnixMs)}\n")
        append("\n[This device]\n")
        appendLines(input.device)
        val host = input.host ?: return@buildString
        append("\n[Download host: ${host.name}]\n")
        append("host: ${host.appVersion} · ${host.platform}\n")
        append("agent data dir: ${host.dataDir}\n")
        append("daemon connected: ${host.daemonConnected}\n")
        host.daemonSummary?.let { append("daemon: $it\n") }
        append("\n")
        appendLines(host.lines)
    }

    private fun StringBuilder.appendLines(items: List<ReportLine>) {
        for (item in items) {
            append("[${item.level.reportTag}] ${item.label}")
            if (item.target.isNotEmpty()) append(" (${item.target})")
            append(": ${item.detail}\n")
            if (item.hint.isNotEmpty()) append("  hint: ${item.hint}\n")
        }
    }

    private fun isoTime(unixMs: Long): String = Instant.ofEpochMilli(unixMs).truncatedTo(ChronoUnit.SECONDS).toString()
}

/**
 * 设置首页「诊断」读数的记录：最近一次运行的问题数（按主机；换主机即丢弃）。
 * 纯数据，线程不安全（UI 主线程使用）。
 */
class DiagnosticsRecord {
    var hostId: String? = null
        private set
    var issues: Int? = null
        private set

    fun record(hostId: String, issues: Int) {
        this.hostId = hostId
        this.issues = issues
    }

    /** 当前主机不是记录所属的主机时丢弃读数。 */
    fun reset(ifHostIsNot: String) {
        val recorded = hostId ?: return
        if (recorded == ifHostIsNot) return
        hostId = null
        issues = null
    }
}
