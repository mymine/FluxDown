import { logError, logWarn } from "./error-log";
/**
 * 专用 Native Messaging 端口池：为耗时请求（页面脚本 `plugin_invoke`）按 key 各开一条独立连接。
 *
 * 为什么不复用 `native-messaging.ts` 的常驻端口：NMH 中继（native/nmh）与 agent 的连接处理都是
 * 严格「一问一答」串行模型，一次数十秒的插件处理器调用会堵住同一端口上的下载 / capabilities /
 * 规则同步。专用端口把慢请求隔离在自己的 NMH 进程里。
 *
 * 语义：
 * - 每个 key（插件 identity）一条端口，按需 `connect()`；同 key 的请求在该端口内排队，一次只发一个；
 * - 帧格式与常驻端口一致：`{ action, msg_id, ...payload }` ↔ `{ success, msg_id, message?, data? }`；
 * - 空闲 `idleMs` 后自动 disconnect（有在途 / 排队请求时不会关）；
 * - 请求超时：该请求以 `timeout` 失败并断开端口（中继仍在等应答，继续复用会让后续请求排在它后面），
 *   排队中的请求在新端口上继续；
 * - 端口断开：在途请求以 `port disconnected` 失败，排队请求会在新端口上重试一次连接，
 *   连接本身失败（`connect()` 抛错）时全部以 `native_messaging_unavailable` 失败。
 */

export interface NmhPortLike {
  postMessage(message: unknown): void;
  disconnect(): void;
  onMessage: { addListener(listener: (message: unknown) => void): void };
  onDisconnect: { addListener(listener: () => void): void };
}

export interface NmhCallResult<T = unknown> {
  success: boolean;
  message?: string;
  data?: T;
}

export interface DedicatedPoolOptions {
  /** 建立一条新的 NMH 连接（`runtime.connectNative`）。 */
  connect: () => NmhPortLike;
  /** 空闲多久后关闭端口，默认 30 s。 */
  idleMs?: number;
  /** 测试注入。 */
  setTimer?: (run: () => void, ms: number) => unknown;
  clearTimer?: (handle: unknown) => void;
}

export interface DedicatedPool {
  call<T = unknown>(key: string, action: string, payload: object, timeoutMs: number): Promise<NmhCallResult<T>>;
  /** 当前打开的专用端口数（测试 / 诊断用）。 */
  openPorts(): number;
}

export const DEFAULT_IDLE_MS = 30_000;

interface Request {
  msgId: number;
  action: string;
  payload: object;
  timeoutMs: number;
  resolve: (result: NmhCallResult) => void;
  timer?: unknown;
}

interface Lane {
  port: NmhPortLike | null;
  queue: Request[];
  inflight: Request | null;
  idleTimer?: unknown;
}

export function createDedicatedPool(options: DedicatedPoolOptions): DedicatedPool {
  const idleMs = options.idleMs ?? DEFAULT_IDLE_MS;
  const setTimer = options.setTimer ?? ((run, ms) => setTimeout(run, ms));
  const clearTimer = options.clearTimer ?? ((handle) => clearTimeout(handle as ReturnType<typeof setTimeout>));
  const lanes = new Map<string, Lane>();
  let nextMsgId = 1;

  function disarmIdle(lane: Lane): void {
    if (lane.idleTimer !== undefined) {
      clearTimer(lane.idleTimer);
      lane.idleTimer = undefined;
    }
  }

  function closePort(lane: Lane): void {
    const port = lane.port;
    lane.port = null;
    if (!port) return;
    try {
      port.disconnect();
    } catch (e) {
      console.debug("[FluxDown NMH] dedicated port disconnect failed:", e);
    }
  }

  function armIdle(key: string, lane: Lane): void {
    disarmIdle(lane);
    lane.idleTimer = setTimer(() => {
      lane.idleTimer = undefined;
      if (lane.inflight || lane.queue.length > 0) return;
      closePort(lane);
      if (lanes.get(key) === lane) lanes.delete(key);
    }, idleMs);
  }

  function settle(lane: Lane, request: Request, result: NmhCallResult): void {
    if (request.timer !== undefined) clearTimer(request.timer);
    request.timer = undefined;
    if (lane.inflight === request) lane.inflight = null;
    request.resolve(result);
  }

  /** 端口失效：在途请求失败，丢弃端口；排队请求留给下一次 pump 在新端口上发。 */
  function onPortGone(key: string, lane: Lane, port: NmhPortLike, message: string): void {
    if (lane.port !== port) return;
    lane.port = null;
    const inflight = lane.inflight;
    if (inflight) settle(lane, inflight, { success: false, message });
    pump(key, lane);
  }

  function ensurePort(key: string, lane: Lane): NmhPortLike | null {
    if (lane.port) return lane.port;
    let port: NmhPortLike;
    try {
      port = options.connect();
    } catch (e) {
      logError("[FluxDown NMH] dedicated connectNative() threw:", e);
      return null;
    }
    lane.port = port;
    port.onMessage.addListener((raw) => {
      if (lane.port !== port) return;
      const msg = raw as { msg_id?: unknown; success?: unknown; message?: unknown; data?: unknown } | null;
      const inflight = lane.inflight;
      if (!inflight || msg?.msg_id !== inflight.msgId) return;
      settle(lane, inflight, {
        success: msg.success === true,
        message: typeof msg.message === "string" ? msg.message : undefined,
        data: msg.data,
      });
      pump(key, lane);
    });
    port.onDisconnect.addListener(() => onPortGone(key, lane, port, "port disconnected"));
    return port;
  }

  function pump(key: string, lane: Lane): void {
    if (lane.inflight) return;
    const next = lane.queue.shift();
    if (!next) {
      if (lane.port) armIdle(key, lane);
      else if (lanes.get(key) === lane) lanes.delete(key);
      return;
    }
    disarmIdle(lane);
    const port = ensurePort(key, lane);
    if (!port) {
      // 连不上：排队的全部失败，避免请求永远挂着。
      const failed = [next, ...lane.queue.splice(0)];
      for (const request of failed) request.resolve({ success: false, message: "native_messaging_unavailable" });
      if (lanes.get(key) === lane) lanes.delete(key);
      return;
    }
    lane.inflight = next;
    next.timer = setTimer(() => {
      next.timer = undefined;
      if (lane.inflight !== next) return;
      settle(lane, next, { success: false, message: "timeout" });
      closePort(lane);
      pump(key, lane);
    }, next.timeoutMs);
    try {
      port.postMessage({ action: next.action, msg_id: next.msgId, ...next.payload });
    } catch (e) {
      logWarn("[FluxDown NMH] dedicated postMessage failed:", e);
      settle(lane, next, { success: false, message: "postMessage failed" });
      closePort(lane);
      pump(key, lane);
    }
  }

  return {
    call<T>(key: string, action: string, payload: object, timeoutMs: number): Promise<NmhCallResult<T>> {
      return new Promise<NmhCallResult>((resolve) => {
        let lane = lanes.get(key);
        if (!lane) {
          lane = { port: null, queue: [], inflight: null };
          lanes.set(key, lane);
        }
        lane.queue.push({ msgId: nextMsgId++, action, payload, timeoutMs, resolve });
        pump(key, lane);
      }) as Promise<NmhCallResult<T>>;
    },
    openPorts(): number {
      let count = 0;
      for (const lane of lanes.values()) if (lane.port) count++;
      return count;
    },
  };
}
