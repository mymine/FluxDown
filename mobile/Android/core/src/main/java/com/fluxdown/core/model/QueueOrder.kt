package com.fluxdown.core.model

/** 排队任务的引擎排序键（↔ Rust `QueueOrderDto`）：任务留在队列期间不变，队首出队不改其余任务的键。 */
data class QueueOrderEntry(val taskId: String, val order: Long)

/**
 * 排队顺序：按键升序的不可变视图，名次（「排队 #n」）= 键在有序键里的下标 + 1。
 *
 * 名次不落进每个任务行：队首出队只删一个键，其余行的稳定键不变，渲染可见行时再按键二分取名次。
 * 实例只在顺序变化时重建（[HostStore] 工作副本复用），未变化时前后状态引用同一对象。
 */
class QueueOrder private constructor(
    val entries: List<QueueOrderEntry>,
    private val orderById: Map<String, Long>,
    private val orders: LongArray,
) {
    val size: Int get() = orders.size

    /** 任务的排序键；不在排队 = null。 */
    fun orderOf(taskId: String): Long? = orderById[taskId]

    /** 任务的排队名次（1 起）；不在排队 = 0。 */
    fun rank(taskId: String): Int {
        val key = orderById[taskId] ?: return 0
        val index = lowerBound(key)
        return if (index < orders.size && orders[index] == key) index + 1 else 0
    }

    private fun lowerBound(key: Long): Int {
        var lo = 0
        var hi = orders.size
        while (lo < hi) {
            val mid = (lo + hi) ushr 1
            if (orders[mid] < key) lo = mid + 1 else hi = mid
        }
        return lo
    }

    companion object {
        val Empty = QueueOrder(emptyList(), emptyMap(), LongArray(0))

        /** [entries] 必须已按 `order` 升序（协议全量表与 [apply] 的结果都满足）。 */
        fun of(entries: List<QueueOrderEntry>): QueueOrder {
            if (entries.isEmpty()) return Empty
            val orderById = HashMap<String, Long>(entries.size * 2)
            val orders = LongArray(entries.size)
            entries.forEachIndexed { i, e ->
                orderById[e.taskId] = e.order
                orders[i] = e.order
            }
            return QueueOrder(entries, orderById, orders)
        }

        /**
         * 把增量并入升序表：先移除 [removed]，再按键插入 / 更新 [added]（同任务多项以最后一项为准）。
         * 对应 Rust `fluxdown_protocol::apply_queue_order`；单趟 O(N + k log k)。
         */
        fun apply(
            current: List<QueueOrderEntry>,
            removed: Collection<String>,
            added: List<QueueOrderEntry>,
        ): List<QueueOrderEntry> {
            if (removed.isEmpty() && added.isEmpty()) return current
            val dropped = HashSet<String>(removed.size + added.size)
            dropped.addAll(removed)
            added.forEach { dropped.add(it.taskId) }
            val incoming = LinkedHashMap<String, QueueOrderEntry>(added.size)
            for (entry in added) {
                incoming.remove(entry.taskId)
                incoming[entry.taskId] = entry
            }
            val fresh = incoming.values.sortedBy { it.order }
            val merged = ArrayList<QueueOrderEntry>(current.size + fresh.size)
            var next = 0
            for (entry in current) {
                if (entry.taskId in dropped) continue
                while (next < fresh.size && fresh[next].order < entry.order) merged.add(fresh[next++])
                merged.add(entry)
            }
            while (next < fresh.size) merged.add(fresh[next++])
            return merged
        }
    }
}
