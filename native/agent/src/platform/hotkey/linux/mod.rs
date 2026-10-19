//! Linux 全局快捷键后端：按会话类型选择 Wayland（XDG GlobalShortcuts portal）或 X11（XGrabKey +
//! XInput2）。两个后端都是纯安全代码（x11rb 的 `RustConnection` 与 ashpd）。

mod keys;
mod portal;
mod x11;

use std::sync::Arc;

use crate::hotkey::{HotkeyPort, HotkeySink};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Session {
    Wayland,
    X11,
}

/// `WAYLAND_DISPLAY` 或 `XDG_SESSION_TYPE=wayland` → Wayland（XWayland 下也走 portal，
/// X11 抓取在 Wayland 合成器里收不到原生窗口的按键）；否则有 `DISPLAY` → X11。
fn detect_session(
    wayland_display: Option<&str>,
    session_type: Option<&str>,
    display: Option<&str>,
) -> Option<Session> {
    let set = |value: Option<&str>| value.is_some_and(|value| !value.is_empty());
    if set(wayland_display) || session_type.is_some_and(|kind| kind.eq_ignore_ascii_case("wayland"))
    {
        Some(Session::Wayland)
    } else if set(display) {
        Some(Session::X11)
    } else {
        None
    }
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

pub(crate) fn start(sink: Arc<dyn HotkeySink>) -> Result<Box<dyn HotkeyPort>, String> {
    let session = detect_session(
        env("WAYLAND_DISPLAY").as_deref(),
        env("XDG_SESSION_TYPE").as_deref(),
        env("DISPLAY").as_deref(),
    );
    match session {
        Some(Session::Wayland) => portal::start(sink),
        Some(Session::X11) => x11::start(sink),
        None => Err("no graphical session (WAYLAND_DISPLAY and DISPLAY are unset)".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wayland_wins_even_with_xwayland_display() {
        assert_eq!(
            detect_session(Some("wayland-0"), Some("wayland"), Some(":0")),
            Some(Session::Wayland)
        );
        assert_eq!(
            detect_session(None, Some("Wayland"), Some(":0")),
            Some(Session::Wayland)
        );
    }

    #[test]
    fn x11_needs_display_and_empty_values_do_not_count() {
        assert_eq!(
            detect_session(None, Some("x11"), Some(":0")),
            Some(Session::X11)
        );
        assert_eq!(
            detect_session(Some(""), None, Some(":1")),
            Some(Session::X11)
        );
        assert_eq!(detect_session(None, None, Some("")), None);
        assert_eq!(detect_session(None, Some("tty"), None), None);
    }
}
