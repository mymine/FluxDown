import type { JSX } from "preact";
import { signal } from "@preact/signals";
import "./components.css";

interface ToastState {
  id: number;
  msg: string;
  kind: "info" | "success" | "error";
}

const current = signal<ToastState | null>(null);
let seq = 0;
let timer: ReturnType<typeof setTimeout> | undefined;

/** 全局提示，2000 ms 自动消失；需在页面树中挂一个 <ToastHost />。 */
export function showToast(msg: string, kind: ToastState["kind"] = "info"): void {
  const id = ++seq;
  current.value = { id, msg, kind };
  if (timer) clearTimeout(timer);
  timer = setTimeout(() => {
    if (current.value?.id === id) current.value = null;
  }, 2000);
}

export function ToastHost(): JSX.Element | null {
  const toast = current.value;
  if (!toast) return null;
  return (
    <div class={`fd-toast fd-toast-${toast.kind}`} role="status" key={toast.id}>
      {toast.msg}
    </div>
  );
}
