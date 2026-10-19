//! `refresh` 阶段：运行中的请求被拒（401 / 403 / 410）时，请插件换发凭据，
//! 本次运行内的后续请求立即改用新凭据，无需整轮失败重来。
//!
//! 共享状态（当前请求身份、单飞闸门、刷新计数）由 [`crate::downloader::DownloadSpec`]
//! 持有；本模块只负责与插件交互并把补丁应用到副本上。

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::ctx::{RunMeta, TaskCtx, task_ctx};
use super::patch::RequestTarget;
use super::{ActivityNotes, InterceptAbort, InterceptStage, ProtocolKind, TaskInterceptor, wants};
use crate::downloader::{DownloadParams, RequestSpec};

/// 连续换发凭据却都没换来成功响应的上限：凭据始终无效时停止换发，避免无限刷新。
/// 被拒身份此前成功过（凭据被正常消耗，如每请求一个令牌）时计数归零。
pub const MAX_UNCONFIRMED_REFRESHES: u32 = 5;

/// 触发 `refresh` 的响应状态：凭据缺失 / 失效 / 已作废。
pub fn is_refresh_status(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 401 | 403 | 410)
}

/// 一次运行的凭据刷新器（仅当有插件声明 `refresh` 阶段时存在）。
pub struct Refresher {
    ic: Arc<dyn TaskInterceptor>,
    notes: ActivityNotes,
    task: TaskCtx,
    meta: RunMeta,
    cancel: CancellationToken,
    kind: ProtocolKind,
    has_audio_track: bool,
    origin_url: String,
}

impl std::fmt::Debug for Refresher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Refresher")
            .field("task", &self.task.id)
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

impl Refresher {
    /// 无插件声明 `refresh` 时返回 `None`（零开销）。
    pub fn for_params(p: &DownloadParams, kind: ProtocolKind) -> Option<Self> {
        if !wants(p.interceptor.as_ref(), InterceptStage::Refresh) {
            return None;
        }
        Some(Self {
            ic: p.interceptor.clone(),
            notes: ActivityNotes::of(p),
            task: task_ctx(p, kind),
            meta: p.run_meta.clone(),
            cancel: p.cancel_token.clone(),
            kind,
            has_audio_track: p.audio_url.is_some(),
            origin_url: p.url.clone(),
        })
    }

    /// 本次运行起跑时的任务 URL；URL 类刷新只替换发往它的请求。
    pub fn origin_url(&self) -> &str {
        &self.origin_url
    }

    /// 依次询问声明了 `refresh` 的插件；返回是否有凭据被改写。`url` / `spec`
    /// 为当前请求身份的副本，改写结果写回其中。
    pub(crate) async fn refresh(
        &self,
        url: &mut String,
        spec: &mut RequestSpec,
        status: u16,
        refresh_count: u32,
    ) -> Result<bool, InterceptAbort> {
        let mut target = RequestTarget {
            url,
            spec,
            file_name: None,
            segment_count: None,
            connection_cap: None,
            hint_file_size: None,
            range_verified: None,
            is_resume: self.task.is_resume,
            kind: self.kind,
            has_audio_track: self.has_audio_track,
        };
        super::drive::run_refresh(
            self.ic.as_ref(),
            &self.notes,
            &self.cancel,
            &self.task,
            &self.meta,
            status,
            refresh_count,
            &mut target,
        )
        .await
    }
}
