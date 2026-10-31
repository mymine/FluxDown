//! Linux：X11 现有窗口的 `_NET_WM_ICON` + 用户级 XDG 图标覆盖。零 `unsafe`。
//!
//! Wayland 没有运行期逐窗口图标协议，窗口 / Dash 图标靠 `app_id` 匹配 `.desktop` 的 `Icon=`，
//! 由 XDG 覆盖文件负责；X11 新窗口的图标由桌面进程建窗时设置。

use std::path::Path;

use fluxdown_protocol::app_icon::AppIconChoice;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, PropMode, Window};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use super::{window_filter, xdg};
use crate::app_icon::{DEFAULT_LOGO_PNG, IconStore, image as pipeline, platform_error};
use crate::platform::PlatformError;

/// `_NET_WM_ICON` 提供的尺寸（降序，窗口管理器按需挑选最接近的）。
const WINDOW_ICON_SIZES: [u32; 6] = [256, 128, 64, 48, 32, 16];
/// XDG 覆盖文件的边长（对应 `hicolor/256x256/apps`）。
const XDG_ICON_SIZE: u32 = 256;
/// `WM_CLASS` 读取上限（4 字节单位）。
const WM_CLASS_LONGS: u32 = 256;

fn failed(context: &str, error: impl std::fmt::Display) -> PlatformError {
    PlatformError::Failed(format!("{context}: {error}"))
}

/// 给桌面进程所有 X11 顶层窗口设置 `_NET_WM_ICON`；没有 `DISPLAY`（纯 Wayland / 无图形）无操作。
pub(super) fn sync_windows(store: &IconStore, choice: &AppIconChoice) -> Result<(), PlatformError> {
    if std::env::var_os("DISPLAY").is_none_or(|display| display.is_empty()) {
        tracing::debug!("no X11 display; skipping window icon sync");
        return Ok(());
    }
    let Some(desktop_exe) =
        crate::platform::desktop_executable().and_then(|path| std::fs::canonicalize(path).ok())
    else {
        tracing::debug!("desktop executable not found; skipping window icon sync");
        return Ok(());
    };
    let image = match store.image_for(choice).map_err(platform_error)? {
        Some(image) => image,
        None => pipeline::decode_limited(DEFAULT_LOGO_PNG)
            .map_err(|error| failed("default logo", error))?,
    };
    let data: Vec<u32> = WINDOW_ICON_SIZES
        .iter()
        .flat_map(|&size| pipeline::argb_cardinals(&image, size))
        .collect();
    set_window_icons(&data, &desktop_exe)
}

fn set_window_icons(data: &[u32], desktop_exe: &Path) -> Result<(), PlatformError> {
    let (conn, screen_num) = RustConnection::connect(None)
        .map_err(|error| failed("cannot connect to X server", error))?;
    let root = conn
        .setup()
        .roots
        .get(screen_num)
        .map(|screen| screen.root)
        .ok_or_else(|| PlatformError::Failed("X screen missing".to_owned()))?;
    let intern = |name: &[u8]| -> Result<u32, PlatformError> {
        conn.intern_atom(false, name)
            .map_err(|error| failed("intern atom", error))?
            .reply()
            .map(|reply| reply.atom)
            .map_err(|error| failed("intern atom", error))
    };
    let client_list = intern(b"_NET_CLIENT_LIST")?;
    let wm_icon = intern(b"_NET_WM_ICON")?;
    let wm_pid = intern(b"_NET_WM_PID")?;

    let clients: Vec<Window> = conn
        .get_property(false, root, client_list, AtomEnum::WINDOW, 0, u32::MAX)
        .map_err(|error| failed("read _NET_CLIENT_LIST", error))?
        .reply()
        .map_err(|error| failed("read _NET_CLIENT_LIST", error))?
        .value32()
        .map(Iterator::collect)
        .unwrap_or_default();

    let mut updated = 0usize;
    for window in clients {
        match is_desktop_window(&conn, window, wm_pid, desktop_exe) {
            Ok(true) => {}
            Ok(false) => continue,
            // 窗口在枚举与查询之间被销毁：不是错误。
            Err(error) => {
                tracing::debug!(window, %error, "could not inspect X11 window");
                continue;
            }
        }
        let result = conn
            .change_property32(PropMode::REPLACE, window, wm_icon, AtomEnum::CARDINAL, data)
            .map_err(|error| failed("set _NET_WM_ICON", error))?
            .check();
        match result {
            Ok(()) => updated += 1,
            Err(error) => tracing::debug!(window, %error, "could not set _NET_WM_ICON"),
        }
    }
    conn.flush()
        .map_err(|error| failed("flush X connection", error))?;
    tracing::debug!(updated, "X11 window icons synced");
    Ok(())
}

fn is_desktop_window(
    conn: &RustConnection,
    window: Window,
    wm_pid: u32,
    desktop_exe: &Path,
) -> Result<bool, x11rb::errors::ReplyError> {
    let class = conn
        .get_property(
            false,
            window,
            AtomEnum::WM_CLASS,
            AtomEnum::STRING,
            0,
            WM_CLASS_LONGS,
        )?
        .reply()?;
    if !window_filter::class_matches(&class.value) {
        return Ok(false);
    }
    let pid = conn
        .get_property(false, window, wm_pid, AtomEnum::CARDINAL, 0, 1)?
        .reply()?
        .value32()
        .and_then(|mut values| values.next());
    let process_exe = pid.and_then(|pid| std::fs::canonicalize(format!("/proc/{pid}/exe")).ok());
    Ok(window_filter::is_desktop_window(
        &class.value,
        process_exe.as_deref(),
        desktop_exe,
    ))
}

/// 写 / 删用户 XDG 图标覆盖；真的变了才刷新主题目录 mtime 与图标缓存。
pub(super) fn sync_xdg_override(
    store: &IconStore,
    choice: &AppIconChoice,
) -> Result<(), PlatformError> {
    let png = match store.image_for(choice).map_err(platform_error)? {
        Some(image) => Some(
            pipeline::resized_png(&image, XDG_ICON_SIZE)
                .map_err(|error| failed("encode XDG icon", error))?,
        ),
        None => None,
    };
    let data_home = xdg::data_home_from_env().ok_or(PlatformError::Unsupported(
        "user data directory unavailable",
    ))?;
    let change = xdg::apply_override(&data_home, png.as_deref())?;
    if change == xdg::Change::Unchanged {
        return Ok(());
    }
    xdg::touch_theme_dir(&data_home)?;
    refresh_icon_cache(&xdg::theme_dir(&data_home));
    Ok(())
}

/// 用户主题目录里**已有** `icon-theme.cache` 时才重建：GTK 信任缓存，旧缓存里没有新图标会把它藏起来。
/// 没有缓存时不新建（新建的缓存会反过来藏住其他应用之后放进子目录的图标），靠主题目录 mtime 触发重扫。
/// 工具缺失或失败只记调试日志。
fn refresh_icon_cache(theme_dir: &Path) {
    if !theme_dir.join("icon-theme.cache").is_file() {
        return;
    }
    match std::process::Command::new("gtk-update-icon-cache")
        .args(["-f", "-t"])
        .arg(theme_dir)
        .output()
    {
        Ok(output) if output.status.success() => {}
        Ok(output) => tracing::debug!(
            status = %output.status,
            stderr = %String::from_utf8_lossy(&output.stderr).trim(),
            "gtk-update-icon-cache failed"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            tracing::debug!("gtk-update-icon-cache not installed");
        }
        Err(error) => tracing::debug!(%error, "could not run gtk-update-icon-cache"),
    }
}
