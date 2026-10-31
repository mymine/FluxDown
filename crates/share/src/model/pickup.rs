//! 取件码与分享链接的输入归一：6 位取件码的格式化 / 校验，以及「取件码还是链接」的判别。

use fluxdown_protocol::capture_link::is_share_link;

/// 取件码位数（云端策略 `pickupCodeLength` 缺省值）。
pub const DEFAULT_PICKUP_CODE_LEN: usize = 6;

/// 接收入口接受的输入。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReceiveInput {
    /// 归一后的纯数字取件码（无空格）。
    Code(String),
    /// 分享链接（`https://…/s/<slug>` 或 `fluxdown://share/<slug>`），已去首尾空白。
    Link(String),
}

impl ReceiveInput {
    /// 交给 `agent.share.resolve` / `receive` 的 `codeOrLink`。
    #[must_use]
    pub fn as_wire(&self) -> &str {
        match self {
            Self::Code(value) | Self::Link(value) => value,
        }
    }
}

/// 把用户输入的取件码归一为纯数字：忽略空格与连字符（`482 931`、`482-931`）；
/// 非数字或位数不符返回 `None`。
#[must_use]
pub fn normalize_pickup_code(input: &str, len: usize) -> Option<String> {
    let digits: String = input
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect();
    (digits.len() == len && digits.bytes().all(|b| b.is_ascii_digit())).then_some(digits)
}

/// 判别输入：先看是否为分享链接，再看是否为取件码；都不是返回 `None`。
#[must_use]
pub fn parse_receive_input(input: &str, code_len: usize) -> Option<ReceiveInput> {
    let trimmed = input.trim();
    if is_share_link(trimmed) {
        return Some(ReceiveInput::Link(trimmed.to_owned()));
    }
    normalize_pickup_code(trimmed, code_len).map(ReceiveInput::Code)
}

/// 取件码的展示形式：每 3 位一组（`482931` → `482 931`）。非纯数字原样返回。
#[must_use]
pub fn format_pickup_code(code: &str) -> String {
    if code.is_empty() || !code.bytes().all(|b| b.is_ascii_digit()) {
        return code.to_owned();
    }
    let mut out = String::with_capacity(code.len() + code.len() / 3);
    for (index, digit) in code.chars().enumerate() {
        if index > 0 && index % 3 == 0 {
            out.push(' ');
        }
        out.push(digit);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_PICKUP_CODE_LEN, ReceiveInput, format_pickup_code, normalize_pickup_code,
        parse_receive_input,
    };

    #[test]
    fn code_is_grouped_by_three() {
        assert_eq!(format_pickup_code("482931"), "482 931");
        assert_eq!(format_pickup_code("48293"), "482 93");
        assert_eq!(format_pickup_code(""), "");
        assert_eq!(format_pickup_code("ab12"), "ab12");
    }

    #[test]
    fn code_validation_ignores_separators_only() {
        assert_eq!(
            normalize_pickup_code("482 931", 6).as_deref(),
            Some("482931")
        );
        assert_eq!(
            normalize_pickup_code(" 482-931 ", 6).as_deref(),
            Some("482931")
        );
        assert_eq!(normalize_pickup_code("48293", 6), None);
        assert_eq!(normalize_pickup_code("4829311", 6), None);
        assert_eq!(normalize_pickup_code("48293a", 6), None);
        assert_eq!(normalize_pickup_code("４８２９３１", 6), None);
        assert_eq!(normalize_pickup_code("", 6), None);
    }

    #[test]
    fn input_is_classified_as_code_or_share_link() {
        assert_eq!(
            parse_receive_input(" 482 931 ", DEFAULT_PICKUP_CODE_LEN),
            Some(ReceiveInput::Code("482931".into()))
        );
        assert_eq!(
            parse_receive_input(" https://fluxdown.com/s/Ab-1_x ", DEFAULT_PICKUP_CODE_LEN),
            Some(ReceiveInput::Link("https://fluxdown.com/s/Ab-1_x".into()))
        );
        assert_eq!(
            parse_receive_input("fluxdown://share/abc", DEFAULT_PICKUP_CODE_LEN),
            Some(ReceiveInput::Link("fluxdown://share/abc".into()))
        );
        assert_eq!(
            parse_receive_input("https://example.com/a.zip", DEFAULT_PICKUP_CODE_LEN),
            None
        );
        assert_eq!(parse_receive_input("hello", DEFAULT_PICKUP_CODE_LEN), None);
    }

    #[test]
    fn wire_value_is_the_normalised_input() {
        let code = ReceiveInput::Code("482931".into());
        assert_eq!(code.as_wire(), "482931");
    }
}
