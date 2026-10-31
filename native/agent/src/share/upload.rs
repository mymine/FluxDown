//! 云端暂存（`agent.share.convertToCloud` / `deleteCloudCopy` / 重启续传）在 `ShareService` 里的装配。
//! 上传本身（方案、分片、断点）见 `upload_job.rs`，断点文件见 `upload_state.rs`。
//!
//! 每个分享至多一个上传任务，登记在 `Inner::uploads`；条目的 `progress` 只在内存里，经 `state_dto`
//! 叠加到 `ShareDto.upload`（不落盘）。条目随上传完成 / 取消 / 删除副本移除，失败的条目保留
//! （`status = Failed`，UI 提供重试）。空闲时没有任何条目、任务与定时器。

use std::sync::Arc;

use fluxdown_protocol::{
    ApplicationErrorCode, ErrorReason, RpcErrorData, ShareCloudState, ShareConvertToCloudParams,
    ShareDto, ShareMode, ShareStatus, ShareUploadDto, ShareUploadStatus,
};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use super::cloud::{CloudShare, UploadPlan, share_error};
use super::lifecycle::apply_cloud;
use super::service::{Inner, ShareService, invalid};
use super::store::StoredShare;
use super::upload_job::{Job, JobFile, JobParams, Outcome, request_plan};

/// 一个分享的上传登记。
pub(super) struct UploadEntry {
    pub cancel: CancellationToken,
    /// 任务身份（`Arc::ptr_eq`）+ 唤醒暂停中的退避睡眠。
    pub wake: Arc<Notify>,
    pub progress: ShareUploadDto,
}

fn failure(code: ApplicationErrorCode, retryable: bool, reason: ErrorReason) -> RpcErrorData {
    RpcErrorData::new(code, retryable).with_reason(reason)
}

fn cloud_unavailable() -> RpcErrorData {
    failure(
        ApplicationErrorCode::Unavailable,
        false,
        ErrorReason::ShareCloudUnavailable,
    )
}

fn file_changed() -> RpcErrorData {
    failure(
        ApplicationErrorCode::Conflict,
        false,
        ErrorReason::ShareFileChanged,
    )
}

fn not_found() -> RpcErrorData {
    failure(
        ApplicationErrorCode::NotFound,
        false,
        ErrorReason::ShareNotFound,
    )
}

fn unauthorized() -> RpcErrorData {
    RpcErrorData::new(ApplicationErrorCode::Unauthorized, false)
}

impl Inner {
    /// 把进行中的上传叠加到分享 DTO（纯内存投影）。
    pub(super) fn overlay_upload(&self, dto: &mut ShareDto) {
        if let Some(entry) = self.uploads.get(&dto.id) {
            dto.cloud_state = ShareCloudState::Uploading;
            dto.upload = Some(entry.progress.clone());
        }
    }
}

impl ShareService {
    // ───────────── RPC ─────────────

    /// `agent.share.convertToCloud`：预检（登录、存储可用、保留期档位、源文件未变）并同步取到上传方案，
    /// 然后在后台上传；通过后立即返回 `cloudState = uploading`。已在上传 → 幂等；失败的上传 → 续传。
    pub(super) async fn convert_to_cloud(
        &self,
        params: ShareConvertToCloudParams,
    ) -> Result<ShareDto, RpcErrorData> {
        let id = params.id.as_str();
        let stored = self.find_share(id)?;
        if !self.cloud.has_account().await {
            return Err(unauthorized());
        }
        // 游客分享没有可计费的账号：云端不提供暂存（服务端同样拒绝）。
        if stored.guest_owned {
            return Err(cloud_unavailable());
        }
        match stored.dto.status {
            ShareStatus::Active => {}
            ShareStatus::Expired => {
                return Err(failure(
                    ApplicationErrorCode::Conflict,
                    false,
                    ErrorReason::ShareExpired,
                ));
            }
            _ => {
                return Err(failure(
                    ApplicationErrorCode::Conflict,
                    false,
                    ErrorReason::ShareStopped,
                ));
            }
        }
        if stored.dto.mode == ShareMode::Cloud && stored.dto.cloud_state == ShareCloudState::Ready {
            return self.view(id);
        }
        let cfg = self.config(false).await?;
        if !cfg.cloud_enabled {
            return Err(cloud_unavailable());
        }
        let tier = match params.tier_id.as_deref() {
            Some(tier) if cfg.has_retention(tier) => tier.to_owned(),
            Some(_) => return Err(invalid("tierId")),
            None => cfg.default_retention(None).ok_or_else(cloud_unavailable)?,
        };
        let running = self.with(|i| {
            i.uploads
                .get(id)
                .filter(|e| e.progress.status != ShareUploadStatus::Failed)
                .map(|e| Arc::clone(&e.wake))
        });
        if let Some(wake) = running {
            // 幂等；顺带让暂停中的退避立即重试。
            wake.notify_one();
            return self.view(id);
        }
        let files = Self::upload_files(&stored).await?;
        let Some((cancel, wake)) = self.reserve_upload(id, &files) else {
            return self.view(id);
        };
        // 续传（上次已转过暂存）时保留期已定，不再带档位。
        let resuming = stored.dto.cloud_state == ShareCloudState::Uploading;
        let plan = match request_plan(
            &self.cloud,
            id,
            &files,
            (!resuming).then_some(tier.as_str()),
        )
        .await
        {
            Ok(plan) => plan,
            Err(error) => {
                self.release_upload(id, &wake);
                self.publish().await;
                return Err(share_error(&error));
            }
        };
        self.with(|i| {
            i.share_epoch += 1;
            if let Some(share) = i.shares.iter_mut().find(|s| s.dto.id == id) {
                share.dto.cloud_state = ShareCloudState::Uploading;
            }
        });
        self.persist_shares().await;
        self.launch_upload(id, files, cancel, wake, Some(plan));
        self.publish().await;
        self.view(id)
    }

    /// `agent.share.deleteCloudCopy`：取消进行中的上传、删断点、删云端副本；分享回到实时模式。
    pub(super) async fn delete_cloud_copy(&self, id: &str) -> Result<ShareDto, RpcErrorData> {
        self.find_share(id)?;
        if !self.cloud.has_account().await {
            return Err(unauthorized());
        }
        let previous = self.take_upload(id).await;
        let cloud: CloudShare = match self.cloud.share_delete_cloud(id).await {
            Ok(cloud) => cloud,
            Err(error) => {
                let rpc = share_error(&error);
                if let Some(mut entry) = previous {
                    // 上传已被取消但云端没删成：留一个失败条目，UI 可重试。
                    entry.progress.status = ShareUploadStatus::Failed;
                    entry.progress.rate_bps = 0;
                    entry.progress.reason = rpc.reason;
                    self.with(|i| i.uploads.insert(id.to_owned(), entry));
                    self.publish().await;
                }
                return Err(rpc);
            }
        };
        self.with(|i| {
            i.share_epoch += 1;
            if let Some(share) = i.shares.iter_mut().find(|s| s.dto.id == id) {
                share.dto.cloud_state = ShareCloudState::Idle;
                share.dto.cloud_bytes = 0;
                apply_cloud(&mut share.dto, &cloud);
            }
        });
        self.persist_shares().await;
        self.wake.notify_one();
        self.publish().await;
        self.view(id)
    }

    // ───────────── 生命周期钩子 ─────────────

    /// 重启后：把仍标记为「上传中」的有效分享接着传（不需要断点文件，云端方案只列缺失分片）。
    pub(super) async fn resume_uploads(&self) {
        let candidates: Vec<StoredShare> = self.with(|i| {
            i.shares
                .iter()
                .filter(|s| {
                    s.dto.cloud_state == ShareCloudState::Uploading
                        && s.dto.status == ShareStatus::Active
                        && s.dto.mode == ShareMode::Live
                        && !i.uploads.contains_key(&s.dto.id)
                })
                .cloned()
                .collect()
        });
        if candidates.is_empty() {
            return;
        }
        for stored in candidates {
            let id = stored.dto.id.as_str();
            match Self::upload_files(&stored).await {
                Ok(files) => {
                    if let Some((cancel, wake)) = self.reserve_upload(id, &files) {
                        self.launch_upload(id, files, cancel, wake, None);
                    }
                }
                Err(error) => {
                    let total = stored.manifest.files.iter().map(|f| f.size).sum();
                    self.with(|i| {
                        i.uploads.insert(
                            id.to_owned(),
                            UploadEntry {
                                cancel: CancellationToken::new(),
                                wake: Arc::new(Notify::new()),
                                progress: ShareUploadDto {
                                    status: ShareUploadStatus::Failed,
                                    done_bytes: 0,
                                    total_bytes: total,
                                    rate_bps: 0,
                                    reason: error.reason,
                                },
                            },
                        );
                    });
                }
            }
        }
        self.publish().await;
    }

    /// 取消一个分享的上传并删断点（分享停止 / 过期 / 云端已就绪）。
    pub(super) async fn cancel_upload(&self, id: &str) {
        if self.take_upload(id).await.is_some() {
            self.publish().await;
        }
    }

    /// 云端对账后：分享已不再有效，或云端副本已就绪，本机上传没有继续的意义。
    pub(super) async fn cancel_stale_uploads(&self) {
        let stale: Vec<String> = self.with(|i| {
            i.uploads
                .keys()
                .filter(|id| {
                    i.shares.iter().find(|s| &s.dto.id == *id).is_none_or(|s| {
                        s.dto.status != ShareStatus::Active
                            || s.dto.cloud_state == ShareCloudState::Ready
                    })
                })
                .cloned()
                .collect()
        });
        for id in stale {
            self.cancel_upload(&id).await;
        }
    }

    /// 账号变化 / 网络恢复：唤醒暂停中的退避睡眠，立即重试一次。
    pub(super) fn wake_uploads(&self) {
        self.with(|i| {
            for entry in i.uploads.values() {
                if entry.progress.status == ShareUploadStatus::Paused {
                    entry.wake.notify_one();
                }
            }
        });
    }

    /// 服务关停：结束所有上传任务（断点保留，下次启动续传）。
    pub(super) fn stop_uploads(&self) {
        self.with(|i| {
            for entry in i.uploads.values() {
                entry.cancel.cancel();
            }
        });
    }

    /// 引擎汇报进度 / 状态；只认当前登记的那个任务。
    pub(super) async fn upload_progress(&self, id: &str, task: &Arc<Notify>, dto: ShareUploadDto) {
        let changed = self.with(|i| match i.uploads.get_mut(id) {
            Some(entry) if Arc::ptr_eq(&entry.wake, task) && entry.progress != dto => {
                entry.progress = dto;
                true
            }
            _ => false,
        });
        if changed {
            self.publish().await;
        }
    }

    // ───────────── 内部 ─────────────

    fn view(&self, id: &str) -> Result<ShareDto, RpcErrorData> {
        self.with(|i| {
            let mut dto = i.shares.iter().find(|s| s.dto.id == id)?.dto.clone();
            i.overlay_upload(&mut dto);
            Some(dto)
        })
        .ok_or_else(not_found)
    }

    /// 源文件仍与清单一致（大小 + 修改时间）才可上传。
    async fn upload_files(stored: &StoredShare) -> Result<Vec<JobFile>, RpcErrorData> {
        if stored.paths.len() != stored.manifest.files.len() {
            return Err(failure(
                ApplicationErrorCode::InvalidArgument,
                false,
                ErrorReason::ShareTaskNotShareable,
            ));
        }
        let mut files = Vec::with_capacity(stored.paths.len());
        for (position, (path, file)) in stored.paths.iter().zip(&stored.manifest.files).enumerate()
        {
            let index = u32::try_from(position).map_err(|_| file_changed())?;
            let job_file = JobFile::from_manifest(index, path.clone(), file);
            if !job_file.matches_disk().await {
                return Err(file_changed());
            }
            files.push(job_file);
        }
        Ok(files)
    }

    /// 占位：同一分享同一时刻只有一个上传。失败的旧条目被替换；已有运行中的返回 `None`。
    fn reserve_upload(
        &self,
        id: &str,
        files: &[JobFile],
    ) -> Option<(CancellationToken, Arc<Notify>)> {
        let cancel = CancellationToken::new();
        let wake = Arc::new(Notify::new());
        let entry = UploadEntry {
            cancel: cancel.clone(),
            wake: Arc::clone(&wake),
            progress: ShareUploadDto {
                status: ShareUploadStatus::Uploading,
                done_bytes: 0,
                total_bytes: files.iter().map(|f| f.size).sum(),
                rate_bps: 0,
                reason: None,
            },
        };
        let reserved = self.with(|i| {
            if i.uploads
                .get(id)
                .is_some_and(|e| e.progress.status != ShareUploadStatus::Failed)
            {
                return false;
            }
            if let Some(old) = i.uploads.insert(id.to_owned(), entry) {
                old.cancel.cancel();
            }
            true
        });
        reserved.then_some((cancel, wake))
    }

    /// 预检失败时撤回自己的占位。
    fn release_upload(&self, id: &str, task: &Arc<Notify>) {
        self.with(|i| {
            if i.uploads
                .get(id)
                .is_some_and(|e| Arc::ptr_eq(&e.wake, task))
            {
                i.uploads.remove(id);
            }
        });
    }

    /// 取出（并取消）条目，删断点文件。
    async fn take_upload(&self, id: &str) -> Option<UploadEntry> {
        let entry = self.with(|i| i.uploads.remove(id));
        if let Some(entry) = &entry {
            entry.cancel.cancel();
        }
        if let Err(error) = self.store.remove_upload(id).await {
            tracing::warn!(error = %error, "could not remove the upload state");
        }
        entry
    }

    fn launch_upload(
        &self,
        id: &str,
        files: Vec<JobFile>,
        cancel: CancellationToken,
        wake: Arc<Notify>,
        first_plan: Option<UploadPlan>,
    ) {
        let timing = self.with(|i| i.upload_timing);
        let job = Job::new(JobParams {
            share_id: id.to_owned(),
            files,
            cloud: self.cloud.clone(),
            egress: self.egress.clone(),
            store: self.store.clone(),
            service: self.weak.clone(),
            cancel,
            wake: Arc::clone(&wake),
            timing,
        });
        let weak = self.weak.clone();
        let id = id.to_owned();
        tokio::spawn(async move {
            let outcome = job.run(first_plan).await;
            if let Some(service) = weak.upgrade() {
                service.upload_ended(&id, &wake, outcome).await;
            }
        });
    }

    async fn upload_ended(&self, id: &str, task: &Arc<Notify>, outcome: Outcome) {
        match outcome {
            Outcome::Cancelled => {}
            Outcome::Failed(reason) => {
                let changed = self.with(|i| match i.uploads.get_mut(id) {
                    Some(entry) if Arc::ptr_eq(&entry.wake, task) => {
                        entry.progress.status = ShareUploadStatus::Failed;
                        entry.progress.rate_bps = 0;
                        entry.progress.reason = reason;
                        true
                    }
                    _ => false,
                });
                if changed {
                    self.publish().await;
                }
            }
            Outcome::Done => {
                let finished = self.with(|i| {
                    if !i
                        .uploads
                        .get(id)
                        .is_some_and(|e| Arc::ptr_eq(&e.wake, task))
                    {
                        return false;
                    }
                    i.share_epoch += 1;
                    let total = i.uploads.remove(id).map_or(0, |e| e.progress.total_bytes);
                    if let Some(share) = i.shares.iter_mut().find(|s| s.dto.id == id) {
                        share.dto.cloud_state = ShareCloudState::Ready;
                        share.dto.mode = ShareMode::Cloud;
                        share.dto.cloud_bytes = total;
                    }
                    true
                });
                if finished {
                    self.persist_shares().await;
                    // 以云端为准：保留期 / 到期时间 / 云端字节数随之校正。
                    self.sync_shares().await;
                    self.wake.notify_one();
                    self.publish().await;
                }
            }
        }
    }
}
