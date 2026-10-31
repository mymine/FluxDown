//! 状态派生：本机网络健康度与摘要行、路径 / NAT / 分享状态的文案键、分享列表排序。

use fluxdown_protocol::{
    ShareAppealStatus, ShareCloudState, ShareDto, ShareNatType, ShareNetStatusDto, SharePathKind,
    ShareProvideState, ShareStatus,
};

/// 本机作为分享方「别人此刻能否找到并直连」的粗分级，驱动状态点颜色与文案。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetHealth {
    /// 端点未绑定（空闲静默，按需启动）。
    Idle,
    /// 已绑定但未连上 home relay / 没有公网映射：别人暂时找不到本机。
    Offline,
    /// 在线，但对称 NAT：直连成功率低，多半走中继。
    RelayOnly,
    /// 在线且具备直连条件。
    Direct,
}

/// 由端点状态推出健康度。
#[must_use]
pub fn net_health(net: &ShareNetStatusDto) -> NetHealth {
    if !net.bound {
        NetHealth::Idle
    } else if !net.online {
        NetHealth::Offline
    } else if net.nat_type == ShareNatType::Symmetric {
        NetHealth::RelayOnly
    } else {
        NetHealth::Direct
    }
}

impl NetHealth {
    /// 状态行文案键。
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Idle => "shareNetHealthIdle",
            Self::Offline => "shareNetHealthOffline",
            Self::RelayOnly => "shareNetHealthRelayOnly",
            Self::Direct => "shareNetHealthDirect",
        }
    }
}

/// 本机网络摘要行的一段。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetSegment {
    Nat(ShareNatType),
    /// 运营商 / 省份（已拼好，如 `中国联通 · 北京`）。
    Carrier(String),
    Ipv6(bool),
    /// home relay 延迟（毫秒）。
    RelayLatency(u64),
}

/// 比较中继 URL：忽略末尾 `/`。
fn same_relay(a: &str, b: &str) -> bool {
    a.trim_end_matches('/') == b.trim_end_matches('/')
}

/// home relay 的延迟（毫秒）；home relay 未知或没有该中继的测量值为 `None`。
#[must_use]
pub fn home_relay_latency(net: &ShareNetStatusDto) -> Option<u64> {
    let home = net.home_relay.as_ref()?;
    net.relay_latency_ms
        .iter()
        .find(|(url, _)| same_relay(url, &home.url))
        .map(|(_, ms)| *ms)
}

/// 「NAT 类型 · 运营商 / 省份 · IPv6 · 首选中继延迟」摘要行的分段；端点未绑定时为空。
#[must_use]
pub fn net_segments(net: &ShareNetStatusDto) -> Vec<NetSegment> {
    if !net.bound {
        return Vec::new();
    }
    let mut segments = vec![NetSegment::Nat(net.nat_type)];
    let isp = net.isp.as_deref().filter(|text| !text.is_empty());
    let province = net.province.as_deref().filter(|text| !text.is_empty());
    let carrier = match (isp, province) {
        (Some(isp), Some(province)) => Some(format!("{isp} · {province}")),
        (Some(text), None) | (None, Some(text)) => Some(text.to_owned()),
        (None, None) => None,
    };
    if let Some(carrier) = carrier {
        segments.push(NetSegment::Carrier(carrier));
    }
    segments.push(NetSegment::Ipv6(net.ipv6));
    if let Some(ms) = home_relay_latency(net) {
        segments.push(NetSegment::RelayLatency(ms));
    }
    segments
}

/// NAT 类型文案键。
#[must_use]
pub const fn nat_key(nat: ShareNatType) -> &'static str {
    match nat {
        ShareNatType::Cone => "shareNatCone",
        ShareNatType::Symmetric => "shareNatSymmetric",
        ShareNatType::Unknown => "shareNatUnknown",
    }
}

/// 传输路径文案键（接收方行、任务来源徽标共用同一套文案）。
#[must_use]
pub const fn path_kind_key(kind: SharePathKind) -> &'static str {
    match kind {
        SharePathKind::DirectLan => "sharePathDirectLan",
        SharePathKind::DirectWan => "sharePathDirectWan",
        SharePathKind::Relay => "sharePathRelay",
        SharePathKind::Cdn => "shareBadgeCloud",
        SharePathKind::Bridge => "shareBadgeBridge",
        SharePathKind::Swarm => "sharePathSwarm",
        SharePathKind::Unknown => "sharePathUnknown",
    }
}

/// 共享状态文案键；未共享（`Off`）不显示。
#[must_use]
pub const fn provide_state_key(state: ShareProvideState) -> Option<&'static str> {
    match state {
        ShareProvideState::Off => None,
        ShareProvideState::Providing => Some("shareProvideActive"),
        ShareProvideState::Ended => Some("shareProvideEnded"),
        ShareProvideState::Stopped => Some("shareProvideStopped"),
    }
}

/// 分享状态文案键。
#[must_use]
pub const fn share_status_key(status: ShareStatus) -> &'static str {
    match status {
        ShareStatus::Active => "shareStatusActive",
        ShareStatus::Expired => "shareStatusExpired",
        ShareStatus::Stopped => "shareStatusStopped",
        ShareStatus::Unknown => "shareStatusUnknown",
        ShareStatus::Takedown => "shareStatusTakedown",
    }
}

/// 「我的分享」列表顺序：有效的在前，其余（已过期 / 已停止 / 未知）在后；同组保持 agent 给出的顺序。
#[must_use]
pub fn ordered_shares(shares: &[ShareDto]) -> Vec<&ShareDto> {
    let mut ordered: Vec<&ShareDto> = shares.iter().collect();
    ordered.sort_by_key(|share| share.status != ShareStatus::Active);
    ordered
}

/// 只有仍有效的分享可以「停止」与就地改设置。
#[must_use]
pub fn is_editable(share: &ShareDto) -> bool {
    share.status == ShareStatus::Active
}

/// 是否显示「文件缺失」：已停止 / 已过期的分享只要源文件缺失就显示；
/// 分享中的仅在没有就绪云端副本（云端副本仍可提供）时显示。
#[must_use]
pub fn source_missing_badge(share: &ShareDto) -> bool {
    share.source_missing
        && (share.status != ShareStatus::Active || share.cloud_state != ShareCloudState::Ready)
}

/// 「我的分享」列表上的批量 / 行内操作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShareAction {
    /// 恢复分享（同一分享，重新分配取件码）：仅已停止 / 已过期且源文件仍在的分享。
    Start,
    /// 停止分享：仅仍有效的分享。
    Stop,
    /// 删除分享记录：任何状态都可以（分享中的由 agent 先停止）。
    Delete,
}

impl ShareAction {
    /// 该操作是否适用于这条分享。
    #[must_use]
    pub fn applies(self, share: &ShareDto) -> bool {
        match self {
            Self::Start => {
                matches!(share.status, ShareStatus::Stopped | ShareStatus::Expired)
                    && !share.source_missing
            }
            Self::Stop => share.status == ShareStatus::Active,
            Self::Delete => true,
        }
    }
}

/// 下架分享的申诉入口状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppealUi {
    /// 已下架且尚未申诉：可发起申诉。
    CanAppeal,
    /// 申诉处理中。
    Pending,
    /// 申诉未通过。
    Rejected,
    /// 不显示申诉相关入口。
    None,
}

/// 由分享状态与申诉记录推导申诉入口。
#[must_use]
pub fn appeal_state(share: &ShareDto) -> AppealUi {
    if share.status != ShareStatus::Takedown {
        return AppealUi::None;
    }
    match &share.appeal {
        None => AppealUi::CanAppeal,
        Some(appeal) => match appeal.status {
            ShareAppealStatus::Pending => AppealUi::Pending,
            ShareAppealStatus::Rejected => AppealUi::Rejected,
            ShareAppealStatus::Approved | ShareAppealStatus::Unknown => AppealUi::None,
        },
    }
}

/// 一组选中分享上各批量操作是否可用（至少有一项适用）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BatchActions {
    pub start: bool,
    pub stop: bool,
    pub delete: bool,
}

/// 汇总选中分享的批量操作可用性；空集合全部不可用。
#[must_use]
pub fn batch_actions<'a>(shares: impl IntoIterator<Item = &'a ShareDto>) -> BatchActions {
    shares
        .into_iter()
        .fold(BatchActions::default(), |acc, share| BatchActions {
            start: acc.start || ShareAction::Start.applies(share),
            stop: acc.stop || ShareAction::Stop.applies(share),
            delete: acc.delete || ShareAction::Delete.applies(share),
        })
}

/// 分享标题行的主文案：只有一个文件时显示该文件名，否则用分享标题。
#[must_use]
pub fn share_headline(share: &ShareDto) -> &str {
    if !share.title.is_empty() {
        &share.title
    } else {
        share.files.first().map_or("", |file| file.name.as_str())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use fluxdown_protocol::{
        ShareAppealDto, ShareAppealStatus, ShareCloudState, ShareDto, ShareFileDto,
        ShareHomeRelayDto, ShareNatType, ShareNetStatusDto, ShareStatus,
    };

    use super::{
        AppealUi, BatchActions, NetHealth, NetSegment, ShareAction, appeal_state, batch_actions,
        home_relay_latency, is_editable, net_health, net_segments, ordered_shares, share_headline,
        share_status_key, source_missing_badge,
    };

    fn net() -> ShareNetStatusDto {
        ShareNetStatusDto {
            bound: true,
            online: true,
            nat_type: ShareNatType::Cone,
            ipv6: true,
            isp: Some("China Unicom".into()),
            province: Some("Beijing".into()),
            home_relay: Some(ShareHomeRelayDto {
                url: "https://relay.example/".into(),
                connected: true,
            }),
            relay_latency_ms: BTreeMap::from([("https://relay.example".to_owned(), 18)]),
            ..ShareNetStatusDto::default()
        }
    }

    #[test]
    fn health_follows_binding_reachability_and_nat() {
        let mut status = net();
        assert_eq!(net_health(&status), NetHealth::Direct);
        status.nat_type = ShareNatType::Unknown;
        assert_eq!(net_health(&status), NetHealth::Direct);
        status.nat_type = ShareNatType::Symmetric;
        assert_eq!(net_health(&status), NetHealth::RelayOnly);
        status.online = false;
        assert_eq!(net_health(&status), NetHealth::Offline);
        status.bound = false;
        assert_eq!(net_health(&status), NetHealth::Idle);
    }

    #[test]
    fn summary_line_lists_nat_carrier_ipv6_and_home_relay_latency() {
        assert_eq!(
            net_segments(&net()),
            vec![
                NetSegment::Nat(ShareNatType::Cone),
                NetSegment::Carrier("China Unicom · Beijing".into()),
                NetSegment::Ipv6(true),
                NetSegment::RelayLatency(18),
            ]
        );
    }

    #[test]
    fn summary_line_omits_unknown_parts_and_is_empty_when_unbound() {
        let mut status = net();
        status.isp = None;
        status.province = Some(String::new());
        status.home_relay = None;
        assert_eq!(
            net_segments(&status),
            vec![NetSegment::Nat(ShareNatType::Cone), NetSegment::Ipv6(true)]
        );
        status.bound = false;
        assert!(net_segments(&status).is_empty());
        assert_eq!(home_relay_latency(&status), None);
    }

    #[test]
    fn active_shares_sort_first_and_keep_relative_order() {
        let make = |id: &str, status| ShareDto {
            id: id.into(),
            status,
            ..ShareDto::default()
        };
        let shares = vec![
            make("a", ShareStatus::Stopped),
            make("b", ShareStatus::Active),
            make("c", ShareStatus::Expired),
            make("d", ShareStatus::Active),
        ];
        let order: Vec<&str> = ordered_shares(&shares)
            .iter()
            .map(|share| share.id.as_str())
            .collect();
        assert_eq!(order, ["b", "d", "a", "c"]);
        assert!(is_editable(&shares[1]));
        assert!(!is_editable(&shares[0]));
    }

    #[test]
    fn takedown_only_allows_delete_and_appeal_follows_record() {
        let mut share = ShareDto {
            status: ShareStatus::Takedown,
            ..ShareDto::default()
        };
        assert!(!ShareAction::Start.applies(&share));
        assert!(!ShareAction::Stop.applies(&share));
        assert!(ShareAction::Delete.applies(&share));
        assert_eq!(share_status_key(share.status), "shareStatusTakedown");
        assert_eq!(appeal_state(&share), AppealUi::CanAppeal);
        let mut appeal = ShareAppealDto {
            status: ShareAppealStatus::Pending,
            ..ShareAppealDto::default()
        };
        share.appeal = Some(appeal.clone());
        assert_eq!(appeal_state(&share), AppealUi::Pending);
        appeal.status = ShareAppealStatus::Rejected;
        share.appeal = Some(appeal.clone());
        assert_eq!(appeal_state(&share), AppealUi::Rejected);
        appeal.status = ShareAppealStatus::Approved;
        share.appeal = Some(appeal);
        assert_eq!(appeal_state(&share), AppealUi::None);
        share.status = ShareStatus::Active;
        share.appeal = None;
        assert_eq!(appeal_state(&share), AppealUi::None);
    }

    #[test]
    fn headline_prefers_title_then_first_file() {
        let mut share = ShareDto {
            title: "Holiday".into(),
            files: vec![ShareFileDto {
                name: "a.zip".into(),
                size: 1,
            }],
            ..ShareDto::default()
        };
        assert_eq!(share_headline(&share), "Holiday");
        share.title.clear();
        assert_eq!(share_headline(&share), "a.zip");
        share.files.clear();
        assert_eq!(share_headline(&share), "");
    }

    #[test]
    fn action_applicability_follows_status_and_source() {
        let make = |status, source_missing| ShareDto {
            status,
            source_missing,
            ..ShareDto::default()
        };
        let cases = [
            // (状态, 源文件缺失, 开始, 停止, 删除)
            (ShareStatus::Active, false, false, true, true),
            (ShareStatus::Active, true, false, true, true),
            (ShareStatus::Stopped, false, true, false, true),
            (ShareStatus::Stopped, true, false, false, true),
            (ShareStatus::Expired, false, true, false, true),
            (ShareStatus::Expired, true, false, false, true),
            (ShareStatus::Unknown, false, false, false, true),
        ];
        for (status, missing, start, stop, delete) in cases {
            let share = make(status, missing);
            assert_eq!(
                ShareAction::Start.applies(&share),
                start,
                "{status:?}/{missing}"
            );
            assert_eq!(
                ShareAction::Stop.applies(&share),
                stop,
                "{status:?}/{missing}"
            );
            assert_eq!(
                ShareAction::Delete.applies(&share),
                delete,
                "{status:?}/{missing}"
            );
        }
    }

    #[test]
    fn badge_ignores_ready_cloud_copy_only_while_active() {
        let make = |status, cloud_state| ShareDto {
            status,
            source_missing: true,
            cloud_state,
            ..ShareDto::default()
        };
        let ready = ShareCloudState::Ready;
        let idle = ShareCloudState::Idle;
        assert!(!source_missing_badge(&make(ShareStatus::Active, ready)));
        assert!(source_missing_badge(&make(ShareStatus::Active, idle)));
        assert!(source_missing_badge(&make(ShareStatus::Stopped, ready)));
        assert!(source_missing_badge(&make(ShareStatus::Expired, idle)));
        assert!(!ShareAction::Start.applies(&make(ShareStatus::Stopped, ready)));
        assert!(!source_missing_badge(&ShareDto::default()));
    }

    #[test]
    fn batch_actions_need_at_least_one_applicable_share() {
        let make = |status, source_missing| ShareDto {
            status,
            source_missing,
            ..ShareDto::default()
        };
        assert_eq!(batch_actions([]), BatchActions::default());
        let only_missing = [make(ShareStatus::Stopped, true)];
        assert_eq!(
            batch_actions(&only_missing),
            BatchActions {
                start: false,
                stop: false,
                delete: true
            }
        );
        let mixed = [
            make(ShareStatus::Stopped, true),
            make(ShareStatus::Expired, false),
        ];
        assert!(batch_actions(&mixed).start);
        assert!(!batch_actions(&mixed).stop);
        let with_active = [
            make(ShareStatus::Active, false),
            make(ShareStatus::Stopped, true),
        ];
        assert_eq!(
            batch_actions(&with_active),
            BatchActions {
                start: false,
                stop: true,
                delete: true
            }
        );
    }
}
