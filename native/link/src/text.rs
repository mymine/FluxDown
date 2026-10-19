//! 对端自报文本的清洗。
//!
//! 设备名 / 平台 / 版本 / 云设备 ID 都来自网络，既会进审批弹窗、名册，也会进日志。
//! 一律不信任：剔除控制字符与双向控制字符（用来在 UI 里伪造别的设备名），再按字符数截断。

/// 设备展示名的字符数上限。
pub const MAX_NAME_CHARS: usize = 64;
/// 平台标识的字符数上限。
pub const MAX_PLATFORM_CHARS: usize = 32;
/// 客户端版本的字符数上限。
pub const MAX_VERSION_CHARS: usize = 64;
/// 云设备 ID（UUID）的字符数上限。
pub const MAX_CLOUD_DEVICE_ID_CHARS: usize = 64;

/// Unicode 双向控制字符：能让后面的文字在界面上倒序显示，伪造设备名。
fn is_bidi_control(c: char) -> bool {
    matches!(
        c,
        '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    )
}

/// 控制字符或双向控制字符。
#[must_use]
pub fn is_unsafe_display_char(c: char) -> bool {
    c.is_control() || is_bidi_control(c)
}

/// 清洗一段对端自报文本：去掉控制 / 双向控制字符，去首尾空白，按字符数截断（截断后再去一次
/// 尾部空白）。结果可能为空串。
#[must_use]
pub fn sanitize_text(raw: &str, max_chars: usize) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|c| !is_unsafe_display_char(*c))
        .collect();
    let truncated: String = cleaned.trim().chars().take(max_chars).collect();
    truncated.trim().to_owned()
}

/// [`sanitize_text`] 的可选形态：清洗后为空视为没有。
#[must_use]
pub fn sanitize_optional(raw: Option<&str>, max_chars: usize) -> Option<String> {
    raw.map(|text| sanitize_text(text, max_chars))
        .filter(|text| !text.is_empty())
}

/// 云设备 ID 只接受 `[A-Za-z0-9_-]{1,64}`（FluxCloud 的设备 ID 是 UUID）。
#[must_use]
pub fn is_valid_cloud_device_id(id: &str) -> bool {
    !id.is_empty()
        && id.chars().count() <= MAX_CLOUD_DEVICE_ID_CHARS
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn strips_control_and_bidi_characters() {
        assert_eq!(sanitize_text("Lap\u{0}top\n", 64), "Laptop");
        // 右到左覆盖符 + 隔离符不能留在展示名里。
        assert_eq!(sanitize_text("a\u{202E}b\u{2066}c\u{200F}", 64), "abc");
    }

    #[test]
    fn truncates_by_characters_not_bytes() {
        let name: String = "设".repeat(100);
        assert_eq!(sanitize_text(&name, MAX_NAME_CHARS).chars().count(), 64);
        assert_eq!(sanitize_text("ab  ", 64), "ab");
        assert_eq!(sanitize_text("ab cd", 3), "ab");
    }

    #[test]
    fn blank_optional_text_is_none() {
        assert_eq!(sanitize_optional(Some(" \u{7} "), 32), None);
        assert_eq!(sanitize_optional(None, 32), None);
        assert_eq!(
            sanitize_optional(Some("linux"), 32).as_deref(),
            Some("linux")
        );
    }

    #[test]
    fn cloud_device_id_shape() {
        assert!(is_valid_cloud_device_id(
            "3f2b8c1e-5d4a-4e7b-9a10-0c1d2e3f4a5b"
        ));
        assert!(!is_valid_cloud_device_id(""));
        assert!(!is_valid_cloud_device_id("a b"));
        assert!(!is_valid_cloud_device_id(&"a".repeat(65)));
    }
}
