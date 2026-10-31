//! 应用图标的系统集成（`desktop` feature）：把当前选择落到 agent 之外的系统表面。
//!
//! - Windows：桌面进程已有窗口的 `WM_SETICON`（跨进程）、桌面 / 开始菜单 / 任务栏固定项 `.lnk` 的
//!   `IconLocation`（见 `windows`），以及桌面程序 exe 自身的图标资源（见 `exe_icon`）；
//! - Linux：X11 现有窗口的 `_NET_WM_ICON` 与用户级 XDG 图标覆盖，见 `linux`；ELF 没有内嵌图标，
//!   系统监视器 / 应用菜单的图标来自 `.desktop` 的 `Icon=` 经图标主题解析，已被 XDG 覆盖涵盖；
//! - macOS：Dock / Cmd-Tab 由桌面进程自己设置；agent 负责 bundle 本身的 Finder 自定义图标
//!   （外层与辅助 bundle，见 `macos`）。
//!
//! 窗口只按「类名 + 进程映像 = 桌面程序」识别，快捷方式只按「目标 = 桌面程序」识别；两者都不接受
//! 调用方传入句柄 / 路径。所有函数同步阻塞，调用方放进 `spawn_blocking`。

#[cfg(any(target_os = "macos", windows, test))]
mod binary_record;
#[cfg(windows)]
mod exe_icon;
#[cfg(any(windows, test))]
mod ico_resource;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(any(windows, test))]
mod lnk_policy;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(any(windows, test))]
mod pe_resources;
#[cfg(any(target_os = "linux", test))]
mod window_filter;
#[cfg(windows)]
mod windows;
#[cfg(any(target_os = "linux", test))]
mod xdg;

use fluxdown_protocol::app_icon::AppIconChoice;

use super::PlatformError;
use crate::app_icon::IconStore;

/// 给桌面进程已有的顶层窗口设置当前图标（Windows / X11）；其余平台无操作。
///
/// # Errors
///
/// 图标文件生成失败或系统调用失败时返回 [`PlatformError`]。
#[cfg(windows)]
pub(crate) fn sync_window_icons(
    store: &IconStore,
    choice: &AppIconChoice,
) -> Result<(), PlatformError> {
    let ico = store
        .ensure_ico(choice)
        .map_err(crate::app_icon::platform_error)?;
    windows::sync_windows(ico.as_deref()).map_err(PlatformError::Failed)
}

/// 同 Windows 版：X11 窗口按 `_NET_WM_ICON` 刷新；无 `DISPLAY` 时无操作。
#[cfg(target_os = "linux")]
pub(crate) fn sync_window_icons(
    store: &IconStore,
    choice: &AppIconChoice,
) -> Result<(), PlatformError> {
    linux::sync_windows(store, choice)
}

#[cfg(not(any(windows, target_os = "linux")))]
pub(crate) fn sync_window_icons(
    _store: &IconStore,
    _choice: &AppIconChoice,
) -> Result<(), PlatformError> {
    Ok(())
}

/// 刷新持久入口：Windows `.lnk`、Linux 用户 XDG 图标覆盖；macOS 无此类入口（bundle 见 [`sync_binary_icon`]）。
///
/// # Errors
///
/// 图标文件生成失败或系统调用失败时返回 [`PlatformError`]。
#[cfg(windows)]
pub(crate) fn sync_persistent_entries(
    store: &IconStore,
    choice: &AppIconChoice,
) -> Result<(), PlatformError> {
    let ico = store
        .ensure_ico(choice)
        .map_err(crate::app_icon::platform_error)?;
    windows::sync_shortcuts(ico.as_deref(), store.dir()).map_err(PlatformError::Failed)
}

#[cfg(target_os = "linux")]
pub(crate) fn sync_persistent_entries(
    store: &IconStore,
    choice: &AppIconChoice,
) -> Result<(), PlatformError> {
    linux::sync_xdg_override(store, choice)
}

#[cfg(not(any(windows, target_os = "linux")))]
pub(crate) fn sync_persistent_entries(
    _store: &IconStore,
    _choice: &AppIconChoice,
) -> Result<(), PlatformError> {
    Ok(())
}

/// 程序本体（Windows 的桌面程序 exe / macOS 的 `.app` bundle）的图标跟随当前选择。
///
/// 返回 `Ok(true)` 表示补丁**待办**：目标还被运行中的进程占用（Windows 桌面进程在跑），调用方应在
/// 官方 UI 断开后再调用一次（比较后再写，重复调用安全）。其余情况返回 `Ok(false)`。
///
/// # Errors
///
/// 图标文件生成失败或系统调用失败时返回 [`PlatformError`]。
#[cfg(windows)]
pub(crate) fn sync_binary_icon(
    store: &IconStore,
    choice: &AppIconChoice,
) -> Result<bool, PlatformError> {
    let desired = match store
        .ensure_ico(choice)
        .map_err(crate::app_icon::platform_error)?
    {
        Some(path) => Some(std::fs::read(&path).map_err(|error| {
            PlatformError::Failed(format!("could not read {}: {error}", path.display()))
        })?),
        None => None,
    };
    exe_icon::sync(&choice.to_preference(), desired.as_deref(), store.dir())
        .map_err(PlatformError::Failed)
}

#[cfg(target_os = "macos")]
pub(crate) fn sync_binary_icon(
    store: &IconStore,
    choice: &AppIconChoice,
) -> Result<bool, PlatformError> {
    macos::sync_bundle_icons(store, choice, store.dir())
        .map(|()| false)
        .map_err(PlatformError::Failed)
}

/// Linux：ELF 没有内嵌图标，进程 / 应用列表的图标来自 `.desktop` 的 `Icon=`，由 XDG 覆盖负责。
#[cfg(not(any(windows, target_os = "macos")))]
pub(crate) fn sync_binary_icon(
    _store: &IconStore,
    _choice: &AppIconChoice,
) -> Result<bool, PlatformError> {
    Ok(false)
}
