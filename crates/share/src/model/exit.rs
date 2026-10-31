//! 退出流程的纯决策：退出时有没有「实时分享仍有接收方没收完」，按退出处理设置决定
//! 直接退出 / 询问 / 自动转为云端暂存；以及转换期间的聚合进度与完成判定。

use fluxdown_protocol::{ErrorReason, ShareExitBehavior, ShareMode, ShareStateDto, ShareStatus};

use super::cloud::{CloudPhase, cloud_phase};

/// 需要处理的分享：仍有效、实时、没有云端副本（从未转过或副本已删）、有接收方在取件。
/// 上次上传已失败的不再算：用户已经看到过失败，退出不再替他重试。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExitCandidates {
    pub share_ids: Vec<String>,
    /// 这些分享里仍在接收的接收方总数。
    pub receivers: usize,
}

/// 退出时应做什么。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExitDecision {
    /// 不拦截，直接退出。
    Quit,
    /// 弹出确认：转为云端暂存后退出 / 直接退出 / 取消。
    Prompt(ExitCandidates),
    /// 不询问，直接转换并等待上传完成。
    Convert(ExitCandidates),
}

/// 找出需要在退出前处理的分享。云端存储未配置、没有账号会话、不能发起分享时没有候选
/// （转换必然被拒，不能因此拦住退出）。
#[must_use]
pub fn exit_candidates(state: &ShareStateDto, signed_in: bool) -> Option<ExitCandidates> {
    if !(state.available && state.can_send && signed_in && state.policy_summary.cloud_enabled) {
        return None;
    }
    let mut share_ids = Vec::new();
    let mut receivers = 0;
    for share in &state.shares {
        if share.status == ShareStatus::Active
            && share.mode == ShareMode::Live
            && matches!(cloud_phase(share), CloudPhase::Idle | CloudPhase::Deleted)
            && !share.active_receivers.is_empty()
        {
            share_ids.push(share.id.clone());
            receivers += share.active_receivers.len();
        }
    }
    (!share_ids.is_empty()).then_some(ExitCandidates {
        share_ids,
        receivers,
    })
}

/// 由状态、登录情况与退出处理设置得出退出决定。
#[must_use]
pub fn decide_exit(
    state: &ShareStateDto,
    signed_in: bool,
    behavior: ShareExitBehavior,
) -> ExitDecision {
    let Some(candidates) = exit_candidates(state, signed_in) else {
        return ExitDecision::Quit;
    };
    match behavior {
        ShareExitBehavior::Quit => ExitDecision::Quit,
        ShareExitBehavior::Prompt => ExitDecision::Prompt(candidates),
        ShareExitBehavior::ConvertToCloud => ExitDecision::Convert(candidates),
    }
}

/// 转换等待的聚合进度。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitWait {
    /// 还有分享在上传：聚合已传 / 总字节与速率，`paused` 表示至少一个暂停中（agent 会自动重试）。
    Uploading {
        done_bytes: u64,
        total_bytes: u64,
        rate_bps: u64,
        paused: bool,
    },
    /// 全部副本已就绪，可以退出。
    Ready,
    /// 至少一个上传失败：不能静默退出，交给用户决定。
    Failed(Option<ErrorReason>),
}

/// 按 hub 里的最新状态汇总 `share_ids` 的转换进度。已从列表消失（分享结束）的分享不再等待。
#[must_use]
pub fn exit_wait(state: &ShareStateDto, share_ids: &[String]) -> ExitWait {
    let mut done_bytes = 0u64;
    let mut total_bytes = 0u64;
    let mut rate_bps = 0u64;
    let mut paused = false;
    let mut pending = false;
    for id in share_ids {
        let Some(share) = state.shares.iter().find(|share| &share.id == id) else {
            continue;
        };
        match cloud_phase(share) {
            CloudPhase::Ready => {
                done_bytes = done_bytes.saturating_add(share.total_bytes);
                total_bytes = total_bytes.saturating_add(share.total_bytes);
            }
            CloudPhase::Failed => {
                return ExitWait::Failed(share.upload.as_ref().and_then(|upload| upload.reason));
            }
            phase => {
                pending = true;
                paused |= phase == CloudPhase::Paused;
                match &share.upload {
                    Some(upload) => {
                        done_bytes = done_bytes.saturating_add(upload.done_bytes);
                        total_bytes = total_bytes.saturating_add(upload.total_bytes);
                        rate_bps = rate_bps.saturating_add(upload.rate_bps);
                    }
                    None => total_bytes = total_bytes.saturating_add(share.total_bytes),
                }
            }
        }
    }
    if pending {
        ExitWait::Uploading {
            done_bytes,
            total_bytes,
            rate_bps,
            paused,
        }
    } else {
        ExitWait::Ready
    }
}

/// 聚合进度的百分比（`0.0..=100.0`）；总量未知为 `None`（显示不定进度）。
#[must_use]
pub fn wait_percent(done_bytes: u64, total_bytes: u64) -> Option<f32> {
    if total_bytes == 0 {
        return None;
    }
    let done = done_bytes.min(total_bytes);
    Some((done as f64 / total_bytes as f64 * 100.) as f32)
}

#[cfg(test)]
mod tests {
    use fluxdown_protocol::{
        ErrorReason, ShareCloudState, ShareDto, ShareExitBehavior, ShareMode,
        SharePolicySummaryDto, ShareReceiverDto, ShareStateDto, ShareStatus, ShareUploadDto,
        ShareUploadStatus,
    };

    use super::{ExitDecision, ExitWait, decide_exit, exit_candidates, exit_wait, wait_percent};

    fn receiving(id: &str) -> ShareDto {
        ShareDto {
            id: id.into(),
            total_bytes: 100,
            active_receivers: vec![ShareReceiverDto::default(), ShareReceiverDto::default()],
            ..ShareDto::default()
        }
    }

    fn state(shares: Vec<ShareDto>) -> ShareStateDto {
        ShareStateDto {
            available: true,
            can_send: true,
            policy_summary: SharePolicySummaryDto {
                cloud_enabled: true,
                ..SharePolicySummaryDto::default()
            },
            shares,
            ..ShareStateDto::default()
        }
    }

    fn with_upload(mut share: ShareDto, status: ShareUploadStatus, done: u64) -> ShareDto {
        share.cloud_state = ShareCloudState::Uploading;
        share.upload = Some(ShareUploadDto {
            status,
            done_bytes: done,
            total_bytes: 100,
            rate_bps: 10,
            reason: None,
        });
        share
    }

    #[test]
    fn no_candidates_means_quit_for_every_behavior() {
        let idle = ShareDto {
            active_receivers: Vec::new(),
            ..receiving("a")
        };
        for behavior in [
            ShareExitBehavior::Prompt,
            ShareExitBehavior::ConvertToCloud,
            ShareExitBehavior::Quit,
        ] {
            assert_eq!(
                decide_exit(&state(vec![idle.clone()]), true, behavior),
                ExitDecision::Quit
            );
            assert_eq!(
                decide_exit(&state(Vec::new()), true, behavior),
                ExitDecision::Quit
            );
        }
    }

    #[test]
    fn candidates_require_active_live_shares_without_a_copy() {
        let mut stopped = receiving("stopped");
        stopped.status = ShareStatus::Stopped;
        let mut cloud = receiving("cloud");
        cloud.mode = ShareMode::Cloud;
        cloud.cloud_state = ShareCloudState::Ready;
        let uploading = with_upload(receiving("uploading"), ShareUploadStatus::Uploading, 1);
        let mut deleted = receiving("deleted");
        deleted.cloud_state = ShareCloudState::Deleted;
        let failed = with_upload(receiving("failed"), ShareUploadStatus::Failed, 1);
        let candidates = exit_candidates(
            &state(vec![
                receiving("live"),
                stopped,
                cloud,
                uploading,
                deleted,
                failed,
            ]),
            true,
        );
        assert_eq!(
            candidates.map(|c| (c.share_ids, c.receivers)),
            Some((vec!["live".to_owned(), "deleted".to_owned()], 4))
        );
    }

    #[test]
    fn exit_is_never_blocked_when_conversion_cannot_succeed() {
        let shares = vec![receiving("a")];
        let base = state(shares);
        assert_eq!(
            decide_exit(&base, false, ShareExitBehavior::Prompt),
            ExitDecision::Quit
        );
        let mut no_storage = base.clone();
        no_storage.policy_summary.cloud_enabled = false;
        assert_eq!(
            decide_exit(&no_storage, true, ShareExitBehavior::ConvertToCloud),
            ExitDecision::Quit
        );
        let mut cannot_send = base.clone();
        cannot_send.can_send = false;
        assert_eq!(
            decide_exit(&cannot_send, true, ShareExitBehavior::Prompt),
            ExitDecision::Quit
        );
        let mut unavailable = base;
        unavailable.available = false;
        assert_eq!(
            decide_exit(&unavailable, true, ShareExitBehavior::Prompt),
            ExitDecision::Quit
        );
    }

    #[test]
    fn behavior_selects_prompt_convert_or_quit() {
        let base = state(vec![receiving("a")]);
        assert!(matches!(
            decide_exit(&base, true, ShareExitBehavior::Prompt),
            ExitDecision::Prompt(c) if c.receivers == 2 && c.share_ids == ["a"]
        ));
        assert!(matches!(
            decide_exit(&base, true, ShareExitBehavior::ConvertToCloud),
            ExitDecision::Convert(c) if c.share_ids == ["a"]
        ));
        assert_eq!(
            decide_exit(&base, true, ShareExitBehavior::Quit),
            ExitDecision::Quit
        );
    }

    #[test]
    fn wait_aggregates_uploads_and_finishes_when_all_ready() {
        let ids = vec!["a".to_owned(), "b".to_owned()];
        let a = with_upload(receiving("a"), ShareUploadStatus::Uploading, 40);
        let b = with_upload(receiving("b"), ShareUploadStatus::Paused, 10);
        assert_eq!(
            exit_wait(&state(vec![a.clone(), b]), &ids),
            ExitWait::Uploading {
                done_bytes: 50,
                total_bytes: 200,
                rate_bps: 20,
                paused: true,
            }
        );
        let mut a_ready = receiving("a");
        a_ready.cloud_state = ShareCloudState::Ready;
        a_ready.mode = ShareMode::Cloud;
        let mut b_ready = receiving("b");
        b_ready.cloud_state = ShareCloudState::Ready;
        assert_eq!(
            exit_wait(&state(vec![a, b_ready.clone()]), &ids),
            ExitWait::Uploading {
                done_bytes: 140,
                total_bytes: 200,
                rate_bps: 10,
                paused: false,
            }
        );
        assert_eq!(
            exit_wait(&state(vec![a_ready, b_ready]), &ids),
            ExitWait::Ready
        );
    }

    #[test]
    fn wait_before_the_first_progress_counts_the_share_total_and_keeps_waiting() {
        let ids = vec!["a".to_owned()];
        // 转换刚返回、hub 还没收到上传状态：仍视为等待（不能误判完成）。
        assert_eq!(
            exit_wait(&state(vec![receiving("a")]), &ids),
            ExitWait::Uploading {
                done_bytes: 0,
                total_bytes: 100,
                rate_bps: 0,
                paused: false,
            }
        );
    }

    #[test]
    fn wait_reports_failure_and_ignores_vanished_shares() {
        let ids = vec!["a".to_owned(), "gone".to_owned()];
        let mut failed = with_upload(receiving("a"), ShareUploadStatus::Failed, 1);
        if let Some(upload) = failed.upload.as_mut() {
            upload.reason = Some(ErrorReason::ShareQuotaExceeded);
        }
        assert_eq!(
            exit_wait(&state(vec![failed]), &ids),
            ExitWait::Failed(Some(ErrorReason::ShareQuotaExceeded))
        );
        // 全部分享都已结束：没有可等的。
        assert_eq!(exit_wait(&state(Vec::new()), &ids), ExitWait::Ready);
    }

    #[test]
    fn percent_clamps_and_is_unknown_without_a_total() {
        assert_eq!(wait_percent(0, 0), None);
        assert_eq!(wait_percent(50, 200), Some(25.));
        assert_eq!(wait_percent(500, 200), Some(100.));
    }
}
