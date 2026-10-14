//! 托盘「显示窗口」去重：一次双击会产生两次左键抬起，排队的点击也可能在控制循环空闲后
//! 成批到达；每次都拉起桌面进程既浪费又会在日志里留下成串的转发进程。

use std::time::{Duration, Instant};

/// 两次拉起的最小间隔：覆盖系统双击间隔（Windows 默认 500ms）。
const MIN_INTERVAL: Duration = Duration::from_millis(600);
/// 拉起的进程既没退出也没连上 UI 时，最多压住后续点击这么久，避免卡死的进程让托盘失灵。
const IN_FLIGHT_LIMIT: Duration = Duration::from_secs(10);

/// 托盘拉起主窗口的单飞状态：进程退出或有官方 UI 连上即视为落定。
#[derive(Debug, Default)]
pub(super) struct ShowWindowGuard {
    last: Option<Instant>,
    in_flight: bool,
}

impl ShowWindowGuard {
    /// 允许本次拉起时登记并返回 `true`。
    pub(super) fn try_begin(&mut self, now: Instant) -> bool {
        if let Some(last) = self.last {
            let elapsed = now.saturating_duration_since(last);
            if elapsed < MIN_INTERVAL || (self.in_flight && elapsed < IN_FLIGHT_LIMIT) {
                return false;
            }
        }
        self.last = Some(now);
        self.in_flight = true;
        true
    }

    /// 拉起的进程已退出（含转发后退出）、没能启动，或官方 UI 已连上。
    pub(super) fn settled(&mut self) {
        self.in_flight = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn double_click_and_queued_clicks_launch_once() {
        let start = Instant::now();
        let mut guard = ShowWindowGuard::default();
        assert!(guard.try_begin(start));
        // 双击的第二次抬起，以及进程退出前成批到达的点击。
        assert!(!guard.try_begin(start + Duration::from_millis(150)));
        assert!(!guard.try_begin(start + Duration::from_secs(2)));
        guard.settled();
        assert!(guard.try_begin(start + Duration::from_secs(2)));
    }

    #[test]
    fn settling_does_not_bypass_double_click_interval() {
        let start = Instant::now();
        let mut guard = ShowWindowGuard::default();
        assert!(guard.try_begin(start));
        // 转发进程可能在第二次抬起之前就已退出。
        guard.settled();
        assert!(!guard.try_begin(start + Duration::from_millis(200)));
        assert!(guard.try_begin(start + MIN_INTERVAL));
    }

    #[test]
    fn stuck_launch_releases_after_limit() {
        let start = Instant::now();
        let mut guard = ShowWindowGuard::default();
        assert!(guard.try_begin(start));
        assert!(!guard.try_begin(start + IN_FLIGHT_LIMIT - Duration::from_millis(1)));
        assert!(guard.try_begin(start + IN_FLIGHT_LIMIT));
    }
}
