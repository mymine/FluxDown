//! 额度取数的「只在可见时」节流：分享页摘要行与账户页额度视图共用。
//!
//! 数据本身在 `ShareStateDto.usage`（agent 经云端推送 / 事件刷新）；这里只负责在持有者被渲染（可见）时
//! 经 [`VisibleRefresh`] 节流调用 `agent.share.usageRefresh`，hub 变化只重绘不取数，所以空闲时没有任何请求。

use std::time::Instant;

use gpui::{Context, Entity};

use crate::{
    hub::ShareHub,
    model::refresh::{GateAction, VisibleRefresh},
};

/// 两次自动取数之间的最短间隔（毫秒）。
const MIN_REFRESH_MS: u64 = 2_000;

/// 从持有者视图里取出自己的 [`UsageSync`]（定时器与取数回调用它回到状态）。
pub(crate) type SyncAccess<V> = fn(&mut V) -> &mut UsageSync;

pub(crate) struct UsageSync {
    epoch: Instant,
    refresh: VisibleRefresh,
    request: u64,
    pub(crate) loading: bool,
    /// 至少完成过一次取数（区分「还没取」与「未登录」）。
    pub(crate) loaded: bool,
    pub(crate) failed: bool,
}

impl UsageSync {
    pub(crate) fn new() -> Self {
        Self {
            epoch: Instant::now(),
            refresh: VisibleRefresh::new(MIN_REFRESH_MS),
            request: 0,
            loading: false,
            loaded: false,
            failed: false,
        }
    }

    fn now_ms(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// 持有者渲染时调用：首次显示 / 隐藏后再显示才会取数。
    pub(crate) fn refresh_if_needed<V: 'static>(
        &mut self,
        hub: &Entity<ShareHub>,
        access: SyncAccess<V>,
        cx: &mut Context<V>,
    ) {
        if !hub.read(cx).available() {
            return;
        }
        let now = self.now_ms();
        match self.refresh.on_render(now) {
            Some(GateAction::Fetch) => self.start_fetch(hub, access, cx),
            Some(GateAction::Wait(delay)) => {
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(delay).await;
                    let Ok(()) = this.update(cx, |this, cx| {
                        access(this).refresh.timer_fired();
                        cx.notify();
                    }) else {
                        return;
                    };
                })
                .detach();
            }
            Some(GateAction::Skip) | None => {}
        }
    }

    /// 用户手动刷新：不经节流立即取数。
    pub(crate) fn refresh_now<V: 'static>(
        &mut self,
        hub: &Entity<ShareHub>,
        access: SyncAccess<V>,
        cx: &mut Context<V>,
    ) {
        if !hub.read(cx).available() {
            return;
        }
        self.refresh.manual_started(self.now_ms());
        self.start_fetch(hub, access, cx);
        cx.notify();
    }

    fn start_fetch<V: 'static>(
        &mut self,
        hub: &Entity<ShareHub>,
        access: SyncAccess<V>,
        cx: &mut Context<V>,
    ) {
        self.request += 1;
        let request = self.request;
        self.loading = true;
        let future = hub.read(cx).api().usage_refresh();
        cx.spawn(async move |this, cx| {
            let result = future.await;
            let Ok(()) = this.update(cx, |this, cx| {
                let sync = access(this);
                if request != sync.request {
                    return;
                }
                sync.loading = false;
                sync.loaded = true;
                sync.failed = result.is_err();
                cx.notify();
            }) else {
                return;
            };
        })
        .detach();
    }
}
