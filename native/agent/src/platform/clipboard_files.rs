//! 把文件写入系统剪贴板（`agent.platform.copyTaskFiles`）：macOS 写 `NSURL` 文件对象、Windows 写
//! `CF_HDROP`、Linux 写 `text/uri-list`（X11 / Wayland data-control），均经 arboard 的安全接口。
//!
//! 剪贴板实例常驻进程：X11 上剪贴板内容由持有它的进程按需提供，实例一释放内容就只剩交给剪贴板
//! 管理器的那份（没有管理器就丢失）。agent 常驻，故只建一次并复用到进程退出；写入失败时丢弃
//! 实例，下次重建（显示服务器重启等）。

use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use super::PlatformError;

static CLIPBOARD: Mutex<Option<arboard::Clipboard>> = Mutex::new(None);

/// 用 `paths`（均已存在）整体替换系统剪贴板内容。
pub(super) fn write(paths: &[PathBuf]) -> Result<(), PlatformError> {
    // arboard 调用不 panic；万一中毒，槽位里至多是一个可重建的实例，直接沿用。
    let mut slot = CLIPBOARD.lock().unwrap_or_else(PoisonError::into_inner);
    let mut clipboard = match slot.take() {
        Some(clipboard) => clipboard,
        None => arboard::Clipboard::new().map_err(clipboard_error)?,
    };
    clipboard.set().file_list(paths).map_err(clipboard_error)?;
    *slot = Some(clipboard);
    Ok(())
}

fn clipboard_error(error: arboard::Error) -> PlatformError {
    if matches!(error, arboard::Error::ClipboardNotSupported) {
        return PlatformError::Unsupported("no system clipboard is available in this session");
    }
    PlatformError::Failed(format!("clipboard: {error}"))
}
