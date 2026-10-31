/**
 * 常驻共享 Native Messaging 端口：下载、capabilities、规则同步、任务面板等请求共用一条连接。
 *
 * NMH 中继（native/nmh）与 agent 的连接处理都是严格「一问一答」串行模型：端口上的请求按发送顺序
 * 逐个处理，每条请求恰好按序得到一次应答（扩展侧已超时的请求也会迟到应答）。
 *
 * 语义：
 * - 按需 `connect()`；连接常驻复用，断开后下一次请求重建；
 * - 帧格式：`{ action, msg_id, ...payload }` ↔ `{ success, msg_id, message?, ... }`，应答原样交给调用方；
 * - 请求超时：该请求以 `timeout` 失败。若队首（最早发出仍未应答的请求，含已超时的）已超过自身时限，
 *   说明连接卡死，排在后面的请求注定全部超时 → 立即回收连接，下一次请求换新的 NMH 进程；
 *   只是排在合法慢请求（如强制规则同步）后面而超时的请求不回收；
 * - 连接断开 / 回收：其上等待中的请求以 `port disconnected` 失败（下载类请求据此先探活再决定是否重发）；
 *   旧连接迟到的断开事件不会波及新连接。
 */
import type { ConnectionDiagnostic } from "./connection-diagnostics-store";
import type { NmhPortLike } from "./nmh-dedicated";
import { logError, logWarn } from "./error-log";

type SharedFailure = "native_messaging_unavailable" | "timeout" | "port disconnected" | "postMessage failed";

/** 主机应答（原样）或连接层失败。 */
export type SharedReply = Record<string, unknown>;

export interface SharedChannelOptions {
  /** 建立一条新的 NMH 连接（`runtime.connectNative`），API 不可用时抛错。 */
  connect: () => NmhPortLike;
  /** 连接断开的原因（Chrome：`runtime.lastError`；Firefox：`port.error`）。 */
  disconnectReason: (port: NmhPortLike) => string | undefined;
  /** 连接诊断；只记录，不影响结果。 */
  record: (event: ConnectionDiagnostic) => void;
}

export interface SharedChannel {
  send(action: string, payload: Record<string, unknown>, timeoutMs: number): Promise<SharedReply>;
  /** 主动回收当前连接（重试前换新的 NMH 进程）；没有连接时无操作。 */
  recycle(): void;
}

/** 已发出、尚未收到应答的请求（含扩展侧已超时的），按发送顺序排列。 */
interface InFlight {
  msgId: number;
  deadline: number;
}

interface Lane {
  port: NmhPortLike;
  inFlight: InFlight[];
}

interface Pending {
  action: string;
  lane: Lane;
  timer: ReturnType<typeof setTimeout>;
  resolve: (reply: SharedReply) => void;
}

function failure(message: SharedFailure): SharedReply {
  return { success: false, message };
}

export function createSharedChannel(options: SharedChannelOptions): SharedChannel {
  let current: Lane | null = null;
  let nextMsgId = 1;
  let connected = false;
  const pending = new Map<number, Pending>();

  /** `reason` 为 null 时调用方已另行记录诊断。 */
  function close(lane: Lane, reason: string | null): void {
    if (current !== lane) return;
    current = null;
    connected = false;
    if (reason !== null) options.record({ event: "native.disconnected", detail: reason });
    try {
      lane.port.disconnect();
    } catch {
      // 已断开
    }
    for (const [id, request] of pending) {
      if (request.lane !== lane) continue;
      clearTimeout(request.timer);
      pending.delete(id);
      request.resolve(failure("port disconnected"));
    }
  }

  function onReply(lane: Lane, message: unknown): void {
    const reply: SharedReply = message !== null && typeof message === "object" ? (message as SharedReply) : {};
    const msgId = reply.msg_id;
    // 每条请求恰好按序应答一次；无法解析的请求应答 msg_id 为 0，按队首出队。
    const index = lane.inFlight.findIndex((entry) => entry.msgId === msgId);
    lane.inFlight.splice(index >= 0 ? index : 0, 1);
    if (typeof msgId !== "number") return;
    const request = pending.get(msgId);
    if (!request) return;
    pending.delete(msgId);
    clearTimeout(request.timer);
    if (reply.success === true) {
      if (request.action !== "warmup") {
        if (!connected) options.record({ event: "native.connected", action: request.action });
        connected = true;
      }
    } else {
      if (reply.message === "app_not_running") connected = false;
      options.record({
        event: "native.request_failed",
        action: request.action,
        detail: reply.message === "app_not_running" ? "app_not_running" : "host_rejected_request",
      });
    }
    request.resolve(reply);
  }

  function lane(): Lane | null {
    if (current) return current;
    let port: NmhPortLike;
    try {
      port = options.connect();
    } catch (e) {
      logError("[FluxDown NMH] connectNative() threw:", e);
      options.record({ event: "native.connect_failed", detail: e instanceof Error ? e.message : String(e) });
      return null;
    }
    const opened: Lane = { port, inFlight: [] };
    current = opened;
    port.onMessage.addListener((message) => onReply(opened, message));
    port.onDisconnect.addListener(() => {
      // 常见原因："No such native application"、"Access to the specified native messaging host
      // is forbidden"（扩展 ID 与 NMH manifest 的 allowed_origins 不符）。
      const reason = options.disconnectReason(port);
      if (reason) logError("[FluxDown NMH] port disconnected, reason:", reason);
      else logWarn("[FluxDown NMH] port disconnected (no error reason)");
      close(opened, reason || "port disconnected (no error reason)");
    });
    return opened;
  }

  function send(action: string, payload: Record<string, unknown>, timeoutMs: number): Promise<SharedReply> {
    return new Promise<SharedReply>((resolve) => {
      const target = lane();
      if (!target) {
        resolve(failure("native_messaging_unavailable"));
        return;
      }
      const msgId = nextMsgId++;
      const timer = setTimeout(() => {
        pending.delete(msgId);
        connected = false;
        options.record({ event: "native.request_failed", action, detail: "timeout" });
        resolve(failure("timeout"));
        const head = target.inFlight[0];
        if (head && (head.msgId === msgId || head.deadline <= Date.now())) {
          close(target, `stalled: ${action} timed out`);
        }
      }, timeoutMs);
      pending.set(msgId, { action, lane: target, timer, resolve });
      target.inFlight.push({ msgId, deadline: Date.now() + timeoutMs });
      try {
        target.port.postMessage({ action, msg_id: msgId, ...payload });
      } catch {
        pending.delete(msgId);
        clearTimeout(timer);
        target.inFlight.pop();
        close(target, null);
        options.record({ event: "native.request_failed", action, detail: "postMessage failed" });
        resolve(failure("postMessage failed"));
      }
    });
  }

  return {
    send,
    recycle() {
      if (current) close(current, null);
    },
  };
}
