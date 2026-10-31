//! 「FluxMesh」设置页的纯逻辑：文本输入的解析 / 回显、选项列表、整体提交前的差异判定。
//!
//! `agent.share.setLocalConfig` 整体替换 [`ShareLocalConfigDto`]：视图以当前值为底稿改一个字段后整体提交，
//! 这里负责「改什么」的纯函数（端口 / 额外地址 / 限速的解析，档位 / 次数 / 中继的候选，是否 no-op），不依赖 GPUI。

use std::{collections::HashSet, net::SocketAddr};

use fluxdown_protocol::{ShareLocalConfigDto, ShareRelayDto, ShareTierDto};

use super::format::{CLAIM_PRESETS, CountLabel, tier_label};

// ───────────────────────── 端口 ─────────────────────────

/// 端口文本无法解析。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidPort;

/// 解析 UDP 端口输入：空白 / `0` = 随机（`None`）；只接受 1..=65535 的十进制 ASCII 数字。
pub fn parse_port(text: &str) -> Result<Option<u16>, InvalidPort> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    if !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(InvalidPort);
    }
    match text.parse::<u16>() {
        Ok(0) => Ok(None),
        Ok(port) => Ok(Some(port)),
        Err(_) => Err(InvalidPort),
    }
}

/// 端口回显：`None` / `0` 显示为空（占位符「随机」）。
#[must_use]
pub fn port_text(port: Option<u16>) -> String {
    match port {
        Some(port) if port != 0 => port.to_string(),
        _ => String::new(),
    }
}

// ───────────────────────── 额外公布地址 ─────────────────────────

/// 额外地址输入里第一个不合法的条目（原样回显给用户）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidAddr {
    pub entry: String,
}

/// 条目回显最长字符数（错误提示里的条目可能是任意粘贴的长文本）。
const ENTRY_PREVIEW_CHARS: usize = 48;

/// 解析额外公布地址：逗号 / 分号 / 换行 / 空白分隔，每项须为 `ip:port`（IPv6 写作 `[::1]:7000`，端口 1..=65535），
/// 输出规范化文本并按首次出现去重。
pub fn parse_extra_addrs(text: &str) -> Result<Vec<String>, InvalidAddr> {
    let mut seen = HashSet::new();
    let mut addrs = Vec::new();
    for entry in text
        .split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '，' | '；'))
        .filter(|entry| !entry.is_empty())
    {
        let invalid = || InvalidAddr {
            entry: truncate_middle(entry, ENTRY_PREVIEW_CHARS),
        };
        let addr: SocketAddr = entry.parse().map_err(|_| invalid())?;
        if addr.port() == 0 {
            return Err(invalid());
        }
        if seen.insert(addr) {
            addrs.push(addr.to_string());
        }
    }
    Ok(addrs)
}

/// 额外地址回显：每行一个。
#[must_use]
pub fn extra_addrs_text(addrs: &[String]) -> String {
    addrs.join("\n")
}

// ───────────────────────── 发送限速 ─────────────────────────

/// 限速输入的单位（1024 进制，与 `format_bytes` 一致）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RateUnit {
    KbPerSec,
    MbPerSec,
}

impl RateUnit {
    pub const ALL: [Self; 2] = [Self::KbPerSec, Self::MbPerSec];

    /// 每单位的字节数。
    #[must_use]
    pub const fn bytes(self) -> u64 {
        match self {
            Self::KbPerSec => 1 << 10,
            Self::MbPerSec => 1 << 20,
        }
    }

    /// 下拉选项 / 控件 ID 用的稳定值。
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::KbPerSec => "kb",
            Self::MbPerSec => "mb",
        }
    }

    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|unit| unit.id() == id)
    }

    #[must_use]
    pub const fn label_key(self) -> &'static str {
        match self {
            Self::KbPerSec => "shareSetUnitKb",
            Self::MbPerSec => "shareSetUnitMb",
        }
    }
}

/// 限速上限：1 TiB/s，远超任何链路；更大的输入视为打错。
pub const MAX_RATE_BYTES_PER_SEC: u64 = 1 << 40;

/// 限速文本无法解析或超出范围。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidRate;

/// 解析限速输入：空白 / `0` = 不限（`None`）；接受非负十进制数（最多一个小数点），
/// 换算成字节 / 秒（四舍五入），上限 [`MAX_RATE_BYTES_PER_SEC`]。
pub fn parse_rate_limit(text: &str, unit: RateUnit) -> Result<Option<u64>, InvalidRate> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let (int_part, frac_part) = match text.split_once('.') {
        Some((int_part, frac_part)) => (int_part, frac_part),
        None => (text, ""),
    };
    let digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
    if !digits(int_part) || !digits(frac_part) || (int_part.is_empty() && frac_part.is_empty()) {
        return Err(InvalidRate);
    }
    let int_value: u64 = if int_part.is_empty() {
        0
    } else {
        int_part.parse().map_err(|_| InvalidRate)?
    };
    let unit_bytes = unit.bytes();
    let mut bytes = int_value.checked_mul(unit_bytes).ok_or(InvalidRate)?;
    // 小数部分最多取 6 位（对 MB 已精确到字节），其余忽略。
    let frac_digits = frac_part.get(..6).unwrap_or(frac_part);
    if !frac_digits.is_empty() {
        let frac_value: u64 = frac_digits.parse().map_err(|_| InvalidRate)?;
        let scale = 10u64.pow(u32::try_from(frac_digits.len()).map_err(|_| InvalidRate)?);
        // 四舍五入：unit_bytes ≤ 2^20、frac_value < 10^6，乘积远小于 u64 上限。
        let frac_bytes = (frac_value * unit_bytes + scale / 2) / scale;
        bytes = bytes.checked_add(frac_bytes).ok_or(InvalidRate)?;
    }
    if bytes > MAX_RATE_BYTES_PER_SEC {
        return Err(InvalidRate);
    }
    Ok((bytes != 0).then_some(bytes))
}

/// 回显用的单位：不限速沿用 `current`；至少 1 MB/s 且是整 MB 时用 MB/s，其余 KB/s。
#[must_use]
pub fn preferred_rate_unit(bytes: Option<u64>, current: RateUnit) -> RateUnit {
    match bytes {
        None | Some(0) => current,
        Some(bytes) if bytes.is_multiple_of(RateUnit::MbPerSec.bytes()) => RateUnit::MbPerSec,
        Some(_) => RateUnit::KbPerSec,
    }
}

/// 限速回显：`None` / `0` 为空（占位符「不限速」）；否则最多两位小数并去掉尾随 0。
#[must_use]
pub fn rate_text(bytes: Option<u64>, unit: RateUnit) -> String {
    let Some(bytes) = bytes.filter(|bytes| *bytes != 0) else {
        return String::new();
    };
    let unit_bytes = unit.bytes();
    let hundredths =
        (u128::from(bytes) * 100 + u128::from(unit_bytes) / 2) / u128::from(unit_bytes);
    let whole = hundredths / 100;
    let frac = hundredths % 100;
    if frac == 0 {
        whole.to_string()
    } else if frac.is_multiple_of(10) {
        format!("{whole}.{}", frac / 10)
    } else {
        format!("{whole}.{frac:02}")
    }
}

// ───────────────────────── 每日共享上传上限 ─────────────────────────

/// 每日上限的单位：GB（2^30 字节）。
const DAILY_CAP_UNIT_BYTES: u128 = 1 << 30;
/// 每日上限的上限：1 PiB，与云端 `provideDailyCapDefaultBytes` 校验一致。
pub const MAX_DAILY_CAP_BYTES: u64 = 1 << 50;

/// 每日上限文本无法解析或超出范围。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidDailyCap;

/// 解析每日共享上传上限（单位 GB，可带小数）：空串 = 跟随云端缺省（`None`）；`0` = 不限（`Some(0)`）；
/// 其余换算成字节（四舍五入），上限 [`MAX_DAILY_CAP_BYTES`]。
pub fn parse_daily_cap(text: &str) -> Result<Option<u64>, InvalidDailyCap> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let (int_part, frac_part) = text.split_once('.').unwrap_or((text, ""));
    let digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
    if !digits(int_part) || !digits(frac_part) || (int_part.is_empty() && frac_part.is_empty()) {
        return Err(InvalidDailyCap);
    }
    let int_value: u64 = if int_part.is_empty() {
        0
    } else {
        int_part.parse().map_err(|_| InvalidDailyCap)?
    };
    let mut bytes = u128::from(int_value) * DAILY_CAP_UNIT_BYTES;
    // 小数部分最多取 6 位，其余忽略。
    let frac_digits = frac_part.get(..6).unwrap_or(frac_part);
    if !frac_digits.is_empty() {
        let frac_value: u64 = frac_digits.parse().map_err(|_| InvalidDailyCap)?;
        let scale = 10u128.pow(u32::try_from(frac_digits.len()).map_err(|_| InvalidDailyCap)?);
        bytes += (u128::from(frac_value) * DAILY_CAP_UNIT_BYTES + scale / 2) / scale;
    }
    match u64::try_from(bytes) {
        Ok(bytes) if bytes <= MAX_DAILY_CAP_BYTES => Ok(Some(bytes)),
        _ => Err(InvalidDailyCap),
    }
}

/// 每日上限回显：`None` 为空（占位符显示云端缺省）；否则 GB，最多两位小数并去掉尾随 0。
#[must_use]
pub fn daily_cap_text(bytes: Option<u64>) -> String {
    let Some(bytes) = bytes else {
        return String::new();
    };
    let hundredths = (u128::from(bytes) * 100 + DAILY_CAP_UNIT_BYTES / 2) / DAILY_CAP_UNIT_BYTES;
    let whole = hundredths / 100;
    let frac = hundredths % 100;
    if frac == 0 {
        whole.to_string()
    } else if frac.is_multiple_of(10) {
        format!("{whole}.{}", frac / 10)
    } else {
        format!("{whole}.{frac:02}")
    }
}

// ───────────────────────── 选项列表 ─────────────────────────

/// 默认有效期下拉的一项。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TierChoice {
    /// 跟随云端策略的第一个档位（`default_tier_id = None`）。
    FollowFirst,
    Tier {
        id: String,
        label: CountLabel,
    },
    /// 已保存的档位不在当前策略里（策略变更 / 策略未加载）：照实显示 ID，避免下拉谎报。
    Missing {
        id: String,
    },
}

impl TierChoice {
    /// 下拉值：`FollowFirst` 为空串，其余为档位 ID。
    #[must_use]
    pub fn value(&self) -> &str {
        match self {
            Self::FollowFirst => "",
            Self::Tier { id, .. } | Self::Missing { id } => id,
        }
    }
}

/// 有效期候选：「跟随第一个档位」+ 策略的全部实时档位；已保存的档位缺失时追加 `Missing`。
#[must_use]
pub fn tier_choices(tiers: &[ShareTierDto], current: Option<&str>) -> Vec<TierChoice> {
    let mut choices = vec![TierChoice::FollowFirst];
    choices.extend(tiers.iter().map(|tier| TierChoice::Tier {
        id: tier.id.clone(),
        label: tier_label(tier.hours),
    }));
    if let Some(current) = current.filter(|id| !id.is_empty())
        && !tiers.iter().any(|tier| tier.id == current)
    {
        choices.push(TierChoice::Missing {
            id: current.to_owned(),
        });
    }
    choices
}

/// 下拉选中值 → `default_tier_id`（空串 = 跟随第一个档位）。
#[must_use]
pub fn tier_id_from_value(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_owned())
}

/// 默认领取次数候选：预设档位；已保存值不在预设里（别的客户端写入）时按序插入，保证下拉如实显示。
#[must_use]
pub fn claims_choices(current: u32) -> Vec<u32> {
    let mut choices = CLAIM_PRESETS.to_vec();
    if !choices.contains(&current) {
        choices.push(current);
        choices.sort_unstable();
    }
    choices
}

/// 中继下拉的一项。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RelayChoice {
    /// 自动（按延迟选择），`pinned_relay = None`。
    Auto,
    Relay {
        url: String,
        region: String,
    },
    /// 已固定的中继不在当前策略里：照实显示 URL。
    Missing {
        url: String,
    },
}

impl RelayChoice {
    /// 下拉值：`Auto` 为空串，其余为 URL。
    #[must_use]
    pub fn value(&self) -> &str {
        match self {
            Self::Auto => "",
            Self::Relay { url, .. } | Self::Missing { url } => url,
        }
    }
}

/// 中继候选：「自动」+ 策略全部中继；已固定的中继缺失时追加 `Missing`。
#[must_use]
pub fn relay_choices(relays: &[ShareRelayDto], pinned: Option<&str>) -> Vec<RelayChoice> {
    let mut choices = vec![RelayChoice::Auto];
    choices.extend(relays.iter().map(|relay| RelayChoice::Relay {
        url: relay.url.clone(),
        region: relay.region.clone(),
    }));
    if let Some(pinned) = pinned.filter(|url| !url.is_empty())
        && !relays.iter().any(|relay| relay.url == pinned)
    {
        choices.push(RelayChoice::Missing {
            url: pinned.to_owned(),
        });
    }
    choices
}

/// 下拉选中值 → `pinned_relay`（空串 = 自动）。
#[must_use]
pub fn pinned_relay_from_value(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_owned())
}

/// 中继条目的单行展示：`区域 · URL`，区域为空只显示 URL；整体按字符数中间省略。
#[must_use]
pub fn relay_label(url: &str, region: &str, max_chars: usize) -> String {
    let region = region.trim();
    if region.is_empty() {
        truncate_middle(url, max_chars)
    } else {
        // 区域名留完整（通常很短），把省略预算给 URL。
        let budget = max_chars.saturating_sub(region.chars().count() + 3).max(8);
        format!("{region} · {}", truncate_middle(url, budget))
    }
}

/// 超过 `max_chars` 个字符时保留首尾、中间换成 `…`；按字符边界切，不会截断多字节字符。
#[must_use]
pub fn truncate_middle(text: &str, max_chars: usize) -> String {
    let total = text.chars().count();
    if total <= max_chars || max_chars < 2 {
        return text.to_owned();
    }
    let keep = max_chars - 1;
    let head = keep.div_ceil(2);
    let tail = keep - head;
    let head_text: String = text.chars().take(head).collect();
    let tail_text: String = text.chars().skip(total - tail).collect();
    format!("{head_text}…{tail_text}")
}

// ───────────────────────── 差异 ─────────────────────────

/// 规范化口径与 agent 一致的比较底稿：端口 `0` → `None`，限速 `0` → `None`，空 URL / 空档位 ID → `None`。
#[must_use]
pub fn normalized(config: &ShareLocalConfigDto) -> ShareLocalConfigDto {
    let mut config = config.clone();
    config.udp_port_v4 = config.udp_port_v4.filter(|port| *port != 0);
    config.udp_port_v6 = config.udp_port_v6.filter(|port| *port != 0);
    config.send_rate_limit_bytes_per_sec = config
        .send_rate_limit_bytes_per_sec
        .filter(|bytes| *bytes != 0);
    config.pinned_relay = config.pinned_relay.filter(|url| !url.is_empty());
    config.default_tier_id = config.default_tier_id.filter(|id| !id.is_empty());
    config.default_swarm = config.default_swarm.filter(|v| !*v);
    config.auto_provide = config.auto_provide.filter(|v| !*v);
    // 时区偏移由提交时统一写入，不参与 no-op 判定。
    config.provide_day_offset_secs = None;
    config
}

/// 提交前的 no-op 判定：规范化后相同则不必调用 agent。
#[must_use]
pub fn is_noop(current: &ShareLocalConfigDto, next: &ShareLocalConfigDto) -> bool {
    normalized(current) == normalized(next)
}

/// 提交前的收尾（设置页与分享页摘要的开关共用）：与底稿相比是 no-op 返回 `None`；否则带上本地
/// UTC 偏移（每日上限按本地时区自然日划分，每次提交都要带，所以不参与 no-op 判定）。
#[must_use]
pub fn prepare_commit(
    shown: &ShareLocalConfigDto,
    mut next: ShareLocalConfigDto,
    local_offset_secs: i32,
) -> Option<ShareLocalConfigDto> {
    if is_noop(shown, &next) {
        return None;
    }
    next.provide_day_offset_secs = Some(local_offset_secs);
    Some(next)
}

/// 文本输入解析后的结果：能解析的字段给出新值，不能解析的保持底稿并记入错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextEdits {
    pub config: ShareLocalConfigDto,
    pub udp_v4_invalid: bool,
    pub udp_v6_invalid: bool,
    pub extra_addrs_invalid: Option<InvalidAddr>,
    pub rate_invalid: bool,
    pub daily_cap_invalid: bool,
}

impl TextEdits {
    #[must_use]
    pub fn has_error(&self) -> bool {
        self.udp_v4_invalid
            || self.udp_v6_invalid
            || self.extra_addrs_invalid.is_some()
            || self.rate_invalid
            || self.daily_cap_invalid
    }
}

/// 把四个文本输入当前内容合并进底稿：合法的字段生效，非法的字段保持底稿并标记错误。
#[must_use]
pub fn apply_text_inputs(
    base: &ShareLocalConfigDto,
    udp_v4: &str,
    udp_v6: &str,
    extra_addrs: &str,
    rate: &str,
    unit: RateUnit,
    daily_cap: &str,
) -> TextEdits {
    let mut config = base.clone();
    let mut edits = TextEdits {
        config: base.clone(),
        udp_v4_invalid: false,
        udp_v6_invalid: false,
        extra_addrs_invalid: None,
        rate_invalid: false,
        daily_cap_invalid: false,
    };
    match parse_port(udp_v4) {
        Ok(port) => config.udp_port_v4 = port,
        Err(InvalidPort) => edits.udp_v4_invalid = true,
    }
    match parse_port(udp_v6) {
        Ok(port) => config.udp_port_v6 = port,
        Err(InvalidPort) => edits.udp_v6_invalid = true,
    }
    match parse_extra_addrs(extra_addrs) {
        Ok(addrs) => config.extra_addrs = addrs,
        Err(error) => edits.extra_addrs_invalid = Some(error),
    }
    match parse_rate_limit(rate, unit) {
        Ok(bytes) => config.send_rate_limit_bytes_per_sec = bytes,
        Err(InvalidRate) => edits.rate_invalid = true,
    }
    match parse_daily_cap(daily_cap) {
        Ok(bytes) => config.provide_daily_cap_bytes = bytes,
        Err(InvalidDailyCap) => edits.daily_cap_invalid = true,
    }
    edits.config = config;
    edits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_parsing_handles_blank_zero_and_bounds() {
        assert_eq!(parse_port(""), Ok(None));
        assert_eq!(parse_port("   "), Ok(None));
        assert_eq!(parse_port("0"), Ok(None));
        assert_eq!(parse_port("000"), Ok(None));
        assert_eq!(parse_port(" 7000 "), Ok(Some(7000)));
        assert_eq!(parse_port("1"), Ok(Some(1)));
        assert_eq!(parse_port("65535"), Ok(Some(65535)));
        assert_eq!(parse_port("65536"), Err(InvalidPort));
        assert_eq!(parse_port("99999999999999999999"), Err(InvalidPort));
        assert_eq!(parse_port("-1"), Err(InvalidPort));
        assert_eq!(parse_port("+80"), Err(InvalidPort));
        assert_eq!(parse_port("80a"), Err(InvalidPort));
        assert_eq!(parse_port("８０"), Err(InvalidPort));
        assert_eq!(parse_port("7 000"), Err(InvalidPort));
    }

    #[test]
    fn port_text_round_trips_and_hides_random() {
        assert_eq!(port_text(None), "");
        assert_eq!(port_text(Some(0)), "");
        assert_eq!(port_text(Some(7000)), "7000");
        assert_eq!(parse_port(&port_text(Some(65535))), Ok(Some(65535)));
    }

    #[test]
    fn extra_addrs_accept_mixed_separators_and_ipv6() {
        let parsed =
            parse_extra_addrs(" 203.0.113.5:7000,\n[::1]:7001 ; [2001:db8::1]:443\t10.0.0.1:1 ");
        assert_eq!(
            parsed,
            Ok(vec![
                "203.0.113.5:7000".to_owned(),
                "[::1]:7001".to_owned(),
                "[2001:db8::1]:443".to_owned(),
                "10.0.0.1:1".to_owned(),
            ])
        );
    }

    #[test]
    fn extra_addrs_blank_is_empty_and_duplicates_collapse() {
        assert_eq!(parse_extra_addrs(""), Ok(Vec::new()));
        assert_eq!(parse_extra_addrs(" ,\n; "), Ok(Vec::new()));
        assert_eq!(
            parse_extra_addrs("1.2.3.4:5\n1.2.3.4:5, [::1]:7000, [0:0:0:0:0:0:0:1]:7000"),
            Ok(vec!["1.2.3.4:5".to_owned(), "[::1]:7000".to_owned()])
        );
    }

    #[test]
    fn extra_addrs_report_the_first_offending_entry() {
        let bad = |text: &str| parse_extra_addrs(text).map_err(|error| error.entry);
        assert_eq!(
            bad("1.2.3.4:5, example.com:80"),
            Err("example.com:80".to_owned())
        );
        assert_eq!(bad("1.2.3.4"), Err("1.2.3.4".to_owned()));
        assert_eq!(bad("::1:7000"), Err("::1:7000".to_owned()));
        assert_eq!(bad("1.2.3.4:0"), Err("1.2.3.4:0".to_owned()));
        assert_eq!(bad("1.2.3.4:65536"), Err("1.2.3.4:65536".to_owned()));
        assert_eq!(bad("1.2.3.4:80, ok"), Err("ok".to_owned()));
        // 全角逗号也是分隔符。
        assert_eq!(
            bad("1.2.3.4:80，5.6.7.8:90"),
            Ok(vec!["1.2.3.4:80".to_owned(), "5.6.7.8:90".to_owned()])
        );
    }

    #[test]
    fn extra_addr_error_entry_is_bounded_and_char_safe() {
        let long = "地".repeat(500);
        let error = parse_extra_addrs(&long).err().unwrap_or(InvalidAddr {
            entry: String::new(),
        });
        assert!(error.entry.chars().count() <= ENTRY_PREVIEW_CHARS);
        assert!(error.entry.contains('…'));
    }

    #[test]
    fn extra_addrs_text_is_one_per_line() {
        let addrs = vec!["1.2.3.4:5".to_owned(), "[::1]:7".to_owned()];
        assert_eq!(parse_extra_addrs(&extra_addrs_text(&addrs)), Ok(addrs));
        assert_eq!(extra_addrs_text(&[]), "");
    }

    #[test]
    fn rate_parsing_converts_units() {
        assert_eq!(parse_rate_limit("", RateUnit::MbPerSec), Ok(None));
        assert_eq!(parse_rate_limit("0", RateUnit::KbPerSec), Ok(None));
        assert_eq!(parse_rate_limit("0.0", RateUnit::KbPerSec), Ok(None));
        assert_eq!(
            parse_rate_limit("512", RateUnit::KbPerSec),
            Ok(Some(512 * 1024))
        );
        assert_eq!(
            parse_rate_limit(" 2 ", RateUnit::MbPerSec),
            Ok(Some(2 << 20))
        );
        assert_eq!(
            parse_rate_limit("1.5", RateUnit::MbPerSec),
            Ok(Some(3 << 19))
        );
        assert_eq!(
            parse_rate_limit(".5", RateUnit::MbPerSec),
            Ok(Some(1 << 19))
        );
        assert_eq!(parse_rate_limit("1.", RateUnit::KbPerSec), Ok(Some(1024)));
        assert_eq!(parse_rate_limit("0.5", RateUnit::KbPerSec), Ok(Some(512)));
        // 小数位超出 6 位的部分忽略。
        assert_eq!(
            parse_rate_limit("1.00000000001", RateUnit::KbPerSec),
            Ok(Some(1024))
        );
    }

    #[test]
    fn rate_parsing_rejects_garbage_and_huge_values() {
        for text in [
            ".", "-1", "+1", "1e3", "1,5", "abc", "1.2.3", "1 2", "NaN", "inf", "１",
        ] {
            assert_eq!(
                parse_rate_limit(text, RateUnit::MbPerSec),
                Err(InvalidRate),
                "{text}"
            );
        }
        // 1 TiB/s 恰好是上限。
        assert_eq!(
            parse_rate_limit("1048576", RateUnit::MbPerSec),
            Ok(Some(MAX_RATE_BYTES_PER_SEC))
        );
        assert_eq!(
            parse_rate_limit("1048577", RateUnit::MbPerSec),
            Err(InvalidRate)
        );
        assert_eq!(
            parse_rate_limit("18446744073709551615", RateUnit::MbPerSec),
            Err(InvalidRate)
        );
        assert_eq!(
            parse_rate_limit("99999999999999999999999", RateUnit::KbPerSec),
            Err(InvalidRate)
        );
    }

    #[test]
    fn rate_text_round_trips_through_the_parser() {
        for (bytes, unit) in [
            (512 * 1024, RateUnit::KbPerSec),
            (3 << 19, RateUnit::MbPerSec),
            (5 << 20, RateUnit::MbPerSec),
        ] {
            let text = rate_text(Some(bytes), unit);
            let parsed = parse_rate_limit(&text, unit).ok().flatten();
            assert!(parsed.is_some(), "{text}");
        }
        assert_eq!(rate_text(Some(3 << 19), RateUnit::MbPerSec), "1.5");
        assert_eq!(rate_text(Some(5 << 20), RateUnit::MbPerSec), "5");
        assert_eq!(rate_text(Some(1536), RateUnit::KbPerSec), "1.5");
        assert_eq!(rate_text(Some(1034), RateUnit::KbPerSec), "1.01");
        assert_eq!(rate_text(None, RateUnit::MbPerSec), "");
        assert_eq!(rate_text(Some(0), RateUnit::MbPerSec), "");
        // 极大值不溢出。
        assert!(!rate_text(Some(u64::MAX), RateUnit::KbPerSec).is_empty());
    }

    #[test]
    fn preferred_unit_prefers_exact_mb() {
        assert_eq!(
            preferred_rate_unit(None, RateUnit::KbPerSec),
            RateUnit::KbPerSec
        );
        assert_eq!(
            preferred_rate_unit(Some(0), RateUnit::MbPerSec),
            RateUnit::MbPerSec
        );
        assert_eq!(
            preferred_rate_unit(Some(4 << 20), RateUnit::KbPerSec),
            RateUnit::MbPerSec
        );
        assert_eq!(
            preferred_rate_unit(Some(3 << 19), RateUnit::MbPerSec),
            RateUnit::KbPerSec
        );
        assert_eq!(
            preferred_rate_unit(Some(100), RateUnit::MbPerSec),
            RateUnit::KbPerSec
        );
        assert_eq!(RateUnit::from_id("mb"), Some(RateUnit::MbPerSec));
        assert_eq!(RateUnit::from_id("gb"), None);
    }

    fn tier(id: &str, hours: u32) -> ShareTierDto {
        ShareTierDto {
            id: id.to_owned(),
            hours,
        }
    }

    #[test]
    fn tier_choices_lead_with_follow_first_and_keep_unknown_saved_tier() {
        let tiers = [tier("a", 24), tier("b", 168)];
        let choices = tier_choices(&tiers, None);
        assert_eq!(choices.len(), 3);
        assert_eq!(choices[0], TierChoice::FollowFirst);
        assert_eq!(choices[0].value(), "");
        assert_eq!(choices[2].value(), "b");
        assert_eq!(tier_choices(&tiers, Some("b")).len(), 3);
        assert_eq!(tier_choices(&tiers, Some("")).len(), 3);
        let with_missing = tier_choices(&tiers, Some("gone"));
        assert_eq!(
            with_missing.last(),
            Some(&TierChoice::Missing {
                id: "gone".to_owned()
            })
        );
        // 策略尚未加载：只有「跟随」与已保存值。
        assert_eq!(tier_choices(&[], Some("x")).len(), 2);
        assert_eq!(tier_choices(&[], None), vec![TierChoice::FollowFirst]);
        assert_eq!(tier_id_from_value(""), None);
        assert_eq!(tier_id_from_value("a"), Some("a".to_owned()));
    }

    #[test]
    fn claims_choices_insert_unknown_saved_value_in_order() {
        assert_eq!(claims_choices(5), CLAIM_PRESETS.to_vec());
        assert_eq!(claims_choices(0)[0], 0);
        let custom = claims_choices(7);
        assert_eq!(custom, vec![0, 1, 3, 5, 7, 10, 20, 50]);
        assert_eq!(claims_choices(u32::MAX).last(), Some(&u32::MAX));
    }

    #[test]
    fn relay_choices_cover_auto_policy_and_missing_pin() {
        let relays = [
            ShareRelayDto {
                url: "https://a.example".to_owned(),
                region: "cn-east".to_owned(),
            },
            ShareRelayDto {
                url: "https://b.example".to_owned(),
                region: String::new(),
            },
        ];
        let choices = relay_choices(&relays, None);
        assert_eq!(choices[0], RelayChoice::Auto);
        assert_eq!(choices.len(), 3);
        assert_eq!(relay_choices(&relays, Some("https://b.example")).len(), 3);
        let missing = relay_choices(&relays, Some("https://old.example"));
        assert_eq!(
            missing.last(),
            Some(&RelayChoice::Missing {
                url: "https://old.example".to_owned()
            })
        );
        assert_eq!(pinned_relay_from_value(""), None);
        assert_eq!(
            pinned_relay_from_value("https://a.example"),
            Some("https://a.example".to_owned())
        );
        assert_eq!(relay_choices(&[], None), vec![RelayChoice::Auto]);
    }

    #[test]
    fn relay_label_is_single_line_and_bounded() {
        assert_eq!(
            relay_label("https://a.example", "", 40),
            "https://a.example"
        );
        assert_eq!(
            relay_label("https://a.example", "华东", 40),
            "华东 · https://a.example"
        );
        let long = format!("https://{}.example/path", "子域".repeat(100));
        let label = relay_label(&long, "上海 🇨🇳", 48);
        assert!(label.chars().count() <= 48, "{label}");
        assert!(label.starts_with("上海 🇨🇳 · "));
        assert!(!label.contains('\n'));
    }

    #[test]
    fn truncate_middle_respects_char_boundaries() {
        assert_eq!(truncate_middle("short", 10), "short");
        assert_eq!(truncate_middle("", 10), "");
        assert_eq!(truncate_middle("abcdefghij", 6), "abc…ij");
        assert_eq!(truncate_middle("abcdefghij", 10), "abcdefghij");
        let emoji = "😀".repeat(30);
        let cut = truncate_middle(&emoji, 9);
        assert_eq!(cut.chars().count(), 9);
        assert!(cut.contains('…'));
        assert_eq!(truncate_middle("abc", 1), "abc");
    }

    fn base() -> ShareLocalConfigDto {
        ShareLocalConfigDto::default()
    }

    #[test]
    fn noop_ignores_equivalent_spellings() {
        let mut other = base();
        other.udp_port_v4 = Some(0);
        other.send_rate_limit_bytes_per_sec = Some(0);
        other.pinned_relay = Some(String::new());
        other.default_tier_id = Some(String::new());
        assert!(is_noop(&base(), &other));
        other.default_max_claims = 3;
        assert!(!is_noop(&base(), &other));
        let mut swarm = base();
        swarm.default_swarm = Some(true);
        swarm.auto_provide = Some(true);
        assert!(is_noop(&base(), &swarm));
        swarm.default_swarm = Some(false);
        assert!(!is_noop(&base(), &swarm));
        swarm.default_swarm = None;
        swarm.auto_provide = Some(false);
        assert!(!is_noop(&base(), &swarm));
    }

    #[test]
    fn prepare_commit_skips_noops_and_stamps_the_local_offset() {
        let mut on = base();
        on.auto_provide = Some(true);
        // 缺省即开：改成显式开是 no-op。
        assert!(prepare_commit(&base(), on, 28_800).is_none());

        let mut off = base();
        off.auto_provide = Some(false);
        let committed = prepare_commit(&base(), off, -18_000);
        assert_eq!(
            committed.as_ref().map(|config| config.auto_provide),
            Some(Some(false))
        );
        assert_eq!(
            committed.and_then(|config| config.provide_day_offset_secs),
            Some(-18_000)
        );
    }

    #[test]
    fn text_edits_apply_valid_fields_and_flag_invalid_ones() {
        let mut current = base();
        current.udp_port_v4 = Some(1111);
        current.extra_addrs = vec!["9.9.9.9:9".to_owned()];
        let edits = apply_text_inputs(
            &current,
            "7000",
            "70000",
            "1.2.3.4:5, nope",
            "2",
            RateUnit::MbPerSec,
            "1.5",
        );
        assert_eq!(edits.config.udp_port_v4, Some(7000));
        assert!(edits.udp_v6_invalid);
        assert_eq!(edits.config.udp_port_v6, None);
        assert_eq!(
            edits.extra_addrs_invalid,
            Some(InvalidAddr {
                entry: "nope".to_owned()
            })
        );
        // 非法字段保持底稿。
        assert_eq!(edits.config.extra_addrs, vec!["9.9.9.9:9".to_owned()]);
        assert_eq!(edits.config.send_rate_limit_bytes_per_sec, Some(2 << 20));
        assert!(edits.has_error());

        let clean = apply_text_inputs(&current, "", "", "", "", RateUnit::KbPerSec, "");
        assert!(!clean.has_error());
        assert_eq!(clean.config.udp_port_v4, None);
        assert!(clean.config.extra_addrs.is_empty());
        assert_eq!(clean.config.send_rate_limit_bytes_per_sec, None);
        let bad_rate = apply_text_inputs(&current, "1111", "", "", "9e9", RateUnit::KbPerSec, "");
        assert!(bad_rate.rate_invalid && bad_rate.has_error());
        let bad_cap = apply_text_inputs(&current, "", "", "", "", RateUnit::KbPerSec, "-1");
        assert!(bad_cap.daily_cap_invalid && bad_cap.has_error());
    }

    #[test]
    fn daily_cap_parsing_distinguishes_default_unlimited_and_gigabytes() {
        assert_eq!(parse_daily_cap(""), Ok(None));
        assert_eq!(parse_daily_cap("   "), Ok(None));
        assert_eq!(parse_daily_cap("0"), Ok(Some(0)));
        assert_eq!(parse_daily_cap("0.0"), Ok(Some(0)));
        assert_eq!(parse_daily_cap(" 20 "), Ok(Some(20 << 30)));
        assert_eq!(parse_daily_cap("1.5"), Ok(Some(3 << 29)));
        assert_eq!(parse_daily_cap(".5"), Ok(Some(1 << 29)));
        assert_eq!(parse_daily_cap("1048576"), Ok(Some(1 << 50)));
        assert_eq!(parse_daily_cap("1048577"), Err(InvalidDailyCap));
        assert_eq!(
            parse_daily_cap("99999999999999999999"),
            Err(InvalidDailyCap)
        );
        assert_eq!(parse_daily_cap("-1"), Err(InvalidDailyCap));
        assert_eq!(parse_daily_cap("1e3"), Err(InvalidDailyCap));
        assert_eq!(parse_daily_cap("."), Err(InvalidDailyCap));
    }

    #[test]
    fn daily_cap_text_round_trips() {
        assert_eq!(daily_cap_text(None), "");
        assert_eq!(daily_cap_text(Some(0)), "0");
        assert_eq!(daily_cap_text(Some(20 << 30)), "20");
        assert_eq!(daily_cap_text(Some(3 << 29)), "1.5");
    }

    #[test]
    fn day_offset_does_not_affect_noop() {
        let mut next = base();
        next.provide_day_offset_secs = Some(28_800);
        assert!(is_noop(&base(), &next));
        next.provide_daily_cap_bytes = Some(0);
        assert!(!is_noop(&base(), &next));
    }
}
