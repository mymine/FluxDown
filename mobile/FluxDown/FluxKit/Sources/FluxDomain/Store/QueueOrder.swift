import Foundation

/// 排队任务的引擎排序键（↔ Rust `QueueOrderDto`）：任务留在队列期间不变，队首出队不改其余任务的键。
public struct QueueOrderEntry: Hashable, Sendable {
    public let taskId: String
    public let order: Int64

    public init(taskId: String, order: Int64) {
        self.taskId = taskId
        self.order = order
    }
}

/// 排队顺序：按键升序的不可变视图，名次（「排队 #n」）= 键在有序键里的下标 + 1。
///
/// 名次不落进每个任务行：队首出队只删一个键，其余行的稳定键不变，渲染可见行时再按键二分取名次。
/// 值只在顺序变化时重建（`HostStore` 工作副本复用），未变化时前后状态共享同一份存储。
public struct QueueOrder: Sendable, Equatable {
    public let entries: [QueueOrderEntry]
    private let orderById: [String: Int64]
    private let orders: [Int64]

    public static let empty = QueueOrder(sorted: [])

    /// `entries` 必须已按 `order` 升序（协议全量表与 ``applying(removed:added:to:)`` 的结果都满足）。
    public init(sorted entries: [QueueOrderEntry]) {
        self.entries = entries
        var byId: [String: Int64] = [:]
        byId.reserveCapacity(entries.count)
        var keys: [Int64] = []
        keys.reserveCapacity(entries.count)
        for entry in entries {
            byId[entry.taskId] = entry.order
            keys.append(entry.order)
        }
        orderById = byId
        orders = keys
    }

    public var count: Int { orders.count }

    public static func == (lhs: QueueOrder, rhs: QueueOrder) -> Bool { lhs.entries == rhs.entries }

    /// 任务的排序键；不在排队 = nil。
    public func order(of taskId: String) -> Int64? { orderById[taskId] }

    /// 任务的排队名次（1 起）；不在排队 = 0。
    public func rank(of taskId: String) -> Int {
        guard let key = orderById[taskId] else { return 0 }
        var lo = 0
        var hi = orders.count
        while lo < hi {
            let mid = (lo + hi) / 2
            if orders[mid] < key { lo = mid + 1 } else { hi = mid }
        }
        return lo < orders.count && orders[lo] == key ? lo + 1 : 0
    }

    /// 把增量并入升序表：先移除 `removed`，再按键插入 / 更新 `added`（同任务多项以最后一项为准）。
    /// 对应 Rust `fluxdown_protocol::apply_queue_order`；单趟 O(N + k log k)。
    public static func applying(
        removed: [String], added: [QueueOrderEntry], to current: [QueueOrderEntry]
    ) -> [QueueOrderEntry] {
        if removed.isEmpty && added.isEmpty { return current }
        var dropped = Set(removed)
        var latest: [String: QueueOrderEntry] = [:]
        latest.reserveCapacity(added.count)
        for entry in added {
            dropped.insert(entry.taskId)
            latest[entry.taskId] = entry
        }
        let fresh = latest.values.sorted { ($0.order, $0.taskId) < ($1.order, $1.taskId) }
        var merged: [QueueOrderEntry] = []
        merged.reserveCapacity(current.count + fresh.count)
        var next = 0
        for entry in current where !dropped.contains(entry.taskId) {
            while next < fresh.count, fresh[next].order < entry.order {
                merged.append(fresh[next])
                next += 1
            }
            merged.append(entry)
        }
        merged.append(contentsOf: fresh[next...])
        return merged
    }
}
