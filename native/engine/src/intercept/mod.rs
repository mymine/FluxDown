//! 插件拦截平面：任务运行期的各阶段（请求准备、探测后、凭据刷新、画质 / Period
//! 选择、分片过滤、完成前处理、失败后）把引擎掌握的任务信息交给插件读取与改写。
//!
//! 本模块与插件运行时解耦：引擎只经 [`TaskInterceptor`] 调用，未启用插件或
//! 无匹配插件时使用 [`NoopInterceptor`]，各阶段零调用、不构建任何上下文。

pub mod ctx;
pub mod drive;
pub mod patch;
pub mod refresh;

use std::sync::Arc;

use crate::db::Db;
use crate::downloader::{DownloadError, DownloadParams};
use crate::events::EventSink;
use crate::logger::log_error;

pub use ctx::{RunMeta, SegmentItem, TaskCtx};
pub use patch::{PatchOutcome, RequestPatch, RequestTarget, apply_request_patch};
pub use refresh::Refresher;

/// 拦截阶段。线协议名见 [`STAGE_NAMES`]（与 [`InterceptStage::as_str`] 同序）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InterceptStage {
    Prepare,
    Probed,
    Refresh,
    SelectVariant,
    SelectPeriod,
    Segments,
    LiveSegments,
    Finalize,
    Failed,
}

/// 全部阶段的线协议名，manifest 校验与 SDK 共用此顺序。
pub const STAGE_NAMES: [&str; 9] = [
    "prepare",
    "probed",
    "refresh",
    "selectVariant",
    "selectPeriod",
    "segments",
    "liveSegments",
    "finalize",
    "failed",
];

impl InterceptStage {
    const ALL: [InterceptStage; 9] = [
        InterceptStage::Prepare,
        InterceptStage::Probed,
        InterceptStage::Refresh,
        InterceptStage::SelectVariant,
        InterceptStage::SelectPeriod,
        InterceptStage::Segments,
        InterceptStage::LiveSegments,
        InterceptStage::Finalize,
        InterceptStage::Failed,
    ];

    /// 线协议名。
    pub fn as_str(self) -> &'static str {
        match self {
            InterceptStage::Prepare => STAGE_NAMES[0],
            InterceptStage::Probed => STAGE_NAMES[1],
            InterceptStage::Refresh => STAGE_NAMES[2],
            InterceptStage::SelectVariant => STAGE_NAMES[3],
            InterceptStage::SelectPeriod => STAGE_NAMES[4],
            InterceptStage::Segments => STAGE_NAMES[5],
            InterceptStage::LiveSegments => STAGE_NAMES[6],
            InterceptStage::Finalize => STAGE_NAMES[7],
            InterceptStage::Failed => STAGE_NAMES[8],
        }
    }

    /// 由线协议名解析；未知名返回 `None`。
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == name)
    }
}

/// 任务的下载协议类别（决定哪些改写合法）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolKind {
    Http,
    Ftp,
    Hls,
    Dash,
}

impl ProtocolKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ProtocolKind::Http => "http",
            ProtocolKind::Ftp => "ftp",
            ProtocolKind::Hls => "hls",
            ProtocolKind::Dash => "dash",
        }
    }
}

/// 与 `do_start_task` 的协议分派同优先级：FTP → HLS → DASH（含音视频轨对）→ HTTP。
pub fn protocol_kind(url: &str, has_audio_track: bool) -> ProtocolKind {
    if crate::download_manager::is_ftp_url(url) {
        ProtocolKind::Ftp
    } else if crate::hls_downloader::is_hls_url(url) {
        ProtocolKind::Hls
    } else if crate::dash_downloader::is_dash_url(url) || has_audio_track {
        ProtocolKind::Dash
    } else {
        ProtocolKind::Http
    }
}

/// 一个参与本任务的拦截插件（起跑时已按 URL 匹配好）。
#[derive(Debug, Clone)]
pub struct InterceptorEntry {
    pub identity: String,
    pub stages: Vec<InterceptStage>,
    /// `onFailure: "fail"`：插件出错即让任务失败；否则记录活动后忽略。
    pub fail_task: bool,
}

/// 引擎调用插件拦截器的唯一入口。
#[async_trait::async_trait]
pub trait TaskInterceptor: Send + Sync {
    /// 参与本任务的插件，identity 升序。
    fn entries(&self) -> &[InterceptorEntry];
    /// 任务持久化的原始 URL。
    fn source_url(&self) -> &str;
    /// 调用 `identity` 的 `intercept(ctx)`；`Ok(None)` = 插件不做改动。
    /// 调用方丢弃返回的 future 即中止插件侧执行。
    async fn call(
        &self,
        identity: &str,
        stage: InterceptStage,
        arg_json: String,
        file_root: Option<std::path::PathBuf>,
    ) -> Result<Option<String>, String>;
}

/// 无插件参与时的拦截器：所有阶段零调用。
pub struct NoopInterceptor;

#[async_trait::async_trait]
impl TaskInterceptor for NoopInterceptor {
    fn entries(&self) -> &[InterceptorEntry] {
        &[]
    }

    fn source_url(&self) -> &str {
        ""
    }

    async fn call(
        &self,
        _identity: &str,
        _stage: InterceptStage,
        _arg_json: String,
        _file_root: Option<std::path::PathBuf>,
    ) -> Result<Option<String>, String> {
        Ok(None)
    }
}

/// 共享的 [`NoopInterceptor`] 实例。
pub fn noop() -> Arc<dyn TaskInterceptor> {
    Arc::new(NoopInterceptor)
}

/// 是否有插件声明了 `stage`——调用方据此决定是否构建上下文。
pub fn wants(ic: &dyn TaskInterceptor, stage: InterceptStage) -> bool {
    ic.entries().iter().any(|e| e.stages.contains(&stage))
}

/// 按连接上限截断并发数：`cap > 0` 时 `min(n, cap)` 且至少 1，否则原值。
pub fn cap_connections(n: usize, cap: i32) -> usize {
    if cap > 0 {
        n.min(cap as usize).max(1)
    } else {
        n
    }
}

/// 插件要求中止任务（`onFailure: "fail"` 的出错、或 `fail` 返回值）。
#[derive(Debug, Clone)]
pub struct InterceptAbort {
    pub identity: String,
    pub stage: InterceptStage,
    pub message: String,
}

impl std::fmt::Display for InterceptAbort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[插件] {}（{}）：{}",
            self.identity,
            self.stage.as_str(),
            self.message
        )
    }
}

impl From<InterceptAbort> for DownloadError {
    fn from(abort: InterceptAbort) -> Self {
        DownloadError::PluginAbort(abort.to_string())
    }
}

/// 写入任务活动（kind = `plugin`）。只记录字段名与文件名，绝不记录请求头值、
/// Cookie 或请求体。
#[derive(Clone)]
pub struct ActivityNotes {
    db: Db,
    sink: Arc<dyn EventSink>,
    task_id: String,
}

impl ActivityNotes {
    pub fn of(p: &DownloadParams) -> Self {
        Self::new(p.db.clone(), p.sink.clone(), p.task_id.clone())
    }

    pub fn new(db: Db, sink: Arc<dyn EventSink>, task_id: String) -> Self {
        Self { db, sink, task_id }
    }

    pub async fn add(&self, message: String) {
        if let Err(e) = crate::task_activity::record(
            &self.db,
            self.sink.as_ref(),
            &self.task_id,
            "plugin",
            message,
            None,
        )
        .await
        {
            log_error!("[task-activity] failed to persist plugin note: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_names_round_trip() {
        for name in STAGE_NAMES {
            let stage = InterceptStage::parse(name).map(InterceptStage::as_str);
            assert_eq!(stage, Some(name));
        }
        assert_eq!(InterceptStage::parse("retry"), None);
    }

    #[test]
    fn protocol_kind_follows_dispatch_priority() {
        assert_eq!(
            protocol_kind("ftp://example.com/a.m3u8", false),
            ProtocolKind::Ftp
        );
        assert_eq!(
            protocol_kind("https://example.com/a.m3u8", true),
            ProtocolKind::Hls
        );
        assert_eq!(
            protocol_kind("https://example.com/a.mpd", false),
            ProtocolKind::Dash
        );
        assert_eq!(
            protocol_kind("https://example.com/v.mp4", true),
            ProtocolKind::Dash
        );
        assert_eq!(
            protocol_kind("https://example.com/v.mp4", false),
            ProtocolKind::Http
        );
    }

    #[test]
    fn cap_connections_bounds_only_when_positive() {
        assert_eq!(cap_connections(8, 2), 2);
        assert_eq!(cap_connections(8, 0), 8);
        assert_eq!(cap_connections(0, 3), 1);
    }
}
