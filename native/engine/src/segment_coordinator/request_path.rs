//! 同一 coordinator generation 内跨 worker 共享的请求路径观测。
//!
//! - **重定向终点复用**：原始 URL 经 302 落到 CDN / 对象存储时，每个分段请求都
//!   重放一遍跳转链。实测热连接下每跳 +0.4~0.7s（GitHub release → release-assets、
//!   VS Code update → prss CDN），冷连接 +1~2s，而直连终点只要几十毫秒。首个成功
//!   响应学到终点后，后续 SYS 租约的 Range 请求直接发往终点；终点请求一旦失败
//!   （签名过期 / 一次性终点 / 按请求分配的镜像），本任务内永久退回原始 URL。
//!   直连请求以任务 URL 为凭据锚点（`HttpRequest::anchor`），与经跳转链抵达终点
//!   携带的 Cookie / Authorization 完全相同，跨源终点不会多拿到源站凭据。
//! - **请求建立延迟**：发出请求到收到响应头的 EWMA（含建连与跳转），供拆分判据
//!   估计「帮手开始收字节前要等多久」。

use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

#[derive(Default)]
pub(super) struct RequestPath {
    redirect_target: StdMutex<Option<String>>,
    reuse_disabled: AtomicBool,
    /// 请求建立延迟 EWMA（微秒）；0 = 尚无样本。
    setup_micros: AtomicU64,
}

impl RequestPath {
    /// 已学到且仍可复用的重定向终点；`None` = 使用原始 URL。
    pub(super) fn redirect_target(&self) -> Option<String> {
        if self.reuse_disabled.load(Ordering::Relaxed) {
            return None;
        }
        self.redirect_target
            .lock()
            .ok()
            .and_then(|target| target.clone())
    }

    /// 发往原始 URL 的请求最终落在 `final_url`：记住终点供后续分段直连。
    pub(super) fn learn_redirect(&self, origin: &str, final_url: &str) {
        if final_url == origin || self.reuse_disabled.load(Ordering::Relaxed) {
            return;
        }
        if let Ok(mut target) = self.redirect_target.lock()
            && target.as_deref() != Some(final_url)
        {
            *target = Some(final_url.to_owned());
        }
    }

    /// 直连终点失败：本任务内不再复用（终点可能过期、一次性或按请求分配）。
    pub(super) fn abandon_redirect(&self) {
        self.reuse_disabled.store(true, Ordering::Relaxed);
        if let Ok(mut target) = self.redirect_target.lock() {
            *target = None;
        }
    }

    /// 记录一次「发出请求 → 收到响应头」耗时（新样本权重 1/4）。
    pub(super) fn record_setup(&self, elapsed: Duration) {
        let sample = u64::try_from(elapsed.as_micros())
            .unwrap_or(u64::MAX)
            .max(1);
        self.setup_micros
            .update(Ordering::Relaxed, Ordering::Relaxed, |old| {
                if old == 0 {
                    sample
                } else {
                    old.saturating_mul(3).saturating_add(sample) / 4
                }
            });
    }

    /// 请求建立延迟估计（秒）；无样本 = 0（不对拆分施加延迟约束）。
    pub(super) fn setup_secs(&self) -> f64 {
        Duration::from_micros(self.setup_micros.load(Ordering::Relaxed)).as_secs_f64()
    }
}

#[cfg(test)]
mod tests {
    use super::RequestPath;
    use std::time::Duration;

    #[test]
    fn learned_target_is_reused_until_abandoned_for_good() {
        let path = RequestPath::default();
        assert_eq!(path.redirect_target(), None);
        path.learn_redirect("https://a/x", "https://a/x");
        assert_eq!(path.redirect_target(), None, "未跳转不产生终点");

        path.learn_redirect("https://a/x", "https://cdn/x?sig=1");
        assert_eq!(
            path.redirect_target().as_deref(),
            Some("https://cdn/x?sig=1")
        );

        path.abandon_redirect();
        assert_eq!(path.redirect_target(), None);
        path.learn_redirect("https://a/x", "https://cdn/x?sig=2");
        assert_eq!(path.redirect_target(), None, "失败过的任务不再复用终点");
    }

    #[test]
    fn setup_latency_is_smoothed() {
        let path = RequestPath::default();
        assert_eq!(path.setup_secs(), 0.0);
        path.record_setup(Duration::from_millis(800));
        assert!((path.setup_secs() - 0.8).abs() < 1e-9);
        path.record_setup(Duration::from_millis(0));
        // (800ms × 3 + 1µs) / 4 ≈ 600ms：单个快样本不会把估计清零。
        assert!((path.setup_secs() - 0.6).abs() < 1e-3);
    }
}
