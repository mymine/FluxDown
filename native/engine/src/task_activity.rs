//! 引擎源端活动：同步事件只入有界队列；持久化成功后才发布活动通知。

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};

use crate::db::{Db, DbError};
use crate::events::{EngineEvent, EventSink};

/// 源端任务活动；持久化前 ID 为 0，提交后获得数据库分配的递增 ID。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskActivity {
    pub id: i64,
    pub task_id: String,
    pub timestamp_ms: i64,
    pub kind: String,
    pub message: String,
    pub status: Option<i32>,
}

/// 按任务查询活动历史；前向、后向游标互斥，页内结果按 ID 升序排列。
#[derive(Clone, Debug, Default)]
pub struct TaskActivityQuery {
    pub task_id: String,
    pub before_id: Option<i64>,
    pub after_id: Option<i64>,
    pub limit: u32,
}

/// 活动历史页及当前保留范围；`truncated` 表示请求游标已越过保留边界。
#[derive(Clone, Debug, Default)]
pub struct TaskActivityPage {
    pub entries: Vec<TaskActivity>,
    pub has_more: bool,
    pub oldest_id: Option<i64>,
    pub newest_id: Option<i64>,
    pub truncated: bool,
}

/// 记录事件产生时的 UTC 毫秒时间，不用于判断运行态样本的新旧。
pub fn timestamp_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// 用于源端真实重试等异步调用点；任务已删除时不再生成孤儿活动。
/// DB 失败会返回错误，不会伪造一个成功的推送。
pub async fn record(
    db: &Db,
    sink: &dyn EventSink,
    task_id: &str,
    kind: &str,
    message: impl Into<String>,
    status: Option<i32>,
) -> Result<Option<TaskActivity>, DbError> {
    let entry = TaskActivity {
        id: 0,
        task_id: task_id.to_owned(),
        timestamp_ms: timestamp_ms(),
        kind: kind.to_owned(),
        message: message.into(),
        status,
    };
    let saved = db.append_task_activity(&entry).await?;
    if let Some(saved) = &saved {
        sink.emit(EngineEvent::TaskActivityAdded(saved.clone()));
    }
    Ok(saved)
}

enum JournalCommand {
    Add(TaskActivity),
    Flush(oneshot::Sender<Result<(), String>>),
}

/// 每任务因队列/存储故障丢失的活动数；DB 恢复后写入显式缺口。
#[derive(Clone, Copy, Default)]
struct ActivityLoss {
    overflow: u64,
    storage_failures: u64,
}

/// EventSink::emit 不得阻塞 current-thread runtime。队列满时记录
/// journal_overflow 缺口；要求严格持久化的源端重试直接调用异步 record。
pub struct JournalSink {
    downstream: Arc<dyn EventSink>,
    tx: mpsc::Sender<JournalCommand>,
    last_state: Mutex<HashMap<String, (i32, String)>>,
    overflows: Arc<Mutex<HashMap<String, ActivityLoss>>>,
}

impl JournalSink {
    #[must_use]
    pub fn start(db: Db, downstream: Arc<dyn EventSink>) -> Arc<Self> {
        let (tx, mut rx) = mpsc::channel(1024);
        let overflows = Arc::new(Mutex::new(HashMap::new()));
        let sink = Arc::new(Self {
            downstream: downstream.clone(),
            tx,
            last_state: Mutex::new(HashMap::new()),
            overflows: overflows.clone(),
        });
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            let mut unavailable = false;
            loop {
                let cmd = tokio::select! {
                    command = rx.recv() => command,
                    _ = tick.tick() => {
                        unavailable = persist_overflows(&db, downstream.as_ref(), &overflows).await.is_err();
                        continue;
                    }
                };
                let Some(cmd) = cmd else { break };
                match cmd {
                    JournalCommand::Add(entry) => {
                        // 有限退避；永久损坏时快速排空待写队列成明确缺口，
                        // tick 定期尝试持久化缺口，flush 始终可以返回错误。
                        if unavailable {
                            let mut pending = overflows.lock().unwrap_or_else(|e| e.into_inner());
                            pending.entry(entry.task_id).or_default().storage_failures += 1;
                            continue;
                        }
                        let mut saved = None;
                        for attempt in 0..3 {
                            match db.append_task_activity(&entry).await {
                                Ok(value) => {
                                    saved = Some(value);
                                    break;
                                }
                                Err(error) => {
                                    tracing::error!(%error, task_id=%entry.task_id, attempt, "task activity persistence failed");
                                    if attempt < 2 {
                                        tokio::time::sleep(Duration::from_millis(
                                            100 * (1 << attempt),
                                        ))
                                        .await;
                                    }
                                }
                            }
                        }
                        if let Some(saved) = saved {
                            if let Some(saved) = saved {
                                downstream.emit(EngineEvent::TaskActivityAdded(saved));
                            }
                        } else {
                            unavailable = true;
                            let mut pending = overflows.lock().unwrap_or_else(|e| e.into_inner());
                            pending.entry(entry.task_id).or_default().storage_failures += 1;
                        }
                    }
                    JournalCommand::Flush(reply) => {
                        let result = persist_overflows(&db, downstream.as_ref(), &overflows).await;
                        unavailable = result.is_err();
                        if reply.send(result).is_err() {
                            tracing::debug!("activity journal flush caller stopped waiting");
                        }
                    }
                }
            }
            if let Err(error) = persist_overflows(&db, downstream.as_ref(), &overflows).await {
                tracing::error!(%error, "task activity tail could not be flushed");
            }
        });
        sink
    }

    pub async fn flush(&self) -> Result<(), String> {
        tokio::time::timeout(Duration::from_secs(4), async {
            let (reply, receiver) = oneshot::channel();
            self.tx
                .send(JournalCommand::Flush(reply))
                .await
                .map_err(|_| "activity journal stopped".to_owned())?;
            receiver
                .await
                .map_err(|_| "activity journal stopped before flush".to_owned())?
        })
        .await
        .map_err(|_| "activity journal flush timed out".to_owned())?
    }
}

async fn persist_overflows(
    db: &Db,
    sink: &dyn EventSink,
    pending: &Mutex<HashMap<String, ActivityLoss>>,
) -> Result<(), String> {
    let entries = {
        let mut guard = pending.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut *guard)
    };
    let mut first_error = None;
    for (task_id, loss) in entries {
        let message = format!(
            "activity journal gap: queue_overflow={}, storage_failures={}",
            loss.overflow, loss.storage_failures
        );
        if let Err(error) = record(db, sink, &task_id, "journal_overflow", message, None).await {
            tracing::error!(%error, %task_id, "task activity gap report failed");
            let mut guard = pending.lock().unwrap_or_else(|e| e.into_inner());
            let count = guard.entry(task_id).or_default();
            count.overflow += loss.overflow;
            count.storage_failures += loss.storage_failures;
            first_error.get_or_insert_with(|| error.to_string());
        }
    }
    first_error.map_or(Ok(()), Err)
}

impl JournalSink {
    /// 任务状态/错误文案相对上次记录发生变化时记一条活动；相同状态不重复记录。
    fn record_state(&self, task_id: &str, status: i32, error_message: &str) {
        let changed = {
            let mut previous = self.last_state.lock().unwrap_or_else(|e| e.into_inner());
            if previous
                .get(task_id)
                .is_some_and(|(old_status, old_error)| {
                    *old_status == status && old_error == error_message
                })
            {
                false
            } else {
                previous.insert(task_id.to_owned(), (status, error_message.to_owned()));
                true
            }
        };
        if changed {
            self.push_activity(
                task_id,
                if error_message.is_empty() {
                    "status"
                } else {
                    "error"
                },
                error_message.to_owned(),
                Some(status),
            );
        }
    }

    /// 同步入队；队列满时记入溢出缺口，不阻塞 `emit`。
    fn push_activity(&self, task_id: &str, kind: &str, message: String, status: Option<i32>) {
        let entry = TaskActivity {
            id: 0,
            task_id: task_id.to_owned(),
            timestamp_ms: timestamp_ms(),
            kind: kind.to_owned(),
            message,
            status,
        };
        if let Err(error) = self.tx.try_send(JournalCommand::Add(entry)) {
            let mut pending = self.overflows.lock().unwrap_or_else(|e| e.into_inner());
            pending.entry(task_id.to_owned()).or_default().overflow += 1;
            tracing::error!(%task_id, %error, "task activity journal overflow (reported in persistent history)");
        }
    }
}

impl EventSink for JournalSink {
    fn emit(&self, event: EngineEvent) {
        match &event {
            EngineEvent::TaskProgress {
                task_id,
                status,
                error_message,
                ..
            } => self.record_state(task_id, *status, error_message),
            EngineEvent::TaskChanged(task) => {
                self.record_state(&task.task_id, task.status, &task.error_message);
            }
            EngineEvent::TasksChanged(tasks) => {
                for task in tasks {
                    self.record_state(&task.task_id, task.status, &task.error_message);
                }
            }
            EngineEvent::TasksSnapshot(tasks) => {
                let retained: HashSet<_> = tasks.iter().map(|task| task.task_id.as_str()).collect();
                self.last_state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .retain(|task_id, _| retained.contains(task_id.as_str()));
            }
            EngineEvent::SegmentSplit {
                task_id,
                parent_index,
                child_index,
                child_start,
                child_end,
                ..
            } => self.push_activity(
                task_id,
                "split",
                format!(
                    "segment {parent_index} split child={child_index} range={child_start}..={child_end}"
                ),
                None,
            ),
            EngineEvent::TaskCdnEvent {
                task_id,
                kind,
                host,
                nodes,
                ip,
                reason,
                candidates,
                alive,
                cap,
                auto_cap,
            } if kind != "leases" => {
                let nodes = if kind == "pool" || kind == "summary" || kind == "links" {
                    nodes
                        .iter()
                        .map(|node| {
                            format!(
                                "{}({}) bytes={} bps={}",
                                node.ip, node.origin, node.bytes, node.ewma_bps
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(";")
                } else {
                    String::new()
                };
                let message = format!(
                    "CDN {kind} host={host} ip={ip} reason={reason} candidates={candidates} alive={alive} cap={cap} auto_cap={auto_cap} nodes=[{nodes}]"
                );
                self.push_activity(
                    task_id,
                    match kind.as_str() {
                        "pool" => "cdn_pool",
                        "kick" => "cdn_kick",
                        "breaker" => "cdn_breaker",
                        "fallback" => "cdn_fallback",
                        "summary" => "cdn_summary",
                        "links" => "nic_links",
                        "links_off" => "nic_off",
                        _ => "cdn",
                    },
                    message,
                    None,
                );
            }
            _ => {}
        }
        self.downstream.emit(event);
    }
}
