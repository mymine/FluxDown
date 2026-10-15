// 全量快照与可重放事件帧（native/protocol/src/event.rs）。

import type {
  AgentPreferencesDto,
  AgentSessionDto,
  CloudConnectionDto,
  CloudDevice,
  GatewayStatusDto,
  LinkDeviceInfo,
  LinkDiscoveredPeer,
  LinkPairingRequestDto,
  PendingCaptureDto,
  PowerStatusDto,
  RemoteTaskDto,
  ShellStatusDto,
  SyncStatusDto,
  UpdateStatusDto,
} from './agent';
import type { ErrorReason } from './error';
import type { DaemonConfigSnapshot, DaemonRuntimeStatsDto } from './config';
import type { ComponentStatusDto, PluginDto } from './plugin';
import type { GroupDto, QueueDto, QueuePositionDto } from './queue';
import type { RssSourceDto } from './rss';
import type { SelectionRequestDto } from './selection';
import type { TaskActivityDto, TaskDto, TaskRuntimeDto } from './task';
import type { CloudNotifyStateDto, TaskNoticeDto } from './cloudNotify';
import type { WebhookDeliveryDto } from './webhook';
import type { WsServerMsg } from './ws';

/** daemon 的完整物化投影。 */
export interface DaemonSnapshot {
  /** 任务最新传输采样，按 taskId 索引。 */
  taskRuntime: Record<string, TaskRuntimeDto>;
  tasks: TaskDto[];
  queues: QueueDto[];
  queuePositions: QueuePositionDto[];
  groups: GroupDto[];
  config: DaemonConfigSnapshot;
  rssSources: RssSourceDto[];
  /** 每个订阅的条目流版本；变化时重拉 `daemon.rss.getItems`。 */
  rssItemRevisions: Record<string, number>;
  plugins: PluginDto[];
  components: ComponentStatusDto[];
  webhookDeliveries: WebhookDeliveryDto[];
  /** Boost 优先任务 ID 列表。 */
  priority: string[];
  runtimeStats: DaemonRuntimeStatsDto;
  pendingSelections: SelectionRequestDto[];
}

/** agent 的完整物化投影；下载事实只来自嵌套的 `daemon`。 */
export interface AgentSnapshot {
  daemon: DaemonSnapshot;
  daemonConnected: boolean;
  session: AgentSessionDto | null;
  sync: SyncStatusDto;
  /** 旧快照缺失时视为 disconnected。 */
  cloudConnection?: CloudConnectionDto;
  update: UpdateStatusDto;
  preferences: AgentPreferencesDto;
  gateway: GatewayStatusDto;
  cloudDevices: CloudDevice[];
  linkedDevices: LinkDeviceInfo[];
  remoteTasks: RemoteTaskDto[];
  pendingCaptures: PendingCaptureDto[];
  linkPairingRequests?: LinkPairingRequestDto[];
  linkDiscovered?: LinkDiscoveredPeer[];
  shell: ShellStatusDto;
  power: PowerStatusDto;
  /** 旧快照缺失时视为默认（上报关、无概览）。 */
  cloudNotify?: CloudNotifyStateDto;
}

/** `system.snapshot` 主体（`role` 相邻标记，`snapshot` 为内容）。 */
export type SnapshotBody =
  | { role: 'daemon'; snapshot: DaemonSnapshot }
  | { role: 'agent'; snapshot: AgentSnapshot };

/** 带原子事件位置的全量快照：之后应从 `sequence + 1` 起消费同 `epoch` 的事件帧。 */
export interface Snapshot {
  epoch: string;
  sequence: number;
  body: SnapshotBody;
}

/** daemon 状态变化事件（`type` + `data` 相邻标记）。 */
export type DaemonEvent =
  | { type: 'taskRuntimeChanged'; data: TaskRuntimeDto }
  | { type: 'taskActivityAdded'; data: TaskActivityDto }
  | { type: 'snapshotReplaced'; data: DaemonSnapshot }
  | { type: 'engine'; data: WsServerMsg }
  | { type: 'taskChanged'; data: TaskDto }
  /** 一批任务事实：等价于按序逐条 `taskChanged`（`daemon.task.createMany` 一块一条）。 */
  | { type: 'tasksChanged'; data: TaskDto[] }
  | { type: 'taskDeleted'; data: { taskId: string } }
  /** 一批任务已删除：等价于按序逐条 `taskDeleted`（`daemon.task.deleteMany` 一批一条）。 */
  | { type: 'tasksDeleted'; data: { taskIds: string[] } }
  | { type: 'queuesChanged'; data: QueueDto[] }
  | { type: 'groupsChanged'; data: GroupDto[] }
  | { type: 'configChanged'; data: DaemonConfigSnapshot }
  | { type: 'rssChanged'; data: { sourceId: string; itemRevision: number } }
  | { type: 'pluginsChanged'; data: PluginDto[] }
  | { type: 'componentsChanged'; data: ComponentStatusDto[] }
  /** 投递日志增量：按 `deliveryId` 合并（上限 1000，`timestampMs` 降序）；空增量不改变列表。 */
  | { type: 'webhooksChanged'; data: WebhookDeliveryDto[] }
  /** 投递日志被显式清空。 */
  | { type: 'webhooksCleared' }
  | { type: 'runtimeStatsChanged'; data: DaemonRuntimeStatsDto }
  | { type: 'selectionPending'; data: SelectionRequestDto }
  | { type: 'selectionResolved'; data: { requestId: string } }
  /** 任务生命周期语义事件：只供 agent 消费，不转发给 UI，不改变快照。 */
  | { type: 'taskNotice'; data: TaskNoticeDto };

/** agent 自有或转发的状态变化事件（`type` + `data` 相邻标记）。 */
export type AgentEvent =
  /** 转发的 daemon 事件：注意嵌套两层 `{type:'daemon', data:{type, data}}`。 */
  | { type: 'daemon'; data: DaemonEvent }
  | { type: 'daemonSnapshotReplaced'; data: DaemonSnapshot }
  | { type: 'daemonConnectionChanged'; data: boolean }
  /** 会话被撤销（一次性通知，不进快照；随后是 `sessionChanged(null)`；主动登出 / 删本设备不发）。 */
  | { type: 'sessionRevoked'; data: ErrorReason }
  | { type: 'sessionChanged'; data: AgentSessionDto | null }
  | { type: 'syncChanged'; data: SyncStatusDto }
  | { type: 'cloudConnectionChanged'; data: CloudConnectionDto }
  | { type: 'updateChanged'; data: UpdateStatusDto }
  | { type: 'preferencesChanged'; data: AgentPreferencesDto }
  | { type: 'gatewayChanged'; data: GatewayStatusDto }
  | { type: 'cloudDevicesChanged'; data: CloudDevice[] }
  | { type: 'linkedDevicesChanged'; data: LinkDeviceInfo[] }
  | { type: 'linkPairingRequestsChanged'; data: LinkPairingRequestDto[] }
  | { type: 'linkDiscoveredChanged'; data: LinkDiscoveredPeer[] }
  | { type: 'remoteTasksChanged'; data: RemoteTaskDto[] }
  | { type: 'pendingCapturesChanged'; data: PendingCaptureDto[] }
  | { type: 'shellChanged'; data: ShellStatusDto }
  | { type: 'powerChanged'; data: PowerStatusDto }
  /** 外部捕获未经确认直接建成的任务 ID（一次性通知，不进快照）。 */
  | { type: 'captureTasksStarted'; data: string[] }
  /** 云端推送状态整体替换。 */
  | { type: 'cloudNotifyChanged'; data: CloudNotifyStateDto };

/** `service.event` 事件主体（`service` + `event` 相邻标记）。 */
export type ServiceEvent =
  | { service: 'daemon'; event: DaemonEvent }
  | { service: 'agent'; event: AgentEvent };

/** 单个严格递增的事件帧（`service.event` 通知的 params）。 */
export interface EventFrame {
  epoch: string;
  sequence: number;
  event: ServiceEvent;
}
