// 任务表 / 队列位置表分页（native/protocol/src/table_sync.rs）。

import type { QueueOrderEntryDto } from './queue';
import type { TaskDto, TaskDtoWire } from './task';

/** 每页默认行数（服务端缺省值）。 */
export const TASK_PAGE_DEFAULT_LIMIT = 2_000;
/** 每页行数硬上限：请求超过时服务端按它截断。 */
export const TASK_PAGE_MAX_LIMIT = 5_000;
/** 载入期间最多缓冲的事件帧数；超过即放弃本轮、整体重同步。 */
export const TABLE_SYNC_BACKLOG_LIMIT = 65_536;

/** 分页的表。 */
export type TableSection = 'tasks' | 'queue';

/** `system.taskPage` 参数。 */
export interface TablePageParams {
  section: TableSection;
  /** 上一页的 `nextCursor`；缺省 = 从头开始。 */
  cursor?: string;
  /** 每页行数；缺省 {@link TASK_PAGE_DEFAULT_LIMIT}，超过 {@link TASK_PAGE_MAX_LIMIT} 按上限截断。 */
  limit?: number;
}

/**
 * `system.taskPage` 应答（线上形态）：一页行，加读取它时服务端的事件位置。
 * 任务按 `taskId` 升序（游标 = 上一页最后一个 taskId），队列页按 `order` 升序（游标 = 上一页最后一个
 * `order` 的十进制文本）；`nextCursor` 缺省 = 末页。`tasks` / `queue` 为空时可能整个省略。
 */
export interface TablePageWire {
  epoch: string;
  sequence: number;
  section: TableSection;
  /** 该表此刻的总行数。 */
  total: number;
  tasks?: TaskDtoWire[];
  queue?: QueueOrderEntryDto[];
  nextCursor?: string;
}

/** 归一化后的页（`normalizeTablePage` 产物）：任务补齐默认值，空行集合为空数组。 */
export interface TablePage {
  epoch: string;
  sequence: number;
  section: TableSection;
  total: number;
  tasks: TaskDto[];
  queue: QueueOrderEntryDto[];
  nextCursor?: string;
}
