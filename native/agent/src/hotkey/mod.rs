//! 全局快捷键：读取设备本地偏好 `desktop.shortcuts.global` → 平台后端注册系统级热键 →
//! 触发时执行外壳动作。界面关闭后 agent 驻留期间仍生效；状态经 `ShellStatusDto.global_shortcuts`
//! 回报给设置页。
//!
//! 平台后端（`platform::hotkey`）只认 [`HotkeyBinding`]（平台无关触发器 + 不透明 id），经
//! [`HotkeyPort`] 接收整表替换，经 [`HotkeySink`] 回报触发与注册结果；动作语义只在本模块。

mod controller;
pub(crate) mod double_tap;

use std::sync::Arc;

pub(crate) use controller::{HotkeyController, HotkeyEvent};
use fluxdown_protocol::shortcuts::{GlobalShortcutBackend, GlobalShortcutState, ShortcutTrigger};

/// 后端注册单元；`id` 在一次 [`HotkeyPort::apply`] 内唯一，触发时原样回报。
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HotkeyBinding {
    pub id: u32,
    pub trigger: ShortcutTrigger,
    /// 动作的本地化名称：Wayland portal 在桌面环境的快捷键设置里展示，其它后端不使用。
    pub description: String,
}

/// 单个绑定的注册结果。
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HotkeyOutcome {
    pub id: u32,
    pub state: GlobalShortcutState,
    /// 诊断文本（仅 `Failed`）。
    pub detail: Option<String>,
}

/// 平台后端 → 控制器回调；可在任意线程调用，实现不得阻塞。
pub(crate) trait HotkeySink: Send + Sync + 'static {
    /// 某绑定被触发。
    fn fired(&self, id: u32);
    /// 一次 apply（或权限等外部状态变化）后的完整结果，整体覆盖此前的报告。
    fn report(&self, backend: GlobalShortcutBackend, outcomes: Vec<HotkeyOutcome>);
}

/// 控制器 → 平台后端：以新表整体替换已注册的绑定（空表 = 全部注销）。实现不得阻塞调用方，
/// 结果异步经 [`HotkeySink::report`] 回报。
pub(crate) trait HotkeyPort: Send + Sync {
    fn apply(&self, bindings: Vec<HotkeyBinding>);
}

/// 宿主主线程上的后端承载。macOS 的 Carbon 热键与 NSEvent 监听只能在主线程（tao 事件循环）
/// 运行，由宿主实现；其它平台后端自带线程，不使用。
pub(crate) trait MainThreadHost: Send + Sync {
    /// 在主线程创建后端并返回其端口；非阻塞，创建失败经 [`HotkeySink::report`] 回报
    /// `Unavailable`。
    #[cfg_attr(
        not(target_os = "macos"),
        expect(dead_code, reason = "只有 macOS 后端需要主线程承载")
    )]
    fn spawn(&self, sink: Arc<dyn HotkeySink>) -> Box<dyn HotkeyPort>;
}
