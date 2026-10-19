//! 全局热键平台后端门面：各平台后端实现 [`crate::hotkey::HotkeyPort`]，由这里按目标平台选择。
//!
//! - Windows / Linux：后端自带线程，经 `start` 在 agent 运行时上下文里启动，返回线程安全的端口。
//! - macOS：Carbon 热键与 NSEvent 监听只能在主线程运行，后端对象留在宿主（tao）事件循环里，
//!   `start` 经 [`MainThreadHost`] 把创建与后续 `apply` 都投递到主线程。

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
pub(crate) mod macos;
#[cfg(windows)]
mod windows;

use std::sync::Arc;

use crate::hotkey::{HotkeyPort, HotkeySink, MainThreadHost};

/// 启动当前平台的热键后端；`Err(detail)` = 后端不可用（调用方按「不可用」处理）。
#[cfg(windows)]
pub(crate) fn start(
    sink: Arc<dyn HotkeySink>,
    _main_thread: Option<&Arc<dyn MainThreadHost>>,
) -> Result<Box<dyn HotkeyPort>, String> {
    windows::start(sink)
}

#[cfg(target_os = "linux")]
pub(crate) fn start(
    sink: Arc<dyn HotkeySink>,
    _main_thread: Option<&Arc<dyn MainThreadHost>>,
) -> Result<Box<dyn HotkeyPort>, String> {
    linux::start(sink)
}

#[cfg(target_os = "macos")]
pub(crate) fn start(
    sink: Arc<dyn HotkeySink>,
    main_thread: Option<&Arc<dyn MainThreadHost>>,
) -> Result<Box<dyn HotkeyPort>, String> {
    main_thread
        .map(|host| host.spawn(sink))
        .ok_or_else(|| "no host event loop to run global hotkeys on".to_owned())
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub(crate) fn start(
    _sink: Arc<dyn HotkeySink>,
    _main_thread: Option<&Arc<dyn MainThreadHost>>,
) -> Result<Box<dyn HotkeyPort>, String> {
    Err("global hotkeys are not supported on this platform".to_owned())
}
