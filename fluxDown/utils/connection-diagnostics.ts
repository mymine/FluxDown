import { browser } from "wxt/browser";
import { redactConnectionError } from "./connection-diagnostics-store";
import type { ConnectionDiagnostic } from "./connection-diagnostics-store";
import { logWarn } from "./error-log";

let backgroundRecorder: ((event: ConnectionDiagnostic) => Promise<void>) | undefined;

export function setDiagnosticRecorder(recorder: typeof backgroundRecorder): void {
  backgroundRecorder = recorder;
}

export function recordConnectionDiagnostic(event: ConnectionDiagnostic): void {
  // Instrumentation must not alter native-messaging results, including when
  // an extension context was invalidated and sendMessage throws synchronously.
  try {
    const safe = { ...event, ...(event.detail ? { detail: redactConnectionError(event.detail) } : {}) };
    const write = backgroundRecorder
      ? backgroundRecorder(safe)
      : browser.runtime.sendMessage({ type: "diagnostics-record", event: safe }).then((reply) => {
          if (!reply?.ok) throw new Error("Diagnostic event was not saved");
        });
    void write.catch((error) => {
      logWarn("[FluxDown Diagnostics] Could not save connection event:", error);
    });
  } catch (error) {
    logWarn("[FluxDown Diagnostics] Could not record connection event:", error);
  }
}

export async function exportConnectionDiagnostics(): Promise<void> {
  const response = await browser.runtime.sendMessage({ type: "diagnostics-export" });
  if (!response?.ok) throw new Error("Diagnostic export failed");
}
