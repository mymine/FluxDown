//! 退出处理：`exitBehavior = convertToCloud` 时，完全退出前把仍有接收方在取件的实时分享转为云端暂存。
//!
//! GPUI 的退出流程（菜单 / 快捷键 / 关主窗口）会自己弹进度框并等副本就绪后再请求退出；
//! 这里覆盖没有界面参与的退出（托盘「退出」、`system.shutdown`、SIGTERM 走的完全退出路径）。
//! 两者不冲突：副本已在上传或已就绪的分享不再是候选，转换调用本身对「已在上传」幂等。
//!
//! 等待有上限：超时仍退出，断点文件留在本机，下次启动自动续传——上传没传完时接收方只能等分享方回来，
//! 不能因此让退出无限期卡住。

use std::collections::HashMap;
use std::time::{Duration, Instant};

use fluxdown_protocol::{
    ShareCloudState, ShareConvertToCloudParams, ShareExitBehavior, ShareMode, ShareStatus,
    ShareUploadStatus,
};
use fluxdown_share::SessionRole;

use super::service::{SessionMeta, ShareService};
use super::store::StoredShare;

/// 退出前等上传完成的上限。
const EXIT_UPLOAD_BUDGET: Duration = Duration::from_secs(120);
/// 等待期间查看进度的间隔（一次性、仅在退出流程里）。
const EXIT_POLL: Duration = Duration::from_millis(250);

/// 需要在退出前转为暂存的分享：有效、实时、没有云端副本，且此刻仍有接收方在取件。
pub(super) fn exit_candidates(
    shares: &[StoredShare],
    sessions: &HashMap<String, SessionMeta>,
) -> Vec<String> {
    shares
        .iter()
        .filter(|stored| {
            // 游客分享没有云端暂存。
            !stored.guest_owned
                && stored.dto.status == ShareStatus::Active
                && stored.dto.mode == ShareMode::Live
                && matches!(
                    stored.dto.cloud_state,
                    ShareCloudState::Idle | ShareCloudState::Deleted
                )
                && sessions.values().any(|meta| {
                    meta.role == SessionRole::Sender
                        && !meta.ended
                        && meta.share_id == stored.dto.id
                })
        })
        .map(|stored| stored.dto.id.clone())
        .collect()
}

impl ShareService {
    /// 完全退出前的收尾；只在设置为「自动转为云端暂存」时动作，任何一步失败都不阻止退出。
    pub(super) async fn convert_before_quit(&self) {
        if !self.is_enabled()
            || self.with(|i| i.local.exit_behavior) != ShareExitBehavior::ConvertToCloud
            || !self.cloud.has_account().await
        {
            return;
        }
        let ids = self.with(|i| exit_candidates(&i.shares, &i.sessions));
        let mut started = Vec::with_capacity(ids.len());
        for id in ids {
            let params = ShareConvertToCloudParams {
                id: id.clone(),
                tier_id: None,
            };
            match self.convert_to_cloud(params).await {
                Ok(_) => started.push(id),
                Err(error) => {
                    tracing::warn!(reason = ?error.reason, "could not convert a share to cloud storage before quitting");
                }
            }
        }
        if started.is_empty() {
            return;
        }
        tracing::info!(
            shares = started.len(),
            "waiting for cloud uploads before quitting"
        );
        let deadline = Instant::now() + EXIT_UPLOAD_BUDGET;
        while Instant::now() < deadline {
            let has_account = self.cloud.has_account().await;
            let pending = self.with(|i| {
                let state = i.state_dto(has_account);
                state.shares.iter().any(|share| {
                    started.contains(&share.id)
                        && share.cloud_state != ShareCloudState::Ready
                        && share
                            .upload
                            .as_ref()
                            .is_none_or(|upload| upload.status != ShareUploadStatus::Failed)
                })
            });
            if !pending {
                return;
            }
            tokio::time::sleep(EXIT_POLL).await;
        }
        tracing::warn!(
            "cloud uploads did not finish before the quit deadline; they resume at the next start"
        );
    }
}

#[async_trait::async_trait]
impl crate::lifecycle::BeforeQuit for ShareService {
    async fn before_quit(&self) {
        self.convert_before_quit().await;
    }
}

#[cfg(test)]
mod tests {
    use fluxdown_protocol::ShareDto;
    use fluxdown_share::{PathKind, ShareManifest};

    use super::*;

    fn stored(
        id: &str,
        status: ShareStatus,
        mode: ShareMode,
        cloud: ShareCloudState,
    ) -> StoredShare {
        StoredShare {
            dto: ShareDto {
                id: id.to_owned(),
                status,
                mode,
                cloud_state: cloud,
                ..ShareDto::default()
            },
            manifest: ShareManifest {
                share_id: id.to_owned(),
                files: Vec::new(),
            },
            paths: Vec::new(),
            guest_owned: false,
        }
    }

    fn session(share: &str, role: SessionRole, ended: bool) -> SessionMeta {
        SessionMeta {
            share_id: share.to_owned(),
            role,
            peer: String::new(),
            path: PathKind::Unknown,
            rate_bps: 0,
            bytes: 0,
            total_bytes: 0,
            started_at: String::new(),
            device_name: String::new(),
            ended,
            relay_stuck: false,
        }
    }

    #[test]
    fn only_live_active_shares_with_a_running_receiver_are_converted() {
        let shares = vec![
            stored(
                "busy",
                ShareStatus::Active,
                ShareMode::Live,
                ShareCloudState::Idle,
            ),
            stored(
                "idle",
                ShareStatus::Active,
                ShareMode::Live,
                ShareCloudState::Idle,
            ),
            stored(
                "done",
                ShareStatus::Active,
                ShareMode::Live,
                ShareCloudState::Idle,
            ),
            stored(
                "cloud",
                ShareStatus::Active,
                ShareMode::Cloud,
                ShareCloudState::Ready,
            ),
            stored(
                "uploading",
                ShareStatus::Active,
                ShareMode::Live,
                ShareCloudState::Uploading,
            ),
            stored(
                "stopped",
                ShareStatus::Stopped,
                ShareMode::Live,
                ShareCloudState::Idle,
            ),
            stored(
                "redo",
                ShareStatus::Active,
                ShareMode::Live,
                ShareCloudState::Deleted,
            ),
        ];
        let mut sessions = HashMap::new();
        for id in ["busy", "cloud", "uploading", "stopped", "redo"] {
            sessions.insert(format!("s-{id}"), session(id, SessionRole::Sender, false));
        }
        // 已结束的接收方、本机作为接收方的会话都不算。
        sessions.insert("s-done".into(), session("done", SessionRole::Sender, true));
        sessions.insert(
            "s-idle".into(),
            session("idle", SessionRole::Receiver, false),
        );
        let mut got = exit_candidates(&shares, &sessions);
        got.sort();
        assert_eq!(got, ["busy", "redo"]);
    }
}
