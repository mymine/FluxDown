//! 表格行顺序稳定器：把「数据刷新」与「重新排序」分开，避免行在光标下跳动。
//!
//! - 结构性变化（行增删、筛选 / 搜索 / 视图偏好变化、用户点表头）立即按新排序应用。
//! - 仅内容变化（进度、速度、状态等字段变了，行集合不变）时，若处于保持期，已有行
//!   保持上次的相对顺序，只有新进入视图的行按新排序落位（见 [`stable_merge`]）；
//!   被推迟的重排记为「过期」，由宿主在 [`RowOrder::deadline`] 到期后强制补上。
//! - 保持期：最近一次表格内指针活动后 [`INTERACTION_HOLD`] 内；或排序键是动态键
//!   （速度 / 进度）且距上次真正重排不足 [`LIVE_RESORT_INTERVAL`]。

use std::{
    cell::Cell,
    rc::Rc,
    time::{Duration, Instant},
};

use super::RowId;

/// 指针在表格内活动（移动 / 滚动 / 按下 / 菜单打开）后推迟重排的时长。
pub(crate) const INTERACTION_HOLD: Duration = Duration::from_millis(1500);
/// 动态排序键（速度 / 进度）两次重排的最小间隔。
pub(crate) const LIVE_RESORT_INTERVAL: Duration = Duration::from_secs(2);

/// 指针活动记录句柄：表格的指针监听器直接写它，而不是 `Entity::update` 表格。
///
/// gpui-fast 把渲染期外的 entity 写入记为内容变化；鼠标移动 / 滚轮每次都写表格会让
/// 随后的滚动 notify 变成整页重建。保持期只在刷新 / 重排定时器（非渲染路径）里读取，
/// 无需被渲染依赖追踪。
#[derive(Clone, Default)]
pub(crate) struct InteractionHold(Rc<Cell<Option<Instant>>>);

impl InteractionHold {
    /// 记录一次表格内指针活动，开启 / 顺延保持期。
    pub(crate) fn note(&self, now: Instant) {
        self.0.set(Some(now + INTERACTION_HOLD));
    }

    fn until(&self) -> Option<Instant> {
        self.0.get()
    }
}

#[derive(Default)]
pub(crate) struct RowOrder {
    /// 上次应用的行顺序（分组前）。
    order: Vec<RowId>,
    hold: InteractionHold,
    last_sorted_at: Option<Instant>,
    /// 有被推迟的重排：当前顺序与最新排序结果不同。
    stale: bool,
}

impl RowOrder {
    /// 记录一次表格内指针活动，开启 / 顺延保持期。
    pub(crate) fn note_interaction(&mut self, now: Instant) {
        self.hold.note(now);
    }

    /// 共享的指针活动句柄（交给不持有表格可变引用的事件监听器）。
    pub(crate) fn interaction_hold(&self) -> InteractionHold {
        self.hold.clone()
    }

    /// 应用一次排序结果。`sorted` 是按当前比较器排好的行；`content_only` 表示自上次
    /// 应用以来行集合与下标布局未变（[`RowId`] 仍指向同一任务），只有它才允许推迟重排。
    pub(crate) fn apply<T: Copy>(
        &mut self,
        sorted: Vec<(RowId, T)>,
        content_only: bool,
        live_key: bool,
        now: Instant,
    ) -> Vec<(RowId, T)> {
        let rows = if content_only && self.holding(live_key, now) {
            let merged = stable_merge(&self.order, &sorted);
            self.stale = merged
                .iter()
                .map(|(id, _)| id)
                .ne(sorted.iter().map(|(id, _)| id));
            merged
        } else {
            self.stale = false;
            self.last_sorted_at = Some(now);
            sorted
        };
        self.order.clear();
        self.order.extend(rows.iter().map(|(id, _)| *id));
        rows
    }

    /// 被推迟的重排最早可以执行的时刻；没有过期顺序时为 `None`。
    pub(crate) fn deadline(&self, live_key: bool) -> Option<Instant> {
        if !self.stale {
            return None;
        }
        let throttle = self
            .last_sorted_at
            .filter(|_| live_key)
            .map(|at| at + LIVE_RESORT_INTERVAL);
        // 过期必然源于保持期，两者至少有一个；`Option::max` 取较晚者。
        self.hold.until().max(throttle)
    }

    /// 有被推迟的重排：当前显示顺序与最新排序结果不同。
    pub(crate) fn is_stale(&self) -> bool {
        self.stale
    }

    /// 等价于对「与上次完全相同的排序结果」调用 [`Self::apply`]（要求 [`Self::is_stale`]
    /// 为假且 `content_only`）：保持期内什么都不变，保持期外只刷新重排时刻。调用方因此
    /// 不必为一份没有变化的排序结果再整表复制 / 合并一遍。
    pub(crate) fn settle_unchanged(&mut self, live_key: bool, now: Instant) {
        if !self.holding(live_key, now) {
            self.last_sorted_at = Some(now);
        }
    }

    fn holding(&self, live_key: bool, now: Instant) -> bool {
        self.hold.until().is_some_and(|until| now < until)
            || (live_key
                && self
                    .last_sorted_at
                    .is_some_and(|at| now < at + LIVE_RESORT_INTERVAL))
    }
}

/// 稳定合并：`sorted` 中属于上次已显示行的位置，按上次的相对顺序依次填回这些行；
/// 新进入视图的行留在 `sorted` 给出的位置；已离开视图的行直接消失。
fn stable_merge<T: Copy>(previous: &[RowId], sorted: &[(RowId, T)]) -> Vec<(RowId, T)> {
    let slots = sorted
        .iter()
        .map(|(id, _)| slot(*id))
        .chain(previous.iter().map(|id| slot(*id)))
        .max()
        .map_or(0, |max| max + 1);
    let mut current: Vec<Option<T>> = vec![None; slots];
    for &(id, row) in sorted {
        current[slot(id)] = Some(row);
    }
    let mut shown_before = vec![false; slots];
    for &id in previous {
        shown_before[slot(id)] = true;
    }
    let mut survivors = previous
        .iter()
        .filter_map(|&id| current[slot(id)].map(|row| (id, row)));
    sorted
        .iter()
        .map(|&(id, row)| {
            if shown_before[slot(id)] {
                survivors.next().unwrap_or((id, row))
            } else {
                (id, row)
            }
        })
        .collect()
}

/// 本地 / 远程两套下标交错映射到同一张位图。
fn slot(id: RowId) -> usize {
    match id {
        RowId::Local(ix) => ix * 2,
        RowId::Remote(ix) => ix * 2 + 1,
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{INTERACTION_HOLD, LIVE_RESORT_INTERVAL, RowOrder};
    use crate::model::RowId;

    fn rows(ids: &[usize]) -> Vec<(RowId, ())> {
        ids.iter().map(|&ix| (RowId::Local(ix), ())).collect()
    }

    fn ids(rows: &[(RowId, ())]) -> Vec<usize> {
        rows.iter()
            .map(|(id, _)| match id {
                RowId::Local(ix) | RowId::Remote(ix) => *ix,
            })
            .collect()
    }

    #[test]
    fn held_content_change_keeps_survivors_and_places_new_rows_by_sort() {
        let t0 = Instant::now();
        let mut order = RowOrder::default();
        order.apply(rows(&[1, 2, 3]), false, false, t0);
        order.note_interaction(t0);

        // 行 3 升到最前、行 2 离开视图、行 4 新进入视图（排在第二）。
        let held = order.apply(
            rows(&[3, 4, 1]),
            true,
            false,
            t0 + Duration::from_millis(100),
        );
        assert_eq!(ids(&held), [1, 4, 3]);
        let deadline = order.deadline(false).expect("reorder deferred");
        assert!(deadline >= t0 + INTERACTION_HOLD);

        // 保持期结束后内容变化直接按新排序应用。
        let settled = order.apply(rows(&[3, 4, 1]), true, false, t0 + INTERACTION_HOLD);
        assert_eq!(ids(&settled), [3, 4, 1]);
        assert_eq!(order.deadline(false), None);
    }

    #[test]
    fn structural_change_applies_immediately_even_while_held() {
        let t0 = Instant::now();
        let mut order = RowOrder::default();
        order.apply(rows(&[1, 2]), false, false, t0);
        order.note_interaction(t0);
        let applied = order.apply(rows(&[2, 1]), false, false, t0);
        assert_eq!(ids(&applied), [2, 1]);
        assert_eq!(order.deadline(false), None);
    }

    #[test]
    fn unchanged_order_while_held_is_not_stale() {
        let t0 = Instant::now();
        let mut order = RowOrder::default();
        order.apply(rows(&[1, 2]), false, false, t0);
        order.note_interaction(t0);
        order.apply(rows(&[1, 2]), true, false, t0);
        assert_eq!(order.deadline(false), None);
    }

    #[test]
    fn live_sort_key_reorders_at_most_once_per_interval() {
        let t0 = Instant::now();
        let mut order = RowOrder::default();
        order.apply(rows(&[1, 2]), false, true, t0);

        let throttled = order.apply(rows(&[2, 1]), true, true, t0 + Duration::from_millis(500));
        assert_eq!(ids(&throttled), [1, 2]);
        assert_eq!(order.deadline(true), Some(t0 + LIVE_RESORT_INTERVAL));

        let resorted = order.apply(rows(&[2, 1]), true, true, t0 + LIVE_RESORT_INTERVAL);
        assert_eq!(ids(&resorted), [2, 1]);
        // 非动态键不受限频影响。
        let immediate = order.apply(rows(&[1, 2]), true, false, t0 + LIVE_RESORT_INTERVAL);
        assert_eq!(ids(&immediate), [1, 2]);
    }
}
