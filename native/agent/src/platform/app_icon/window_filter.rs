//! X11 窗口筛选谓词（纯函数）：只给 FluxDown 桌面进程自己的顶层窗口设图标。

use std::path::Path;

/// 桌面进程窗口的 `WM_CLASS`（GPUI `app_id`）。
pub(super) const WM_CLASS: &str = "com.fluxdown.app";

/// `WM_CLASS` 属性是两个 NUL 结尾的字符串（instance、class）；任一段包含 [`WM_CLASS`] 即匹配。
pub(super) fn class_matches(wm_class: &[u8]) -> bool {
    wm_class
        .split(|byte| *byte == 0)
        .any(|part| String::from_utf8_lossy(part).contains(WM_CLASS))
}

/// 类名匹配，且窗口所属进程的可执行文件（`/proc/<pid>/exe` 规范化后）就是桌面程序。
///
/// 两个路径都需由调用方规范化；进程信息缺失（无 `_NET_WM_PID`、`/proc` 不可读、
/// 可执行文件已被替换）按不匹配处理。
pub(super) fn is_desktop_window(
    wm_class: &[u8],
    process_exe: Option<&Path>,
    desktop_exe: &Path,
) -> bool {
    class_matches(wm_class) && process_exe == Some(desktop_exe)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESKTOP: &str = "/opt/fluxdown/fluxdown-desktop";

    #[test]
    fn class_must_contain_the_app_id_in_either_field() {
        assert!(class_matches(b"com.fluxdown.app\0com.fluxdown.app\0"));
        assert!(class_matches(b"fluxdown\0com.fluxdown.app\0"));
        assert!(!class_matches(b"firefox\0Navigator\0"));
        assert!(!class_matches(b""));
    }

    #[test]
    fn same_class_from_another_executable_is_rejected() {
        let class = b"com.fluxdown.app\0com.fluxdown.app\0";

        assert!(is_desktop_window(
            class,
            Some(Path::new(DESKTOP)),
            Path::new(DESKTOP)
        ));
        assert!(!is_desktop_window(
            class,
            Some(Path::new("/usr/bin/impostor")),
            Path::new(DESKTOP)
        ));
    }

    #[test]
    fn unknown_process_is_rejected_even_with_matching_class() {
        assert!(!is_desktop_window(
            b"com.fluxdown.app\0",
            None,
            Path::new(DESKTOP)
        ));
    }

    #[test]
    fn matching_executable_with_foreign_class_is_rejected() {
        assert!(!is_desktop_window(
            b"other\0Other\0",
            Some(Path::new(DESKTOP)),
            Path::new(DESKTOP)
        ));
    }
}
