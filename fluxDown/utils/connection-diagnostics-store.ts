export const DIAGNOSTICS_KEY = "connectionDiagnostics";
export const MAX_DIAGNOSTIC_ENTRIES = 200;
export const DIAGNOSTIC_MAX_AGE_MS = 7 * 24 * 60 * 60 * 1000;

const EVENTS = [
  "background.started", "native.connect_failed", "native.disconnected",
  "native.connected", "native.request_failed", "native.retry",
] as const;
const ACTIONS = ["ping", "warmup", "tasks", "download", "batch_download", "task_op", "open_file", "reveal_file"];
export type DiagnosticSource = "background" | "popup" | "options" | "sidepanel";
export interface ConnectionDiagnostic {
  event: typeof EVENTS[number];
  action?: string;
  detail?: string;
}
export interface DiagnosticEntry extends ConnectionDiagnostic {
  source: DiagnosticSource;
  firstAt: number;
  lastAt: number;
  count: number;
}
interface Storage {
  get(key: string): Promise<Record<string, unknown>>;
  set(items: Record<string, unknown>): Promise<void>;
}

// Only browser-generated errors enter this function. Host reply text and request
// payloads are deliberately excluded: they can contain arbitrary user secrets.
export function redactConnectionError(value: string): string {
  return value
    .replace(/\b(?:https?|ftp|file|chrome-extension|moz-extension):\/\/[^\s<>"']+/gi, "[URL]")
    .replace(/\b[A-Z]:[\\/][^\r\n"']*/gi, "[PATH]")
    .replace(/\\\\[^\r\n"']+/g, "[PATH]")
    .replace(/\/(?:Users|home|tmp|var|private)\/[^\r\n"']*/g, "[PATH]")
    .replace(/\b(?:authorization|proxy-authorization|cookie|set-cookie|[\w-]*token|password|secret)\b\s*[:=]\s*[^\r\n]+/gi, "[CREDENTIAL]")
    .replace(/\b(?:Bearer|Basic)\s+[A-Za-z0-9._~+/=-]+/gi, "[CREDENTIAL]")
    .replace(/[\u0000-\u001f\u007f]/g, " ")
    .slice(0, 512);
}

function normalize(input: unknown): ConnectionDiagnostic | null {
  if (!input || typeof input !== "object") return null;
  const value = input as Record<string, unknown>;
  if (!EVENTS.includes(value.event as ConnectionDiagnostic["event"])) return null;
  return {
    event: value.event as ConnectionDiagnostic["event"],
    ...(typeof value.action === "string" && ACTIONS.includes(value.action) ? { action: value.action } : {}),
    ...(typeof value.detail === "string" ? { detail: redactConnectionError(value.detail) } : {}),
  };
}

// One instance, owned by background. UI contexts send events via runtime messages;
// separate read-modify-write loops in popup/options would lose concurrent errors.
export class ConnectionDiagnosticsStore {
  private tail: Promise<unknown> = Promise.resolve();

  constructor(private storage: Storage, private now = Date.now) {}

  private enqueue<T>(operation: () => Promise<T>): Promise<T> {
    const result = this.tail.then(operation);
    // Keep the queue usable after a storage failure; the caller still receives
    // the original rejection and must surface it.
    this.tail = result.catch(() => undefined);
    return result;
  }

  private async read(): Promise<DiagnosticEntry[]> {
    const stored = (await this.storage.get(DIAGNOSTICS_KEY))[DIAGNOSTICS_KEY];
    if (!Array.isArray(stored)) return [];
    const cutoff = this.now() - DIAGNOSTIC_MAX_AGE_MS;
    return stored.slice(-MAX_DIAGNOSTIC_ENTRIES).flatMap((item) => {
      const event = normalize(item);
      if (!event || !["background", "popup", "options", "sidepanel"].includes(item.source) ||
          !Number.isFinite(item.firstAt) || !Number.isFinite(item.lastAt) ||
          item.firstAt > item.lastAt || item.lastAt < cutoff ||
          !Number.isSafeInteger(item.count) || item.count < 1) return [];
      return [{ ...event, source: item.source as DiagnosticSource,
        firstAt: item.firstAt, lastAt: item.lastAt, count: item.count }];
    });
  }

  append(input: unknown, source: DiagnosticSource): Promise<void> {
    const event = normalize(input);
    if (!event) return Promise.reject(new Error("Invalid diagnostic event"));
    return this.enqueue(async () => {
      const entries = await this.read();
      const now = this.now();
      const last = entries.at(-1);
      if (last && last.source === source && last.event === event.event &&
          last.action === event.action && last.detail === event.detail) {
        last.lastAt = now;
        last.count = Math.min(Number.MAX_SAFE_INTEGER, last.count + 1);
      } else {
        entries.push({ ...event, source, firstAt: now, lastAt: now, count: 1 });
      }
      await this.storage.set({ [DIAGNOSTICS_KEY]: entries.slice(-MAX_DIAGNOSTIC_ENTRIES) });
    });
  }

  snapshot(): Promise<DiagnosticEntry[]> {
    return this.enqueue(() => this.read());
  }
}
