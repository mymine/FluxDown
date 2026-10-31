//! 云端暂存（阶段 E）的纯模型：副本阶段推导、转换 / 删除的可用性、上传进度行、保留期候选。
//!
//! 分享面板、「我的分享」页与退出流程共用同一份判定，保证三处对「能不能转 / 删」口径一致。

use fluxdown_protocol::{
    ErrorReason, ShareCloudState, ShareDto, ShareMode, SharePolicySummaryDto, ShareTierDto,
    ShareUploadStatus,
};

use super::{panel::progress_percent, status::is_editable};

/// 一个分享的云端副本所处阶段。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloudPhase {
    /// 没有云端副本。
    Idle,
    /// 正在上传。
    Uploading,
    /// 上传暂停（agent 自动重试）。
    Paused,
    /// 上传失败，等用户重试。
    Failed,
    /// 副本已就绪。
    Ready,
    /// 副本已被删除（分享方删除、过期清理或后台下架）。
    Deleted,
}

/// 由 `cloudState` 与 `upload` 推出阶段：副本就绪优先；有上传进度按其状态；其余看 `cloudState`。
#[must_use]
pub fn cloud_phase(share: &ShareDto) -> CloudPhase {
    if share.cloud_state == ShareCloudState::Ready {
        return CloudPhase::Ready;
    }
    if let Some(upload) = &share.upload {
        return match upload.status {
            ShareUploadStatus::Uploading => CloudPhase::Uploading,
            ShareUploadStatus::Paused => CloudPhase::Paused,
            ShareUploadStatus::Failed => CloudPhase::Failed,
        };
    }
    match share.cloud_state {
        ShareCloudState::Uploading => CloudPhase::Uploading,
        ShareCloudState::Deleted => CloudPhase::Deleted,
        ShareCloudState::Ready => CloudPhase::Ready,
        ShareCloudState::Idle | ShareCloudState::Unknown => CloudPhase::Idle,
    }
}

impl CloudPhase {
    /// 可以发起（或重试）转换：没有副本、副本已删、上次上传失败。
    #[must_use]
    pub const fn can_start(self) -> bool {
        matches!(self, Self::Idle | Self::Deleted | Self::Failed)
    }

    /// 有副本或上传可删除 / 取消。
    #[must_use]
    pub const fn has_copy(self) -> bool {
        matches!(
            self,
            Self::Uploading | Self::Paused | Self::Failed | Self::Ready
        )
    }

    /// 上传还没结束（进行中 / 暂停 / 失败）：显示进度行。
    #[must_use]
    pub const fn has_upload(self) -> bool {
        matches!(self, Self::Uploading | Self::Paused | Self::Failed)
    }

    /// 状态芯片文案键；没有副本时不显示。
    #[must_use]
    pub const fn chip_key(self) -> Option<&'static str> {
        match self {
            Self::Idle => None,
            Self::Uploading | Self::Paused => Some("shareStageUploading"),
            Self::Failed => Some("shareStageFailed"),
            Self::Ready => Some("shareStageReady"),
            Self::Deleted => Some("shareStageDeleted"),
        }
    }

    /// 状态芯片着色语气。
    #[must_use]
    pub const fn tone(self) -> CloudTone {
        match self {
            Self::Ready => CloudTone::Good,
            Self::Uploading | Self::Paused => CloudTone::Warn,
            Self::Failed => CloudTone::Bad,
            Self::Idle | Self::Deleted => CloudTone::Muted,
        }
    }
}

/// 状态芯片着色语气。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloudTone {
    Good,
    Warn,
    Bad,
    Muted,
}

/// 「转为云端暂存」被禁用的原因（决定悬浮提示）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConvertBlock {
    /// 分享已结束（过期 / 停止）。
    NotActive,
    /// 云端存储未配置。
    Unavailable,
    /// 未登录（游客不能创建云端暂存）。
    SignedOut,
}

impl ConvertBlock {
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::NotActive => "shareStageBlockedNotActive",
            Self::Unavailable => "shareStageBlockedUnavailable",
            Self::SignedOut => "shareStageBlockedSignedOut",
        }
    }
}

/// 一个分享能否转为云端暂存（不含阶段判断，见 [`CloudPhase::can_start`]）。
#[must_use]
pub fn convert_block(
    share: &ShareDto,
    policy: &SharePolicySummaryDto,
    signed_in: bool,
    can_send: bool,
) -> Option<ConvertBlock> {
    if !is_editable(share) {
        Some(ConvertBlock::NotActive)
    } else if !policy.cloud_enabled {
        Some(ConvertBlock::Unavailable)
    } else if !signed_in || !can_send {
        Some(ConvertBlock::SignedOut)
    } else {
        None
    }
}

/// 一个分享的云端动作入口取值。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CloudActions {
    pub phase: CloudPhase,
    /// 「转为云端暂存」（失败时为「重试」）：`Some(None)` 可用，`Some(Some(原因))` 禁用并给原因，
    /// `None` 当前阶段不提供。
    pub convert: Option<Option<ConvertBlock>>,
    /// 「删除云端副本」（上传中为取消）：删除要求已登录。
    pub delete: bool,
}

/// 汇总一个分享当前提供的云端动作。云端存储未配置且没有副本时不提供任何入口（隐藏）。
#[must_use]
pub fn cloud_actions(
    share: &ShareDto,
    policy: &SharePolicySummaryDto,
    signed_in: bool,
    can_send: bool,
) -> CloudActions {
    let phase = cloud_phase(share);
    let convert = (phase.can_start() && (policy.cloud_enabled || phase == CloudPhase::Failed))
        .then(|| convert_block(share, policy, signed_in, can_send));
    CloudActions {
        phase,
        convert,
        delete: phase.has_copy() && signed_in,
    }
}

/// 上传进度行。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UploadLine {
    pub status: ShareUploadStatus,
    pub done_bytes: u64,
    pub total_bytes: u64,
    pub rate_bps: u64,
    /// `0.0..=100.0`，总量未知按 0。
    pub percent: f32,
    pub reason: Option<ErrorReason>,
}

/// 分享当前的上传进度行；没有进行中的上传为 `None`。
#[must_use]
pub fn upload_line(share: &ShareDto) -> Option<UploadLine> {
    let upload = share.upload.as_ref()?;
    let fraction = if upload.total_bytes == 0 {
        0.
    } else {
        upload.done_bytes as f64 / upload.total_bytes as f64
    };
    Some(UploadLine {
        status: upload.status,
        done_bytes: upload.done_bytes,
        total_bytes: upload.total_bytes,
        rate_bps: if upload.status == ShareUploadStatus::Uploading {
            upload.rate_bps
        } else {
            0
        },
        percent: progress_percent(fraction),
        reason: upload.reason,
    })
}

/// 云端暂存的有效期候选所用的档位表：云端暂存分享（或档位 ID 只在保留期表里）用保留期档位，
/// 否则用实时档位。
#[must_use]
pub fn expiry_tiers<'a>(share: &ShareDto, policy: &'a SharePolicySummaryDto) -> &'a [ShareTierDto] {
    let in_retention = policy
        .retention_tiers
        .iter()
        .any(|tier| tier.id == share.tier_id);
    if share.mode == ShareMode::Cloud || in_retention {
        &policy.retention_tiers
    } else {
        &policy.live_tiers
    }
}

/// 转换对话框默认选中的档位下标：取云端给出的第一档。
#[must_use]
pub const fn default_tier_index(tiers: &[ShareTierDto]) -> Option<usize> {
    if tiers.is_empty() { None } else { Some(0) }
}

/// 失败后重试沿用的保留期档位：分享当前档位仍在保留期表里就用它，否则交给云端取默认档。
#[must_use]
pub fn retry_tier<'a>(share: &'a ShareDto, policy: &SharePolicySummaryDto) -> Option<&'a str> {
    policy
        .retention_tiers
        .iter()
        .any(|tier| tier.id == share.tier_id)
        .then_some(share.tier_id.as_str())
}

/// 「我的分享」方式列：方式文案键 + 云端状态后缀（状态键、上传百分比）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MethodLabel {
    pub mode_key: &'static str,
    /// 云端状态文案键；云端暂存已就绪时方式本身已说明，不重复。
    pub state_key: Option<&'static str>,
    /// 上传中 / 暂停时的百分比（`0.0..=100.0`）。
    pub percent: Option<f32>,
}

#[must_use]
pub fn method_label(share: &ShareDto) -> MethodLabel {
    let phase = cloud_phase(share);
    let mode_key = if share.mode == ShareMode::Cloud {
        "shareModeCloud"
    } else {
        "shareModeLive"
    };
    let state_key = if phase == CloudPhase::Ready && share.mode == ShareMode::Cloud {
        None
    } else {
        phase.chip_key()
    };
    let percent = matches!(phase, CloudPhase::Uploading | CloudPhase::Paused)
        .then(|| upload_line(share).map(|line| line.percent))
        .flatten();
    MethodLabel {
        mode_key,
        state_key,
        percent,
    }
}

#[cfg(test)]
mod tests {
    use fluxdown_protocol::{
        ErrorReason, ShareCloudState, ShareDto, ShareMode, SharePolicySummaryDto, ShareStatus,
        ShareTierDto, ShareUploadDto, ShareUploadStatus,
    };

    use super::{
        CloudPhase, CloudTone, ConvertBlock, cloud_actions, cloud_phase, convert_block,
        default_tier_index, expiry_tiers, method_label, retry_tier, upload_line,
    };

    fn share() -> ShareDto {
        ShareDto {
            id: "s1".into(),
            tier_id: "24h".into(),
            ..ShareDto::default()
        }
    }

    fn uploading(status: ShareUploadStatus, done: u64, total: u64) -> ShareDto {
        ShareDto {
            cloud_state: ShareCloudState::Uploading,
            upload: Some(ShareUploadDto {
                status,
                done_bytes: done,
                total_bytes: total,
                rate_bps: 100,
                reason: None,
            }),
            ..share()
        }
    }

    fn policy(cloud: bool) -> SharePolicySummaryDto {
        SharePolicySummaryDto {
            cloud_enabled: cloud,
            live_tiers: vec![ShareTierDto {
                id: "24h".into(),
                hours: 24,
            }],
            retention_tiers: vec![
                ShareTierDto {
                    id: "cloud-24h".into(),
                    hours: 24,
                },
                ShareTierDto {
                    id: "cloud-7d".into(),
                    hours: 168,
                },
            ],
            ..SharePolicySummaryDto::default()
        }
    }

    #[test]
    fn phase_follows_upload_then_cloud_state() {
        assert_eq!(cloud_phase(&share()), CloudPhase::Idle);
        assert_eq!(
            cloud_phase(&uploading(ShareUploadStatus::Uploading, 1, 2)),
            CloudPhase::Uploading
        );
        assert_eq!(
            cloud_phase(&uploading(ShareUploadStatus::Paused, 1, 2)),
            CloudPhase::Paused
        );
        assert_eq!(
            cloud_phase(&uploading(ShareUploadStatus::Failed, 1, 2)),
            CloudPhase::Failed
        );
        let mut ready = uploading(ShareUploadStatus::Uploading, 2, 2);
        ready.cloud_state = ShareCloudState::Ready;
        assert_eq!(cloud_phase(&ready), CloudPhase::Ready);
        let deleted = ShareDto {
            cloud_state: ShareCloudState::Deleted,
            ..share()
        };
        assert_eq!(cloud_phase(&deleted), CloudPhase::Deleted);
        let no_progress_yet = ShareDto {
            cloud_state: ShareCloudState::Uploading,
            ..share()
        };
        assert_eq!(cloud_phase(&no_progress_yet), CloudPhase::Uploading);
    }

    #[test]
    fn phase_capabilities() {
        assert!(CloudPhase::Idle.can_start());
        assert!(CloudPhase::Deleted.can_start());
        assert!(CloudPhase::Failed.can_start());
        assert!(!CloudPhase::Uploading.can_start());
        assert!(!CloudPhase::Ready.can_start());
        assert!(!CloudPhase::Idle.has_copy());
        assert!(!CloudPhase::Deleted.has_copy());
        assert!(CloudPhase::Paused.has_copy());
        assert!(CloudPhase::Ready.has_copy());
        assert!(CloudPhase::Failed.has_upload());
        assert!(!CloudPhase::Ready.has_upload());
        assert_eq!(CloudPhase::Idle.chip_key(), None);
        assert_eq!(CloudPhase::Paused.chip_key(), Some("shareStageUploading"));
        assert_eq!(CloudPhase::Ready.tone(), CloudTone::Good);
    }

    #[test]
    fn convert_block_orders_reasons() {
        let mut ended = share();
        ended.status = ShareStatus::Stopped;
        assert_eq!(
            convert_block(&ended, &policy(false), false, false),
            Some(ConvertBlock::NotActive)
        );
        assert_eq!(
            convert_block(&share(), &policy(false), true, true),
            Some(ConvertBlock::Unavailable)
        );
        assert_eq!(
            convert_block(&share(), &policy(true), false, true),
            Some(ConvertBlock::SignedOut)
        );
        assert_eq!(
            convert_block(&share(), &policy(true), true, false),
            Some(ConvertBlock::SignedOut)
        );
        assert_eq!(convert_block(&share(), &policy(true), true, true), None);
    }

    #[test]
    fn actions_hide_convert_without_storage_and_gate_delete_on_sign_in() {
        // 存储未配置且没有副本：不提供转换入口。
        assert_eq!(
            cloud_actions(&share(), &policy(false), true, true).convert,
            None
        );
        // 可用。
        assert_eq!(
            cloud_actions(&share(), &policy(true), true, true).convert,
            Some(None)
        );
        // 游客：入口显示但禁用。
        assert_eq!(
            cloud_actions(&share(), &policy(true), false, true).convert,
            Some(Some(ConvertBlock::SignedOut))
        );
        // 上传中：不提供转换，可取消（删除）。
        let busy = uploading(ShareUploadStatus::Uploading, 1, 2);
        let actions = cloud_actions(&busy, &policy(true), true, true);
        assert_eq!(actions.convert, None);
        assert!(actions.delete);
        assert!(!cloud_actions(&busy, &policy(true), false, true).delete);
        // 失败：即使存储配置被关也允许重试入口（给出禁用原因）。
        let failed = uploading(ShareUploadStatus::Failed, 1, 2);
        assert_eq!(
            cloud_actions(&failed, &policy(false), true, true).convert,
            Some(Some(ConvertBlock::Unavailable))
        );
        // 无副本无上传：没有可删除的。
        assert!(!cloud_actions(&share(), &policy(true), true, true).delete);
    }

    #[test]
    fn upload_line_reports_percent_and_zeroes_rate_when_not_uploading() {
        assert_eq!(upload_line(&share()), None);
        let line = upload_line(&uploading(ShareUploadStatus::Uploading, 25, 100));
        assert_eq!(
            line.map(|l| (l.done_bytes, l.total_bytes, l.rate_bps)),
            Some((25, 100, 100))
        );
        assert_eq!(line.map(|l| l.percent), Some(25.));
        let mut paused = uploading(ShareUploadStatus::Paused, 25, 100);
        if let Some(upload) = paused.upload.as_mut() {
            upload.reason = Some(ErrorReason::ShareCloudUnavailable);
        }
        let line = upload_line(&paused);
        assert_eq!(line.map(|l| l.rate_bps), Some(0));
        assert_eq!(
            line.and_then(|l| l.reason),
            Some(ErrorReason::ShareCloudUnavailable)
        );
        // 总量未知。
        assert_eq!(
            upload_line(&uploading(ShareUploadStatus::Uploading, 5, 0)).map(|l| l.percent),
            Some(0.)
        );
    }

    #[test]
    fn expiry_tiers_prefer_retention_for_cloud_shares() {
        let policy = policy(true);
        assert_eq!(
            expiry_tiers(&share(), &policy),
            policy.live_tiers.as_slice()
        );
        let cloud = ShareDto {
            mode: ShareMode::Cloud,
            tier_id: "cloud-7d".into(),
            ..share()
        };
        assert_eq!(
            expiry_tiers(&cloud, &policy),
            policy.retention_tiers.as_slice()
        );
        let converting = ShareDto {
            tier_id: "cloud-24h".into(),
            ..share()
        };
        assert_eq!(
            expiry_tiers(&converting, &policy),
            policy.retention_tiers.as_slice()
        );
    }

    #[test]
    fn default_tier_is_the_first_offered() {
        assert_eq!(default_tier_index(&[]), None);
        assert_eq!(default_tier_index(&policy(true).retention_tiers), Some(0));
    }

    #[test]
    fn retry_keeps_the_tier_only_when_it_is_a_retention_tier() {
        let policy = policy(true);
        let kept = ShareDto {
            tier_id: "cloud-7d".into(),
            ..share()
        };
        assert_eq!(retry_tier(&kept, &policy), Some("cloud-7d"));
        assert_eq!(retry_tier(&share(), &policy), None);
    }

    #[test]
    fn method_label_adds_cloud_state_and_upload_percent() {
        let live = method_label(&share());
        assert_eq!(
            (live.mode_key, live.state_key, live.percent),
            ("shareModeLive", None, None)
        );
        let busy = method_label(&uploading(ShareUploadStatus::Uploading, 25, 100));
        assert_eq!(
            (busy.mode_key, busy.state_key, busy.percent),
            ("shareModeLive", Some("shareStageUploading"), Some(25.))
        );
        let failed = method_label(&uploading(ShareUploadStatus::Failed, 25, 100));
        assert_eq!(
            (failed.state_key, failed.percent),
            (Some("shareStageFailed"), None)
        );
        let ready = ShareDto {
            mode: ShareMode::Cloud,
            cloud_state: ShareCloudState::Ready,
            ..share()
        };
        let label = method_label(&ready);
        assert_eq!((label.mode_key, label.state_key), ("shareModeCloud", None));
        let deleted = ShareDto {
            cloud_state: ShareCloudState::Deleted,
            ..share()
        };
        assert_eq!(method_label(&deleted).state_key, Some("shareStageDeleted"));
    }
}
