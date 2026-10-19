/** 任务面板：完成通知轮询（chrome.alarms）与通知交互。 */
import { browser } from "wxt/browser";
import type { Browser } from "wxt/browser";
import { t } from "@/utils/i18n";
import { nmhListTasks, nmhOpenFile, nmhRevealFile } from "@/utils/native-messaging";
import type { TaskBrief } from "@/utils/native-messaging";
import { loadSettings } from "@/utils/settings";

// ──────────────────────────────────────────────────────────────
// 任务面板：完成通知轮询（chrome.alarms）
// ──────────────────────────────────────────────────────────────
// 仅当"已知存在活跃任务"（pending/downloading/preparing）时才创建低频
// periodic alarm 轮询任务列表；无活跃任务时清除 alarm，避免长期空转。
// 活跃感知来源：
//   a) 本地下载请求发送成功后（sendToFluxDown / batchDownload），乐观地
//      认为"可能有活跃任务"，启动 alarm；
//   b) 每次拿到任务列表（无论来自 alarm 轮询还是 popup 的 nmh-tasks 消息）
//      时按其中是否存在活跃状态任务续期或停止。
// 远程模式下投递到远程服务器的任务不经过本地 NMH，nmhListTasks 天然拿不
// 到它们，故本轮询自动不参与远程任务的完成通知。

const TASK_POLL_ALARM_NAME = "fluxdown-task-poll";
// Chrome 对 periodInMinutes < 0.5 不予兑现（会被夹到 30s 一次；未打包的
// 开发者模式下不受限）。这里按需求写 0.1（6s）表达"尽可能快"的意图，
// 生产环境实际以 Chrome 的 30s 下限为准，属预期内的可接受降级。
const TASK_POLL_ALARM_PERIOD_MIN = 0.1;
// 判定"活跃"的任务状态：0=pending, 1=downloading, 5=preparing。
const ACTIVE_TASK_STATUSES = new Set([0, 1, 5]);
const TASK_STATUS_COMPLETED = 3;

async function ensureTaskPollAlarm(): Promise<void> {
  try {
    const existing = await browser.alarms.get(TASK_POLL_ALARM_NAME);
    if (existing) return;
    await browser.alarms.create(TASK_POLL_ALARM_NAME, {
      periodInMinutes: TASK_POLL_ALARM_PERIOD_MIN,
    });
  } catch (e) {
    console.warn("[FluxDown] ensureTaskPollAlarm failed:", e);
  }
}

async function clearTaskPollAlarm(): Promise<void> {
  try {
    await browser.alarms.clear(TASK_POLL_ALARM_NAME);
  } catch {
    /* ignore */
  }
}

/** download/batchDownload 成功发送后调用：远程通道不参与本地完成通知。 */
export function maybeArmTaskPollForChannel(channel?: "local" | "remote"): void {
  if (channel === "remote") return;
  ensureTaskPollAlarm().catch(() => {});
}

// 任务状态快照（taskId → 上次已知 status），用于 diff 出"上次非完成 →
// 本次完成"的任务。MV3 Service Worker 会被随时终止，快照必须落 storage
// 才能跨重启存活；storage.session 不可用时（极旧浏览器）退化到
// storage.local。
const TASK_SNAPSHOT_SESSION_KEY = "fluxdownTaskSnapshot";
const TASK_SNAPSHOT_LOCAL_KEY = "fluxdownTaskSnapshotFallback";
let _taskSnapshotCache: Record<string, number> | null = null;

async function loadTaskSnapshot(): Promise<Record<string, number>> {
  if (_taskSnapshotCache) return _taskSnapshotCache;
  try {
    const session = (browser.storage as any).session;
    if (session) {
      const stored = await session.get(TASK_SNAPSHOT_SESSION_KEY);
      _taskSnapshotCache = stored?.[TASK_SNAPSHOT_SESSION_KEY] ?? {};
      return _taskSnapshotCache!;
    }
  } catch {
    /* 落到 storage.local 兜底 */
  }
  try {
    const stored = await browser.storage.local.get(TASK_SNAPSHOT_LOCAL_KEY);
    _taskSnapshotCache =
      (stored?.[TASK_SNAPSHOT_LOCAL_KEY] as Record<string, number> | undefined) ??
      {};
  } catch {
    _taskSnapshotCache = {};
  }
  return _taskSnapshotCache!;
}

function persistTaskSnapshot(snapshot: Record<string, number>): void {
  _taskSnapshotCache = snapshot;
  try {
    const session = (browser.storage as any).session;
    if (session?.set) {
      session.set({ [TASK_SNAPSHOT_SESSION_KEY]: snapshot }).catch(() => {});
      return;
    }
  } catch {
    /* 落到 storage.local 兜底 */
  }
  browser.storage.local
    .set({ [TASK_SNAPSHOT_LOCAL_KEY]: snapshot })
    .catch(() => {});
}

const TASK_NOTIFICATION_PREFIX = "fluxdown-task-";

function extractTaskIdFromNotificationId(
  notificationId: string,
): string | null {
  return notificationId.startsWith(TASK_NOTIFICATION_PREFIX)
    ? notificationId.slice(TASK_NOTIFICATION_PREFIX.length)
    : null;
}

/**
 * 发出"任务已完成"桌面通知。
 * Chrome：带「打开文件」「打开文件夹」两个按钮（onButtonClicked 处理）。
 * Firefox（MV2 不支持 buttons）：降级为点击通知主体 = 打开文件夹
 * （onClicked 处理，见下方监听器）。
 */
function notifyTaskCompleted(task: TaskBrief): void {
  if (!browser.notifications?.create) return;
  const notificationId = `${TASK_NOTIFICATION_PREFIX}${task.taskId}`;
  const opts: Record<string, any> = {
    type: "basic",
    iconUrl: "/icon/128.png",
    title: t("notify.taskCompletedTitle"),
    message: t("notify.taskCompletedDetail", {
      name: task.fileName || task.taskId,
    }),
  };
  if (!import.meta.env.FIREFOX) {
    opts.buttons = [
      { title: t("notify.openFile") },
      { title: t("notify.openFolder") },
    ];
  }
  try {
    Promise.resolve(
      browser.notifications.create(notificationId, opts as any),
    ).catch(() => {});
  } catch (e) {
    console.warn("[FluxDown] notifyTaskCompleted: failed to create:", e);
  }
}

// 串行 Promise 链：防止 alarm 轮询与 popup nmh-tasks 轮询并发触发时对
// 快照的 read-modify-write 产生竞态（同一次"完成"被两条路径各通知一次）。
// 与文件顶部 _statChain / _queueChain 是同一 Bug 4 修复模式。
let _taskSnapshotChain: Promise<void> = Promise.resolve();

async function processTasksPollResultInner(
  tasks: TaskBrief[],
): Promise<void> {
  const prevSnapshot = await loadTaskSnapshot();
  const nextSnapshot: Record<string, number> = {};
  const newlyCompleted: TaskBrief[] = [];

  for (const task of tasks) {
    nextSnapshot[task.taskId] = task.status;
    const prevStatus = prevSnapshot[task.taskId];
    if (
      prevStatus !== undefined &&
      prevStatus !== TASK_STATUS_COMPLETED &&
      task.status === TASK_STATUS_COMPLETED
    ) {
      newlyCompleted.push(task);
    }
  }
  persistTaskSnapshot(nextSnapshot);

  if (newlyCompleted.length > 0) {
    const settings = await loadSettings();
    if (settings.notifyLocalTask === true) {
      for (const task of newlyCompleted) {
        notifyTaskCompleted(task);
      }
    }
  }

  const hasActive = tasks.some((task) =>
    ACTIVE_TASK_STATUSES.has(task.status),
  );
  if (hasActive) {
    await ensureTaskPollAlarm();
  } else {
    await clearTaskPollAlarm();
  }
}

/**
 * 处理一批任务快照：diff 出新完成的任务并按设置弹通知，同时按是否还有
 * 活跃任务决定续期还是清除轮询 alarm。alarm 触发与 popup 的 nmh-tasks
 * 轮询共用此函数，保证两条路径下的通知/alarm 状态完全一致。
 */
export function processTasksPollResult(tasks: TaskBrief[]): Promise<void> {
  _taskSnapshotChain = _taskSnapshotChain.then(() =>
    processTasksPollResultInner(tasks),
  );
  return _taskSnapshotChain;
}

/** 同步注册通知与 alarm 监听；由 startBackground 调用一次。 */
export function installTaskListeners(): void {
  if (browser.notifications?.onButtonClicked) {
    browser.notifications.onButtonClicked.addListener(
      (notificationId, buttonIndex) => {
        const taskId = extractTaskIdFromNotificationId(notificationId);
        if (!taskId) return;
        const op = buttonIndex === 0 ? nmhOpenFile : nmhRevealFile;
        op(taskId).catch(() => {});
        Promise.resolve(browser.notifications.clear(notificationId)).catch(
          () => {},
        );
      },
    );
  }

  if (browser.notifications?.onClicked) {
    browser.notifications.onClicked.addListener((notificationId) => {
      // Chrome 下点击通知主体不做动作（两个按钮已覆盖打开文件/文件夹诉求）；
      // Firefox MV2 不支持 buttons，点击主体降级为"打开文件夹"。
      if (!import.meta.env.FIREFOX) return;
      const taskId = extractTaskIdFromNotificationId(notificationId);
      if (!taskId) return;
      nmhRevealFile(taskId).catch(() => {});
      Promise.resolve(browser.notifications.clear(notificationId)).catch(
        () => {},
      );
    });
  }

  browser.alarms.onAlarm.addListener((alarm) => {
    if (alarm.name !== TASK_POLL_ALARM_NAME) return;
    nmhListTasks()
      .then((result) => {
        if (!result.success) return; // App 暂不可达，等下一轮
        return processTasksPollResult(result.tasks);
      })
      .catch((e) => {
        console.warn("[FluxDown] task poll alarm handler failed:", e);
      });
  });
}
