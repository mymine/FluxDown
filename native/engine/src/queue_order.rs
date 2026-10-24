//! 队列顺序的增量发布：为排队任务维护稳定排序键，把「当前队列」与「上次已发布」
//! 的差异压缩成与变化量成正比的增量事件。
//!
//! 键的语义（与协议 v15 对齐）：任务在队列期间键不变，队首出队不改变其余任务的键；
//! 只有用户调序（已发布键的相对顺序被破坏）或相邻键间无空位时才整表重新分配并发全量。

use std::collections::{HashMap, HashSet};

use crate::model::{QueueOrderEntry, QueuePosition};

/// 整表分配与头尾追加使用的键步长。
const KEY_STEP: i64 = 1 << 20;

/// 键的绝对值上限（< 2^53，Web 端 JS number 仍精确）。
const KEY_LIMIT: i64 = 1 << 52;

/// 一次发布的差异。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueOrderDelta {
    /// 与上次已发布的顺序一致，不发事件。
    Unchanged,
    /// 全量替换（首次发布 / 重排 / 队列变空），按键升序，`position` = 名次。
    Full(Vec<QueuePosition>),
    /// 增量：先移除 `removed`，再插入 `added`（按键升序）。
    Incremental {
        removed: Vec<String>,
        added: Vec<QueueOrderEntry>,
    },
}

/// 「上次已发布」的 task_id → 排序键表。
#[derive(Debug, Default)]
pub struct QueueOrderPublisher {
    keys: HashMap<String, i64>,
}

impl QueueOrderPublisher {
    /// 以当前队列顺序（可含重复 id，以首次出现为准）计算相对上次发布的差异，
    /// 并把键表推进到该状态。
    pub fn diff<'a>(&mut self, queue: impl Iterator<Item = &'a str>) -> QueueOrderDelta {
        let mut seen: HashSet<&'a str> = HashSet::new();
        let ordered: Vec<&'a str> = queue.filter(|id| seen.insert(*id)).collect();

        if ordered.is_empty() {
            if self.keys.is_empty() {
                return QueueOrderDelta::Unchanged;
            }
            self.keys.clear();
            return QueueOrderDelta::Full(Vec::new());
        }
        if self.keys.is_empty() {
            return self.reassign(&ordered);
        }

        // 已有键必须沿当前顺序严格递增；否则说明发生了调序。
        let mut existing: Vec<Option<i64>> = Vec::with_capacity(ordered.len());
        let mut retained = 0usize;
        let mut last: Option<i64> = None;
        for id in &ordered {
            let key = self.keys.get(*id).copied();
            if let Some(key) = key {
                if last.is_some_and(|last| key <= last) {
                    return self.reassign(&ordered);
                }
                last = Some(key);
                retained += 1;
            }
            existing.push(key);
        }

        let Some(assigned) = assign_new_keys(&existing) else {
            return self.reassign(&ordered);
        };

        let removed: Vec<String> = if retained < self.keys.len() {
            self.keys
                .keys()
                .filter(|id| !seen.contains(id.as_str()))
                .cloned()
                .collect()
        } else {
            Vec::new()
        };
        if removed.is_empty() && assigned.is_empty() {
            return QueueOrderDelta::Unchanged;
        }

        for id in &removed {
            self.keys.remove(id);
        }
        let mut added = Vec::with_capacity(assigned.len());
        for (index, key) in assigned {
            let task_id = ordered[index].to_owned();
            self.keys.insert(task_id.clone(), key);
            added.push(QueueOrderEntry {
                task_id,
                order: key,
            });
        }
        QueueOrderDelta::Incremental { removed, added }
    }

    /// 整表重新分配键并产出全量。
    fn reassign(&mut self, ordered: &[&str]) -> QueueOrderDelta {
        self.keys.clear();
        self.keys.reserve(ordered.len());
        let positions = ordered
            .iter()
            .enumerate()
            .map(|(index, id)| {
                let order = (index as i64 + 1) * KEY_STEP;
                self.keys.insert((*id).to_owned(), order);
                QueuePosition {
                    task_id: (*id).to_owned(),
                    position: i32::try_from(index + 1).unwrap_or(i32::MAX),
                    order,
                }
            })
            .collect();
        QueueOrderDelta::Full(positions)
    }
}

/// 为 `existing` 里没有键（`None`）的下标在前后已有键之间分配键；返回 `(下标, 键)`
/// 升序。任一连续新增段放不下（无空位 / 越过键上限）时返回 `None`，由调用方整表重排。
/// 调用方保证已有键严格递增。
fn assign_new_keys(existing: &[Option<i64>]) -> Option<Vec<(usize, i64)>> {
    let mut assigned = Vec::new();
    let mut index = 0usize;
    let mut prev: Option<i64> = None;
    while index < existing.len() {
        if let Some(key) = existing[index] {
            prev = Some(key);
            index += 1;
            continue;
        }
        let run_end = existing[index..]
            .iter()
            .position(Option::is_some)
            .map_or(existing.len(), |offset| index + offset);
        let next = existing.get(run_end).copied().flatten();
        let count = (run_end - index) as i64;
        match (prev, next) {
            // 尾部追加 = 末键 + 步长。
            (Some(prev), None) => {
                let last = prev.checked_add(KEY_STEP.checked_mul(count)?)?;
                if last > KEY_LIMIT {
                    return None;
                }
                for (offset, i) in (index..run_end).enumerate() {
                    assigned.push((i, prev + KEY_STEP * (offset as i64 + 1)));
                }
            }
            // 头部插入 = 首键 - 步长。
            (None, Some(next)) => {
                let first = next.checked_sub(KEY_STEP.checked_mul(count)?)?;
                if first < -KEY_LIMIT {
                    return None;
                }
                for (offset, i) in (index..run_end).enumerate() {
                    assigned.push((i, first + KEY_STEP * offset as i64));
                }
            }
            // 中间：在两侧键之间等距取键（单个即中点）。
            (Some(prev), Some(next)) => {
                let step = (next - prev) / (count + 1);
                if step < 1 {
                    return None;
                }
                for (offset, i) in (index..run_end).enumerate() {
                    assigned.push((i, prev + step * (offset as i64 + 1)));
                }
            }
            // 没有任何已有键：调用方在键表非空时已排除（全部为新任务时走整表分配）。
            (None, None) => return None,
        }
        index = run_end;
    }
    Some(assigned)
}

#[cfg(test)]
mod tests {
    use super::{KEY_LIMIT, KEY_STEP, QueueOrderDelta, QueueOrderPublisher};

    fn diff(publisher: &mut QueueOrderPublisher, ids: &[&str]) -> QueueOrderDelta {
        publisher.diff(ids.iter().copied())
    }

    /// 按协议语义（先删后按键插入、同键后者覆盖）把增量应用到已发布列表。
    fn apply(published: &mut Vec<(String, i64)>, delta: &QueueOrderDelta) {
        match delta {
            QueueOrderDelta::Unchanged => {}
            QueueOrderDelta::Full(positions) => {
                *published = positions
                    .iter()
                    .map(|p| (p.task_id.clone(), p.order))
                    .collect();
            }
            QueueOrderDelta::Incremental { removed, added } => {
                published.retain(|(id, _)| !removed.contains(id));
                for entry in added {
                    published.retain(|(id, _)| id != &entry.task_id);
                    published.push((entry.task_id.clone(), entry.order));
                }
                published.sort_by_key(|(_, order)| *order);
            }
        }
    }

    fn ids(published: &[(String, i64)]) -> Vec<&str> {
        published.iter().map(|(id, _)| id.as_str()).collect()
    }

    fn order_of(publisher: &QueueOrderPublisher, id: &str) -> i64 {
        publisher.keys[id]
    }

    #[test]
    fn first_publish_is_full_and_empty_queue_is_silent() {
        let mut publisher = QueueOrderPublisher::default();
        assert_eq!(diff(&mut publisher, &[]), QueueOrderDelta::Unchanged);
        let QueueOrderDelta::Full(positions) = diff(&mut publisher, &["a", "b", "c"]) else {
            panic!("first publish must be a full table");
        };
        assert_eq!(
            positions
                .iter()
                .map(|p| (p.task_id.as_str(), p.position, p.order))
                .collect::<Vec<_>>(),
            [
                ("a", 1, KEY_STEP),
                ("b", 2, 2 * KEY_STEP),
                ("c", 3, 3 * KEY_STEP)
            ]
        );
        assert_eq!(
            diff(&mut publisher, &["a", "b", "c"]),
            QueueOrderDelta::Unchanged
        );
    }

    #[test]
    fn head_dequeue_only_removes_and_keeps_other_keys() {
        let mut publisher = QueueOrderPublisher::default();
        diff(&mut publisher, &["a", "b", "c", "d"]);
        let before = order_of(&publisher, "c");
        assert_eq!(
            diff(&mut publisher, &["b", "c", "d"]),
            QueueOrderDelta::Incremental {
                removed: vec!["a".to_owned()],
                added: Vec::new()
            }
        );
        assert_eq!(order_of(&publisher, "c"), before);
    }

    #[test]
    fn emptying_the_queue_publishes_an_empty_full_table_and_resets() {
        let mut publisher = QueueOrderPublisher::default();
        diff(&mut publisher, &["a", "b"]);
        assert_eq!(diff(&mut publisher, &[]), QueueOrderDelta::Full(Vec::new()));
        assert!(matches!(
            diff(&mut publisher, &["x"]),
            QueueOrderDelta::Full(_)
        ));
    }

    #[test]
    fn tail_append_extends_past_the_last_key() {
        let mut publisher = QueueOrderPublisher::default();
        diff(&mut publisher, &["a", "b"]);
        let QueueOrderDelta::Incremental { removed, added } =
            diff(&mut publisher, &["a", "b", "c", "d"])
        else {
            panic!("append must be incremental");
        };
        assert!(removed.is_empty());
        assert_eq!(
            added
                .iter()
                .map(|e| (e.task_id.as_str(), e.order))
                .collect::<Vec<_>>(),
            [("c", 3 * KEY_STEP), ("d", 4 * KEY_STEP)]
        );
    }

    #[test]
    fn head_insert_goes_before_the_first_key() {
        let mut publisher = QueueOrderPublisher::default();
        diff(&mut publisher, &["a", "b"]);
        let QueueOrderDelta::Incremental { removed, added } =
            diff(&mut publisher, &["x", "y", "a", "b"])
        else {
            panic!("head insert must be incremental");
        };
        assert!(removed.is_empty());
        assert_eq!(
            added
                .iter()
                .map(|e| (e.task_id.as_str(), e.order))
                .collect::<Vec<_>>(),
            [("x", -KEY_STEP), ("y", 0)]
        );
    }

    #[test]
    fn middle_insert_takes_the_midpoint_and_exhaustion_falls_back_to_full() {
        let mut publisher = QueueOrderPublisher::default();
        diff(&mut publisher, &["a", "b"]);
        let QueueOrderDelta::Incremental { added, .. } = diff(&mut publisher, &["a", "m", "b"])
        else {
            panic!("middle insert must be incremental");
        };
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].order, KEY_STEP + KEY_STEP / 2);

        // 反复在同一缝隙折半，空位耗尽后整表重排。
        let mut queue: Vec<String> = vec!["a".into(), "b".into()];
        let mut full_seen = false;
        for n in 0..64 {
            queue.insert(1, format!("n{n}"));
            let refs: Vec<&str> = queue.iter().map(String::as_str).collect();
            if matches!(publisher.diff(refs.into_iter()), QueueOrderDelta::Full(_)) {
                full_seen = true;
                break;
            }
        }
        assert!(full_seen, "a closed gap must trigger a full reassignment");
    }

    #[test]
    fn reorder_triggers_full_with_rank_positions() {
        let mut publisher = QueueOrderPublisher::default();
        diff(&mut publisher, &["a", "b", "c"]);
        let QueueOrderDelta::Full(positions) = diff(&mut publisher, &["c", "a", "b"]) else {
            panic!("reorder must be a full table");
        };
        assert_eq!(
            positions
                .iter()
                .map(|p| (p.task_id.as_str(), p.position))
                .collect::<Vec<_>>(),
            [("c", 1), ("a", 2), ("b", 3)]
        );
        assert!(positions.windows(2).all(|w| w[0].order < w[1].order));
    }

    #[test]
    fn duplicate_ids_count_once_at_first_occurrence() {
        let mut publisher = QueueOrderPublisher::default();
        let QueueOrderDelta::Full(positions) = diff(&mut publisher, &["a", "b", "a", "c", "b"])
        else {
            panic!("first publish must be a full table");
        };
        assert_eq!(
            positions
                .iter()
                .map(|p| p.task_id.as_str())
                .collect::<Vec<_>>(),
            ["a", "b", "c"]
        );
        assert_eq!(
            diff(&mut publisher, &["a", "b", "a", "c", "b"]),
            QueueOrderDelta::Unchanged
        );
    }

    #[test]
    fn key_overflow_falls_back_to_full_instead_of_leaving_the_safe_range() {
        let mut publisher = QueueOrderPublisher::default();
        diff(&mut publisher, &["a"]);
        publisher.keys.insert("a".to_owned(), KEY_LIMIT);
        assert!(matches!(
            diff(&mut publisher, &["a", "b"]),
            QueueOrderDelta::Full(_)
        ));
        publisher.keys.insert("a".to_owned(), -KEY_LIMIT);
        assert!(matches!(
            diff(&mut publisher, &["z", "a"]),
            QueueOrderDelta::Full(_)
        ));
    }

    /// 任意操作序列下「上次状态 + 增量」必须得到与当前队列一致的顺序。
    #[test]
    fn applied_deltas_always_reproduce_the_queue_order() {
        let mut publisher = QueueOrderPublisher::default();
        let mut published: Vec<(String, i64)> = Vec::new();
        let steps: [&[&str]; 9] = [
            &["a", "b", "c", "d", "e"],
            &["b", "c", "d", "e"],
            &["b", "c", "d", "e", "f", "g"],
            &["x", "b", "c", "d", "e", "f", "g"],
            &["x", "b", "y", "c", "z", "d", "e", "f", "g"],
            &["y", "c", "z", "d", "e", "f", "g", "h"],
            &["g", "y", "c", "z", "d", "e", "f", "h"],
            &["g", "y", "g", "c", "z", "d", "e", "f", "h"],
            &[],
        ];
        for step in steps {
            let delta = diff(&mut publisher, step);
            apply(&mut published, &delta);
            let mut expected: Vec<&str> = Vec::new();
            for id in step {
                if !expected.contains(id) {
                    expected.push(id);
                }
            }
            assert_eq!(ids(&published), expected, "after {step:?}");
            assert!(published.windows(2).all(|w| w[0].1 < w[1].1));
        }
    }

    #[test]
    fn dequeue_from_a_large_queue_publishes_only_the_removed_ids() {
        let mut publisher = QueueOrderPublisher::default();
        let all: Vec<String> = (0..30_000).map(|i| format!("t{i}")).collect();
        publisher.diff(all.iter().map(String::as_str));
        for head in 1..=5 {
            let delta = publisher.diff(all[head..].iter().map(String::as_str));
            assert_eq!(
                delta,
                QueueOrderDelta::Incremental {
                    removed: vec![format!("t{}", head - 1)],
                    added: Vec::new()
                }
            );
        }
    }
}
