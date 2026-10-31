//! 手动触发的异步动作的「进行中」最短可见时长。
//!
//! 本机 RPC 常在一帧之内返回：`loading` 置真又置假之间没有任何一帧被绘制，按钮转圈
//! 根本不出现，用户看到的就是「点了没反应」。[`BusyHold`] 在点击时开始计时，保证
//! 转圈至少可见 [`MIN_BUSY_VISIBLE`]；数据照常即时落地，只延长视觉反馈。
//!
//! 只在**用户点击**时调用 [`BusyHold::start`]：后台轮询 / 自动刷新共用的 `loading` 标志
//! 不要接它，否则每次轮询都会转一下。

use std::{cell::Cell, rc::Rc, time::Duration};

use gpui::{Context, Task};

/// 手动触发的异步动作最短显示「进行中」的时长：短于此的请求也完整转过约一圈，
/// 长于此的请求按真实时长显示。
pub const MIN_BUSY_VISIBLE: Duration = Duration::from_millis(500);

/// 一次手动操作的最短「进行中」保持。放在视图（或 store 实体）字段里；渲染时与真实的
/// 进行中标志取或：`loading || hold.is_held()`。
#[derive(Default)]
pub struct BusyHold {
    held: Rc<Cell<bool>>,
    timer: Option<Task<()>>,
}

impl BusyHold {
    /// 用户点击时调用：从现在起至少保持 [`MIN_BUSY_VISIBLE`]，到期后通知所在实体重绘。
    /// 期间再次调用会重新计时（旧计时器随替换取消）。
    pub fn start<V: 'static>(&mut self, cx: &mut Context<V>) {
        // 每次开始用新的标志：被取消的旧计时器即使已越过 await 也碰不到当前标志。
        let held = Rc::new(Cell::new(true));
        self.held = held.clone();
        self.timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(MIN_BUSY_VISIBLE).await;
            held.set(false);
            let Ok(()) = this.update(cx, |_, cx| cx.notify()) else {
                // 所在实体已释放：没有需要重绘的读者。
                return;
            };
        }));
        cx.notify();
    }

    /// 是否仍在最短可见期内。
    #[must_use]
    pub fn is_held(&self) -> bool {
        self.held.get()
    }
}
