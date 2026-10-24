//! 局域网直连配对的纯逻辑：输入校验、发现列表投影、配对码倒计时、配对流程状态。
//!
//! 对话框 UI 只负责渲染与发起 `agent.link.*` 调用，状态转换与边界判断集中在这里以便单测。

use fluxdown_protocol::{LinkDeviceInfo, LinkDiscoveredPeer};

/// 配对码位数（对端设备设置里展示的数字码）。
pub(crate) const PAIRING_CODE_LEN: usize = 6;

/// 手动输入无法提交的原因（映射到本地化提示）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PairInputError {
    AddressMissing,
    CodeIncomplete,
}

impl PairInputError {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::AddressMissing => "localPairingHostRequired",
            Self::CodeIncomplete => "localPairingCodeIncomplete",
        }
    }
}

/// 校验并规整 `agent.link.pairBegin` 的输入：地址原样交给 agent 解析
/// （`host` / `host:port` / `http(s)://…` 均可），配对码去掉空白与连字符后必须是 6 位数字。
pub(crate) fn validate_pair_input(
    address: &str,
    code: &str,
) -> Result<(String, String), PairInputError> {
    let address = address.trim();
    if address.is_empty() {
        return Err(PairInputError::AddressMissing);
    }
    let code: String = code
        .chars()
        .filter(|ch| !ch.is_whitespace() && *ch != '-')
        .collect();
    if code.chars().count() != PAIRING_CODE_LEN || !code.chars().all(|ch| ch.is_ascii_digit()) {
        return Err(PairInputError::CodeIncomplete);
    }
    Ok((address.to_owned(), code))
}

/// 发现列表里一台设备的展示条目。
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DiscoveredEntry {
    pub name: String,
    pub platform: Option<String>,
    /// 交给 `pairBegin` / `probe` 的地址（`host:port`，IPv6 加方括号）。
    pub address: String,
    /// 已经与本机配对（按指纹判断），不可再选。
    pub paired: bool,
}

pub(crate) fn peer_address(peer: &LinkDiscoveredPeer) -> String {
    if peer.host.contains(':') && !peer.host.starts_with('[') {
        format!("[{}]:{}", peer.host, peer.port)
    } else {
        format!("{}:{}", peer.host, peer.port)
    }
}

/// 把发现快照投影成列表：已配对设备标记出来，同一地址只保留一条，按名称排序。
pub(crate) fn discovered_entries(
    peers: &[LinkDiscoveredPeer],
    paired: &[LinkDeviceInfo],
) -> Vec<DiscoveredEntry> {
    let mut entries: Vec<DiscoveredEntry> = Vec::new();
    for peer in peers {
        let address = peer_address(peer);
        if entries.iter().any(|entry| entry.address == address) {
            continue;
        }
        let is_paired = peer.fingerprint.as_deref().is_some_and(|fingerprint| {
            // 被对端解除（revoked）的设备不算已配对，保留可选以便重新配对。
            paired
                .iter()
                .any(|device| !device.revoked && device.fingerprint == fingerprint)
        });
        entries.push(DiscoveredEntry {
            name: if peer.name.trim().is_empty() {
                peer.host.clone()
            } else {
                peer.name.clone()
            },
            platform: peer.platform.clone(),
            address,
            paired: is_paired,
        });
    }
    entries.sort_by_key(|entry| entry.name.to_lowercase());
    entries
}

/// 当前 Unix 毫秒时间（配对码 / 请求过期时间与之比较）。
pub(crate) fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .unwrap_or(0)
}

/// 配对码 / 入站请求距离过期还剩多少秒（已过期为 0）。
pub(crate) fn seconds_until(expires_at_unix_ms: i64, now_unix_ms: i64) -> u64 {
    let remaining_ms = expires_at_unix_ms.saturating_sub(now_unix_ms);
    if remaining_ms <= 0 {
        0
    } else {
        // 向上取整：还剩 0.4s 时仍显示 1s，避免提前显示「已过期」。
        u64::try_from(remaining_ms.saturating_add(999) / 1000).unwrap_or(0)
    }
}

/// 配对码 / 入站请求的本机截止时刻：以收到 DTO 的本机时刻 + `ttl_ms` 计算（免疫主机与本机的时钟偏差）；
/// `ttl_ms == 0`（旧主机未下发）才回退主机绝对时间 `expires_at_unix_ms`。
pub(crate) fn local_deadline_ms(ttl_ms: u64, expires_at_unix_ms: i64, received_at_ms: i64) -> i64 {
    if ttl_ms == 0 {
        expires_at_unix_ms
    } else {
        received_at_ms.saturating_add(i64::try_from(ttl_ms).unwrap_or(i64::MAX))
    }
}

/// 短认证串按 3 位一组展示（`123456` → `123 456`），便于两台设备肉眼比对。
pub(crate) fn group_sas(sas: &str) -> String {
    let chars: Vec<char> = sas.chars().filter(|ch| !ch.is_whitespace()).collect();
    chars
        .chunks(3)
        .map(|chunk| chunk.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join(" ")
}

/// 发起端配对流程。
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PairFlow {
    /// 选择设备 / 填写地址与配对码。
    Form,
    /// `pairBegin` 在途。
    Beginning,
    /// 双方核对 SAS；`token` 交给 `pairFinish`。
    Verify {
        token: String,
        sas: String,
        peer_name: String,
    },
    /// `pairFinish` 在途（等待对端确认）。
    Finishing { token: String, peer_name: String },
    /// 已完成。
    Done { device_name: String },
}

impl PairFlow {
    /// 仍持有 agent 会话（核对中 / 等待对端确认）时的 `pairFinish` token；放弃时据此释放或中止。
    pub(crate) fn session_token(&self) -> Option<&str> {
        match self {
            Self::Verify { token, .. } | Self::Finishing { token, .. } => Some(token),
            _ => None,
        }
    }

    /// 表单可编辑且允许发起。
    pub(crate) fn can_begin(&self) -> bool {
        matches!(self, Self::Form)
    }

    pub(crate) fn is_busy(&self) -> bool {
        matches!(self, Self::Beginning | Self::Finishing { .. })
    }

    /// `pairBegin` 成功。只有在 `Beginning` 才接受（用户中途取消后的迟到响应被丢弃）。
    pub(crate) fn began(self, token: String, sas: String, peer_name: String) -> Self {
        match self {
            Self::Beginning => Self::Verify {
                token,
                sas,
                peer_name,
            },
            other => other,
        }
    }

    /// 用户点击「确认配对」。
    pub(crate) fn confirming(self) -> Self {
        match self {
            Self::Verify {
                token, peer_name, ..
            } => Self::Finishing { token, peer_name },
            other => other,
        }
    }

    /// `pairFinish` 返回：`paired=false`（被拒绝 / 放弃）回到表单，成功进入完成态。
    pub(crate) fn finished(self, paired: bool, device_name: Option<String>) -> Self {
        match self {
            Self::Finishing { peer_name, .. } if paired => Self::Done {
                device_name: device_name
                    .filter(|name| !name.is_empty())
                    .unwrap_or(peer_name),
            },
            Self::Finishing { .. } => Self::Form,
            other => other,
        }
    }

    /// 任一步骤失败：回到可重试的表单。
    pub(crate) fn failed(self) -> Self {
        match self {
            Self::Done { .. } => self,
            _ => Self::Form,
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    fn peer(name: &str, host: &str, port: u16, fingerprint: Option<&str>) -> LinkDiscoveredPeer {
        LinkDiscoveredPeer {
            fingerprint: fingerprint.map(str::to_owned),
            name: name.to_owned(),
            platform: None,
            host: host.to_owned(),
            port,
            app_version: None,
            source: "mdns".to_owned(),
        }
    }

    fn paired(fingerprint: &str) -> LinkDeviceInfo {
        LinkDeviceInfo {
            fingerprint: fingerprint.to_owned(),
            name: "Paired".to_owned(),
            platform: None,
            online: false,
            paired_at: 0,
            last_seen_at: 0,
            default_save_dir: None,
            path_style: None,
            custom_name: None,
            app_version: None,
            cloud_device_id: None,
            grants_control: false,
            controllable: false,
            revoked: false,
        }
    }

    #[test]
    fn pair_input_accepts_any_address_form_and_normalizes_the_code() {
        for address in [
            "192.168.1.20",
            "192.168.1.20:17800",
            "https://nas.example.com",
            "  nas.local  ",
        ] {
            assert!(validate_pair_input(address, "123456").is_ok(), "{address}");
        }
        assert_eq!(
            validate_pair_input(" nas.local ", "123 456"),
            Ok(("nas.local".to_owned(), "123456".to_owned()))
        );
        assert_eq!(
            validate_pair_input("nas.local", "123-456").map(|(_, code)| code),
            Ok("123456".to_owned())
        );
    }

    #[test]
    fn pair_input_rejects_missing_address_and_malformed_code() {
        assert_eq!(
            validate_pair_input("   ", "123456"),
            Err(PairInputError::AddressMissing)
        );
        for code in ["", "12345", "1234567", "12345a", "١٢٣٤٥٦"] {
            assert_eq!(
                validate_pair_input("nas.local", code),
                Err(PairInputError::CodeIncomplete),
                "{code}"
            );
        }
    }

    #[test]
    fn discovered_list_marks_paired_dedups_and_sorts() {
        let peers = [
            peer("zeta", "192.168.1.9", 17800, Some("fp-z")),
            peer("Alpha", "192.168.1.5", 17800, Some("fp-a")),
            peer("dup", "192.168.1.5", 17800, None),
            peer("", "fe80::1", 17800, None),
        ];
        let entries = discovered_entries(&peers, &[paired("fp-a")]);
        let names: Vec<_> = entries.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, ["Alpha", "fe80::1", "zeta"]);
        assert!(entries[0].paired);
        assert!(!entries[2].paired);
        assert_eq!(entries[1].address, "[fe80::1]:17800");
        assert_eq!(entries[0].address, "192.168.1.5:17800");
    }

    #[test]
    fn countdown_rounds_up_and_clamps_at_zero() {
        assert_eq!(seconds_until(10_400, 10_000), 1);
        assert_eq!(seconds_until(12_000, 10_000), 2);
        assert_eq!(seconds_until(10_000, 10_000), 0);
        assert_eq!(seconds_until(9_000, 10_000), 0);
    }

    #[test]
    fn sas_is_grouped_in_threes_for_side_by_side_comparison() {
        assert_eq!(group_sas("123456"), "123 456");
        assert_eq!(group_sas("1234567"), "123 456 7");
        assert_eq!(group_sas(" 12 34 "), "123 4");
        assert_eq!(group_sas(""), "");
    }

    #[test]
    fn pair_flow_happy_path_reaches_done_with_the_reported_device_name() {
        let flow = PairFlow::Beginning.began("tok".into(), "123456".into(), "Peer".into());
        assert_eq!(
            flow,
            PairFlow::Verify {
                token: "tok".into(),
                sas: "123456".into(),
                peer_name: "Peer".into()
            }
        );
        let flow = flow.confirming();
        assert!(flow.is_busy());
        assert_eq!(
            flow.finished(true, Some("Living-room PC".into())),
            PairFlow::Done {
                device_name: "Living-room PC".into()
            }
        );
    }

    #[test]
    fn pair_flow_rejection_or_failure_returns_to_the_form() {
        let verify = PairFlow::Verify {
            token: "t".into(),
            sas: "1".into(),
            peer_name: "P".into(),
        };
        assert_eq!(
            verify.clone().confirming().finished(false, None),
            PairFlow::Form
        );
        assert_eq!(verify.failed(), PairFlow::Form);
        assert_eq!(PairFlow::Beginning.failed(), PairFlow::Form);
        // 完成后的迟到失败不推翻结果。
        let done = PairFlow::Done {
            device_name: "D".into(),
        };
        assert_eq!(done.clone().failed(), done);
    }

    #[test]
    fn stale_begin_response_after_user_cancelled_is_ignored() {
        // 用户在 `pairBegin` 在途时取消（回到表单），迟到的成功响应不得复活验证步骤。
        let flow = PairFlow::Form.began("tok".into(), "1".into(), "Peer".into());
        assert_eq!(flow, PairFlow::Form);
        assert!(flow.can_begin());
    }

    #[test]
    fn paired_without_reported_name_falls_back_to_peer_name() {
        let flow = PairFlow::Finishing {
            token: "tok".into(),
            peer_name: "Peer".into(),
        };
        assert_eq!(
            flow.finished(true, None),
            PairFlow::Done {
                device_name: "Peer".into()
            }
        );
    }

    #[test]
    fn session_token_is_kept_while_waiting_for_the_peer_so_it_can_be_aborted() {
        let verify = PairFlow::Verify {
            token: "tok".into(),
            sas: "1".into(),
            peer_name: "P".into(),
        };
        assert_eq!(verify.session_token(), Some("tok"));
        let finishing = verify.confirming();
        assert_eq!(finishing.session_token(), Some("tok"));
        assert_eq!(finishing.finished(true, None).session_token(), None);
        assert_eq!(PairFlow::Form.session_token(), None);
    }

    #[test]
    fn deadline_uses_relative_ttl_and_ignores_host_clock_skew() {
        // 主机时钟比本机快 10 分钟：绝对过期时间早已「过去」，相对 TTL 仍给出完整窗口。
        assert_eq!(local_deadline_ms(30_000, 1_000, 5_000_000), 5_030_000);
        // 旧主机没有 ttlMs（0）：回退主机绝对时间。
        assert_eq!(local_deadline_ms(0, 1_000, 5_000_000), 1_000);
    }

    #[test]
    fn revoked_devices_can_be_paired_again_from_the_discovered_list() {
        let mut revoked = paired("fp-a");
        revoked.revoked = true;
        let entries = discovered_entries(
            &[peer("Alpha", "192.168.1.5", 17800, Some("fp-a"))],
            &[revoked],
        );
        assert!(!entries[0].paired);
    }
}
