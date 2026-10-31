//! FluxMesh共享（多设备按内容互相取块）的 agent 侧：共享生命周期、云端登记、对端查询。
//!
//! 数据面（`fluxdown/swarm/1`）在 `fluxdown_share`：端点按 [`ContentRegistry`] 提供内容，接收会话按
//! [`SwarmFile`] 列表向对端取块。这里负责三件事：
//!
//! - **提供什么**：[`ShareService::sync_content`] 把「有效的共享分享 + 共享记录里已落盘的文件 + 共享记录下载中的块」
//!   整体算成内容表；所有会改变它的路径最终都经 `refresh_endpoint`，由它在开头调用本函数。
//! - **登记**：已落盘 / 下载中的内容经 `POST /share/swarm/announce` 让云端知道本机持有（`announce`），
//!   撤销与到期同样走这里。云端不认识该接口（404）时本次运行内不再登记与查询。
//! - **向谁取**：接收时向 `POST /share/swarm/peers` 查在线持有者，结果交给会话（`swarm_lookup`），之后按云端给的间隔刷新。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use fluxdown_protocol::{
    ApplicationErrorCode, ErrorReason, RpcErrorData, ShareProvideState, ShareReceiveRecordDto,
    ShareReceiveStatus, ShareStatus, ShareSwarmBlock,
};
use fluxdown_share::hashing::{file_mtime_ms, parse_hash};
use fluxdown_share::timefmt::now_unix_secs;
use fluxdown_share::{
    CHUNK_SIZE, ContentEntry, ContentSource, EndpointId, ReceiveSession, ShareFile, SwarmFile,
    SwarmPeer,
};
use serde_json::{Value, json};

use super::cloud::{AnnounceResponse, PeersResponse};
use super::ops::reason;
use super::service::{Inner, ShareService};
use super::store::{ProvideFile, StoredProvide};
use super::time::parse_rfc3339_secs;
use crate::cloud::CloudError;
use crate::platform::DOWNLOADING_SUFFIX;

/// 登记刷新间隔（云端按 15 分钟判定登记过期）。
const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(300);
/// 登记失败后的重试间隔。
const ANNOUNCE_RETRY: Duration = Duration::from_secs(60);
/// 云端单次登记 / 撤销请求的条数上限（`/share/swarm/announce` 的分页尺寸）；超出的分批发送，不丢条目。
const MAX_ANNOUNCE_ITEMS: usize = 1000;
/// 接收时首次对端查询的等待上限：超时就先开始下载，后台继续等。
pub(super) const FIRST_LOOKUP_TIMEOUT: Duration = Duration::from_secs(3);
/// 查询失败后的重试间隔。
pub(super) const LOOKUP_RETRY: Duration = Duration::from_secs(60);
/// 云端建议的刷新间隔的取值范围。
const REFRESH_MIN: u64 = 30;
const REFRESH_MAX: u64 = 600;
/// 额度用完后的缺省重试间隔与上限（秒）；云端 `details.retryAfterSecs` 优先。
const QUOTA_RETRY_DEFAULT_SECS: u64 = 600;
const QUOTA_RETRY_MAX_SECS: u64 = 3600;

/// 下载完成的接收任务：（任务 ID，记录 ID，清单里的文件）。
pub(super) type FinishedFile = (String, String, ShareFile);

/// 一次对端查询的结果。
pub(super) enum Lookup {
    /// 已交给会话；这段时间之后再查一次。
    Refresh(Duration),
    /// 云端不支持共享。
    Unsupported,
    /// 云端拒绝共享加速（额度用完 / 需要登录）；`Some` = 这段时间之后再查，`None` = 不再重试。
    Blocked(ShareSwarmBlock, Option<Duration>),
}

/// 等待登录的对端查询所需参数。
pub(super) struct LoginWaiting {
    pub share_id: String,
    pub ticket: String,
    pub indexes: Vec<u32>,
}

impl Inner {
    /// 「接收的文件参与共享」（缺省开）。
    pub(super) fn auto_provide(&self) -> bool {
        self.local.auto_provide.unwrap_or(true)
    }

    /// 把共享状态叠加到接收记录上（持久化的记录本身不带它）。
    pub(super) fn record_dto(&self, record: &ShareReceiveRecordDto) -> ShareReceiveRecordDto {
        let mut dto = record.clone();
        dto.provide_state = self.provide_state_of(&record.id);
        dto
    }

    /// 接收记录的共享状态（`record_dto` 与任务事实共用）。
    pub(super) fn provide_state_of(&self, record_id: &str) -> ShareProvideState {
        self.provides
            .iter()
            .find(|s| s.record_id == record_id)
            .map_or(ShareProvideState::Off, |s| s.state)
    }

    fn is_providing(&self, record_id: &str) -> bool {
        self.provides
            .iter()
            .any(|s| s.record_id == record_id && s.state == ShareProvideState::Providing)
    }

    /// 清单里第 `file_index` 个文件的根哈希（来自接收任务所属会话）。
    fn task_root(&self, task: &super::service::ReceiveTask) -> Option<ShareFile> {
        let index = usize::try_from(task.file_index).ok()?;
        self.receive_sessions
            .get(&task.token)?
            .manifest()
            .files
            .get(index)
            .cloned()
    }

    /// 本机已知的「文件根哈希 → 文件名」：分享清单、共享的已落盘文件、下载中的接收文件。
    /// 诊断页的共享提供会话据此把根哈希显示成文件名；查不到的内容由 UI 回退显示短哈希。
    pub(super) fn root_names(&self) -> HashMap<String, String> {
        let mut names = HashMap::new();
        for stored in &self.shares {
            for file in &stored.manifest.files {
                names
                    .entry(file.root_hash.clone())
                    .or_insert_with(|| file.name.clone());
            }
        }
        for provide in &self.provides {
            for file in &provide.files {
                if let Some(name) = file.path.file_name().and_then(|n| n.to_str()) {
                    names
                        .entry(file.root_hash.clone())
                        .or_insert_with(|| name.to_owned());
                }
            }
        }
        for task in self.receive_tasks.values() {
            if let Some(file) = self.task_root(task) {
                names.entry(file.root_hash).or_insert(file.name);
            }
        }
        names
    }

    /// 一次共享涉及的全部根哈希：已落盘的文件 + 同记录仍在下载的文件。
    fn provide_roots(&self, provide: &StoredProvide) -> Vec<String> {
        let mut roots: Vec<String> = provide.files.iter().map(|f| f.root_hash.clone()).collect();
        for task in self
            .receive_tasks
            .values()
            .filter(|t| t.record_id == provide.record_id && t.is_active())
        {
            if let Some(file) = self.task_root(task) {
                roots.push(file.root_hash);
            }
        }
        roots.sort_unstable();
        roots.dedup();
        roots
    }

    fn queue_withdraw(&mut self, roots: Vec<String>) {
        for root in roots {
            if !self.withdraw.contains(&root) {
                self.withdraw.push(root);
            }
        }
    }

    /// 到期的共享置为已结束；返回是否有变化。
    pub(super) fn end_expired_provides(&mut self, now: u64) -> bool {
        let mut changed = false;
        for provide in &mut self.provides {
            if provide.state == ShareProvideState::Providing
                && provide
                    .share_expires_at
                    .as_deref()
                    .and_then(parse_rfc3339_secs)
                    .is_some_and(|t| t <= now)
            {
                provide.state = ShareProvideState::Ended;
                changed = true;
            }
        }
        changed
    }

    /// 没有任何文件、记录也不再有进行中任务的共享无内容可提供：置为已结束。
    pub(super) fn settle_empty_provides(&mut self) -> bool {
        let mut changed = false;
        for idx in 0..self.provides.len() {
            let provide = &self.provides[idx];
            if provide.state != ShareProvideState::Providing || !provide.files.is_empty() {
                continue;
            }
            let active = self
                .records
                .iter()
                .any(|r| r.id == provide.record_id && r.status == ShareReceiveStatus::Active);
            if !active {
                self.provides[idx].state = ShareProvideState::Ended;
                changed = true;
            }
        }
        changed
    }

    /// 停止一次共享并把它的全部根哈希排进撤销队列；原本就不在共享返回 `false`。
    fn stop_provide(&mut self, record_id: &str) -> bool {
        let Some(idx) = self
            .provides
            .iter()
            .position(|s| s.record_id == record_id && s.state == ShareProvideState::Providing)
        else {
            return false;
        };
        let roots = self.provide_roots(&self.provides[idx]);
        self.queue_withdraw(roots);
        self.provides[idx].state = ShareProvideState::Stopped;
        true
    }

    /// 刚完成的接收任务：记录在共享时返回（任务 ID，记录 ID，清单文件），否则 `None`。
    pub(super) fn finished_provide_file(&self, task_id: &str) -> Option<FinishedFile> {
        let task = self.receive_tasks.get(task_id)?;
        if !self.is_providing(&task.record_id) {
            return None;
        }
        let file = self.task_root(task)?;
        Some((task_id.to_owned(), task.record_id.clone(), file))
    }

    /// 本机当前能向别人提供的全部内容。
    fn content_entries(
        &self,
        locations: &HashMap<String, (PathBuf, String)>,
        now: u64,
    ) -> Vec<([u8; 32], ContentEntry)> {
        let mut out = Vec::new();
        // 有效的、加入共享的分享（含本机发起的）。
        for stored in &self.shares {
            if stored.dto.status != ShareStatus::Active
                || !stored.dto.swarm
                || stored.dto.source_missing
                || parse_rfc3339_secs(&stored.dto.expires_at).is_some_and(|t| t <= now)
            {
                continue;
            }
            for (file, path) in stored.manifest.files.iter().zip(&stored.paths) {
                if let Some(root) = parse_hash(&file.root_hash) {
                    out.push((
                        root,
                        ContentEntry {
                            size: file.size,
                            source: ContentSource::Complete {
                                path: path.clone(),
                                mtime_ms: file.mtime_ms,
                            },
                        },
                    ));
                }
            }
        }
        // 共享：已落盘的完整文件。
        for provide in self
            .provides
            .iter()
            .filter(|s| s.state == ShareProvideState::Providing)
            .filter(|s| {
                !s.share_expires_at
                    .as_deref()
                    .and_then(parse_rfc3339_secs)
                    .is_some_and(|t| t <= now)
            })
        {
            for file in &provide.files {
                if let Some(root) = parse_hash(&file.root_hash) {
                    out.push((
                        root,
                        ContentEntry {
                            size: file.size,
                            source: ContentSource::Complete {
                                path: file.path.clone(),
                                mtime_ms: file.mtime_ms,
                            },
                        },
                    ));
                }
            }
        }
        // 共享：下载中已校验的块（只在能确定临时文件位置时提供）。
        if self.auto_provide() {
            for (task_id, task) in &self.receive_tasks {
                if !task.is_active() || !self.is_providing(&task.record_id) {
                    continue;
                }
                let Some((dir, name)) = locations.get(task_id) else {
                    continue;
                };
                let Some(session) = self.receive_sessions.get(&task.token) else {
                    continue;
                };
                let (Some(file), Some(have)) =
                    (self.task_root(task), session.have(task.file_index))
                else {
                    continue;
                };
                let Some(root) = parse_hash(&file.root_hash) else {
                    continue;
                };
                let mut temp = dir.join(name).into_os_string();
                temp.push(DOWNLOADING_SUFFIX);
                out.push((
                    root,
                    ContentEntry {
                        size: file.size,
                        source: ContentSource::Partial {
                            path: PathBuf::from(temp),
                            file,
                            have,
                        },
                    },
                ));
            }
        }
        out
    }

    /// 需要向云端登记的条目：共享的完整文件 + 共享记录下载中的文件（`complete=false`）。
    fn announce_entries(&self) -> Vec<Value> {
        let mut entries = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for provide in self
            .provides
            .iter()
            .filter(|s| s.state == ShareProvideState::Providing)
        {
            for file in &provide.files {
                if seen.insert(file.root_hash.clone()) {
                    entries.push(json!({
                        "shareId": provide.share_id,
                        "ticket": provide.ticket,
                        "rootHash": file.root_hash,
                        "size": file.size,
                        "complete": true,
                        "haveChunks": file.size.div_ceil(CHUNK_SIZE),
                    }));
                }
            }
        }
        if self.auto_provide() {
            for task in self.receive_tasks.values().filter(|t| t.is_active()) {
                let Some(provide) = self.provides.iter().find(|s| {
                    s.record_id == task.record_id && s.state == ShareProvideState::Providing
                }) else {
                    continue;
                };
                let Some(file) = self.task_root(task) else {
                    continue;
                };
                let Some(have) = self
                    .receive_sessions
                    .get(&task.token)
                    .and_then(|s| s.have(task.file_index))
                else {
                    continue;
                };
                if seen.insert(file.root_hash.clone()) {
                    entries.push(json!({
                        "shareId": provide.share_id,
                        "ticket": provide.ticket,
                        "rootHash": file.root_hash,
                        "size": file.size,
                        "complete": false,
                        "haveChunks": have.count(),
                    }));
                }
            }
        }
        entries
    }
}

/// 落盘文件仍是登记时的那一个（大小与修改时间都没变）。
async fn provide_file_intact(file: &ProvideFile) -> bool {
    match tokio::fs::metadata(&file.path).await {
        Ok(meta) => {
            meta.is_file()
                && meta.len() == file.size
                && file_mtime_ms(&meta).is_ok_and(|m| m == file.mtime_ms)
        }
        Err(_) => false,
    }
}

impl ShareService {
    // ───────────── 内容表 ─────────────

    /// 重算本机能提供的内容并整体替换内容表。纯内存 + 一次快照读取，不访问网络与磁盘。
    pub(super) fn sync_content(&self) {
        let active: HashSet<String> = self.with(|i| {
            i.receive_tasks
                .iter()
                .filter(|(_, t)| t.is_active())
                .map(|(id, _)| id.clone())
                .collect()
        });
        let locations: HashMap<String, (PathBuf, String)> = if active.is_empty() {
            HashMap::new()
        } else {
            self.events.inspect(|snapshot| {
                snapshot
                    .daemon
                    .tasks
                    .iter()
                    .filter(|t| {
                        active.contains(&t.task_id)
                            && !t.save_dir.is_empty()
                            && !t.file_name.is_empty()
                    })
                    .map(|t| {
                        (
                            t.task_id.clone(),
                            (PathBuf::from(&t.save_dir), t.file_name.clone()),
                        )
                    })
                    .collect()
            })
        };
        let now = now_unix_secs();
        let entries = self.with(|i| i.content_entries(&locations, now));
        self.content.replace_all(entries);
    }

    // ───────────── 共享生命周期 ─────────────

    pub(super) async fn persist_provides(&self) {
        let _guard = self.persist.lock().await;
        // 记录被 `RECORD_LIMIT` 挤掉后，它的共享成了孤儿：移除并撤销登记。
        let (provides, orphaned) = self.with(|i| {
            let known: HashSet<String> = i.records.iter().map(|r| r.id.clone()).collect();
            let roots: Vec<String> = i
                .provides
                .iter()
                .filter(|s| !known.contains(&s.record_id))
                .flat_map(|s| i.provide_roots(s))
                .collect();
            let before = i.provides.len();
            i.provides.retain(|s| known.contains(&s.record_id));
            let orphaned = i.provides.len() != before;
            i.queue_withdraw(roots);
            (i.provides.clone(), orphaned)
        });
        if let Err(error) = self.store.save_provides(&provides).await {
            tracing::warn!(error = %format!("{error:#}"), "could not persist provides");
        }
        if orphaned {
            self.announce_soon();
        }
    }

    /// 共享状态变了（置为已结束 / 已停止、文件被移除）之后的统一收尾。
    async fn provides_changed(&self) {
        self.persist_provides().await;
        self.announce_soon();
        self.refresh_endpoint().await;
        self.publish().await;
    }

    /// 完成的接收任务转为共享：核对最终文件后加入记录的共享；同时让没有内容可提供的共享结束。
    pub(super) async fn add_provide_files(&self, finished: Vec<FinishedFile>) {
        let mut located = Vec::new();
        for (task_id, record_id, file) in finished {
            let location = self.events.inspect(|snapshot| {
                snapshot
                    .daemon
                    .tasks
                    .iter()
                    .find(|t| t.task_id == task_id)
                    .map(|t| (t.save_dir.clone(), t.file_name.clone()))
            });
            let Some((dir, name)) = location.filter(|(d, n)| !d.is_empty() && !n.is_empty()) else {
                tracing::warn!(task = %task_id, "finished share task has no location; not provideed");
                continue;
            };
            let path = PathBuf::from(dir).join(name);
            match tokio::fs::metadata(&path).await {
                Ok(meta) if meta.is_file() && meta.len() == file.size => {
                    match file_mtime_ms(&meta) {
                        Ok(mtime_ms) => located.push((
                            record_id,
                            ProvideFile {
                                task_id,
                                root_hash: file.root_hash,
                                size: file.size,
                                mtime_ms,
                                path,
                            },
                        )),
                        Err(error) => tracing::warn!(
                            path = %path.display(),
                            error = %error,
                            "finished share file has no modified time; not provideed"
                        ),
                    }
                }
                Ok(_) => tracing::warn!(
                    path = %path.display(),
                    "finished share file does not match the share; not provideed"
                ),
                Err(error) => tracing::warn!(
                    path = %path.display(),
                    error = %error,
                    "finished share file is unreadable; not provideed"
                ),
            }
        }
        let changed =
            self.with(|i| {
                let mut changed = false;
                for (record_id, file) in located {
                    if let Some(provide) = i.provides.iter_mut().find(|s| {
                        s.record_id == record_id && s.state == ShareProvideState::Providing
                    }) && !provide.files.iter().any(|f| f.task_id == file.task_id)
                    {
                        provide.files.push(file);
                        changed = true;
                    }
                }
                i.settle_empty_provides() | changed
            });
        if changed {
            self.persist_provides().await;
            self.announce_soon();
        }
    }

    /// 接收任务被删除：它对应的共享文件不再提供，并撤销登记。返回是否有变化。
    pub(super) async fn drop_provide_tasks(&self, ids: &[String]) -> bool {
        let changed = self.with(|i| {
            let mut roots = Vec::new();
            for provide in &mut i.provides {
                provide.files.retain(|f| {
                    let gone = ids.contains(&f.task_id);
                    if gone {
                        roots.push(f.root_hash.clone());
                    }
                    !gone
                });
            }
            let changed = !roots.is_empty();
            i.queue_withdraw(roots);
            changed
        });
        if changed {
            self.persist_provides().await;
            self.announce_soon();
        }
        changed
    }

    /// 启动：已过期的共享结束，文件不在或已变的移除，没有文件的结束（重启后没有进行中的接收任务）。
    pub(super) async fn restore_provides(&self) {
        if self.with(|i| i.provides.is_empty()) {
            return;
        }
        let now = now_unix_secs();
        let candidates: Vec<(String, ProvideFile)> = self.with(|i| {
            i.end_expired_provides(now);
            i.provides
                .iter()
                .filter(|s| s.state == ShareProvideState::Providing)
                .flat_map(|s| s.files.iter().map(|f| (s.record_id.clone(), f.clone())))
                .collect()
        });
        let mut lost = Vec::new();
        for (record_id, file) in candidates {
            if !provide_file_intact(&file).await {
                lost.push((record_id, file.task_id));
            }
        }
        let providing = self.with(|i| {
            let mut roots = Vec::new();
            for provide in &mut i.provides {
                provide.files.retain(|f| {
                    let gone = lost
                        .iter()
                        .any(|(rec, task)| *rec == provide.record_id && *task == f.task_id);
                    if gone {
                        roots.push(f.root_hash.clone());
                    }
                    !gone
                });
                if provide.state == ShareProvideState::Providing && provide.files.is_empty() {
                    provide.state = ShareProvideState::Ended;
                }
            }
            i.queue_withdraw(roots);
            i.provides
                .iter()
                .any(|s| s.state == ShareProvideState::Providing)
        });
        self.persist_provides().await;
        if providing || self.with(|i| !i.withdraw.is_empty()) {
            self.announce_soon();
        }
        self.refresh_endpoint().await;
    }

    /// `SessionEvent::ContentChanged`：共享提供的完整文件被删 / 被改。
    pub(super) async fn on_content_changed(&self, root_hash: &str) {
        let share_ids: Vec<String> = self.with(|i| {
            i.shares
                .iter()
                .filter(|s| {
                    s.dto.status == ShareStatus::Active
                        && s.manifest
                            .files
                            .iter()
                            .any(|f| f.root_hash.eq_ignore_ascii_case(root_hash))
                })
                .map(|s| s.dto.id.clone())
                .collect()
        });
        for id in share_ids {
            self.on_source_changed(&id).await;
        }
        let candidates: Vec<(String, ProvideFile)> = self.with(|i| {
            i.provides
                .iter()
                .filter(|s| s.state == ShareProvideState::Providing)
                .flat_map(|s| s.files.iter().map(|f| (s.record_id.clone(), f.clone())))
                .filter(|(_, f)| f.root_hash.eq_ignore_ascii_case(root_hash))
                .collect()
        });
        let mut lost = Vec::new();
        for (record_id, file) in candidates {
            if !provide_file_intact(&file).await {
                lost.push((record_id, file.task_id));
            }
        }
        if lost.is_empty() {
            return;
        }
        self.with(|i| {
            let mut roots = Vec::new();
            for provide in &mut i.provides {
                provide.files.retain(|f| {
                    let gone = lost
                        .iter()
                        .any(|(rec, task)| *rec == provide.record_id && *task == f.task_id);
                    if gone {
                        roots.push(f.root_hash.clone());
                    }
                    !gone
                });
            }
            i.queue_withdraw(roots);
            i.settle_empty_provides();
        });
        self.provides_changed().await;
    }

    /// `agent.share.stopProviding`：停止该次接收的共享并撤销登记；已不在共享则原样返回（幂等）。
    pub(super) async fn stop_providing(
        &self,
        record_id: &str,
    ) -> Result<ShareReceiveRecordDto, RpcErrorData> {
        let not_found = || {
            reason(
                ApplicationErrorCode::NotFound,
                false,
                ErrorReason::ShareNotFound,
            )
        };
        if !self.with(|i| i.records.iter().any(|r| r.id == record_id)) {
            return Err(not_found());
        }
        if self.with(|i| i.stop_provide(record_id)) {
            self.provides_changed().await;
        }
        self.with(|i| {
            i.records
                .iter()
                .find(|r| r.id == record_id)
                .map(|r| i.record_dto(r))
        })
        .ok_or_else(not_found)
    }

    /// 关闭「接收的文件参与共享」：停止全部共享。
    pub(super) async fn stop_all_provides(&self) {
        let any = self.with(|i| {
            let ids: Vec<String> = i
                .provides
                .iter()
                .filter(|s| s.state == ShareProvideState::Providing)
                .map(|s| s.record_id.clone())
                .collect();
            let mut any = false;
            for id in ids {
                any |= i.stop_provide(&id);
            }
            any
        });
        if any {
            self.provides_changed().await;
        }
    }

    // ───────────── 登记 ─────────────

    /// 尽快登记（合并到事件循环的下一轮）。
    pub(super) fn announce_soon(&self) {
        self.with(|i| i.announce_at = Some(Instant::now()));
        self.wake.notify_one();
    }

    pub(super) fn next_announce_delay(&self) -> Option<Duration> {
        self.with(|i| {
            i.announce_at
                .map(|at| at.saturating_duration_since(Instant::now()))
        })
    }

    /// 向云端登记本机持有的内容并撤销已不提供的；结果决定下一次登记的时刻。
    pub(super) async fn announce(&self) {
        if !self.is_enabled() {
            self.with(|i| i.announce_at = None);
            return;
        }
        self.persist_daily().await;
        if self.with(|i| i.swarm_unsupported) {
            self.with(|i| i.announce_at = None);
            return;
        }
        if self.with(|i| i.end_expired_provides(now_unix_secs())) {
            self.provides_changed().await;
        }
        let (entries, withdraw) =
            self.with(|i| (i.announce_entries(), std::mem::take(&mut i.withdraw)));
        if entries.is_empty() && withdraw.is_empty() {
            self.with(|i| i.announce_at = None);
            return;
        }
        let mut entry_batches = entries.chunks(MAX_ANNOUNCE_ITEMS);
        let mut withdraw_batches = withdraw.chunks(MAX_ANNOUNCE_ITEMS);
        let mut ended: Vec<String> = Vec::new();
        let mut withdrawn = 0usize;
        let mut failed = false;
        loop {
            let batch_entries = entry_batches.next().unwrap_or_default();
            let batch_withdraw = withdraw_batches.next().unwrap_or_default();
            if batch_entries.is_empty() && batch_withdraw.is_empty() {
                break;
            }
            match self.post_announce(batch_entries, batch_withdraw).await {
                Ok(response) => {
                    ended.extend(
                        response
                            .entries
                            .into_iter()
                            .filter(|e| matches!(e.state.as_str(), "ended" | "rejected"))
                            .map(|e| e.root_hash),
                    );
                    withdrawn += batch_withdraw.len();
                }
                Err(error) if error.status == Some(404) => {
                    self.with(|i| {
                        i.swarm_unsupported = true;
                        i.announce_at = None;
                    });
                    return;
                }
                Err(error) => {
                    tracing::debug!(error = %error, "swarm announce failed; will retry");
                    failed = true;
                    break;
                }
            }
        }
        let changed = self.with(|i| {
            if failed {
                // 未送达的撤销放回队首，排在期间新入队的之前。
                let mut merged = withdraw.get(withdrawn..).unwrap_or_default().to_vec();
                merged.extend(std::mem::take(&mut i.withdraw));
                i.withdraw = merged;
            }
            let hit: Vec<usize> = i
                .provides
                .iter()
                .enumerate()
                .filter(|(_, s)| {
                    s.state == ShareProvideState::Providing
                        && i.provide_roots(s)
                            .iter()
                            .any(|r| ended.iter().any(|e| e.eq_ignore_ascii_case(r)))
                })
                .map(|(idx, _)| idx)
                .collect();
            for idx in &hit {
                if let Some(provide) = i.provides.get_mut(*idx) {
                    provide.state = ShareProvideState::Ended;
                }
            }
            let next = if failed {
                Some(ANNOUNCE_RETRY)
            } else if !i.withdraw.is_empty() {
                Some(Duration::ZERO)
            } else if i.announce_entries().is_empty() {
                None
            } else {
                Some(ANNOUNCE_INTERVAL)
            };
            i.announce_at = next.map(|d| Instant::now() + d);
            !hit.is_empty()
        });
        if changed {
            self.provides_changed().await;
        }
    }

    /// 发送一批登记 / 撤销（每批单独签名）。
    async fn post_announce(
        &self,
        entries: &[Value],
        withdraw: &[String],
    ) -> Result<AnnounceResponse, CloudError> {
        let endpoint = self.secret.public().to_z32();
        let timestamp = now_unix_secs();
        let signature = self
            .secret
            .sign(format!("fluxdown-share-swarm-announce/v1\n{endpoint}\n{timestamp}").as_bytes());
        let body = json!({
            "endpointId": endpoint,
            "timestamp": timestamp,
            "signature": URL_SAFE_NO_PAD.encode(signature.to_bytes()),
            "entries": entries,
            "withdraw": withdraw,
        });
        self.cloud.share_swarm_announce(&body).await
    }

    // ───────────── 对端查询 ─────────────

    /// 向云端查询在线持有同样内容的设备并交给会话。
    pub(super) async fn swarm_lookup(
        &self,
        session: &ReceiveSession,
        share_id: &str,
        ticket: &str,
        indexes: &[u32],
    ) -> Lookup {
        if self.with(|i| i.swarm_unsupported) {
            return Lookup::Unsupported;
        }
        let body = json!({ "shareId": share_id, "ticket": ticket, "files": indexes });
        match self
            .cloud
            .share_swarm_peers::<_, PeersResponse>(&body)
            .await
        {
            Ok(response) => {
                let manifest = session.manifest();
                let files: Vec<SwarmFile> = response
                    .files
                    .into_iter()
                    .filter_map(|f| {
                        // 云端给的根哈希必须与清单一致，否则该票据对不上这个文件。
                        let file = manifest.files.get(usize::try_from(f.file_index).ok()?)?;
                        if !file.root_hash.eq_ignore_ascii_case(&f.root_hash) {
                            return None;
                        }
                        let peers = f
                            .peers
                            .into_iter()
                            .filter_map(|p| {
                                let endpoint_id = EndpointId::from_z32(&p.endpoint_id).ok()?;
                                Some(SwarmPeer {
                                    endpoint_id,
                                    complete: p.complete,
                                    addr_hint: self.addr_hint_for(&p.endpoint_id),
                                })
                            })
                            .collect();
                        Some(SwarmFile {
                            file_index: f.file_index,
                            ticket: f.ticket,
                            peers,
                        })
                    })
                    .collect();
                session.set_swarm(files);
                self.set_swarm_block(share_id, None).await;
                Lookup::Refresh(Duration::from_secs(
                    response.refresh_secs.clamp(REFRESH_MIN, REFRESH_MAX),
                ))
            }
            Err(error) if error.status == Some(404) => {
                self.with(|i| i.swarm_unsupported = true);
                Lookup::Unsupported
            }
            Err(error) if error.code.as_deref() == Some("swarm_quota_exhausted") => {
                let retry = error
                    .details
                    .as_ref()
                    .and_then(|d| d.get("retryAfterSecs"))
                    .and_then(Value::as_u64)
                    .unwrap_or(QUOTA_RETRY_DEFAULT_SECS)
                    .clamp(REFRESH_MIN, QUOTA_RETRY_MAX_SECS);
                self.set_swarm_block(share_id, Some(ShareSwarmBlock::QuotaExhausted))
                    .await;
                self.refresh_usage().await;
                Lookup::Blocked(
                    ShareSwarmBlock::QuotaExhausted,
                    Some(Duration::from_secs(retry)),
                )
            }
            Err(error) if error.code.as_deref() == Some("swarm_login_required") => {
                self.set_swarm_block(share_id, Some(ShareSwarmBlock::LoginRequired))
                    .await;
                Lookup::Blocked(ShareSwarmBlock::LoginRequired, None)
            }
            Err(error) => {
                tracing::debug!(error = %error, "swarm peer lookup failed; will retry");
                Lookup::Refresh(LOOKUP_RETRY)
            }
        }
    }

    /// 记下 / 清除某个分享的共享加速受限原因；变化时刷新任务行。
    async fn set_swarm_block(&self, share_id: &str, block: Option<ShareSwarmBlock>) {
        let changed = self.with(|i| match block {
            Some(block) => i.swarm_blocks.insert(share_id.to_owned(), block) != Some(block),
            None => i.swarm_blocks.remove(share_id).is_some(),
        });
        if changed {
            self.publish().await;
        }
    }

    /// 「需要登录」：停止轮询，登录后（`on_account_changed`）重新开始。
    fn hold_for_login(&self, token: &str, share_id: &str, ticket: &str, indexes: &[u32]) {
        self.with(|i| {
            i.swarm_login_waiting.insert(
                token.to_owned(),
                LoginWaiting {
                    share_id: share_id.to_owned(),
                    ticket: ticket.to_owned(),
                    indexes: indexes.to_vec(),
                },
            );
        });
    }

    /// 首次查询之后接着做什么：按结果开始后台刷新、或等登录、或不再查询。
    pub(super) fn continue_lookup(
        &self,
        first: Lookup,
        token: &str,
        share_id: &str,
        ticket: &str,
        indexes: &[u32],
    ) {
        match first {
            Lookup::Refresh(delay) | Lookup::Blocked(_, Some(delay)) => self.spawn_swarm_lookup(
                token.to_owned(),
                share_id.to_owned(),
                ticket.to_owned(),
                indexes.to_vec(),
                delay,
            ),
            Lookup::Blocked(ShareSwarmBlock::LoginRequired, None) => {
                self.hold_for_login(token, share_id, ticket, indexes);
            }
            Lookup::Blocked(_, None) | Lookup::Unsupported => {}
        }
    }

    /// 登录后：重新开始因「需要登录」而停止的对端查询。
    pub(super) fn resume_login_lookups(&self) {
        let waiting = self.with(|i| std::mem::take(&mut i.swarm_login_waiting));
        for (token, w) in waiting {
            self.spawn_swarm_lookup(token, w.share_id, w.ticket, w.indexes, Duration::ZERO);
        }
    }

    /// 会话存续期间按间隔刷新对端；会话销毁、服务释放或云端不支持时结束。只持 `Weak`。
    pub(super) fn spawn_swarm_lookup(
        &self,
        token: String,
        share_id: String,
        ticket: String,
        indexes: Vec<u32>,
        first_delay: Duration,
    ) {
        let weak = self.weak.clone();
        tokio::spawn(async move {
            let mut delay = first_delay;
            loop {
                tokio::time::sleep(delay).await;
                let Some(service) = weak.upgrade() else {
                    return;
                };
                let Some(session) = service.with(|i| i.receive_sessions.get(&token).cloned())
                else {
                    return;
                };
                delay = match service
                    .swarm_lookup(&session, &share_id, &ticket, &indexes)
                    .await
                {
                    Lookup::Refresh(next) | Lookup::Blocked(_, Some(next)) => next,
                    Lookup::Blocked(ShareSwarmBlock::LoginRequired, None) => {
                        service.hold_for_login(&token, &share_id, &ticket, &indexes);
                        return;
                    }
                    Lookup::Blocked(_, None) | Lookup::Unsupported => return,
                };
                service.sync_content();
            }
        });
    }

    /// 就绪共享对端数变化时刷新任务行徽标；会话销毁后订阅自然结束。
    pub(super) fn watch_swarm_peers(&self, session: &ReceiveSession) {
        let mut changes = session.watch_swarm_peers();
        let weak = self.weak.clone();
        tokio::spawn(async move {
            while changes.changed().await.is_ok() {
                let Some(service) = weak.upgrade() else {
                    break;
                };
                service.publish().await;
            }
        });
    }
}
