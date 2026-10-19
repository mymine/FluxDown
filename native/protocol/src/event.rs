//! 可重放事件帧与全量快照契约。

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use crate::agent::{
    AgentPreferencesDto, AgentSessionDto, CloudDevice, GatewayStatusDto, PendingCaptureDto,
    PowerStatusDto, RemoteTaskDto, ShellStatusDto, SyncStatusDto,
};
use crate::daemon::{
    ComponentStatusDto, DaemonConfigSnapshot, DaemonRuntimeStatsDto, GroupDto, LinkDeviceInfo,
    PluginDto, QueueDto, QueuePositionDto, RssSourceDto, SelectionRequestDto, TaskDto,
    WebhookDeliveryDto, WsServerMsg,
};

/// daemon 的完整物化投影。
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct DaemonSnapshot {
    #[serde(default)]
    pub task_runtime: BTreeMap<String, crate::TaskRuntimeDto>,
    pub tasks: Vec<TaskDto>,
    pub queues: Vec<QueueDto>,
    pub queue_positions: Vec<QueuePositionDto>,
    pub groups: Vec<GroupDto>,
    pub config: DaemonConfigSnapshot,
    pub rss_sources: Vec<RssSourceDto>,
    pub rss_item_revisions: BTreeMap<String, u64>,
    pub plugins: Vec<PluginDto>,
    pub components: Vec<ComponentStatusDto>,
    pub webhook_deliveries: Vec<WebhookDeliveryDto>,
    pub priority: Vec<String>,
    pub runtime_stats: DaemonRuntimeStatsDto,
    pub pending_selections: Vec<SelectionRequestDto>,
    /// `tasks` 的 task_id → 下标缓存，让逐帧进度 / 分段事件的任务定位保持 O(1)。
    /// 不参与序列化；克隆后为空，首次命中后惰性重建。构造快照时用 `..Default::default()` 填充。
    #[serde(skip)]
    #[cfg_attr(feature = "openapi", schema(ignore))]
    pub task_index: TaskIndex,
}

/// [`DaemonSnapshot::tasks`] 的下标缓存（内容不对外开放）。命中必须回读校验，缓存失效时
/// 退回线性扫描。
#[derive(Debug, Default)]
pub struct TaskIndex(HashMap<String, usize>);

impl Clone for TaskIndex {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl DaemonSnapshot {
    /// 任务在 `tasks` 中的下标。缓存命中且回读一致时 O(1)；否则线性扫描。
    fn task_position(&self, task_id: &str) -> Option<usize> {
        if let Some(&index) = self.task_index.0.get(task_id)
            && self
                .tasks
                .get(index)
                .is_some_and(|task| task.task_id == task_id)
        {
            return Some(index);
        }
        self.tasks.iter().position(|task| task.task_id == task_id)
    }

    /// [`Self::task_position`] 的可变版本：线性扫描命中说明缓存已过期（调用方直接改过
    /// `tasks`），顺手重建。
    fn locate_task(&mut self, task_id: &str) -> Option<usize> {
        if let Some(&index) = self.task_index.0.get(task_id)
            && self
                .tasks
                .get(index)
                .is_some_and(|task| task.task_id == task_id)
        {
            return Some(index);
        }
        let index = self.tasks.iter().position(|task| task.task_id == task_id)?;
        self.reindex_tasks();
        Some(index)
    }

    fn task_mut(&mut self, task_id: &str) -> Option<&mut TaskDto> {
        let index = self.locate_task(task_id)?;
        self.tasks.get_mut(index)
    }

    fn reindex_tasks(&mut self) {
        self.task_index.0.clear();
        self.task_index.0.reserve(self.tasks.len());
        for (index, task) in self.tasks.iter().enumerate() {
            self.task_index.0.insert(task.task_id.clone(), index);
        }
    }

    fn push_task(&mut self, task: TaskDto) {
        self.task_index
            .0
            .insert(task.task_id.clone(), self.tasks.len());
        self.tasks.push(task);
    }

    fn remove_task(&mut self, task_id: &str) {
        let before = self.tasks.len();
        self.tasks.retain(|task| task.task_id != task_id);
        if self.tasks.len() != before {
            self.reindex_tasks();
        }
    }

    /// 一次移除一批任务：整表只扫描、重建下标各一次（逐条移除是 O(N·B)）。
    fn remove_tasks(&mut self, task_ids: &[String]) {
        let removed: std::collections::HashSet<&str> =
            task_ids.iter().map(String::as_str).collect();
        let before = self.tasks.len();
        self.tasks
            .retain(|task| !removed.contains(task.task_id.as_str()));
        if self.tasks.len() != before {
            self.reindex_tasks();
        }
        self.task_runtime
            .retain(|task_id, _| !removed.contains(task_id.as_str()));
    }
}

/// agent 的完整物化投影。下载事实只来自嵌套 daemon 快照。
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct AgentSnapshot {
    pub daemon: DaemonSnapshot,
    #[serde(default)]
    pub daemon_connected: bool,
    pub session: Option<AgentSessionDto>,
    pub sync: SyncStatusDto,
    #[serde(default)]
    pub cloud_connection: crate::CloudConnectionDto,
    pub preferences: AgentPreferencesDto,
    pub gateway: GatewayStatusDto,
    pub cloud_devices: Vec<CloudDevice>,
    pub linked_devices: Vec<LinkDeviceInfo>,
    pub remote_tasks: Vec<RemoteTaskDto>,
    pub pending_captures: Vec<PendingCaptureDto>,
    /// 等待本机确认的入站局域网配对请求。
    #[serde(default)]
    pub link_pairing_requests: Vec<crate::LinkPairingRequestDto>,
    /// 局域网发现开启期间看到的未配对 / 已配对 FluxDown 设备。
    #[serde(default)]
    pub link_discovered: Vec<crate::LinkDiscoveredPeer>,
    #[serde(default)]
    pub shell: ShellStatusDto,
    #[serde(default)]
    pub power: PowerStatusDto,
    #[serde(default)]
    pub update: crate::UpdateStatusDto,
    /// 云端推送状态（未登录时只保留本机上报开关与隐私字段，`overview` 为空）。
    #[serde(default)]
    pub cloud_notify: crate::CloudNotifyStateDto,
}

/// `system.snapshot` 的服务角色对应主体。
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "role", content = "snapshot", rename_all = "camelCase")]
pub enum SnapshotBody {
    Daemon(Box<DaemonSnapshot>),
    Agent(Box<AgentSnapshot>),
}

/// 带原子事件位置的全量快照。
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub epoch: String,
    pub sequence: u64,
    pub body: SnapshotBody,
}

/// daemon 状态变化事件。
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(
    tag = "type",
    content = "data",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum DaemonEvent {
    TaskRuntimeChanged(crate::TaskRuntimeDto),
    TaskActivityAdded(crate::TaskActivityDto),
    SnapshotReplaced(DaemonSnapshot),
    Engine(WsServerMsg),
    TaskChanged(TaskDto),
    /// 一批任务事实（批量建任务）：语义等同按序逐条应用 [`DaemonEvent::TaskChanged`]。
    TasksChanged(Vec<TaskDto>),
    TaskDeleted {
        task_id: String,
    },
    /// 一批任务已删除（`daemon.task.deleteMany` 一批一条）：语义等同按序逐条
    /// [`DaemonEvent::TaskDeleted`]。
    TasksDeleted {
        task_ids: Vec<String>,
    },
    QueuesChanged(Vec<QueueDto>),
    GroupsChanged(Vec<GroupDto>),
    ConfigChanged(DaemonConfigSnapshot),
    RssChanged {
        source_id: String,
        item_revision: u64,
    },
    PluginsChanged(Vec<PluginDto>),
    ComponentsChanged(Vec<ComponentStatusDto>),
    /// 投递日志增量：按 `deliveryId` 合并进 `webhookDeliveries`（上限
    /// [`WEBHOOK_DELIVERY_LIMIT`]，按 `timestampMs` 降序）。空增量不改变列表；
    /// 清空走 [`DaemonEvent::WebhooksCleared`]。
    WebhooksChanged(Vec<WebhookDeliveryDto>),
    /// 投递日志被显式清空。
    WebhooksCleared,
    RuntimeStatsChanged(DaemonRuntimeStatsDto),
    SelectionPending(SelectionRequestDto),
    SelectionResolved {
        request_id: String,
    },
    /// 任务生命周期语义事件（与 webhook 同源）。只供 agent 消费云端推送，agent 不转发给 UI；
    /// 不改变 daemon 快照。
    TaskNotice(crate::TaskNoticeDto),
    /// 常驻插件宿主的可移植视图增量帧。不改变 daemon 快照；agent 原样转发给全部 UI
    /// 连接，客户端按 `session` 过滤、忽略未知会话。
    PluginViewFrame(crate::PluginViewFrameDto),
    /// 可移植视图会话已结束（`reason`：`closed` / `expired` / `pluginUnloaded`）。
    /// 不改变 daemon 快照；agent 原样转发。
    PluginViewClosed {
        session: String,
        reason: String,
    },
    /// 插件经 `flux.ui.toast` 请求的提示（`level`：`info` / `success` / `warning` /
    /// `error`）。不改变 daemon 快照；agent 原样转发。
    PluginToast {
        identity: String,
        level: String,
        text: String,
    },
}

/// agent 自有或转发的状态变化事件。
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(
    tag = "type",
    content = "data",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum AgentEvent {
    Daemon(DaemonEvent),
    DaemonSnapshotReplaced(DaemonSnapshot),
    DaemonConnectionChanged(bool),
    SessionChanged(Box<Option<AgentSessionDto>>),
    SyncChanged(SyncStatusDto),
    CloudConnectionChanged(crate::CloudConnectionDto),
    PreferencesChanged(AgentPreferencesDto),
    GatewayChanged(GatewayStatusDto),
    CloudDevicesChanged(Vec<CloudDevice>),
    LinkedDevicesChanged(Vec<LinkDeviceInfo>),
    LinkPairingRequestsChanged(Vec<crate::LinkPairingRequestDto>),
    LinkDiscoveredChanged(Vec<crate::LinkDiscoveredPeer>),
    RemoteTasksChanged(Vec<RemoteTaskDto>),
    PendingCapturesChanged(Vec<PendingCaptureDto>),
    ShellChanged(ShellStatusDto),
    PowerChanged(PowerStatusDto),
    UpdateChanged(crate::UpdateStatusDto),
    /// 云端推送状态整体替换（`AgentSnapshot.cloud_notify`）。
    CloudNotifyChanged(crate::CloudNotifyStateDto),
    /// 外部捕获未经确认直接建成的任务（免打扰下载 / 系统打开链接 / 拖入）。一次性通知，
    /// 不进快照；官方 UI 据此为单任务弹出进度窗口。失败条目不在列表中。
    CaptureTasksStarted(Vec<String>),
    /// 会话被非用户主动地结束（设备被移除 / 被替换 → `deviceUntrusted`；令牌失效或被管理员撤销 →
    /// `sessionExpired`；账号停用 → `accountDisabled`）。一次性通知，不进快照；紧随其后会有
    /// `SessionChanged(None)`。用户主动登出 / 删除本设备不发送。
    SessionRevoked(crate::ErrorReason),
}

/// `service.event` notification 的事件主体。
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "service", content = "event", rename_all = "camelCase")]
pub enum ServiceEvent {
    Daemon(DaemonEvent),
    Agent(AgentEvent),
}

/// 单个严格递增的服务事件帧。
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct EventFrame {
    pub epoch: String,
    pub sequence: u64,
    pub event: ServiceEvent,
}

/// 将同源 agent 增量应用于完整 DTO 投影；daemon 与桌面会话共享此规则。
pub fn apply_agent_event(snapshot: &mut AgentSnapshot, event: &AgentEvent) {
    match event {
        AgentEvent::Daemon(event) => apply_daemon_event(&mut snapshot.daemon, event),
        AgentEvent::DaemonSnapshotReplaced(daemon) => snapshot.daemon.clone_from(daemon),
        AgentEvent::DaemonConnectionChanged(connected) => {
            snapshot.daemon_connected = *connected;
            if !*connected {
                snapshot.daemon.task_runtime.clear();
            }
        }
        AgentEvent::SessionChanged(session) => {
            snapshot.session.clone_from(session.as_ref());
            // 会话结束（登出 / 被撤销）后账号维度的投影随之失效，不留旧账号的设备与任务。
            if snapshot.session.is_none() {
                snapshot.cloud_devices.clear();
                snapshot.remote_tasks.clear();
                snapshot.cloud_connection = crate::CloudConnectionDto::default();
                snapshot.cloud_notify.overview = None;
                snapshot.cloud_notify.loading = false;
                snapshot.cloud_notify.last_error_reason = None;
                snapshot.cloud_notify.updated_at_unix_ms = None;
                snapshot.cloud_notify.recent_deliveries.clear();
                snapshot.cloud_notify.recent_next_cursor = None;
            }
        }
        AgentEvent::SyncChanged(sync) => snapshot.sync.clone_from(sync),
        AgentEvent::CloudConnectionChanged(connection) => {
            snapshot.cloud_connection.clone_from(connection);
            if connection.state != crate::CloudConnectionState::Connected {
                for device in &mut snapshot.cloud_devices {
                    if device.is_current {
                        device.is_online = false;
                    }
                }
            }
        }
        AgentEvent::PreferencesChanged(preferences) => snapshot.preferences.clone_from(preferences),
        AgentEvent::GatewayChanged(gateway) => snapshot.gateway.clone_from(gateway),
        AgentEvent::CloudDevicesChanged(devices) => snapshot.cloud_devices.clone_from(devices),
        AgentEvent::LinkedDevicesChanged(devices) => snapshot.linked_devices.clone_from(devices),
        AgentEvent::LinkPairingRequestsChanged(requests) => {
            snapshot.link_pairing_requests.clone_from(requests);
        }
        AgentEvent::LinkDiscoveredChanged(peers) => snapshot.link_discovered.clone_from(peers),
        AgentEvent::RemoteTasksChanged(tasks) => snapshot.remote_tasks.clone_from(tasks),
        AgentEvent::PendingCapturesChanged(captures) => {
            snapshot.pending_captures.clone_from(captures)
        }
        AgentEvent::ShellChanged(shell) => snapshot.shell.clone_from(shell),
        AgentEvent::PowerChanged(power) => snapshot.power = *power,
        AgentEvent::UpdateChanged(update) => snapshot.update.clone_from(update),
        AgentEvent::CloudNotifyChanged(state) => snapshot.cloud_notify.clone_from(state),
        AgentEvent::CaptureTasksStarted(_) | AgentEvent::SessionRevoked(_) => {}
    }
}

/// 若任务存在且采样没有落后于已投影的源端序号，返回任务状态。
#[must_use]
pub fn accepted_runtime_status(
    snapshot: &DaemonSnapshot,
    runtime: &crate::TaskRuntimeDto,
) -> Option<i32> {
    if snapshot
        .task_runtime
        .get(&runtime.task_id)
        .is_some_and(|previous| {
            previous.sample_sequence != 0 && runtime.sample_sequence <= previous.sample_sequence
        })
    {
        return None;
    }
    snapshot
        .task_position(&runtime.task_id)
        .and_then(|index| snapshot.tasks.get(index))
        .map(|task| task.status)
}

pub fn apply_daemon_event(snapshot: &mut DaemonSnapshot, event: &DaemonEvent) {
    match event {
        DaemonEvent::TaskRuntimeChanged(runtime) => {
            let Some(status) = accepted_runtime_status(snapshot, runtime) else {
                return;
            };
            let inactive = !matches!(status, 1 | 5);
            let mut runtime = runtime.clone();
            if runtime.segments.is_empty()
                && let Some(previous) = snapshot.task_runtime.get(&runtime.task_id)
            {
                runtime.segments.clone_from(&previous.segments);
            }
            if inactive {
                runtime.active_transfers = Some(0);
                for segment in &mut runtime.segments {
                    segment.active = Some(false);
                }
            }
            snapshot
                .task_runtime
                .insert(runtime.task_id.clone(), runtime);
        }
        DaemonEvent::TaskActivityAdded(_)
        | DaemonEvent::TaskNotice(_)
        | DaemonEvent::PluginViewFrame(_)
        | DaemonEvent::PluginViewClosed { .. }
        | DaemonEvent::PluginToast { .. } => {}
        DaemonEvent::SnapshotReplaced(replacement) => snapshot.clone_from(replacement),
        DaemonEvent::Engine(message) => apply_engine_message(snapshot, message),
        DaemonEvent::TaskChanged(task) => apply_task_changed(snapshot, task),
        DaemonEvent::TasksChanged(tasks) => {
            snapshot.tasks.reserve(tasks.len());
            for task in tasks {
                apply_task_changed(snapshot, task);
            }
        }
        DaemonEvent::TaskDeleted { task_id } => {
            snapshot.remove_task(task_id);
            snapshot.task_runtime.remove(task_id);
        }
        DaemonEvent::TasksDeleted { task_ids } => snapshot.remove_tasks(task_ids),
        DaemonEvent::QueuesChanged(queues) => snapshot.queues.clone_from(queues),
        DaemonEvent::GroupsChanged(groups) => snapshot.groups.clone_from(groups),
        DaemonEvent::ConfigChanged(config) => snapshot.config.clone_from(config),
        DaemonEvent::RssChanged {
            source_id,
            item_revision,
        } => {
            snapshot
                .rss_item_revisions
                .insert(source_id.clone(), *item_revision);
        }
        DaemonEvent::PluginsChanged(plugins) => snapshot.plugins.clone_from(plugins),
        DaemonEvent::ComponentsChanged(components) => snapshot.components.clone_from(components),
        DaemonEvent::WebhooksChanged(deliveries) => {
            merge_webhook_deliveries(&mut snapshot.webhook_deliveries, deliveries);
        }
        DaemonEvent::WebhooksCleared => snapshot.webhook_deliveries.clear(),
        DaemonEvent::RuntimeStatsChanged(stats) => snapshot.runtime_stats.clone_from(stats),
        DaemonEvent::SelectionPending(request) => {
            snapshot
                .pending_selections
                .retain(|item| item.request_id != request.request_id);
            snapshot.pending_selections.push(request.clone());
        }
        DaemonEvent::SelectionResolved { request_id } => {
            snapshot
                .pending_selections
                .retain(|item| item.request_id != *request_id);
        }
    }
}

fn apply_task_changed(snapshot: &mut DaemonSnapshot, task: &TaskDto) {
    if let Some(existing) = snapshot.task_mut(&task.task_id) {
        existing.clone_from(task);
    } else {
        snapshot.push_task(task.clone());
    }
    if !matches!(task.status, 1 | 5) {
        clear_active_runtime(snapshot, &task.task_id);
    }
}

fn apply_engine_message(snapshot: &mut DaemonSnapshot, message: &WsServerMsg) {
    match message {
        WsServerMsg::TasksSnapshot { tasks } => {
            snapshot.tasks.clone_from(tasks);
            snapshot.reindex_tasks();
            let live: std::collections::HashSet<&str> =
                tasks.iter().map(|task| task.task_id.as_str()).collect();
            snapshot
                .task_runtime
                .retain(|id, _| live.contains(id.as_str()));
            for task in tasks {
                if !matches!(task.status, 1 | 5) {
                    clear_active_runtime(snapshot, &task.task_id);
                }
            }
        }
        WsServerMsg::TaskProgress {
            task_id,
            status,
            downloaded_bytes,
            total_bytes,
            file_name,
            save_dir,
            url,
            error_message,
            uploaded_bytes,
            seeding_status,
            seeding_message,
            seeding_time_secs,
            ..
        } => {
            if *status == 4 && error_message == "deleted" {
                snapshot.task_runtime.remove(task_id);
                snapshot.remove_task(task_id);
                return;
            }
            if !matches!(*status, 1 | 5) {
                clear_active_runtime(snapshot, task_id);
            }
            if let Some(task) = snapshot.task_mut(task_id) {
                task.status = *status;
                task.downloaded_bytes = *downloaded_bytes;
                task.total_bytes = *total_bytes;
                if !file_name.is_empty() {
                    task.file_name.clone_from(file_name);
                }
                if !save_dir.is_empty() {
                    task.save_dir.clone_from(save_dir);
                }
                if !url.is_empty() {
                    task.url.clone_from(url);
                }
                task.error_message.clone_from(error_message);
                task.uploaded_bytes = *uploaded_bytes;
                task.seeding_status = *seeding_status;
                task.seeding_message.clone_from(seeding_message);
                task.seeding_time_secs = *seeding_time_secs;
            }
        }
        WsServerMsg::SegmentProgress {
            task_id,
            total_bytes,
            segments,
            ..
        } => {
            let Some(status) = snapshot.task_mut(task_id).map(|task| task.status) else {
                return;
            };
            let inactive = !matches!(status, 1 | 5);
            let runtime = snapshot
                .task_runtime
                .entry(task_id.clone())
                .or_insert_with(|| crate::TaskRuntimeDto {
                    task_id: task_id.clone(),
                    active_transfers: inactive.then_some(0),
                    ..Default::default()
                });
            // 持久几何无传输采样；真实采样存在时不得覆盖它的分段/活跃读数。
            if runtime.sample_sequence == 0 {
                runtime.total_bytes = *total_bytes;
                runtime.segments = segments
                    .iter()
                    .map(|segment| crate::TaskSegmentDto {
                        index: segment.index,
                        start_byte: segment.start_byte,
                        end_byte: segment.end_byte,
                        downloaded_bytes: segment.downloaded_bytes,
                        active: inactive.then_some(false),
                    })
                    .collect();
            }
        }
        WsServerMsg::TaskMetaProbed {
            task_id,
            file_name,
            total_bytes,
        } => {
            if let Some(task) = snapshot.task_mut(task_id) {
                if !file_name.is_empty() {
                    task.file_name.clone_from(file_name);
                }
                task.total_bytes = *total_bytes;
            }
        }
        WsServerMsg::TaskQueueChanged { task_id, queue_id } => {
            if let Some(task) = snapshot.task_mut(task_id) {
                task.queue_id.clone_from(queue_id);
            }
        }
        WsServerMsg::TaskRouteChanged { task_id, route } => {
            if let Some(task) = snapshot.task_mut(task_id) {
                task.auto_route.clone_from(route);
            }
        }
        WsServerMsg::QueuesChanged { queues } => snapshot.queues.clone_from(queues),
        WsServerMsg::QueuePositionsChanged { positions } => {
            snapshot.queue_positions.clone_from(positions);
        }
        WsServerMsg::GroupsChanged { groups } => snapshot.groups.clone_from(groups),
        WsServerMsg::RssSourcesChanged { sources } => snapshot.rss_sources.clone_from(sources),
        WsServerMsg::RssItemsChanged { source_id, .. } => {
            let revision = snapshot
                .rss_item_revisions
                .entry(source_id.clone())
                .or_default();
            *revision = revision.saturating_add(1);
        }
        WsServerMsg::WebhookDeliveriesChanged { deliveries } => {
            merge_webhook_deliveries(&mut snapshot.webhook_deliveries, deliveries);
        }
        WsServerMsg::FileMissingChanged { updates } => {
            for update in updates {
                if let Some(task) = snapshot.task_mut(&update.task_id) {
                    task.file_missing = update.missing;
                }
            }
        }
        WsServerMsg::PriorityTaskChanged {
            priority_task_id, ..
        } => {
            snapshot.priority.clear();
            if !priority_task_id.is_empty() {
                snapshot.priority.push(priority_task_id.clone());
            }
        }
        WsServerMsg::PluginAutoDisabled { identity, reason } => {
            if let Some(plugin) = snapshot
                .plugins
                .iter_mut()
                .find(|plugin| plugin.identity == *identity)
            {
                plugin.enabled = false;
                plugin.disabled_reason.clone_from(reason);
            }
        }
        _ => {}
    }
}

fn clear_active_runtime(snapshot: &mut DaemonSnapshot, task_id: &str) {
    if let Some(runtime) = snapshot.task_runtime.get_mut(task_id) {
        runtime.active_transfers = Some(0);
        runtime.connected_peers = Some(0);
        for segment in &mut runtime.segments {
            segment.active = Some(false);
        }
    }
}

/// 投递日志保留上限（与引擎内存环一致）。
pub const WEBHOOK_DELIVERY_LIMIT: usize = 1000;

/// 把一批投递记录按 `deliveryId` 合并进列表：同 id 以增量为准，结果按 `timestampMs`
/// 降序并截到 [`WEBHOOK_DELIVERY_LIMIT`]。增量为空时列表不变——清空由
/// [`DaemonEvent::WebhooksCleared`] 显式表达。
pub fn merge_webhook_deliveries(
    current: &mut Vec<WebhookDeliveryDto>,
    delta: &[WebhookDeliveryDto],
) {
    if delta.is_empty() {
        return;
    }
    let incoming: std::collections::HashSet<&str> = delta
        .iter()
        .map(|delivery| delivery.delivery_id.as_str())
        .collect();
    current.retain(|delivery| !incoming.contains(delivery.delivery_id.as_str()));
    current.extend(delta.iter().cloned());
    current.sort_by_key(|delivery| std::cmp::Reverse(delivery.timestamp_ms));
    current.truncate(WEBHOOK_DELIVERY_LIMIT);
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{DaemonEvent, EventFrame, ServiceEvent, Snapshot, SnapshotBody};

    #[test]
    fn cloud_connection_defaults_for_old_snapshot_and_resets_on_logout() {
        let mut wire = serde_json::to_value(super::AgentSnapshot::default()).expect("snapshot");
        wire.as_object_mut()
            .expect("object")
            .remove("cloudConnection");
        let mut snapshot: super::AgentSnapshot =
            serde_json::from_value(wire).expect("old snapshot");
        assert_eq!(
            snapshot.cloud_connection,
            crate::CloudConnectionDto::default()
        );
        let connection = crate::CloudConnectionDto {
            state: crate::CloudConnectionState::Connected,
            ..Default::default()
        };
        let event = super::AgentEvent::CloudConnectionChanged(connection.clone());
        assert_eq!(
            serde_json::to_value(&event).expect("event")["type"],
            "cloudConnectionChanged"
        );
        super::apply_agent_event(&mut snapshot, &event);
        assert_eq!(snapshot.cloud_connection, connection);
        super::apply_agent_event(
            &mut snapshot,
            &super::AgentEvent::SessionChanged(Box::new(None)),
        );
        assert_eq!(
            snapshot.cloud_connection,
            crate::CloudConnectionDto::default()
        );
    }

    #[test]
    fn snapshot_carries_atomic_epoch_and_sequence() -> Result<(), serde_json::Error> {
        let snapshot = Snapshot {
            epoch: "epoch-a".to_owned(),
            sequence: 41,
            body: SnapshotBody::Daemon(Box::default()),
        };
        let wire = serde_json::to_value(&snapshot)?;
        assert_eq!(wire["epoch"], "epoch-a");
        assert_eq!(wire["sequence"], 41);
        assert_eq!(wire["body"]["role"], "daemon");
        assert!(wire["body"]["snapshot"]["tasks"].is_array());
        Ok(())
    }

    #[test]
    fn event_frame_uses_strict_monotonic_cursor_shape() -> Result<(), serde_json::Error> {
        let frame = EventFrame {
            epoch: "epoch-a".to_owned(),
            sequence: 42,
            event: ServiceEvent::Daemon(DaemonEvent::TaskDeleted {
                task_id: "task-1".to_owned(),
            }),
        };
        let wire = serde_json::to_value(frame)?;
        assert_eq!(
            wire,
            json!({
                "epoch": "epoch-a",
                "sequence": 42,
                "event": {
                    "service": "daemon",
                    "event": {
                        "type": "taskDeleted",
                        "data": { "taskId": "task-1" }
                    }
                }
            })
        );
        Ok(())
    }

    #[test]
    fn restart_geometry_never_invents_active_reads_or_overrides_live_sample()
    -> Result<(), serde_json::Error> {
        let task: super::TaskDto = serde_json::from_value(json!({
            "taskId":"t", "url":"https://example.com/t", "fileName":"t", "saveDir":"/tmp",
            "status":2, "downloadedBytes":25, "totalBytes":100, "errorMessage":"",
            "createdAt":"1", "proxyUrl":"", "queueId":"main", "checksum":""
        }))?;
        let mut snapshot = super::DaemonSnapshot {
            tasks: vec![task],
            ..Default::default()
        };
        let geometry = DaemonEvent::Engine(super::WsServerMsg::SegmentProgress {
            task_id: "t".into(),
            total_bytes: 100,
            segment_count: 1,
            segments: vec![crate::daemon::SegmentDetailDto {
                index: 0,
                start_byte: 0,
                end_byte: 99,
                downloaded_bytes: 25,
            }],
        });
        super::apply_daemon_event(&mut snapshot, &geometry);
        assert_eq!(snapshot.task_runtime["t"].segments[0].downloaded_bytes, 25);
        assert_eq!(snapshot.task_runtime["t"].active_transfers, Some(0));
        assert_eq!(snapshot.task_runtime["t"].segments[0].active, Some(false));
        let mut active = snapshot.tasks[0].clone();
        active.status = 1;
        super::apply_daemon_event(&mut snapshot, &DaemonEvent::TaskChanged(active.clone()));
        super::apply_daemon_event(
            &mut snapshot,
            &DaemonEvent::TaskRuntimeChanged(crate::TaskRuntimeDto {
                task_id: "t".into(),
                sampled_at_ms: 1234,
                sample_sequence: 10,
                active_transfers: Some(2),
                total_bytes: 100,
                segments: vec![crate::TaskSegmentDto {
                    index: 0,
                    start_byte: 0,
                    end_byte: 99,
                    downloaded_bytes: 40,
                    active: Some(true),
                }],
                ..Default::default()
            }),
        );
        super::apply_daemon_event(&mut snapshot, &geometry);
        super::apply_daemon_event(
            &mut snapshot,
            &DaemonEvent::TaskRuntimeChanged(crate::TaskRuntimeDto {
                task_id: "t".into(),
                sampled_at_ms: 5000,
                sample_sequence: 9,
                active_transfers: Some(1),
                segments: vec![crate::TaskSegmentDto {
                    downloaded_bytes: 20,
                    ..Default::default()
                }],
                ..Default::default()
            }),
        );
        assert_eq!(snapshot.task_runtime["t"].segments[0].downloaded_bytes, 40);
        assert_eq!(snapshot.task_runtime["t"].active_transfers, Some(2));
        active.status = 3;
        super::apply_daemon_event(&mut snapshot, &DaemonEvent::TaskChanged(active));
        assert_eq!(snapshot.task_runtime["t"].active_transfers, Some(0));
        assert_eq!(snapshot.task_runtime["t"].segments[0].active, Some(false));
        super::apply_daemon_event(
            &mut snapshot,
            &DaemonEvent::TaskRuntimeChanged(crate::TaskRuntimeDto {
                task_id: "t".into(),
                sampled_at_ms: 1234,
                sample_sequence: 10,
                active_transfers: Some(1),
                segments: vec![crate::TaskSegmentDto {
                    downloaded_bytes: 20,
                    ..Default::default()
                }],
                ..Default::default()
            }),
        );
        assert_eq!(snapshot.task_runtime["t"].segments[0].downloaded_bytes, 40);
        super::apply_daemon_event(
            &mut snapshot,
            &DaemonEvent::TaskRuntimeChanged(crate::TaskRuntimeDto {
                task_id: "t".into(),
                sampled_at_ms: 100,
                sample_sequence: 11,
                active_transfers: Some(0),
                segments: vec![crate::TaskSegmentDto {
                    downloaded_bytes: 100,
                    ..Default::default()
                }],
                ..Default::default()
            }),
        );
        assert_eq!(snapshot.task_runtime["t"].segments[0].downloaded_bytes, 100);
        assert_eq!(snapshot.task_runtime["t"].active_transfers, Some(0));
        super::apply_daemon_event(
            &mut snapshot,
            &DaemonEvent::TaskRuntimeChanged(crate::TaskRuntimeDto {
                task_id: "t".into(),
                sampled_at_ms: 9000,
                sample_sequence: 0,
                active_transfers: Some(1),
                ..Default::default()
            }),
        );
        assert_eq!(snapshot.task_runtime["t"].segments[0].downloaded_bytes, 100);
        super::apply_daemon_event(
            &mut snapshot,
            &DaemonEvent::TaskDeleted {
                task_id: "t".into(),
            },
        );
        assert!(snapshot.task_runtime.is_empty());
        super::apply_daemon_event(
            &mut snapshot,
            &DaemonEvent::TaskRuntimeChanged(crate::TaskRuntimeDto {
                task_id: "t".into(),
                active_transfers: Some(3),
                ..Default::default()
            }),
        );
        assert!(
            snapshot.task_runtime.is_empty(),
            "late sample must not resurrect deleted task"
        );
        Ok(())
    }

    fn sample_task(id: &str, status: i32) -> Result<super::TaskDto, serde_json::Error> {
        serde_json::from_value(json!({
            "taskId": id, "url": "https://example.com/t", "fileName": id, "saveDir": "/tmp",
            "status": status, "downloadedBytes": 0, "totalBytes": 100, "errorMessage": "",
            "createdAt": "1", "proxyUrl": "", "queueId": "main", "checksum": ""
        }))
    }

    fn sample_delivery(
        id: &str,
        timestamp_ms: i64,
    ) -> Result<super::WebhookDeliveryDto, serde_json::Error> {
        serde_json::from_value(json!({
            "deliveryId": id, "timestampMs": timestamp_ms, "event": "task.completed",
            "endpointId": "e", "endpointName": "e", "url": "https://example.com/hook",
            "requestHeaders": "", "requestBody": "", "statusCode": 200, "responseBody": "",
            "latencyMs": 1, "attempts": 1, "success": true, "error": ""
        }))
    }

    #[test]
    fn webhook_delta_merges_by_delivery_id_and_keeps_newest_first() -> Result<(), serde_json::Error>
    {
        let mut snapshot = super::DaemonSnapshot::default();
        super::apply_daemon_event(
            &mut snapshot,
            &DaemonEvent::WebhooksChanged(vec![
                sample_delivery("b", 20)?,
                sample_delivery("a", 10)?,
            ]),
        );
        let mut retried = sample_delivery("a", 30)?;
        retried.success = false;
        super::apply_daemon_event(
            &mut snapshot,
            &DaemonEvent::WebhooksChanged(vec![retried, sample_delivery("c", 15)?]),
        );
        let ids: Vec<&str> = snapshot
            .webhook_deliveries
            .iter()
            .map(|delivery| delivery.delivery_id.as_str())
            .collect();
        assert_eq!(ids, ["a", "b", "c"]);
        assert!(
            !snapshot.webhook_deliveries[0].success,
            "same id takes the delta version"
        );
        Ok(())
    }

    #[test]
    fn empty_webhook_delta_keeps_history_and_only_cleared_event_empties_it()
    -> Result<(), serde_json::Error> {
        let mut snapshot = super::DaemonSnapshot::default();
        super::apply_daemon_event(
            &mut snapshot,
            &DaemonEvent::WebhooksChanged(vec![sample_delivery("a", 1)?]),
        );
        super::apply_daemon_event(&mut snapshot, &DaemonEvent::WebhooksChanged(Vec::new()));
        super::apply_daemon_event(
            &mut snapshot,
            &DaemonEvent::Engine(super::WsServerMsg::WebhookDeliveriesChanged {
                deliveries: Vec::new(),
            }),
        );
        assert_eq!(snapshot.webhook_deliveries.len(), 1);
        super::apply_daemon_event(&mut snapshot, &DaemonEvent::WebhooksCleared);
        assert!(snapshot.webhook_deliveries.is_empty());
        Ok(())
    }

    #[test]
    fn webhook_history_is_capped_at_the_limit_dropping_oldest() -> Result<(), serde_json::Error> {
        let mut snapshot = super::DaemonSnapshot::default();
        let total = i64::try_from(super::WEBHOOK_DELIVERY_LIMIT).unwrap_or(1000) + 5;
        for chunk_start in (0..total).step_by(100) {
            let delta = (chunk_start..(chunk_start + 100).min(total))
                .rev()
                .map(|n| sample_delivery(&format!("d{n}"), n))
                .collect::<Result<Vec<_>, _>>()?;
            super::apply_daemon_event(&mut snapshot, &DaemonEvent::WebhooksChanged(delta));
        }
        assert_eq!(
            snapshot.webhook_deliveries.len(),
            super::WEBHOOK_DELIVERY_LIMIT
        );
        assert_eq!(snapshot.webhook_deliveries[0].timestamp_ms, total - 1);
        assert_eq!(
            snapshot.webhook_deliveries.last().map(|d| d.timestamp_ms),
            Some(5)
        );
        Ok(())
    }

    #[test]
    fn task_lookup_survives_direct_task_list_edits() -> Result<(), serde_json::Error> {
        let mut snapshot = super::DaemonSnapshot::default();
        super::apply_daemon_event(
            &mut snapshot,
            &DaemonEvent::Engine(super::WsServerMsg::TasksSnapshot {
                tasks: vec![
                    sample_task("a", 2)?,
                    sample_task("b", 2)?,
                    sample_task("c", 2)?,
                ],
            }),
        );
        // 调用方绕过事件直接改了列表顺序：缓存必须自愈而不是指错任务。
        snapshot.tasks.remove(0);
        snapshot.tasks.push(sample_task("d", 1)?);
        for id in ["b", "c", "d"] {
            super::apply_daemon_event(
                &mut snapshot,
                &DaemonEvent::Engine(super::WsServerMsg::TaskMetaProbed {
                    task_id: id.into(),
                    file_name: format!("{id}.bin"),
                    total_bytes: 7,
                }),
            );
        }
        let names: Vec<&str> = snapshot
            .tasks
            .iter()
            .map(|t| t.file_name.as_str())
            .collect();
        assert_eq!(names, ["b.bin", "c.bin", "d.bin"]);
        super::apply_daemon_event(
            &mut snapshot,
            &DaemonEvent::TaskDeleted {
                task_id: "b".into(),
            },
        );
        super::apply_daemon_event(
            &mut snapshot,
            &DaemonEvent::Engine(super::WsServerMsg::TaskMetaProbed {
                task_id: "d".into(),
                file_name: "d2.bin".into(),
                total_bytes: 9,
            }),
        );
        assert_eq!(snapshot.tasks.len(), 2);
        assert_eq!(snapshot.tasks[1].file_name, "d2.bin");
        assert_eq!(snapshot.tasks[1].total_bytes, 9);
        Ok(())
    }

    /// 批量删除 = 按序逐条删除：剩余任务保序、下标仍可用，运行时随任务清除。
    #[test]
    fn tasks_deleted_matches_sequential_task_deleted() -> Result<(), serde_json::Error> {
        let mut batched = super::DaemonSnapshot::default();
        super::apply_daemon_event(
            &mut batched,
            &DaemonEvent::Engine(super::WsServerMsg::TasksSnapshot {
                tasks: vec![
                    sample_task("a", 1)?,
                    sample_task("b", 2)?,
                    sample_task("c", 2)?,
                    sample_task("d", 1)?,
                ],
            }),
        );
        for id in ["a", "d"] {
            super::apply_daemon_event(
                &mut batched,
                &DaemonEvent::TaskRuntimeChanged(crate::TaskRuntimeDto {
                    task_id: id.into(),
                    ..crate::TaskRuntimeDto::default()
                }),
            );
        }
        assert_eq!(batched.task_runtime.len(), 2);
        let mut sequential = batched.clone();
        super::apply_daemon_event(
            &mut batched,
            &DaemonEvent::TasksDeleted {
                task_ids: vec!["d".into(), "missing".into(), "a".into()],
            },
        );
        for id in ["d", "missing", "a"] {
            super::apply_daemon_event(
                &mut sequential,
                &DaemonEvent::TaskDeleted { task_id: id.into() },
            );
        }
        let ids = |snapshot: &super::DaemonSnapshot| -> Vec<String> {
            snapshot.tasks.iter().map(|t| t.task_id.clone()).collect()
        };
        assert_eq!(ids(&batched), ["b", "c"]);
        assert_eq!(ids(&batched), ids(&sequential));
        assert!(batched.task_runtime.is_empty());
        super::apply_daemon_event(
            &mut batched,
            &DaemonEvent::Engine(super::WsServerMsg::TaskMetaProbed {
                task_id: "c".into(),
                file_name: "c2.bin".into(),
                total_bytes: 3,
            }),
        );
        assert_eq!(batched.tasks[1].file_name, "c2.bin");
        Ok(())
    }
}
