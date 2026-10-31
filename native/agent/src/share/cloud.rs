//! FluxCloud `/share/*` 控制面的线上形态（`docs/temp-share-design.md` §9.2）与错误映射。

use fluxdown_protocol::{
    ApplicationErrorCode, ErrorReason, RpcErrorData, ShareCloudState, ShareFileDto, ShareMode,
    SharePolicySummaryDto, ShareRelayDto, ShareStatus, ShareTierDto,
};
use fluxdown_share::{NetworkPolicy, ShareFile};
use serde::{Deserialize, Serialize};

use crate::cloud::CloudError;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CloudTier {
    pub id: String,
    #[serde(default)]
    pub hours: u32,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub default: bool,
}

fn yes() -> bool {
    true
}

/// `GET /share/config` 的 `swarm` 段。
#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CloudSwarmPolicy {
    /// 提供方为优先请求保留的连接槽位数。
    #[serde(default)]
    pub priority_reserved_slots: u32,
    /// 每日共享上传上限的云端缺省值（字节，0 = 不限）。
    #[serde(default)]
    pub provide_daily_cap_default_bytes: u64,
}

/// `GET /share/config`。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CloudConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub guest_receive: bool,
    #[serde(default)]
    pub guest_bridge: bool,
    #[serde(default)]
    pub guest_share: bool,
    #[serde(default)]
    pub pickup_code_length: u8,
    #[serde(default)]
    pub live_tiers: Vec<CloudTier>,
    /// 云端暂存的保留期档位（`cloud-…`，≤ 336 小时）。
    #[serde(default)]
    pub retention_tiers: Vec<CloudTier>,
    /// 云端存储已配置；`false` 时不提供「转为云端暂存」。
    #[serde(default)]
    pub cloud_enabled: bool,
    pub network: NetworkPolicy,
    /// 共享加速策略（旧服务端没有该字段：全 0，即现状）。
    #[serde(default)]
    pub swarm: CloudSwarmPolicy,
}

impl CloudConfig {
    pub(super) fn summary(&self) -> SharePolicySummaryDto {
        SharePolicySummaryDto {
            enabled: self.enabled,
            guest_receive: self.guest_receive,
            guest_bridge: self.guest_bridge,
            guest_share: self.guest_share,
            pickup_code_length: self.pickup_code_length,
            live_tiers: self
                .live_tiers
                .iter()
                .filter(|tier| tier.enabled)
                .map(|tier| ShareTierDto {
                    id: tier.id.clone(),
                    hours: tier.hours,
                })
                .collect(),
            retention_tiers: self
                .retention_tiers
                .iter()
                .filter(|tier| tier.enabled)
                .map(|tier| ShareTierDto {
                    id: tier.id.clone(),
                    hours: tier.hours,
                })
                .collect(),
            cloud_enabled: self.cloud_enabled,
            relays: self
                .network
                .relays
                .iter()
                .map(|relay| ShareRelayDto {
                    url: relay.url.clone(),
                    region: relay.region.clone(),
                })
                .collect(),
            hole_punch_hint_secs: self.network.hole_punch_hint_secs,
            swarm_priority_reserved_slots: self.swarm.priority_reserved_slots,
            provide_daily_cap_default_bytes: self.swarm.provide_daily_cap_default_bytes,
        }
    }

    /// 新建分享缺省的档位：本机默认 > 云端标记的默认 > 第一个启用的档位。
    pub(super) fn default_tier(&self, preferred: Option<&str>) -> Option<String> {
        let enabled = || self.live_tiers.iter().filter(|tier| tier.enabled);
        preferred
            .and_then(|id| enabled().find(|tier| tier.id == id))
            .or_else(|| enabled().find(|tier| tier.default))
            .or_else(|| enabled().next())
            .map(|tier| tier.id.clone())
    }

    /// 云端暂存缺省的保留期档位：调用方指定 > 云端标记的默认 > 第一个启用的档位。
    pub(super) fn default_retention(&self, preferred: Option<&str>) -> Option<String> {
        let enabled = || self.retention_tiers.iter().filter(|tier| tier.enabled);
        preferred
            .and_then(|id| enabled().find(|tier| tier.id == id))
            .or_else(|| enabled().find(|tier| tier.default))
            .or_else(|| enabled().next())
            .map(|tier| tier.id.clone())
    }

    /// `id` 是启用中的云端暂存保留期档位。
    pub(super) fn has_retention(&self, id: &str) -> bool {
        self.retention_tiers
            .iter()
            .any(|tier| tier.enabled && tier.id == id)
    }
}

/// `GET /share/keys`。
pub(super) type CloudKeys = fluxdown_share::KeysResponse;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CreateBody<'a> {
    pub title: &'a str,
    pub files: &'a [ShareFile],
    pub tier_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_claims: Option<u32>,
    pub notify_on_claim: bool,
    pub endpoint_id: &'a str,
    /// 加入共享（多设备按内容互相取块）。
    pub swarm: bool,
}

/// `ShareDto`（云端）。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CloudShare {
    pub id: String,
    #[serde(default)]
    pub slug: String,
    #[serde(default)]
    pub pickup_code: Option<String>,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub tier_id: String,
    #[serde(default)]
    pub expires_at: Option<String>,
    #[serde(default)]
    pub max_claims: Option<i64>,
    #[serde(default)]
    pub claim_count: i64,
    #[serde(default)]
    pub notify_on_claim: bool,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub last_accessed_at: Option<String>,
    /// `none | uploading | ready | deleted`（云端暂存副本）。
    #[serde(default)]
    pub cloud_state: String,
    #[serde(default)]
    pub cloud_bytes: i64,
    /// 加入共享；旧云端不带。
    #[serde(default)]
    pub swarm: bool,
    /// 管理员已关闭并锁定该分享的共享；旧云端不带。
    #[serde(default)]
    pub swarm_locked: bool,
    /// 下架原因（仅 takedown 状态带）。
    #[serde(default)]
    pub takedown_reason: Option<String>,
    /// 下架申诉（提交过才有）。
    #[serde(default)]
    pub appeal: Option<fluxdown_protocol::ShareAppealDto>,
}

impl CloudShare {
    pub(super) fn mode(&self) -> ShareMode {
        if self.mode == "cloud" {
            ShareMode::Cloud
        } else {
            ShareMode::Live
        }
    }

    pub(super) fn cloud_state(&self) -> ShareCloudState {
        match self.cloud_state.as_str() {
            "" | "none" => ShareCloudState::Idle,
            "uploading" => ShareCloudState::Uploading,
            "ready" => ShareCloudState::Ready,
            "deleted" => ShareCloudState::Deleted,
            _ => ShareCloudState::Unknown,
        }
    }

    pub(super) fn status(&self) -> ShareStatus {
        match self.status.as_str() {
            "active" => ShareStatus::Active,
            "expired" => ShareStatus::Expired,
            "stopped" => ShareStatus::Stopped,
            "takedown" => ShareStatus::Takedown,
            _ => ShareStatus::Unknown,
        }
    }
}

#[derive(Debug, Deserialize)]
pub(super) struct SharesList {
    #[serde(default)]
    pub shares: Vec<CloudShare>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ResolveFile {
    pub name: String,
    pub size: i64,
    #[serde(default)]
    pub mtime_ms: i64,
    #[serde(default)]
    pub chunk_hashes: Vec<String>,
    #[serde(default)]
    pub root_hash: String,
}

impl ResolveFile {
    pub(super) fn to_share_file(&self) -> ShareFile {
        ShareFile {
            name: self.name.clone(),
            size: u64::try_from(self.size).unwrap_or(0),
            mtime_ms: self.mtime_ms,
            chunk_hashes: self.chunk_hashes.clone(),
            root_hash: self.root_hash.clone(),
        }
    }

    pub(super) fn dto(&self) -> ShareFileDto {
        ShareFileDto {
            name: self.name.clone(),
            size: u64::try_from(self.size).unwrap_or(0),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ResolveShare {
    pub id: String,
    #[serde(default)]
    pub title: String,
    pub files: Vec<ResolveFile>,
    #[serde(default)]
    pub sender_device_name: String,
    pub sender_endpoint_id: String,
    #[serde(default)]
    pub expires_at: Option<String>,
    /// 云端副本状态（`none|uploading|ready|deleted`）；旧云端不带。
    #[serde(default)]
    pub cloud_state: Option<String>,
    /// 分享方设备此刻是否在线（云端知道时才带）。
    #[serde(default)]
    pub sender_online: Option<bool>,
    /// 云端此刻能列出的其他在线持有该内容的设备数；旧云端不带。
    #[serde(default)]
    pub swarm_peers: Option<u32>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ResolveResponse {
    pub share: ResolveShare,
    pub ticket: String,
    #[serde(default)]
    pub guest_token: Option<String>,
}

/// `POST /share/swarm/peers` 响应。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PeersResponse {
    #[serde(default)]
    pub files: Vec<PeersFile>,
    #[serde(default)]
    pub refresh_secs: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PeersFile {
    pub file_index: u32,
    #[allow(dead_code)]
    pub root_hash: String,
    pub ticket: String,
    #[serde(default)]
    pub peers: Vec<CloudPeer>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CloudPeer {
    pub endpoint_id: String,
    #[serde(default)]
    pub complete: bool,
}

/// `POST /share/swarm/announce` 响应（顺序与请求的 `entries` 一致）。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AnnounceResponse {
    #[serde(default)]
    pub entries: Vec<AnnounceResult>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AnnounceResult {
    pub root_hash: String,
    pub state: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GeoRecord {
    #[serde(default)]
    pub province: Option<String>,
    #[serde(default)]
    pub isp: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct NetResponse {
    #[serde(default)]
    pub global_v4_geo: Option<GeoRecord>,
    #[serde(default)]
    pub global_v6_geo: Option<GeoRecord>,
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct SessionResponse {
    #[serde(default)]
    pub killed: bool,
}

// ───────────── 云端暂存上传（`docs/temp-share-design.md` §10） ─────────────

/// `POST /share/shares/{id}/upload` 里的一个文件。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UploadFileRef<'a> {
    pub index: u32,
    pub root_hash: &'a str,
    pub size: u64,
}

/// `POST /share/shares/{id}/upload` 请求体；`tier_id` 只在转为暂存时带（云端据此设定保留期）。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UploadPlanBody<'a> {
    pub files: Vec<UploadFileRef<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tier_id: Option<&'a str>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(super) enum UploadFileState {
    /// 云端已有完整副本。
    Done,
    /// 需要（继续）上传 `parts`。
    NeedUpload,
    /// 同用户已有同根哈希对象：先做持有性证明。
    Challenge,
    #[default]
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub(super) struct UploadChallenge {
    pub offset: u64,
    pub len: u64,
}

/// 一个待上传分片的预签名 PUT 地址（编号从 1 起）。
#[derive(Clone, Debug, Deserialize)]
pub(super) struct UploadPart {
    pub number: u32,
    pub url: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UploadFilePlan {
    pub index: u32,
    pub state: UploadFileState,
    #[serde(default)]
    pub challenge: Option<UploadChallenge>,
    #[serde(default)]
    pub upload_id: Option<String>,
    /// 每个分片的字节数（最后一片除外）。
    #[serde(default)]
    pub part_bytes: u64,
    /// 只含缺失的分片。
    #[serde(default)]
    pub parts: Vec<UploadPart>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub(super) struct UploadPlan {
    #[serde(default)]
    pub files: Vec<UploadFilePlan>,
}

/// `POST …/upload/{index}/proof`。
#[derive(Debug, Serialize)]
pub(super) struct ProofBody {
    pub offset: u64,
    pub len: u64,
    pub blake3: String,
}

/// `POST …/upload/{index}/complete`：云端以 ListParts 为准，已知 etag 的子集即可。
#[derive(Debug, Serialize)]
pub(super) struct CompleteBody<'a> {
    pub parts: &'a [super::upload_state::DonePart],
}

/// 云端错误 → 稳定的 RPC 错误（FluxMesh专属原因优先，其余沿用通用映射）。
pub(super) fn share_error(error: &CloudError) -> RpcErrorData {
    use ApplicationErrorCode as Code;
    let mapped = match (error.code.as_deref(), error.status) {
        (Some("share_ended"), _) => Some((Code::Conflict, false, ErrorReason::ShareExpired)),
        (Some("traffic_exhausted"), _) => {
            Some((Code::Conflict, false, ErrorReason::ShareTrafficExhausted))
        }
        (Some("share_storage_unavailable"), _) => {
            Some((Code::Unavailable, true, ErrorReason::ShareCloudUnavailable))
        }
        (Some("share_cloud_not_ready"), _) => {
            Some((Code::Conflict, false, ErrorReason::ShareCloudNotReady))
        }
        (Some("share_swarm_locked"), _) => {
            Some((Code::Conflict, false, ErrorReason::ShareSwarmLocked))
        }
        (Some("share_takedown"), _) => Some((Code::Conflict, false, ErrorReason::ShareStopped)),
        (Some("share_claims_exhausted" | "share_quota_exceeded" | "share_not_entitled"), _) => {
            Some((Code::Conflict, false, ErrorReason::ShareQuotaExceeded))
        }
        (Some("turnstile_required" | "turnstile_failed"), _) => Some((
            Code::Unauthorized,
            false,
            ErrorReason::ShareTurnstileRequired,
        )),
        (Some("share_disabled" | "share_guest_disabled"), _) => {
            Some((Code::Unavailable, false, ErrorReason::ShareUnavailable))
        }
        (Some("share_code_exhausted"), _) => {
            Some((Code::Unavailable, true, ErrorReason::ShareRateLimited))
        }
        (Some("rate_limited"), _) | (_, Some(429)) if !error.unreachable => {
            Some((Code::Unavailable, true, ErrorReason::ShareRateLimited))
        }
        (Some("not_found"), _) | (_, Some(404)) => {
            Some((Code::NotFound, false, ErrorReason::ShareNotFound))
        }
        (_, Some(410)) => Some((Code::Conflict, false, ErrorReason::ShareStopped)),
        (Some("swarm_quota_exhausted"), _) => {
            Some((Code::Conflict, false, ErrorReason::ShareSwarmQuotaExhausted))
        }
        (Some("swarm_login_required"), _) => Some((
            Code::Unauthorized,
            false,
            ErrorReason::ShareSwarmLoginRequired,
        )),
        (Some("share_strike_limited"), _) => {
            let mut data = RpcErrorData::new(Code::Conflict, false)
                .with_reason(ErrorReason::ShareStrikeLimited);
            data.field = error
                .details
                .as_ref()
                .and_then(|d| d.get("until"))
                .and_then(|v| v.as_str())
                .map(str::to_owned);
            return data;
        }
        (Some("share_appeal_exists"), _) => {
            Some((Code::Conflict, false, ErrorReason::ShareAppealExists))
        }
        _ => None,
    };
    match mapped {
        Some((code, retryable, reason)) => RpcErrorData::new(code, retryable).with_reason(reason),
        None => error.to_rpc_error(),
    }
}

/// 文件名可安全用作任务名：替换路径分隔符与控制字符，空名回落。
pub(super) fn safe_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.');
    if trimmed.is_empty() {
        "file".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// 6 位取件码（仅数字）还是分享链接：返回 `(code, slug)` 之一。
pub(super) fn parse_code_or_link(input: &str) -> Option<(Option<String>, Option<String>)> {
    let text = input.trim();
    if text.len() >= 4 && text.len() <= 12 && text.bytes().all(|b| b.is_ascii_digit()) {
        return Some((Some(text.to_owned()), None));
    }
    fluxdown_protocol::capture_link::share_ref(text).map(|share| (None, Some(share.slug)))
}
