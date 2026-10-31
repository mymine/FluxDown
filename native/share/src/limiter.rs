//! 发送限速：字节令牌桶。速率为 0 表示不限，且不限速时没有任何定时器。

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// 令牌桶最小突发量：保证一次放行至少一个读块。
const MIN_BURST: f64 = 64.0 * 1024.0;

#[derive(Debug)]
struct State {
    /// 每秒字节数，0 = 不限。
    rate: u64,
    tokens: f64,
    last: Instant,
}

/// 可克隆的限速器，克隆体共享同一个桶。
#[derive(Debug, Clone)]
pub struct RateLimiter {
    state: Arc<Mutex<State>>,
}

impl RateLimiter {
    /// 创建限速器，`bytes_per_sec == 0` 表示不限。
    pub fn new(bytes_per_sec: u64) -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                rate: bytes_per_sec,
                tokens: burst(bytes_per_sec),
                last: Instant::now(),
            })),
        }
    }

    /// 动态调整速率（0 = 不限），立即对后续 [`Self::acquire`] 生效。
    pub fn set_rate(&self, bytes_per_sec: u64) {
        let mut st = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        st.rate = bytes_per_sec;
        st.tokens = st.tokens.min(burst(bytes_per_sec));
        st.last = Instant::now();
    }

    /// 当前速率（字节 / 秒，0 = 不限）。
    pub fn rate(&self) -> u64 {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .rate
    }

    /// 申请发送 `bytes` 字节；桶里不够时睡到够为止（允许一次性透支，保证大块也能前进）。
    pub async fn acquire(&self, bytes: usize) {
        let wait = {
            let mut st = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            if st.rate == 0 {
                return;
            }
            let now = Instant::now();
            let rate = st.rate as f64;
            let elapsed = now.saturating_duration_since(st.last).as_secs_f64();
            st.last = now;
            st.tokens = (st.tokens + elapsed * rate).min(burst(st.rate));
            st.tokens -= bytes as f64;
            if st.tokens < 0.0 {
                Duration::from_secs_f64(-st.tokens / rate)
            } else {
                Duration::ZERO
            }
        };
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }
}

fn burst(rate: u64) -> f64 {
    (rate as f64 / 10.0).max(MIN_BURST)
}
