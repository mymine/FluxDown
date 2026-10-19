//! 拦截线协议的 Rust 表示（与 SDK `flux.d.ts` 逐字段对齐，全部 camelCase）。
//!
//! 上下文结构只做序列化，`stage` 由 [`super::drive::invoke`] 统一写入；结果结构
//! 宽松反序列化（缺省字段取默认值、忽略未知字段），与 `ResolveResult` 一致。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::ProtocolKind;
use crate::downloader::{CapturedRequestBody, DownloadParams, OriginHeaders};
use crate::model::TaskInfo;

/// `manifestText` 上限；超出时传空串并置 `manifestTextOmitted`。
pub const MANIFEST_TEXT_LIMIT: usize = 4 * 1024 * 1024;

/// 本次运行的只读网络信息与有效 UA。
#[derive(Debug, Clone, Default)]
pub struct RunMeta {
    pub proxy_url: String,
    pub ignore_tls_errors: bool,
    /// 任务 / 队列 / 全局解析出的 UA（请求头未显式携带 UA 时生效的值）。
    pub user_agent: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskCtx {
    pub id: String,
    pub url: String,
    pub source_url: String,
    pub file_name: String,
    pub save_dir: String,
    pub protocol: &'static str,
    pub track_pair: bool,
    pub is_resume: bool,
}

/// 由运行参数构造任务信息。
pub fn task_ctx(p: &DownloadParams, protocol: ProtocolKind) -> TaskCtx {
    TaskCtx {
        id: p.task_id.clone(),
        url: p.url.clone(),
        source_url: p.interceptor.source_url().to_string(),
        file_name: p.file_name.clone(),
        save_dir: p.save_dir.clone(),
        protocol: protocol.as_str(),
        track_pair: p.audio_url.is_some(),
        is_resume: p.is_resume,
    }
}

/// 由任务行构造任务信息（下载器之外的阶段：元数据探测、失败后）。
pub fn task_ctx_from_row(task: &TaskInfo, source_url: &str, protocol: ProtocolKind) -> TaskCtx {
    TaskCtx {
        id: task.task_id.clone(),
        url: task.url.clone(),
        source_url: source_url.to_string(),
        file_name: task.file_name.clone(),
        save_dir: task.save_dir.clone(),
        protocol: protocol.as_str(),
        track_pair: false,
        is_resume: false,
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestCtx {
    pub url: String,
    pub method: String,
    pub cookies: String,
    pub referrer: String,
    pub user_agent: String,
    pub headers: HashMap<String, String>,
    pub body: Option<CapturedRequestBody>,
    pub origin_headers: Vec<OriginHeaders>,
    pub proxy_url: String,
    pub ignore_tls_errors: bool,
}

/// 线程数 / 连接上限；0 = 自动 / 不限。
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionsCtx {
    pub segments: i32,
    pub max_connections: i32,
    pub auto_max_connections: i32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareCtx<'a> {
    pub task: &'a TaskCtx,
    pub request: RequestCtx,
    pub connections: ConnectionsCtx,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeCtx {
    pub total_bytes: i64,
    pub supports_range: bool,
    pub file_name: String,
    pub content_type: String,
    pub etag: String,
    pub last_modified: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbedCtx<'a> {
    pub task: &'a TaskCtx,
    pub request: RequestCtx,
    pub connections: ConnectionsCtx,
    pub probe: ProbeCtx,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VariantCtx {
    pub index: usize,
    pub bandwidth: u64,
    pub width: Option<u64>,
    pub height: Option<u64>,
    pub uri: String,
    pub id: Option<String>,
    pub codecs: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectVariantCtx<'a> {
    pub task: &'a TaskCtx,
    pub format: &'static str,
    pub variants: Vec<VariantCtx>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeriodCtx {
    pub index: usize,
    pub id: Option<String>,
    pub start_seconds: Option<f64>,
    pub duration_seconds: Option<f64>,
    pub has_video: bool,
    pub has_audio: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectPeriodCtx<'a> {
    pub task: &'a TaskCtx,
    pub manifest_text: String,
    pub manifest_text_omitted: bool,
    pub periods: Vec<PeriodCtx>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ByteRangeCtx {
    pub offset: u64,
    pub length: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DateRangeCtx {
    pub id: String,
    pub class: Option<String>,
    pub start_date: String,
    pub end_date: Option<String>,
    pub duration: Option<f64>,
    pub planned_duration: Option<f64>,
    pub end_on_next: bool,
    pub attributes: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyCtx {
    /// 恒为 `"AES-128"`（引擎只解密这一种）。
    pub method: &'static str,
    pub uri: String,
    pub iv: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MapCtx {
    pub uri: String,
    pub byte_range: Option<ByteRangeCtx>,
}

/// 一个媒体分片。`id`：HLS = 绝对 media sequence；DASH = 本轨媒体分片列表中的
/// 0 起序号（不含 init）。
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SegmentItem {
    pub id: u64,
    pub uri: String,
    pub duration: Option<f64>,
    pub byte_range: Option<ByteRangeCtx>,
    pub discontinuity: bool,
    pub title: Option<String>,
    pub program_date_time: Option<String>,
    pub date_range: Option<DateRangeCtx>,
    /// 引擎未识别的标签原文。
    pub tags: Vec<String>,
    pub key: Option<KeyCtx>,
    pub map: Option<MapCtx>,
    pub number: Option<u64>,
    pub time: Option<u64>,
}

/// `segments` / `liveSegments` 上下文中除分片列表外的部分。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SegmentsHeader<'a> {
    pub task: &'a TaskCtx,
    pub format: &'static str,
    pub track: SegmentTrack,
    pub playlist_url: String,
    pub manifest_text: String,
    pub manifest_text_omitted: bool,
    pub media_sequence: Option<u64>,
    pub target_duration: Option<f64>,
    pub period_index: Option<usize>,
    pub representation_id: Option<String>,
}

/// 分片所属轨道。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SegmentTrack {
    Video,
    Audio,
}

impl SegmentTrack {
    /// 活动记录用的中文名。
    pub fn label(self) -> &'static str {
        match self {
            SegmentTrack::Video => "视频",
            SegmentTrack::Audio => "音频",
        }
    }
}

/// `refresh`：运行中的请求被拒（401 / 403 / 410）时请求插件换发凭据。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshCtx<'a> {
    pub task: &'a TaskCtx,
    /// 被拒请求所用的请求身份（`url` 为该请求的实际地址）。
    pub request: RequestCtx,
    pub status: u16,
    /// 本次运行已成功刷新的次数（不含本次）。
    pub refresh_count: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FinalizeCtx<'a> {
    pub task: &'a TaskCtx,
    pub file_path: String,
    pub file_name: String,
    pub save_dir: String,
    pub total_bytes: i64,
    pub audio_path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FailedCtx<'a> {
    pub task: &'a TaskCtx,
    pub error_message: String,
    pub http_status: Option<u16>,
    pub auto_retry_count: u32,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProbedResult {
    pub segments: Option<i32>,
    pub max_connections: Option<i32>,
    pub fail: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SelectResult {
    pub select: Option<usize>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SegmentsResult {
    pub drop: Vec<u64>,
    /// 改写分片地址（逐片签名等）；相对地址按 `playlistUrl` 解析。
    pub rewrite: Vec<SegmentRewrite>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SegmentRewrite {
    pub id: u64,
    pub uri: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FinalizeResult {
    pub fail: Option<String>,
    pub replace_with: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FailedResult {
    pub retry: Option<RetryRequest>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RetryRequest {
    pub delay_ms: Option<u64>,
}

/// 清单原文（UTF-8 lossy）；超过 [`MANIFEST_TEXT_LIMIT`] 时返回 `("", true)`。
pub fn manifest_text(raw: &[u8]) -> (String, bool) {
    if raw.len() > MANIFEST_TEXT_LIMIT {
        (String::new(), true)
    } else {
        (String::from_utf8_lossy(raw).into_owned(), false)
    }
}
