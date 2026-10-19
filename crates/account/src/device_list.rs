//! 云设备列表的展示规则：排序、搜索、折叠数量、名称校验与时间格式。

use fluxdown_protocol::{CloudDevice, Entitlements};

/// 设备卡片默认展示的设备数；更多设备经「管理全部」对话框查看。
pub(crate) const VISIBLE_LIMIT: usize = 5;
/// 设备名长度上限（与 agent / 云端一致，按字符计）。
pub(crate) const NAME_MAX_CHARS: usize = 64;

/// 本机在前，其后在线设备，最后离线；同组按名称（不区分大小写）排序。
pub(crate) fn sorted(devices: &[CloudDevice], presence_known: bool) -> Vec<CloudDevice> {
    let mut list = devices.to_vec();
    list.sort_by(|a, b| {
        b.is_current
            .cmp(&a.is_current)
            .then((presence_known && b.is_online).cmp(&(presence_known && a.is_online)))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    list
}

/// 在线设备数（含本机）。
pub(crate) fn online_count(devices: &[CloudDevice]) -> usize {
    devices.iter().filter(|device| device.is_online).count()
}

/// 权益里的同时在线设备上限；缺失、非正整数或 0（不限）为 `None`。
pub(crate) fn max_online_devices(entitlements: &Entitlements) -> Option<u64> {
    entitlements
        .0
        .get("maxOnlineDevices")
        .and_then(serde_json::Value::as_u64)
        .filter(|max| *max > 0)
}

/// 名称 / 平台 / 版本任一包含查询词（不区分大小写）；空查询匹配全部。
pub(crate) fn matches_query(device: &CloudDevice, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return true;
    }
    [
        Some(device.name.as_str()),
        device.platform.as_deref(),
        device.app_version.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(|field| field.to_lowercase().contains(&query))
}

pub(crate) fn filtered(
    devices: &[CloudDevice],
    query: &str,
    presence_known: bool,
) -> Vec<CloudDevice> {
    sorted(devices, presence_known)
        .into_iter()
        .filter(|device| matches_query(device, query))
        .collect()
}

/// 设备卡片可见的设备与被折叠的数量。
pub(crate) fn summarize(
    devices: &[CloudDevice],
    presence_known: bool,
) -> (Vec<CloudDevice>, usize) {
    let mut list = sorted(devices, presence_known);
    let hidden = list.len().saturating_sub(VISIBLE_LIMIT);
    list.truncate(VISIBLE_LIMIT);
    (list, hidden)
}

/// 规整重命名输入：去首尾空白后必须是 1–64 个字符。
pub(crate) fn normalize_device_name(raw: &str) -> Option<String> {
    let name = raw.trim();
    let count = name.chars().count();
    (1..=NAME_MAX_CHARS)
        .contains(&count)
        .then(|| name.to_owned())
}

/// 平台名 → 已有文案键；未识别的平台原样显示。
pub(crate) fn platform_label_key(platform: &str) -> Option<&'static str> {
    match platform.trim().to_ascii_lowercase().as_str() {
        "windows" | "win32" => Some("accountDevicePlatformWindows"),
        "macos" | "darwin" => Some("accountDevicePlatformMacos"),
        "linux" => Some("accountDevicePlatformLinux"),
        "android" => Some("accountDevicePlatformAndroid"),
        "ios" => Some("accountDevicePlatformIos"),
        "web" => Some("accountDevicePlatformWeb"),
        _ => None,
    }
}

/// ISO-8601 → 本地时区的 `YYYY-MM-DD HH:MM`；无法识别时原样返回（空串保持为空）。
///
/// 带 `Z` 或 `±HH:MM` 偏移的时间换算到本机时区（与 Web 一致）；没有时区信息的值
/// 无从换算，只截断。
pub(crate) fn format_timestamp(raw: &str) -> String {
    format_timestamp_in(raw, &chrono::Local)
}

fn format_timestamp_in<Tz: chrono::TimeZone>(raw: &str, tz: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let raw = raw.trim();
    let bytes = raw.as_bytes();
    let looks_iso = bytes.len() >= 16
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && matches!(bytes[10], b'T' | b' ')
        && bytes[13] == b':';
    if !looks_iso {
        return raw.to_owned();
    }
    if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(raw) {
        return parsed
            .with_timezone(tz)
            .format("%Y-%m-%d %H:%M")
            .to_string();
    }
    raw.get(..16)
        .map_or_else(|| raw.to_owned(), |head| head.replacen('T', " ", 1))
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    fn device(name: &str, current: bool, online: bool) -> CloudDevice {
        CloudDevice {
            id: name.to_owned(),
            device_id: name.to_owned(),
            name: name.to_owned(),
            platform: Some("macos".to_owned()),
            created_at: String::new(),
            last_seen_at: String::new(),
            last_ip: None,
            app_version: Some("1.2.3".to_owned()),
            is_online: online,
            is_current: current,
            default_save_dir: None,
            path_style: None,
        }
    }

    #[test]
    fn online_count_includes_current_device() {
        let devices = [
            device("me", true, true),
            device("a", false, true),
            device("b", false, false),
        ];
        assert_eq!(online_count(&devices), 2);
        assert_eq!(online_count(&[]), 0);
    }

    #[test]
    fn max_online_devices_reads_positive_integer_only() {
        let entitlements = |value: serde_json::Value| {
            let mut map = std::collections::BTreeMap::new();
            map.insert("maxOnlineDevices".to_owned(), value);
            Entitlements(map)
        };
        assert_eq!(
            max_online_devices(&entitlements(serde_json::json!(2))),
            Some(2)
        );
        assert_eq!(
            max_online_devices(&entitlements(serde_json::json!(0))),
            None
        );
        assert_eq!(
            max_online_devices(&entitlements(serde_json::json!("2"))),
            None
        );
        assert_eq!(max_online_devices(&Entitlements::default()), None);
    }

    #[test]
    fn current_device_first_then_online_then_name() {
        let list = sorted(
            &[
                device("zeta", false, true),
                device("alpha", false, false),
                device("Beta", false, true),
                device("me", true, false),
            ],
            true,
        );
        let names: Vec<_> = list.iter().map(|device| device.name.as_str()).collect();
        assert_eq!(names, ["me", "Beta", "zeta", "alpha"]);
    }

    #[test]
    fn search_matches_name_platform_and_version_case_insensitively() {
        let mut win = device("Office PC", false, true);
        win.platform = Some("windows".to_owned());
        let devices = [win, device("MacBook", false, false)];
        assert_eq!(filtered(&devices, "OFFICE", true).len(), 1);
        assert_eq!(filtered(&devices, "  windows ", true).len(), 1);
        assert_eq!(filtered(&devices, "1.2.3", true).len(), 2);
        assert!(filtered(&devices, "linux", true).is_empty());
        assert_eq!(filtered(&devices, "", true).len(), 2);
    }

    #[test]
    fn card_collapses_beyond_the_limit_and_reports_hidden_count() {
        let devices: Vec<_> = (0..8)
            .map(|index| device(&format!("d{index}"), index == 7, false))
            .collect();
        let (visible, hidden) = summarize(&devices, true);
        assert_eq!(visible.len(), VISIBLE_LIMIT);
        assert_eq!(hidden, 3);
        // 本机始终可见。
        assert_eq!(visible[0].name, "d7");
        let (visible, hidden) = summarize(&devices[..2], true);
        assert_eq!((visible.len(), hidden), (2, 0));
    }

    #[test]
    fn unknown_presence_does_not_prioritize_cached_online_devices() {
        let devices = [
            device("zeta", false, true),
            device("alpha", false, false),
            device("me", true, false),
        ];
        let list = sorted(&devices, false);
        let names: Vec<_> = list.iter().map(|device| device.name.as_str()).collect();
        assert_eq!(names, ["me", "alpha", "zeta"]);
        assert_eq!(filtered(&devices, "", false), list);
        assert_eq!(summarize(&devices, false).0, list);
    }

    #[test]
    fn rename_input_is_trimmed_and_bounded_by_characters() {
        assert_eq!(
            normalize_device_name("  Living room  "),
            Some("Living room".to_owned())
        );
        assert_eq!(normalize_device_name("   "), None);
        assert_eq!(normalize_device_name(""), None);
        assert!(normalize_device_name(&"名".repeat(NAME_MAX_CHARS)).is_some());
        assert_eq!(
            normalize_device_name(&"名".repeat(NAME_MAX_CHARS + 1)),
            None
        );
    }

    #[test]
    fn timestamps_convert_to_target_zone_only_when_zoned() {
        let tz = chrono::FixedOffset::east_opt(8 * 3600).expect("offset");
        assert_eq!(
            format_timestamp_in("2026-09-29T06:37:48Z", &tz),
            "2026-09-29 14:37"
        );
        assert_eq!(
            format_timestamp_in("2026-09-29T23:30:00-05:30", &tz),
            "2026-09-30 13:00"
        );
        assert_eq!(
            format_timestamp_in("2026-09-29 06:37:48", &tz),
            "2026-09-29 06:37"
        );
        assert_eq!(format_timestamp_in("yesterday", &tz), "yesterday");
        assert_eq!(format_timestamp_in("", &tz), "");
    }

    #[test]
    fn platform_labels_cover_known_platforms_only() {
        assert_eq!(
            platform_label_key("Windows"),
            Some("accountDevicePlatformWindows")
        );
        assert_eq!(
            platform_label_key("darwin"),
            Some("accountDevicePlatformMacos")
        );
        assert_eq!(platform_label_key("freebsd"), None);
    }
}
