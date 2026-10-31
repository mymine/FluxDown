//! 诊断刷新的节流 / 防抖状态机：hub 每次变化（含传输进度）都可能触发刷新，
//! 但同一视图两次取数之间必须隔开一段时间；输入框键入则用代际号做尾随防抖。
//! 时间由调用方以毫秒数传入，便于单测。

use std::time::Duration;

/// 一次刷新请求的处置。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateAction {
    /// 立即取数。
    Fetch,
    /// 已排队：等待这段时间后调用 [`RefreshGate::fire`]，再按需重新 [`RefreshGate::request`]。
    Wait(Duration),
    /// 已有排队中的刷新，本次请求并入它。
    Skip,
}

/// 尾随节流：保证相邻两次取数至少间隔 `min_interval_ms`；间隔内的变化合并成冷却结束时的一次取数。
#[derive(Clone, Copy, Debug)]
pub struct RefreshGate {
    min_interval_ms: u64,
    last_ms: Option<u64>,
    armed: bool,
}

impl RefreshGate {
    #[must_use]
    pub const fn new(min_interval_ms: u64) -> Self {
        Self {
            min_interval_ms,
            last_ms: None,
            armed: false,
        }
    }

    /// 有变化 / 需要刷新。
    pub fn request(&mut self, now_ms: u64) -> GateAction {
        if self.armed {
            return GateAction::Skip;
        }
        match self.last_ms {
            Some(last) if now_ms.saturating_sub(last) < self.min_interval_ms => {
                self.armed = true;
                GateAction::Wait(Duration::from_millis(
                    self.min_interval_ms - now_ms.saturating_sub(last),
                ))
            }
            _ => {
                self.last_ms = Some(now_ms);
                GateAction::Fetch
            }
        }
    }

    /// 排队的定时器到点：解除排队（不记取数时间；由随后的 `request` 决定是否取数）。
    pub fn fire(&mut self) {
        self.armed = false;
    }

    /// 不经节流的取数（用户手动刷新）：只记录开始时间，已排队的定时器照常到点。
    pub fn mark_started(&mut self, now_ms: u64) {
        self.last_ms = Some(now_ms);
    }

    /// 距离上次取数是否已超过 `age_ms`（从未取过也算）。
    #[must_use]
    pub fn older_than(&self, now_ms: u64, age_ms: u64) -> bool {
        self.last_ms
            .is_none_or(|last| now_ms.saturating_sub(last) >= age_ms)
    }
}

/// 尾随防抖的代际号：每次键入 `bump`，延时到点后只有仍是最新一代的任务才真正执行。
#[derive(Clone, Copy, Debug, Default)]
pub struct Debounce {
    generation: u64,
}

impl Debounce {
    pub fn bump(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.generation
    }

    #[must_use]
    pub fn is_current(&self, generation: u64) -> bool {
        self.generation == generation
    }
}

/// 距离上一次渲染超过这么久（毫秒）视为视图被隐藏过。
pub const HIDDEN_GAP_MS: u64 = 5_000;
/// 隐藏后再显示时，数据超过这么久（毫秒）未取就补取一次。
pub const STALE_AFTER_MS: u64 = 10_000;

/// 「只在可见时取数」：视图只在被渲染时才有机会发起取数。hub 变化 / 定时器只置 `dirty`，
/// 视图被隐藏（不再渲染）期间 `dirty` 一直挂着、不产生请求；重新显示的第一次渲染再决定是否补取。
#[derive(Clone, Copy, Debug)]
pub struct VisibleRefresh {
    gate: RefreshGate,
    last_render_ms: Option<u64>,
    dirty: bool,
}

impl VisibleRefresh {
    /// 首次渲染必取一次。
    #[must_use]
    pub const fn new(min_interval_ms: u64) -> Self {
        Self {
            gate: RefreshGate::new(min_interval_ms),
            last_render_ms: None,
            dirty: true,
        }
    }

    /// 数据可能已变化（hub 通知、用户操作）。
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// 视图渲染时调用：返回需要执行的动作（`None` = 无事）。
    pub fn on_render(&mut self, now_ms: u64) -> Option<GateAction> {
        let resumed = self
            .last_render_ms
            .is_none_or(|last| now_ms.saturating_sub(last) > HIDDEN_GAP_MS);
        self.last_render_ms = Some(now_ms);
        if resumed && self.gate.older_than(now_ms, STALE_AFTER_MS) {
            self.dirty = true;
        }
        if !self.dirty {
            return None;
        }
        self.dirty = false;
        match self.gate.request(now_ms) {
            GateAction::Skip => None,
            action => Some(action),
        }
    }

    /// 排队的定时器到点：解除排队并要求重绘后再判定（页面此刻不可见则不会取数）。
    pub fn timer_fired(&mut self) {
        self.gate.fire();
        self.dirty = true;
    }

    /// 不经节流的取数（用户手动刷新）。
    pub fn manual_started(&mut self, now_ms: u64) {
        self.gate.mark_started(now_ms);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{Debounce, GateAction, RefreshGate, VisibleRefresh};

    #[test]
    fn first_request_fetches_and_changes_inside_the_cooldown_collapse_to_one_wait() {
        let mut gate = RefreshGate::new(2_000);
        assert_eq!(gate.request(10_000), GateAction::Fetch);
        assert_eq!(
            gate.request(10_500),
            GateAction::Wait(Duration::from_millis(1_500))
        );
        assert_eq!(gate.request(10_900), GateAction::Skip);
        assert_eq!(gate.request(11_900), GateAction::Skip);
        gate.fire();
        assert_eq!(gate.request(12_000), GateAction::Fetch);
        // 刚取过数：冷却重新开始。
        assert_eq!(
            gate.request(12_100),
            GateAction::Wait(Duration::from_millis(1_900))
        );
        gate.fire();
        assert_eq!(gate.request(14_000), GateAction::Fetch);
    }

    #[test]
    fn manual_fetch_restarts_the_cooldown_and_age_is_reported() {
        let mut gate = RefreshGate::new(1_000);
        assert!(gate.older_than(0, 5_000));
        gate.mark_started(100);
        assert!(!gate.older_than(4_000, 5_000));
        assert!(gate.older_than(5_100, 5_000));
        assert_eq!(
            gate.request(600),
            GateAction::Wait(Duration::from_millis(500))
        );
        // 时钟回拨不会下溢。
        assert!(!gate.older_than(0, 5_000));
    }

    #[test]
    fn only_the_latest_debounce_generation_is_current() {
        let mut debounce = Debounce::default();
        let first = debounce.bump();
        let second = debounce.bump();
        assert!(!debounce.is_current(first));
        assert!(debounce.is_current(second));
    }

    #[test]
    fn visible_refresh_fetches_on_first_render_and_stays_quiet_while_hidden() {
        let mut refresh = VisibleRefresh::new(2_000);
        assert_eq!(refresh.on_render(0), Some(GateAction::Fetch));
        // 同一次可见期间没有变化：不再取数。
        assert_eq!(refresh.on_render(500), None);

        // 变化在冷却内：排队等待；定时器到点后若页面隐藏则没有渲染 → 没有取数。
        refresh.mark_dirty();
        assert_eq!(
            refresh.on_render(1_000),
            Some(GateAction::Wait(Duration::from_millis(1_000)))
        );
        refresh.timer_fired();
        // 隐藏很久后第一次渲染：数据陈旧，补取一次。
        assert_eq!(refresh.on_render(60_000), Some(GateAction::Fetch));
        assert_eq!(refresh.on_render(60_500), None);
    }

    #[test]
    fn resuming_with_fresh_data_does_not_refetch_and_manual_fetch_counts() {
        let mut refresh = VisibleRefresh::new(1_000);
        assert_eq!(refresh.on_render(0), Some(GateAction::Fetch));
        // 隐藏 6 秒后再显示：未超过陈旧阈值，不取。
        assert_eq!(refresh.on_render(6_000), None);
        // 手动刷新记入取数时间：随后（隐藏后）再显示不算陈旧。
        refresh.manual_started(20_000);
        assert_eq!(refresh.on_render(27_000), None);
        assert_eq!(refresh.on_render(31_000), None);
        // 超过阈值才补取。
        assert_eq!(refresh.on_render(38_000), Some(GateAction::Fetch));
    }
}
