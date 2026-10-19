//! 各阶段的链式执行器：只遍历声明了该阶段的插件（identity 升序）。插件出错
//! 交给 [`tolerate`]：`onFailure: "fail"` 中止任务，否则记录活动后继续。

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio_util::sync::CancellationToken;

use super::ctx::{
    ConnectionsCtx, FailedCtx, FailedResult, FinalizeCtx, FinalizeResult, PrepareCtx, ProbeCtx,
    ProbedCtx, ProbedResult, RefreshCtx, RequestCtx, RunMeta, SegmentItem, SegmentsHeader,
    SegmentsResult, SelectResult, TaskCtx,
};
use super::patch::{
    RequestPatch, RequestTarget, apply_request_patch, check_refresh_patch, request_snapshot,
};
use super::{ActivityNotes, InterceptAbort, InterceptStage, InterceptorEntry, TaskInterceptor};
use crate::db::Db;
use crate::logger::log_warn;

/// 单次插件调用的结果。
pub(crate) enum Invoke<T> {
    Value(T),
    Nothing,
    Failed(String),
    Cancelled,
}

fn entries_for(
    ic: &dyn TaskInterceptor,
    stage: InterceptStage,
) -> impl Iterator<Item = &InterceptorEntry> {
    ic.entries()
        .iter()
        .filter(move |e| e.stages.contains(&stage))
}

fn ctx_value<S: Serialize>(ctx: &S) -> Result<serde_json::Value, String> {
    serde_json::to_value(ctx).map_err(|e| format!("上下文序列化失败: {e}"))
}

/// 调用一个插件：写入 `stage` 后序列化；有 `cancel` 时与取消竞速（取消即丢弃
/// 调用 future，插件侧随之中止）。
pub(crate) async fn invoke<T: DeserializeOwned>(
    ic: &dyn TaskInterceptor,
    entry: &InterceptorEntry,
    stage: InterceptStage,
    mut arg: serde_json::Value,
    cancel: Option<&CancellationToken>,
    file_root: Option<&Path>,
) -> Invoke<T> {
    if let serde_json::Value::Object(map) = &mut arg {
        map.insert(
            "stage".to_string(),
            serde_json::Value::String(stage.as_str().to_string()),
        );
    }
    let json = match serde_json::to_string(&arg) {
        Ok(j) => j,
        Err(e) => return Invoke::Failed(format!("上下文序列化失败: {e}")),
    };
    let call = ic.call(
        &entry.identity,
        stage,
        json,
        file_root.map(Path::to_path_buf),
    );
    let result = match cancel {
        Some(token) => {
            tokio::select! {
                biased;
                _ = token.cancelled() => return Invoke::Cancelled,
                r = call => r,
            }
        }
        None => call.await,
    };
    match result {
        Ok(None) => Invoke::Nothing,
        Ok(Some(raw)) => match serde_json::from_str::<T>(&raw) {
            Ok(v) => Invoke::Value(v),
            Err(e) => Invoke::Failed(format!("返回值非法: {e}")),
        },
        Err(e) => Invoke::Failed(e),
    }
}

/// 插件出错：`fail_task` 时中止，否则记录活动并忽略。
pub(crate) async fn tolerate(
    notes: &ActivityNotes,
    entry: &InterceptorEntry,
    stage: InterceptStage,
    message: String,
) -> Result<(), InterceptAbort> {
    if entry.fail_task {
        return Err(InterceptAbort {
            identity: entry.identity.clone(),
            stage,
            message,
        });
    }
    notes
        .add(format!(
            "插件 {} 的 {} 阶段出错，已忽略：{}",
            entry.identity,
            stage.as_str(),
            message
        ))
        .await;
    Ok(())
}

async fn note_patch(notes: &ActivityNotes, identity: &str, outcome: &super::PatchOutcome) {
    if !outcome.changed.is_empty() {
        notes
            .add(format!(
                "插件 {identity} 改写了请求：{}",
                outcome.changed.join("、")
            ))
            .await;
    }
    if outcome.ignored_file_name {
        notes
            .add(format!(
                "插件 {identity} 的文件名改写已忽略（任务已开始下载）"
            ))
            .await;
    }
}

/// 应用 resolver 下发的请求补丁（失败不可静默，由调用方让任务失败）。
pub(crate) async fn apply_resolver_patch(
    notes: &ActivityNotes,
    identity: &str,
    patch: &RequestPatch,
    target: &mut RequestTarget<'_>,
) -> Result<(), String> {
    let outcome =
        apply_request_patch(target, patch).map_err(|e| format!("[插件] {identity}: {e}"))?;
    note_patch(notes, identity, &outcome).await;
    Ok(())
}

/// `prepare` 的合并结果。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PrepareOutcome {
    /// 执行中任务被取消。
    pub cancelled: bool,
    /// 有插件声明了 `ephemeral: true`：调用方不得发出任何探测请求。
    pub ephemeral: bool,
}

/// `prepare`：每个插件看到前一个改写后的请求。
#[allow(clippy::too_many_arguments)]
pub async fn run_prepare(
    ic: &dyn TaskInterceptor,
    notes: &ActivityNotes,
    cancel: &CancellationToken,
    task: &TaskCtx,
    meta: &RunMeta,
    auto_max_connections: i32,
    target: &mut RequestTarget<'_>,
) -> Result<PrepareOutcome, InterceptAbort> {
    let stage = InterceptStage::Prepare;
    let mut result = PrepareOutcome::default();
    for entry in entries_for(ic, stage) {
        let ctx = PrepareCtx {
            task,
            request: request_snapshot(target.url, target.spec, meta),
            connections: ConnectionsCtx {
                segments: target.segment_count.as_deref().copied().unwrap_or(0),
                max_connections: target.connection_cap.as_deref().copied().unwrap_or(0),
                auto_max_connections,
            },
        };
        let arg = match ctx_value(&ctx) {
            Ok(v) => v,
            Err(e) => {
                tolerate(notes, entry, stage, e).await?;
                continue;
            }
        };
        match invoke::<RequestPatch>(ic, entry, stage, arg, Some(cancel), None).await {
            Invoke::Value(patch) => match apply_request_patch(target, &patch) {
                Ok(outcome) => {
                    result.ephemeral |= outcome.ephemeral;
                    note_patch(notes, &entry.identity, &outcome).await;
                }
                Err(e) => tolerate(notes, entry, stage, e).await?,
            },
            Invoke::Nothing => {}
            Invoke::Failed(e) => tolerate(notes, entry, stage, e).await?,
            Invoke::Cancelled => {
                result.cancelled = true;
                return Ok(result);
            }
        }
    }
    Ok(result)
}

/// `refresh`：被拒请求的凭据换发，每个插件看到前一个换发后的请求身份。
/// 返回是否有插件改写了凭据（未改写则调用方按原错误处理）。
#[allow(clippy::too_many_arguments)]
pub async fn run_refresh(
    ic: &dyn TaskInterceptor,
    notes: &ActivityNotes,
    cancel: &CancellationToken,
    task: &TaskCtx,
    meta: &RunMeta,
    status: u16,
    refresh_count: u32,
    target: &mut RequestTarget<'_>,
) -> Result<bool, InterceptAbort> {
    let stage = InterceptStage::Refresh;
    let mut changed = false;
    for entry in entries_for(ic, stage) {
        let ctx = RefreshCtx {
            task,
            request: request_snapshot(target.url, target.spec, meta),
            status,
            refresh_count,
        };
        let arg = match ctx_value(&ctx) {
            Ok(v) => v,
            Err(e) => {
                tolerate(notes, entry, stage, e).await?;
                continue;
            }
        };
        match invoke::<RequestPatch>(ic, entry, stage, arg, Some(cancel), None).await {
            Invoke::Value(patch) => {
                let applied = check_refresh_patch(&patch, target.kind)
                    .and_then(|()| apply_request_patch(target, &patch));
                match applied {
                    Ok(outcome) if !outcome.changed.is_empty() => {
                        changed = true;
                        notes
                            .add(format!(
                                "插件 {} 刷新了凭据（HTTP {status}）：{}",
                                entry.identity,
                                outcome.changed.join("、")
                            ))
                            .await;
                    }
                    Ok(_) => {}
                    Err(e) => tolerate(notes, entry, stage, e).await?,
                }
            }
            Invoke::Nothing => {}
            Invoke::Failed(e) => tolerate(notes, entry, stage, e).await?,
            Invoke::Cancelled => return Ok(changed),
        }
    }
    Ok(changed)
}

/// `probed` 阶段的合并决定（后一个插件覆盖前一个）。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ProbedDecision {
    pub segments: Option<i32>,
    pub max_connections: Option<i32>,
}

/// `probed`：插件可覆盖线程数 / 连接上限，或以 `fail` 无条件中止任务。
pub async fn run_probed(
    ic: &dyn TaskInterceptor,
    notes: &ActivityNotes,
    cancel: &CancellationToken,
    task: &TaskCtx,
    request: RequestCtx,
    mut connections: ConnectionsCtx,
    probe: ProbeCtx,
) -> Result<ProbedDecision, InterceptAbort> {
    let stage = InterceptStage::Probed;
    let mut decision = ProbedDecision::default();
    for entry in entries_for(ic, stage) {
        let ctx = ProbedCtx {
            task,
            request: request.clone(),
            connections,
            probe: probe.clone(),
        };
        let arg = match ctx_value(&ctx) {
            Ok(v) => v,
            Err(e) => {
                tolerate(notes, entry, stage, e).await?;
                continue;
            }
        };
        match invoke::<ProbedResult>(ic, entry, stage, arg, Some(cancel), None).await {
            Invoke::Value(r) => {
                if let Some(message) = r.fail {
                    return Err(InterceptAbort {
                        identity: entry.identity.clone(),
                        stage,
                        message,
                    });
                }
                if let Some(s) = r.segments
                    && !(0..=crate::segment_coordinator::MAX_SEGMENTS).contains(&s)
                {
                    tolerate(notes, entry, stage, "segments 超出 0..=512".to_string()).await?;
                    continue;
                }
                if let Some(c) = r.max_connections
                    && !(1..=512).contains(&c)
                {
                    tolerate(
                        notes,
                        entry,
                        stage,
                        "maxConnections 超出 1..=512".to_string(),
                    )
                    .await?;
                    continue;
                }
                let mut changed: Vec<&str> = Vec::new();
                if let Some(s) = r.segments {
                    decision.segments = Some(s);
                    connections.segments = s;
                    changed.push("线程数");
                }
                if let Some(c) = r.max_connections {
                    decision.max_connections = Some(c);
                    connections.max_connections = c;
                    changed.push("连接上限");
                }
                if !changed.is_empty() {
                    notes
                        .add(format!(
                            "插件 {} 调整了并发：{}",
                            entry.identity,
                            changed.join("、")
                        ))
                        .await;
                }
            }
            Invoke::Nothing => {}
            Invoke::Failed(e) => tolerate(notes, entry, stage, e).await?,
            Invoke::Cancelled => return Ok(decision),
        }
    }
    Ok(decision)
}

/// `selectVariant` / `selectPeriod`：首个返回合法 `select` 的插件胜出。
pub async fn run_select(
    ic: &dyn TaskInterceptor,
    notes: &ActivityNotes,
    cancel: &CancellationToken,
    stage: InterceptStage,
    ctx: serde_json::Value,
    count: usize,
) -> Result<Option<usize>, InterceptAbort> {
    let label = if stage == InterceptStage::SelectPeriod {
        "Period"
    } else {
        "画质"
    };
    for entry in entries_for(ic, stage) {
        match invoke::<SelectResult>(ic, entry, stage, ctx.clone(), Some(cancel), None).await {
            Invoke::Value(SelectResult { select: Some(i) }) => {
                if i >= count {
                    tolerate(notes, entry, stage, "select 超出范围".to_string()).await?;
                    continue;
                }
                notes
                    .add(format!(
                        "插件 {} 选择了第 {} 个{label}",
                        entry.identity,
                        i + 1
                    ))
                    .await;
                return Ok(Some(i));
            }
            Invoke::Value(SelectResult { select: None }) | Invoke::Nothing => {}
            Invoke::Failed(e) => tolerate(notes, entry, stage, e).await?,
            Invoke::Cancelled => return Ok(None),
        }
    }
    Ok(None)
}

/// `segments` / `liveSegments` 的合并决定。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SegmentDecision {
    /// 被丢弃的分片 id。
    pub dropped: BTreeSet<u64>,
    /// 被改写地址的分片 id → 绝对地址（不含已丢弃分片）。
    pub rewrites: BTreeMap<u64, String>,
}

/// 把插件给出的分片地址按清单地址解析为绝对 http(s) 地址。
fn resolve_segment_uri(base: &str, uri: &str) -> Option<String> {
    if uri.is_empty() || uri.len() > 8 * 1024 {
        return None;
    }
    let joined = reqwest::Url::parse(base)
        .and_then(|b| b.join(uri))
        .or_else(|_| reqwest::Url::parse(uri))
        .ok()?;
    matches!(joined.scheme(), "http" | "https").then(|| joined.to_string())
}

/// `segments` / `liveSegments`：各插件的 drop 取并集、rewrite 后者覆盖前者；后续插件
/// 只看到剩余分片且看到已改写的地址。单个插件的 drop 与 rewrite 全部合法才采纳。
pub async fn run_segments(
    ic: &dyn TaskInterceptor,
    notes: &ActivityNotes,
    cancel: &CancellationToken,
    stage: InterceptStage,
    header: &SegmentsHeader<'_>,
    items: &[SegmentItem],
) -> Result<SegmentDecision, InterceptAbort> {
    let all_ids: HashSet<u64> = items.iter().map(|s| s.id).collect();
    let mut decision = SegmentDecision::default();
    for entry in entries_for(ic, stage) {
        let remaining: Vec<SegmentItem> = items
            .iter()
            .filter(|s| !decision.dropped.contains(&s.id))
            .map(|s| match decision.rewrites.get(&s.id) {
                Some(uri) => SegmentItem {
                    uri: uri.clone(),
                    ..s.clone()
                },
                None => s.clone(),
            })
            .collect();
        let arg = match ctx_value(header).and_then(|mut v| {
            let list = ctx_value(&remaining)?;
            if let serde_json::Value::Object(map) = &mut v {
                map.insert("segments".to_string(), list);
            }
            Ok(v)
        }) {
            Ok(v) => v,
            Err(e) => {
                tolerate(notes, entry, stage, e).await?;
                continue;
            }
        };
        match invoke::<SegmentsResult>(ic, entry, stage, arg, Some(cancel), None).await {
            Invoke::Value(r) => {
                if r.drop.iter().any(|id| !all_ids.contains(id)) {
                    tolerate(notes, entry, stage, "drop 含未知分片 id".to_string()).await?;
                    continue;
                }
                if r.rewrite.iter().any(|w| !all_ids.contains(&w.id)) {
                    tolerate(notes, entry, stage, "rewrite 含未知分片 id".to_string()).await?;
                    continue;
                }
                let mut resolved: Vec<(u64, String)> = Vec::with_capacity(r.rewrite.len());
                for w in &r.rewrite {
                    match resolve_segment_uri(&header.playlist_url, &w.uri) {
                        Some(uri) => resolved.push((w.id, uri)),
                        None => break,
                    }
                }
                if resolved.len() != r.rewrite.len() {
                    tolerate(notes, entry, stage, "rewrite 含非法分片地址".to_string()).await?;
                    continue;
                }
                let mut union = decision.dropped.clone();
                union.extend(r.drop.iter().copied());
                if union.len() >= all_ids.len() {
                    tolerate(notes, entry, stage, "过滤后没有剩余分片".to_string()).await?;
                    continue;
                }
                let added = union.len() - decision.dropped.len();
                decision.dropped = union;
                let rewritten = resolved
                    .iter()
                    .filter(|(id, _)| !decision.dropped.contains(id))
                    .count();
                for (id, uri) in resolved {
                    if !decision.dropped.contains(&id) {
                        decision.rewrites.insert(id, uri);
                    }
                }
                let dropped_ids = &decision.dropped;
                decision.rewrites.retain(|id, _| !dropped_ids.contains(id));
                let track = header.track.label();
                if added > 0 {
                    notes
                        .add(format!(
                            "插件 {} 过滤了 {added} 个{track}分片",
                            entry.identity
                        ))
                        .await;
                }
                if rewritten > 0 {
                    notes
                        .add(format!(
                            "插件 {} 改写了 {rewritten} 个{track}分片地址",
                            entry.identity
                        ))
                        .await;
                }
            }
            Invoke::Nothing => {}
            Invoke::Failed(e) => tolerate(notes, entry, stage, e).await?,
            Invoke::Cancelled => return Ok(decision),
        }
    }
    Ok(decision)
}

/// `finalize` 的输入：下载产物位于 `save_dir/file_name`。
pub struct FinalizeInput<'a> {
    pub task: &'a TaskCtx,
    pub db: &'a Db,
    pub task_id: &'a str,
    pub save_dir: PathBuf,
    pub file_name: String,
    pub total_bytes: i64,
    pub audio_path: Option<String>,
}

/// 把产物替换为 `save_dir/name`：返回 `Ok(None)` 表示与当前同名、无事可做。
async fn replace_output(
    input: &FinalizeInput<'_>,
    current: &str,
    name: &str,
) -> Result<Option<i64>, String> {
    if !super::patch::valid_file_name(name) {
        return Err("replaceWith 非法".to_string());
    }
    let target = input.save_dir.join(name);
    let meta = match tokio::fs::metadata(&target).await {
        Ok(m) if m.is_file() => m,
        _ => return Err("replaceWith 指向的文件不存在".to_string()),
    };
    if name == current {
        return Ok(None);
    }
    match tokio::fs::remove_file(input.save_dir.join(current)).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("删除原文件失败: {e}")),
    }
    let size = i64::try_from(meta.len()).unwrap_or(i64::MAX);
    input
        .db
        .update_task_file_info(input.task_id, name, size)
        .await
        .map_err(|e| format!("更新文件信息失败: {e}"))?;
    input
        .db
        .set_task_file_name_with_source(input.task_id, name, false)
        .await
        .map_err(|e| format!("更新文件名失败: {e}"))?;
    Ok(Some(size))
}

/// `finalize`：依次执行；`replaceWith` 生效后下一个插件看到新文件。返回最终
/// 替换结果 `(文件名, 大小)`，无替换为 `None`。
pub async fn run_finalize(
    ic: &dyn TaskInterceptor,
    notes: &ActivityNotes,
    cancel: &CancellationToken,
    input: FinalizeInput<'_>,
) -> Result<Option<(String, i64)>, InterceptAbort> {
    let stage = InterceptStage::Finalize;
    let mut name = input.file_name.clone();
    let mut size = input.total_bytes;
    let mut replaced: Option<(String, i64)> = None;
    let save_dir = input.save_dir.to_string_lossy().into_owned();
    for entry in entries_for(ic, stage) {
        let ctx = FinalizeCtx {
            task: input.task,
            file_path: input.save_dir.join(&name).to_string_lossy().into_owned(),
            file_name: name.clone(),
            save_dir: save_dir.clone(),
            total_bytes: size,
            audio_path: input.audio_path.clone(),
        };
        let arg = match ctx_value(&ctx) {
            Ok(v) => v,
            Err(e) => {
                tolerate(notes, entry, stage, e).await?;
                continue;
            }
        };
        match invoke::<FinalizeResult>(
            ic,
            entry,
            stage,
            arg,
            Some(cancel),
            Some(input.save_dir.as_path()),
        )
        .await
        {
            Invoke::Value(r) => {
                if let Some(message) = r.fail {
                    return Err(InterceptAbort {
                        identity: entry.identity.clone(),
                        stage,
                        message,
                    });
                }
                if let Some(next) = r.replace_with {
                    match replace_output(&input, &name, &next).await {
                        Ok(Some(new_size)) => {
                            notes
                                .add(format!("插件 {} 将下载文件替换为 {next}", entry.identity))
                                .await;
                            name = next;
                            size = new_size;
                            replaced = Some((name.clone(), size));
                        }
                        Ok(None) => {}
                        Err(e) => tolerate(notes, entry, stage, e).await?,
                    }
                }
            }
            Invoke::Nothing => {}
            Invoke::Failed(e) => tolerate(notes, entry, stage, e).await?,
            Invoke::Cancelled => {
                notes
                    .add("插件后处理已取消，任务按原文件完成".to_string())
                    .await;
                return Ok(replaced);
            }
        }
    }
    Ok(replaced)
}

/// `failed`：首个返回 `retry` 的插件胜出，返回重试延迟（毫秒）。
pub async fn run_failed(
    ic: &dyn TaskInterceptor,
    notes: &ActivityNotes,
    task: &TaskCtx,
    error_message: String,
    http_status: Option<u16>,
    auto_retry_count: u32,
) -> Option<u64> {
    let stage = InterceptStage::Failed;
    let ctx = FailedCtx {
        task,
        error_message,
        http_status,
        auto_retry_count,
    };
    let arg = match ctx_value(&ctx) {
        Ok(v) => v,
        Err(e) => {
            log_warn!("[intercept] task {} failed-stage context: {e}", task.id);
            return None;
        }
    };
    for entry in entries_for(ic, stage) {
        let outcome = match invoke::<FailedResult>(ic, entry, stage, arg.clone(), None, None).await
        {
            Invoke::Value(FailedResult { retry: Some(req) }) => {
                let delay = req.delay_ms.unwrap_or(0).min(300_000);
                notes
                    .add(format!("插件 {} 请求在 {delay} 毫秒后重试", entry.identity))
                    .await;
                return Some(delay);
            }
            Invoke::Value(FailedResult { retry: None }) | Invoke::Nothing | Invoke::Cancelled => {
                Ok(())
            }
            Invoke::Failed(e) => tolerate(notes, entry, stage, e).await,
        };
        if let Err(abort) = outcome {
            log_warn!("[intercept] task {} {abort}", task.id);
        }
    }
    None
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::downloader::RequestSpec;
    use crate::events::{EngineEvent, EventSink};
    use crate::intercept::ProtocolKind;

    /// 按 `(identity, stage)` 返回预设结果的拦截器，并记录收到的上下文。
    pub(crate) struct ScriptedInterceptor {
        pub entries: Vec<InterceptorEntry>,
        pub replies: HashMap<(String, InterceptStage), Result<Option<String>, String>>,
        pub seen: Mutex<Vec<(String, InterceptStage, serde_json::Value)>>,
    }

    impl ScriptedInterceptor {
        pub(crate) fn new(entries: Vec<InterceptorEntry>) -> Self {
            Self {
                entries,
                replies: HashMap::new(),
                seen: Mutex::new(Vec::new()),
            }
        }

        pub(crate) fn reply(
            mut self,
            identity: &str,
            stage: InterceptStage,
            reply: Result<Option<&str>, &str>,
        ) -> Self {
            self.replies.insert(
                (identity.to_string(), stage),
                reply.map(|r| r.map(str::to_string)).map_err(str::to_string),
            );
            self
        }
    }

    #[async_trait::async_trait]
    impl TaskInterceptor for ScriptedInterceptor {
        fn entries(&self) -> &[InterceptorEntry] {
            &self.entries
        }

        fn source_url(&self) -> &str {
            "https://example.com/source"
        }

        async fn call(
            &self,
            identity: &str,
            stage: InterceptStage,
            arg_json: String,
            _file_root: Option<PathBuf>,
        ) -> Result<Option<String>, String> {
            let value: serde_json::Value =
                serde_json::from_str(&arg_json).map_err(|e| e.to_string())?;
            self.seen
                .lock()
                .expect("seen lock")
                .push((identity.to_string(), stage, value));
            self.replies
                .get(&(identity.to_string(), stage))
                .cloned()
                .unwrap_or(Ok(None))
        }
    }

    pub(crate) fn entry(
        identity: &str,
        stages: &[InterceptStage],
        fail_task: bool,
    ) -> InterceptorEntry {
        InterceptorEntry {
            identity: identity.to_string(),
            stages: stages.to_vec(),
            fail_task,
        }
    }

    struct NullSink;
    impl EventSink for NullSink {
        fn emit(&self, _event: EngineEvent) {}
    }

    async fn notes_with_task() -> (Db, ActivityNotes) {
        let db = Db::connect("sqlite::memory:").await.expect("db");
        db.insert_task(
            "t1",
            "https://example.com/a.bin",
            "a.bin",
            "/tmp",
            1,
            0,
            "",
            "",
            "",
            0,
        )
        .await
        .expect("insert task");
        let notes = ActivityNotes::new(db.clone(), Arc::new(NullSink), "t1".to_string());
        (db, notes)
    }

    async fn plugin_notes(db: &Db) -> Vec<String> {
        let page = db
            .query_task_activity(&crate::task_activity::TaskActivityQuery {
                task_id: "t1".to_string(),
                limit: 100,
                ..Default::default()
            })
            .await
            .expect("activity");
        page.entries
            .into_iter()
            .filter(|e| e.kind == "plugin")
            .map(|e| e.message)
            .collect()
    }

    fn task() -> TaskCtx {
        TaskCtx {
            id: "t1".to_string(),
            url: "https://example.com/a.bin".to_string(),
            source_url: "https://example.com/a.bin".to_string(),
            file_name: "a.bin".to_string(),
            save_dir: "/tmp".to_string(),
            protocol: ProtocolKind::Http.as_str(),
            track_pair: false,
            is_resume: false,
        }
    }

    async fn prepare(
        ic: &ScriptedInterceptor,
        notes: &ActivityNotes,
        spec: &mut RequestSpec,
    ) -> Result<PrepareOutcome, InterceptAbort> {
        let mut url = "https://example.com/a.bin".to_string();
        let mut segments = 0;
        let mut cap = 0;
        let mut target = RequestTarget {
            url: &mut url,
            spec,
            file_name: None,
            segment_count: Some(&mut segments),
            connection_cap: Some(&mut cap),
            hint_file_size: None,
            range_verified: None,
            is_resume: false,
            kind: ProtocolKind::Http,
            has_audio_track: false,
        };
        run_prepare(
            ic,
            notes,
            &CancellationToken::new(),
            &task(),
            &RunMeta::default(),
            0,
            &mut target,
        )
        .await
    }

    #[tokio::test]
    async fn continue_policy_notes_and_skips_failed_plugin() {
        let (db, notes) = notes_with_task().await;
        let ic = ScriptedInterceptor::new(vec![
            entry("a@a", &[InterceptStage::Prepare], false),
            entry("b@b", &[InterceptStage::Prepare], false),
        ])
        .reply("a@a", InterceptStage::Prepare, Err("boom"))
        .reply(
            "b@b",
            InterceptStage::Prepare,
            Ok(Some(r#"{"setHeaders":{"X-B":"1"}}"#)),
        );
        let mut spec = RequestSpec::default();
        let outcome = prepare(&ic, &notes, &mut spec).await.expect("continue");
        assert!(!outcome.cancelled);
        assert_eq!(spec.extra_headers.get("X-B").map(String::as_str), Some("1"));
        let notes = plugin_notes(&db).await;
        assert!(
            notes
                .iter()
                .any(|n| n == "插件 a@a 的 prepare 阶段出错，已忽略：boom"),
            "{notes:?}"
        );
        assert!(notes.iter().any(|n| n == "插件 b@b 改写了请求：请求头"));
        assert!(notes.iter().all(|n| !n.contains("X-B")));
    }

    #[tokio::test]
    async fn fail_policy_aborts_with_identity_and_stage() {
        let (_db, notes) = notes_with_task().await;
        let ic = ScriptedInterceptor::new(vec![entry("a@b", &[InterceptStage::Prepare], true)])
            .reply(
                "a@b",
                InterceptStage::Prepare,
                Ok(Some(r#"{"setHeaders":{"Host":"x"}}"#)),
            );
        let mut spec = RequestSpec::default();
        let err = prepare(&ic, &notes, &mut spec).await.expect_err("abort");
        assert!(
            err.to_string().starts_with("[插件] a@b（prepare）："),
            "{err}"
        );
        assert!(spec.extra_headers.is_empty());
    }

    fn items(ids: &[u64]) -> Vec<SegmentItem> {
        ids.iter()
            .map(|&id| SegmentItem {
                id,
                uri: format!("s{id}.ts"),
                ..Default::default()
            })
            .collect()
    }

    #[tokio::test]
    async fn segments_union_and_reject_drop_all() {
        let (db, notes) = notes_with_task().await;
        let stages = [InterceptStage::Segments];
        let ic = ScriptedInterceptor::new(vec![
            entry("a@a", &stages, false),
            entry("b@b", &stages, false),
            entry("c@c", &stages, false),
        ])
        .reply(
            "a@a",
            InterceptStage::Segments,
            Ok(Some(r#"{"drop":[11]}"#)),
        )
        .reply(
            "b@b",
            InterceptStage::Segments,
            Ok(Some(r#"{"drop":[12]}"#)),
        )
        .reply(
            "c@c",
            InterceptStage::Segments,
            Ok(Some(r#"{"drop":[10,13]}"#)),
        );
        let task = task();
        let header = SegmentsHeader {
            task: &task,
            format: "hls",
            track: crate::intercept::ctx::SegmentTrack::Video,
            playlist_url: "https://example.com/v.m3u8".to_string(),
            manifest_text: String::new(),
            manifest_text_omitted: false,
            media_sequence: Some(10),
            target_duration: Some(2.0),
            period_index: None,
            representation_id: None,
        };
        let decision = run_segments(
            &ic,
            &notes,
            &CancellationToken::new(),
            InterceptStage::Segments,
            &header,
            &items(&[10, 11, 12, 13]),
        )
        .await
        .expect("segments");
        assert_eq!(
            decision.dropped.into_iter().collect::<Vec<_>>(),
            vec![11, 12]
        );

        // b 只看到 a 过滤后的分片。
        {
            let seen = ic.seen.lock().expect("seen");
            let b_ids: Vec<u64> = seen[1].2["segments"]
                .as_array()
                .expect("segments")
                .iter()
                .filter_map(|s| s["id"].as_u64())
                .collect();
            assert_eq!(b_ids, vec![10, 12, 13]);
            assert_eq!(seen[1].2["stage"], "segments");
        }

        let notes = plugin_notes(&db).await;
        assert!(notes.iter().any(|n| n == "插件 a@a 过滤了 1 个视频分片"));
        assert!(
            notes
                .iter()
                .any(|n| n.starts_with("插件 c@c 的 segments 阶段出错，已忽略：过滤后没有剩余分片"))
        );
    }

    fn hls_header(task: &TaskCtx) -> SegmentsHeader<'_> {
        SegmentsHeader {
            task,
            format: "hls",
            track: crate::intercept::ctx::SegmentTrack::Video,
            playlist_url: "https://example.com/hls/v.m3u8".to_string(),
            manifest_text: String::new(),
            manifest_text_omitted: false,
            media_sequence: Some(10),
            target_duration: Some(2.0),
            period_index: None,
            representation_id: None,
        }
    }

    #[tokio::test]
    async fn segment_rewrites_resolve_relative_and_chain_to_later_plugins() {
        let (db, notes) = notes_with_task().await;
        let stages = [InterceptStage::Segments];
        let ic = ScriptedInterceptor::new(vec![
            entry("a@a", &stages, false),
            entry("b@b", &stages, false),
            entry("c@c", &stages, false),
        ])
        .reply(
            "a@a",
            InterceptStage::Segments,
            Ok(Some(
                r#"{"rewrite":[{"id":10,"uri":"s10.ts?sig=a"},{"id":11,"uri":"https://cdn.example.com/s11.ts"}]}"#,
            )),
        )
        .reply(
            "b@b",
            InterceptStage::Segments,
            Ok(Some(r#"{"drop":[11],"rewrite":[{"id":12,"uri":"javascript:alert(1)"}]}"#)),
        )
        .reply(
            "c@c",
            InterceptStage::Segments,
            Ok(Some(r#"{"drop":[11]}"#)),
        );
        let task = task();
        let decision = run_segments(
            &ic,
            &notes,
            &CancellationToken::new(),
            InterceptStage::Segments,
            &hls_header(&task),
            &items(&[10, 11, 12]),
        )
        .await
        .expect("segments");
        // b 的地址非法：整条决定（含 drop）不采纳；c 的 drop 生效并清掉 11 的改写。
        assert_eq!(decision.dropped.into_iter().collect::<Vec<_>>(), vec![11]);
        assert_eq!(
            decision.rewrites.into_iter().collect::<Vec<_>>(),
            vec![(10, "https://example.com/hls/s10.ts?sig=a".to_string())]
        );
        {
            let seen = ic.seen.lock().expect("seen");
            assert_eq!(
                seen[1].2["segments"][0]["uri"],
                "https://example.com/hls/s10.ts?sig=a"
            );
        }
        let notes = plugin_notes(&db).await;
        assert!(
            notes
                .iter()
                .any(|n| n == "插件 a@a 改写了 2 个视频分片地址")
        );
        assert!(notes.iter().any(|n| n.ends_with("rewrite 含非法分片地址")));
    }

    #[tokio::test]
    async fn refresh_rejects_non_credential_fields_and_applies_tokens() {
        let (db, notes) = notes_with_task().await;
        let stages = [InterceptStage::Refresh];
        let ic = ScriptedInterceptor::new(vec![
            entry("a@a", &stages, false),
            entry("b@b", &stages, false),
        ])
        .reply(
            "a@a",
            InterceptStage::Refresh,
            Ok(Some(r#"{"segments":8}"#)),
        )
        .reply(
            "b@b",
            InterceptStage::Refresh,
            Ok(Some(r#"{"setHeaders":{"X-Token":"tok-new"}}"#)),
        );
        let mut url = "https://example.com/a.bin".to_string();
        let mut spec = RequestSpec::default();
        let mut target = RequestTarget {
            url: &mut url,
            spec: &mut spec,
            file_name: None,
            segment_count: None,
            connection_cap: None,
            hint_file_size: None,
            range_verified: None,
            is_resume: false,
            kind: ProtocolKind::Http,
            has_audio_track: false,
        };
        let changed = run_refresh(
            &ic,
            &notes,
            &CancellationToken::new(),
            &task(),
            &RunMeta::default(),
            403,
            0,
            &mut target,
        )
        .await
        .expect("refresh");
        assert!(changed);
        assert_eq!(
            spec.extra_headers.get("X-Token").map(String::as_str),
            Some("tok-new")
        );
        {
            let seen = ic.seen.lock().expect("seen");
            assert_eq!(seen[0].2["status"], 403);
            assert_eq!(seen[0].2["refreshCount"], 0);
        }
        let notes = plugin_notes(&db).await;
        assert!(
            notes
                .iter()
                .any(|n| n.ends_with("refresh 阶段不能改写 segments"))
        );
        assert!(
            notes
                .iter()
                .any(|n| n == "插件 b@b 刷新了凭据（HTTP 403）：请求头")
        );
        assert!(notes.iter().all(|n| !n.contains("tok-new")));
    }

    #[tokio::test]
    async fn select_first_valid_wins() {
        let (db, notes) = notes_with_task().await;
        let stages = [InterceptStage::SelectVariant];
        let ic = ScriptedInterceptor::new(vec![
            entry("a@a", &stages, false),
            entry("b@b", &stages, false),
            entry("c@c", &stages, false),
        ])
        .reply(
            "a@a",
            InterceptStage::SelectVariant,
            Ok(Some(r#"{"select":5}"#)),
        )
        .reply(
            "b@b",
            InterceptStage::SelectVariant,
            Ok(Some(r#"{"select":1}"#)),
        )
        .reply(
            "c@c",
            InterceptStage::SelectVariant,
            Ok(Some(r#"{"select":0}"#)),
        );
        let picked = run_select(
            &ic,
            &notes,
            &CancellationToken::new(),
            InterceptStage::SelectVariant,
            serde_json::json!({ "variants": [] }),
            2,
        )
        .await
        .expect("select");
        assert_eq!(picked, Some(1));
        assert_eq!(ic.seen.lock().expect("seen").len(), 2);
        let notes = plugin_notes(&db).await;
        assert!(notes.iter().any(|n| n == "插件 b@b 选择了第 2 个画质"));
        assert!(notes.iter().any(|n| n.ends_with("select 超出范围")));
    }
}
