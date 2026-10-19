//! 快捷键主键（[`fluxdown_protocol::shortcuts::SHORTCUT_KEYS`] 的规范名）到 X11 keysym 与
//! XKB 键名的映射，以及修饰键 keysym 分类。X11 后端用 keysym 反查 keycode，Wayland portal
//! 后端用 XKB 键名拼 `preferred_trigger`（freedesktop「shortcuts」规范）。

use fluxdown_protocol::shortcuts::ModifierKey;

pub(super) const KEYSYM_NUM_LOCK: u32 = 0xff7f;

/// 规范主键名 → X11 keysym。ASCII 可打印字符的 keysym 等于其码点；字母取小写。
pub(super) fn keysym(key: &str) -> Option<u32> {
    if let Some(number) = function_key(key) {
        return Some(0xffbd + number);
    }
    let named = match key {
        "enter" => 0xff0d,
        "tab" => 0xff09,
        "escape" => 0xff1b,
        "backspace" => 0xff08,
        "delete" => 0xffff,
        "insert" => 0xff63,
        "home" => 0xff50,
        "left" => 0xff51,
        "up" => 0xff52,
        "right" => 0xff53,
        "down" => 0xff54,
        "pageup" => 0xff55,
        "pagedown" => 0xff56,
        "end" => 0xff57,
        "space" => 0x20,
        _ => 0,
    };
    if named != 0 {
        return Some(named);
    }
    let mut chars = key.chars();
    match (chars.next(), chars.next()) {
        (Some(ch), None) if ch.is_ascii_graphic() => Some(u32::from(ch.to_ascii_lowercase())),
        _ => None,
    }
}

/// 规范主键名 → XKB 键名（xkbcommon-keysyms.h 去掉 `XKB_KEY_` 前缀）。
pub(super) fn xkb_name(key: &str) -> Option<String> {
    if let Some(number) = function_key(key) {
        return Some(format!("F{number}"));
    }
    let named = match key {
        "enter" => "Return",
        "tab" => "Tab",
        "escape" => "Escape",
        "backspace" => "BackSpace",
        "delete" => "Delete",
        "insert" => "Insert",
        "home" => "Home",
        "left" => "Left",
        "up" => "Up",
        "right" => "Right",
        "down" => "Down",
        "pageup" => "Page_Up",
        "pagedown" => "Page_Down",
        "end" => "End",
        "space" => "space",
        "-" => "minus",
        "=" => "equal",
        "[" => "bracketleft",
        "]" => "bracketright",
        "\\" => "backslash",
        ";" => "semicolon",
        "'" => "apostrophe",
        "," => "comma",
        "." => "period",
        "/" => "slash",
        "`" => "grave",
        _ => "",
    };
    if !named.is_empty() {
        return Some(named.to_owned());
    }
    let mut chars = key.chars();
    match (chars.next(), chars.next()) {
        (Some(ch), None) if ch.is_ascii_alphanumeric() => Some(ch.to_ascii_lowercase().to_string()),
        _ => None,
    }
}

/// 修饰键 keysym（左右两侧；Alt 含 Meta）→ [`ModifierKey`]。
pub(super) fn modifier_of_keysym(keysym: u32) -> Option<ModifierKey> {
    match keysym {
        0xffe3 | 0xffe4 => Some(ModifierKey::Ctrl),
        // Meta_L / Meta_R / Alt_L / Alt_R
        0xffe7..=0xffea => Some(ModifierKey::Alt),
        0xffe1 | 0xffe2 => Some(ModifierKey::Shift),
        0xffeb | 0xffec => Some(ModifierKey::Super),
        _ => None,
    }
}

/// `f1`..`f20` → 1..=20。
fn function_key(key: &str) -> Option<u32> {
    let number: u32 = key.strip_prefix('f')?.parse().ok()?;
    (1..=20).contains(&number).then_some(number)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fluxdown_protocol::shortcuts::SHORTCUT_KEYS;

    #[test]
    fn every_catalog_key_has_keysym_and_xkb_name() {
        for key in SHORTCUT_KEYS {
            assert!(keysym(key).is_some(), "missing keysym for {key}");
            assert!(xkb_name(key).is_some(), "missing xkb name for {key}");
        }
    }

    #[test]
    fn spot_check_values() {
        assert_eq!(keysym("a"), Some(0x61));
        assert_eq!(keysym("0"), Some(0x30));
        assert_eq!(keysym("-"), Some(0x2d));
        assert_eq!(keysym("`"), Some(0x60));
        assert_eq!(keysym("space"), Some(0x20));
        assert_eq!(keysym("f1"), Some(0xffbe));
        assert_eq!(keysym("f12"), Some(0xffc9));
        assert_eq!(keysym("f20"), Some(0xffd1));
        assert_eq!(keysym("pagedown"), Some(0xff56));
        assert_eq!(keysym("f21"), None);
        assert_eq!(keysym("ab"), None);
        assert_eq!(xkb_name("f5").as_deref(), Some("F5"));
        assert_eq!(xkb_name("d").as_deref(), Some("d"));
        assert_eq!(xkb_name("\\").as_deref(), Some("backslash"));
        assert_eq!(xkb_name("enter").as_deref(), Some("Return"));
    }

    #[test]
    fn modifier_keysyms_map_both_sides() {
        assert_eq!(modifier_of_keysym(0xffe3), Some(ModifierKey::Ctrl));
        assert_eq!(modifier_of_keysym(0xffe4), Some(ModifierKey::Ctrl));
        assert_eq!(modifier_of_keysym(0xffe7), Some(ModifierKey::Alt));
        assert_eq!(modifier_of_keysym(0xffec), Some(ModifierKey::Super));
        assert_eq!(modifier_of_keysym(0x61), None);
    }
}
