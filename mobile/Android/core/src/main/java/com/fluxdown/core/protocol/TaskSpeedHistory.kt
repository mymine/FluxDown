package com.fluxdown.core.protocol

/** `daemon.task.speedHistory` 参数。 */
data class TaskSpeedHistoryQuery(val taskId: String) {
    fun toJson(): JsonValue = jsonObject("taskId" to taskId)
}

/**
 * `daemon.task.speedHistory` 结果：任务全生命周期速度曲线与统计。
 * [samples] 每点 [bucketSecs] 秒的平均 B/s（最旧在前，末点可能是尚在累积的部分桶）；
 * x 轴为活跃（下载中）时间 [activeSecs]。
 */
data class TaskSpeedHistory(
    val taskId: String,
    val bucketSecs: Int,
    val samples: List<Long>,
    val activeSecs: Long,
    val transferredBytes: Long,
    val peakBps: Long,
) {
    /** 平均速度 = 已传字节 / 活跃秒数；无活跃时间为 0。 */
    val averageBps: Long get() = if (activeSecs > 0) transferredBytes / activeSecs else 0

    /** 曲线可绘制：至少两点且峰值 > 0。 */
    val hasChart: Boolean get() = samples.size >= 2 && peakBps > 0

    /**
     * 每个采样点对应的已下载时长（秒，0 基下标 i → `min((i+1)·bucketSecs, activeSecs)`），强制严格递增
     * （活跃时间短于点数时按 +1 秒顺延），保证读数的时间不会重复或倒退。
     */
    fun elapsedSecs(): LongArray {
        val out = LongArray(samples.size)
        var prev = 0L
        for (i in out.indices) {
            val e = minOf((i + 1).toLong() * bucketSecs, activeSecs)
            prev = maxOf(e, prev + 1)
            out[i] = prev
        }
        return out
    }

    /** 曲线 y 轴上限（B/s）：`max(1 MiB, 样本最大值) × 1.18`，与 UI 波形自动量程一致。 */
    val chartMaxBps: Long
        get() = ((maxOf(MIN_CHART_BPS, samples.maxOrNull() ?: 0L)) * CHART_HEADROOM).toLong()

    /** 数值轴 3 个刻度（B/s，自下而上）：量程的 1/3、2/3、满格。 */
    val axisTicksBps: List<Long>
        get() = chartMaxBps.let { max -> listOf(max / 3, max * 2 / 3, max) }

    companion object {
        private const val MIN_CHART_BPS = 1L shl 20
        private const val CHART_HEADROOM = 1.18

        /** 时间读数：<1 h → `m:ss`，否则 `h:mm:ss`；负值按 0。 */
        fun formatElapsed(secs: Long): String {
            val s = secs.coerceAtLeast(0)
            val h = s / 3600
            val m = s % 3600 / 60
            val sec = s % 60
            return if (h > 0) String.format(java.util.Locale.ROOT, "%d:%02d:%02d", h, m, sec)
            else String.format(java.util.Locale.ROOT, "%d:%02d", m, sec)
        }

        /** 结果为 null（无记录）或缺少 `taskId` 时返回 null；其余字段宽松、负值夹到 0。 */
        fun fromJson(v: JsonValue?): TaskSpeedHistory? {
            val taskId = v.strOrNull("taskId") ?: return null
            return TaskSpeedHistory(
                taskId = taskId,
                bucketSecs = v.int("bucketSecs", 1).coerceAtLeast(1),
                samples = v.list("samples").map { (it.longOrNull ?: 0L).coerceAtLeast(0) },
                activeSecs = v.long("activeSecs", 0).coerceAtLeast(0),
                transferredBytes = v.long("transferredBytes", 0).coerceAtLeast(0),
                peakBps = v.long("peakBps", 0).coerceAtLeast(0),
            )
        }
    }
}
