//! 网络 / 传输诊断的纯展示规则：指标筛选与取值格式化、路径统计（丢包 / 时延 / 带宽）、
//! 会话字节归属、路径事件时间线与会话列表的排序 / 展开状态。

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Local, TimeZone};
use fluxdown_protocol::{
    ShareDiagnosticsDto, SharePathEventDto, SharePathSummaryDto, ShareSessionBytesDto,
    ShareSessionDetailDto, ShareSwarmServedFileDto,
};
use serde_json::Value;

/// 没有数据时的占位。
pub const PLACEHOLDER: &str = "—";

/// 指标表的一行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MetricRow {
    pub key: String,
    pub value: String,
}

/// 指标取值的展示：整数不带小数点，浮点最多 3 位小数并去掉多余的 0，对象 / 数组用紧凑 JSON。
#[must_use]
pub fn format_metric_value(value: &Value) -> String {
    match value {
        Value::Null => PLACEHOLDER.to_owned(),
        Value::Bool(flag) => flag.to_string(),
        Value::String(text) => text.clone(),
        Value::Number(number) => {
            if let Some(unsigned) = number.as_u64() {
                return unsigned.to_string();
            }
            if let Some(signed) = number.as_i64() {
                return signed.to_string();
            }
            match number.as_f64() {
                Some(float) if float.is_finite() => format_float(float),
                _ => number.to_string(),
            }
        }
        Value::Array(_) | Value::Object(_) => value.to_string(),
    }
}

fn format_float(value: f64) -> String {
    let text = format!("{value:.3}");
    let trimmed = text.trim_end_matches('0').trim_end_matches('.');
    if trimmed.is_empty() || trimmed == "-" {
        "0".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// 按关键字筛选指标：空白分隔的每个词都必须（不分大小写）出现在键或展示值里；
/// 结果沿用 `BTreeMap` 的键顺序，所以同一组数据每次刷新行序稳定。
#[must_use]
pub fn filter_metrics(metrics: &BTreeMap<String, Value>, query: &str) -> Vec<MetricRow> {
    let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    metrics
        .iter()
        .filter_map(|(key, value)| {
            let row = MetricRow {
                key: key.clone(),
                value: format_metric_value(value),
            };
            if terms.is_empty() {
                return Some(row);
            }
            let haystack = format!("{}\n{}", row.key.to_lowercase(), row.value.to_lowercase());
            terms
                .iter()
                .all(|term| haystack.contains(term.as_str()))
                .then_some(row)
        })
        .collect()
}

/// 路径丢包率（百分比）：`lost / sent`；还没有发送过包为 `None`。
#[must_use]
pub fn path_loss_percent(path: &SharePathSummaryDto) -> Option<f64> {
    if path.sent == 0 {
        return None;
    }
    Some(path.lost as f64 * 100. / path.sent as f64)
}

/// 丢包率展示（`0.4%`）；没有发送过包为占位。
#[must_use]
pub fn format_loss(path: &SharePathSummaryDto) -> String {
    path_loss_percent(path).map_or_else(
        || PLACEHOLDER.to_owned(),
        |percent| format!("{:.1}%", percent.clamp(0., 100.)),
    )
}

/// 往返时延展示：小于 10 ms 保留一位小数；非有限或负值为占位。
#[must_use]
pub fn format_rtt(ms: f64) -> String {
    if !ms.is_finite() || ms < 0. {
        PLACEHOLDER.to_owned()
    } else if ms < 10. {
        format!("{ms:.1} ms")
    } else {
        format!("{} ms", ms.round() as u64)
    }
}

/// 拥塞控制的带宽估计（bit/s，十进制单位）；`0` 表示没有估计。
#[must_use]
pub fn format_bandwidth(bits_per_sec: u64) -> String {
    if bits_per_sec == 0 {
        return PLACEHOLDER.to_owned();
    }
    const UNITS: [&str; 4] = ["bps", "Kbps", "Mbps", "Gbps"];
    let mut value = bits_per_sec as f64;
    let mut unit = 0;
    while value >= 1000. && unit < UNITS.len() - 1 {
        value /= 1000.;
        unit += 1;
    }
    if unit == 0 {
        format!("{bits_per_sec} {}", UNITS[0])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// 毫秒耗时展示。
#[must_use]
pub fn format_millis(ms: u64) -> String {
    format!("{ms} ms")
}

/// 会话字节的来源归属（直连 / 中继 / 云端暂存 / 桥接）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BytesSplit {
    pub direct: u64,
    pub relay: u64,
    pub cdn: u64,
    pub bridge: u64,
    pub peer: u64,
    /// 直连占全部来源字节的比例（四舍五入的整数百分比）；还没有字节为 `None`。
    pub direct_percent: Option<u8>,
}

impl BytesSplit {
    /// 直连 / 中继之外有字节的来源：`(文案键, 字节数)`，云端暂存在前，桥接、共享依次在后。
    pub fn extras(&self) -> impl Iterator<Item = (&'static str, u64)> {
        [
            ("shareDiagSrcCloud", self.cdn),
            ("shareDiagSrcBridge", self.bridge),
            ("shareDiagSrcPeer", self.peer),
        ]
        .into_iter()
        .filter(|(_, bytes)| *bytes > 0)
    }
}

#[must_use]
pub fn bytes_split(bytes: &ShareSessionBytesDto) -> BytesSplit {
    let total = u128::from(bytes.direct)
        + u128::from(bytes.relay)
        + u128::from(bytes.cdn)
        + u128::from(bytes.bridge)
        + u128::from(bytes.peer);
    let direct_percent = (u128::from(bytes.direct) * 100 + total / 2)
        .checked_div(total)
        .map(|percent| u8::try_from(percent.min(100)).unwrap_or(100));
    BytesSplit {
        direct: bytes.direct,
        relay: bytes.relay,
        cdn: bytes.cdn,
        bridge: bytes.bridge,
        peer: bytes.peer,
        direct_percent,
    }
}

/// 端点 ID / 对端 ID 的短形式（保留头尾，按字符边界截取）；很短时原样返回。
#[must_use]
pub fn short_id(id: &str) -> String {
    const HEAD: usize = 6;
    const TAIL: usize = 4;
    let count = id.chars().count();
    if count <= HEAD + TAIL + 1 {
        return id.to_owned();
    }
    let head: String = id.chars().take(HEAD).collect();
    let tail: String = id.chars().skip(count - TAIL).collect();
    format!("{head}…{tail}")
}

/// 路径事件类别的文案键；未知类别返回 `None`（界面显示原文）。
#[must_use]
pub fn event_kind_key(kind: &str) -> Option<&'static str> {
    match kind {
        "opened" => Some("shareDiagEventOpened"),
        "selected" => Some("shareDiagEventSelected"),
        "closed" => Some("shareDiagEventClosed"),
        "lagged" => Some("shareDiagEventLagged"),
        _ => None,
    }
}

/// 事件时间线推出的「当前使用中的路径」：最后一次被选用且之后没有被关闭的路径。
#[must_use]
pub fn selected_path_id(events: &[SharePathEventDto]) -> Option<&str> {
    let mut current: Option<&str> = None;
    for event in events {
        match event.kind.as_str() {
            "selected" => current = Some(event.path.as_str()),
            "closed" if current == Some(event.path.as_str()) => current = None,
            _ => {}
        }
    }
    current
}

/// RFC 3339 时间按给定时区格式化为 `HH:MM:SS.mmm`；解析失败返回原文。
#[must_use]
pub fn format_clock_in<Tz>(value: &str, zone: &Tz) -> String
where
    Tz: TimeZone,
    Tz::Offset: std::fmt::Display,
{
    match DateTime::parse_from_rfc3339(value) {
        Ok(parsed) => parsed
            .with_timezone(zone)
            .format("%H:%M:%S%.3f")
            .to_string(),
        Err(_) => value.to_owned(),
    }
}

/// [`format_clock_in`] 取本机时区。
#[must_use]
pub fn format_clock(value: &str) -> String {
    format_clock_in(value, &Local)
}

/// 会话列表顺序：进行中的在前，已结束的在后；同组保持 agent 给出的顺序。
#[must_use]
pub fn ordered_sessions(sessions: &[ShareSessionDetailDto]) -> Vec<&ShareSessionDetailDto> {
    let mut ordered: Vec<&ShareSessionDetailDto> = sessions.iter().collect();
    ordered.sort_by_key(|session| session.ended);
    ordered
}

/// 中继延迟表的一行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelayRow {
    pub url: String,
    pub latency_ms: u64,
    /// net report 选出的首选中继。
    pub preferred: bool,
    /// 当前的 home relay。
    pub home: bool,
}

fn same_relay(a: &str, b: &str) -> bool {
    a.trim_end_matches('/') == b.trim_end_matches('/')
}

/// 各中继延迟按延迟升序排列（同延迟按 URL），并标出首选 / home relay（比较时忽略末尾 `/`）。
#[must_use]
pub fn relay_rows(
    latency: &BTreeMap<String, u64>,
    preferred: Option<&str>,
    home: Option<&str>,
) -> Vec<RelayRow> {
    let mut rows: Vec<RelayRow> = latency
        .iter()
        .map(|(url, latency_ms)| RelayRow {
            url: url.clone(),
            latency_ms: *latency_ms,
            preferred: preferred.is_some_and(|candidate| same_relay(candidate, url)),
            home: home.is_some_and(|candidate| same_relay(candidate, url)),
        })
        .collect();
    rows.sort_by(|a, b| {
        a.latency_ms
            .cmp(&b.latency_ms)
            .then_with(|| a.url.cmp(&b.url))
    });
    rows
}

/// 在某个会话的诊断结果里找出指定会话（任务「传输诊断」页只关心自己的会话）。
#[must_use]
pub fn find_session<'a>(
    diagnostics: &'a ShareDiagnosticsDto,
    session_id: &str,
) -> Option<&'a ShareSessionDetailDto> {
    diagnostics
        .sessions
        .iter()
        .find(|session| session.session_id == session_id)
}

/// 会话列表的展开 / 聚焦状态：网络诊断页收到「定位到某会话」请求时聚焦并展开它，其余会话由用户手动展开。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionExpansion {
    focused: Option<String>,
    expanded: BTreeSet<String>,
}

impl SessionExpansion {
    /// 聚焦并展开指定会话。
    pub fn focus(&mut self, session_id: &str) {
        self.focused = Some(session_id.to_owned());
        self.expanded.insert(session_id.to_owned());
    }

    /// 切换展开；收起被聚焦的会话同时取消高亮。
    pub fn toggle(&mut self, session_id: &str) {
        if !self.expanded.remove(session_id) {
            self.expanded.insert(session_id.to_owned());
        } else if self.focused.as_deref() == Some(session_id) {
            self.focused = None;
        }
    }

    #[must_use]
    pub fn is_expanded(&self, session_id: &str) -> bool {
        self.expanded.contains(session_id)
    }

    #[must_use]
    pub fn is_focused(&self, session_id: &str) -> bool {
        self.focused.as_deref() == Some(session_id)
    }

    /// 被聚焦的会话 ID（可能还没出现在诊断结果里）。
    #[must_use]
    pub fn focused(&self) -> Option<&str> {
        self.focused.as_deref()
    }
}

/// 共享提供会话里一个内容的显示名：本机已知的文件名，否则回退到短根哈希。
#[must_use]
pub fn served_file_label(file: &ShareSwarmServedFileDto) -> String {
    match file.file_name.as_deref().map(str::trim) {
        Some(name) if !name.is_empty() => name.to_owned(),
        _ => short_id(&file.root_hash),
    }
}

/// 原始快照 JSON 按行拆开（供虚拟列表逐行渲染；保留行首缩进）。
#[must_use]
pub fn snapshot_lines(json: &str) -> Vec<String> {
    json.lines().map(str::to_owned).collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::FixedOffset;
    use fluxdown_protocol::{
        ShareDiagnosticsDto, SharePathEventDto, SharePathKind, SharePathSummaryDto,
        ShareSessionBytesDto, ShareSessionDetailDto, ShareSessionRole, ShareSessionSummaryDto,
        ShareSwarmServedFileDto,
    };
    use serde_json::{Value, json};

    use super::{
        PLACEHOLDER, SessionExpansion, bytes_split, event_kind_key, filter_metrics, find_session,
        format_bandwidth, format_clock_in, format_loss, format_metric_value, format_rtt,
        ordered_sessions, path_loss_percent, relay_rows, selected_path_id, served_file_label,
        short_id, snapshot_lines,
    };

    fn path(sent: u64, lost: u64) -> SharePathSummaryDto {
        SharePathSummaryDto {
            id: "p1".into(),
            kind: SharePathKind::DirectWan,
            sent,
            lost,
            ..SharePathSummaryDto::default()
        }
    }

    fn session(id: &str, ended: bool) -> ShareSessionDetailDto {
        ShareSessionDetailDto {
            session_id: id.into(),
            share_id: "s".into(),
            role: ShareSessionRole::Sender,
            peer_endpoint_id: "peer".into(),
            peer_device_name: None,
            peer_nat_type: Default::default(),
            peer_isp: None,
            ended,
            summary: ShareSessionSummaryDto::default(),
            samples: Vec::new(),
            swarm_files: Vec::new(),
        }
    }

    fn event(kind: &str, path: &str) -> SharePathEventDto {
        SharePathEventDto {
            at: "2026-10-27T10:30:00.123Z".into(),
            kind: kind.into(),
            path: path.into(),
        }
    }

    #[test]
    fn served_file_label_prefers_the_file_name_and_falls_back_to_the_short_root() {
        let root = "ab".repeat(32);
        let named = ShareSwarmServedFileDto {
            root_hash: root.clone(),
            file_name: Some("电影 2024 第一季.mkv".into()),
            bytes: 1,
            chunks: 1,
        };
        assert_eq!(served_file_label(&named), "电影 2024 第一季.mkv");
        for name in [None, Some(String::new()), Some("  \t".into())] {
            let unnamed = ShareSwarmServedFileDto {
                file_name: name,
                ..named.clone()
            };
            assert_eq!(served_file_label(&unnamed), short_id(&root));
        }
        assert!(served_file_label(&ShareSwarmServedFileDto::default()).is_empty());
    }

    #[test]
    fn metric_values_render_without_float_noise() {
        assert_eq!(format_metric_value(&json!(42)), "42");
        assert_eq!(format_metric_value(&json!(-7)), "-7");
        assert_eq!(format_metric_value(&json!(u64::MAX)), u64::MAX.to_string());
        assert_eq!(format_metric_value(&json!(12.5)), "12.5");
        assert_eq!(format_metric_value(&json!(3.0)), "3");
        assert_eq!(format_metric_value(&json!(0.0004)), "0");
        assert_eq!(format_metric_value(&json!(true)), "true");
        assert_eq!(format_metric_value(&json!("udp")), "udp");
        assert_eq!(format_metric_value(&Value::Null), PLACEHOLDER);
        assert_eq!(format_metric_value(&json!([1, 2])), "[1,2]");
    }

    #[test]
    fn metrics_filter_matches_every_term_in_key_or_value_and_keeps_key_order() {
        let metrics: BTreeMap<String, Value> = BTreeMap::from([
            ("magicsock.recv_data_ipv4".to_owned(), json!(10)),
            ("magicsock.send_data_relay".to_owned(), json!(2048)),
            ("net_report.reports".to_owned(), json!(3)),
            ("portmapper.probes".to_owned(), json!("ok")),
        ]);
        let all = filter_metrics(&metrics, "  ");
        let keys: Vec<&str> = all.iter().map(|row| row.key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "magicsock.recv_data_ipv4",
                "magicsock.send_data_relay",
                "net_report.reports",
                "portmapper.probes"
            ]
        );

        let magic = filter_metrics(&metrics, "MAGICSOCK data");
        assert_eq!(magic.len(), 2);
        let relay = filter_metrics(&metrics, "magicsock relay");
        assert_eq!(relay.len(), 1);
        assert_eq!(relay[0].value, "2048");
        // 值也参与匹配。
        let by_value = filter_metrics(&metrics, "ok");
        assert_eq!(by_value.len(), 1);
        assert_eq!(by_value[0].key, "portmapper.probes");
        assert!(filter_metrics(&metrics, "nothing-like-this").is_empty());
    }

    #[test]
    fn loss_is_lost_over_sent_and_unknown_before_any_packet() {
        assert_eq!(path_loss_percent(&path(0, 0)), None);
        assert_eq!(format_loss(&path(0, 0)), PLACEHOLDER);
        assert_eq!(path_loss_percent(&path(200, 5)), Some(2.5));
        assert_eq!(format_loss(&path(200, 5)), "2.5%");
        assert_eq!(format_loss(&path(10, 0)), "0.0%");
        // 统计口径偏差（丢失多于发送）不会显示超过 100%。
        assert_eq!(format_loss(&path(1, 3)), "100.0%");
    }

    #[test]
    fn rtt_and_bandwidth_are_humanised() {
        assert_eq!(format_rtt(4.26), "4.3 ms");
        assert_eq!(format_rtt(23.6), "24 ms");
        assert_eq!(format_rtt(f64::NAN), PLACEHOLDER);
        assert_eq!(format_rtt(-1.), PLACEHOLDER);
        assert_eq!(format_bandwidth(0), PLACEHOLDER);
        assert_eq!(format_bandwidth(950), "950 bps");
        assert_eq!(format_bandwidth(12_500_000), "12.5 Mbps");
        assert_eq!(format_bandwidth(u64::MAX), "18446744073.7 Gbps");
    }

    #[test]
    fn bytes_split_reports_direct_share_without_overflow() {
        let split = bytes_split(&ShareSessionBytesDto {
            direct: 3,
            relay: 1,
            ..ShareSessionBytesDto::default()
        });
        assert_eq!(split.direct_percent, Some(75));
        assert_eq!(
            bytes_split(&ShareSessionBytesDto::default()).direct_percent,
            None
        );
        let only_relay = bytes_split(&ShareSessionBytesDto {
            relay: 9,
            ..ShareSessionBytesDto::default()
        });
        assert_eq!(only_relay.direct_percent, Some(0));
        let huge = bytes_split(&ShareSessionBytesDto {
            direct: u64::MAX,
            relay: u64::MAX,
            ..ShareSessionBytesDto::default()
        });
        assert_eq!(huge.direct_percent, Some(50));
    }

    #[test]
    fn bytes_split_counts_cloud_and_bridge_and_lists_only_nonzero_extras() {
        let split = bytes_split(&ShareSessionBytesDto {
            direct: 1,
            relay: 0,
            cdn: 2,
            bridge: 1,
            peer: 0,
        });
        assert_eq!(split.direct_percent, Some(25));
        assert_eq!(
            split.extras().collect::<Vec<_>>(),
            [("shareDiagSrcCloud", 2), ("shareDiagSrcBridge", 1)]
        );
        let direct_only = bytes_split(&ShareSessionBytesDto {
            direct: 5,
            ..ShareSessionBytesDto::default()
        });
        assert_eq!(direct_only.extras().count(), 0);
    }

    #[test]
    fn bytes_split_lists_peer_bytes_after_bridge() {
        let split = bytes_split(&ShareSessionBytesDto {
            direct: 1,
            bridge: 1,
            peer: 2,
            ..ShareSessionBytesDto::default()
        });
        assert_eq!(split.direct_percent, Some(25));
        assert_eq!(
            split.extras().collect::<Vec<_>>(),
            [("shareDiagSrcBridge", 1), ("shareDiagSrcPeer", 2)]
        );
    }

    #[test]
    fn short_ids_keep_head_and_tail_on_char_boundaries() {
        assert_eq!(short_id("abc"), "abc");
        assert_eq!(short_id("0123456789abcdef"), "012345…cdef");
        assert_eq!(
            short_id("设备设备设备设备设备设备设备"),
            "设备设备设备…设备设备"
        );
        assert_eq!(short_id(""), "");
    }

    #[test]
    fn event_kinds_map_to_keys_and_current_path_follows_the_timeline() {
        assert_eq!(event_kind_key("selected"), Some("shareDiagEventSelected"));
        assert_eq!(event_kind_key("weird"), None);

        assert_eq!(selected_path_id(&[]), None);
        let events = [
            event("opened", "relay"),
            event("selected", "relay"),
            event("opened", "udp"),
            event("selected", "udp"),
        ];
        assert_eq!(selected_path_id(&events), Some("udp"));
        let closed = [
            event("selected", "udp"),
            event("closed", "relay"),
            event("closed", "udp"),
        ];
        assert_eq!(selected_path_id(&closed), None);
        let other_closed = [event("selected", "udp"), event("closed", "relay")];
        assert_eq!(selected_path_id(&other_closed), Some("udp"));
    }

    #[test]
    fn clock_uses_the_given_zone_and_falls_back_to_raw() {
        let Some(zone) = FixedOffset::east_opt(8 * 3600) else {
            return;
        };
        assert_eq!(
            format_clock_in("2026-10-27T10:30:00.123Z", &zone),
            "18:30:00.123"
        );
        assert_eq!(format_clock_in("garbage", &zone), "garbage");
    }

    #[test]
    fn active_sessions_sort_first_and_keep_relative_order() {
        let sessions = vec![
            session("a", true),
            session("b", false),
            session("c", true),
            session("d", false),
        ];
        let order: Vec<&str> = ordered_sessions(&sessions)
            .iter()
            .map(|session| session.session_id.as_str())
            .collect();
        assert_eq!(order, ["b", "d", "a", "c"]);

        let diagnostics = ShareDiagnosticsDto {
            sessions,
            ..ShareDiagnosticsDto::default()
        };
        assert!(find_session(&diagnostics, "c").is_some());
        assert!(find_session(&diagnostics, "zzz").is_none());
    }

    #[test]
    fn session_expansion_focus_toggle_state_machine() {
        let mut state = SessionExpansion::default();
        assert!(!state.is_expanded("a"));
        assert_eq!(state.focused(), None);

        state.focus("a");
        assert!(state.is_focused("a") && state.is_expanded("a"));
        state.toggle("b");
        assert!(state.is_expanded("b") && !state.is_focused("b"));
        assert!(state.is_focused("a"));

        // 收起被聚焦的会话同时取消高亮；再次聚焦重新展开。
        state.toggle("a");
        assert!(!state.is_expanded("a") && !state.is_focused("a"));
        state.focus("a");
        assert!(state.is_focused("a") && state.is_expanded("a"));
        // 聚焦另一个会话会换掉高亮，但保留已展开的其它会话。
        state.focus("b");
        assert!(state.is_focused("b") && !state.is_focused("a"));
        assert!(state.is_expanded("a"));
    }

    #[test]
    fn relays_sort_by_latency_and_mark_preferred_and_home() {
        let latency = BTreeMap::from([
            ("https://b.example/".to_owned(), 80),
            ("https://a.example".to_owned(), 80),
            ("https://c.example".to_owned(), 12),
        ]);
        let rows = relay_rows(
            &latency,
            Some("https://c.example/"),
            Some("https://a.example/"),
        );
        let urls: Vec<&str> = rows.iter().map(|row| row.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "https://c.example",
                "https://a.example",
                "https://b.example/"
            ]
        );
        assert!(rows[0].preferred && !rows[0].home);
        assert!(rows[1].home && !rows[1].preferred);
        assert!(!rows[2].preferred && !rows[2].home);
        assert!(relay_rows(&BTreeMap::new(), None, None).is_empty());
    }

    #[test]
    fn snapshot_lines_keep_indentation() {
        assert_eq!(snapshot_lines("{\n  \"a\": 1\n}"), ["{", "  \"a\": 1", "}"]);
        assert!(snapshot_lines("").is_empty());
    }
}
