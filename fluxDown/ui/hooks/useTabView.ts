/**
 * 订阅某 tab 的 TabView（C6 端口协议）。
 * subscribe 为 number（popup/侧边栏指定 tab）或 "sender"（content UI 用端口所在 tab）。
 * 端口断开 1 s 后重连；tabId 变化时重新订阅。
 */
import { useEffect, useRef } from "preact/hooks";
import { signal, type Signal } from "@preact/signals";
import { browser } from "wxt/browser";
import {
  TAB_VIEW_PORT,
  type TabView,
  type TabViewPush,
  type TabViewSubscribe,
} from "@/background/protocol";

const RECONNECT_MS = 1000;

export function useTabView(tabId: number | "sender" | null): Signal<TabView | null> {
  // 每个调用点一个信号；useEffect 依赖 tabId 重建订阅
  const view = useViewSignal();
  useEffect(() => {
    view.value = null;
    if (tabId === null) return;
    let port: ReturnType<typeof browser.runtime.connect> | null = null;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let disposed = false;

    const connect = () => {
      if (disposed) return;
      try {
        port = browser.runtime.connect({ name: TAB_VIEW_PORT });
      } catch {
        timer = setTimeout(connect, RECONNECT_MS);
        return;
      }
      port.onMessage.addListener((msg: unknown) => {
        const push = msg as TabViewPush;
        if (push?.type === "tabView") view.value = push.view;
      });
      port.onDisconnect.addListener(() => {
        port = null;
        if (!disposed) timer = setTimeout(connect, RECONNECT_MS);
      });
      const sub: TabViewSubscribe = { subscribe: tabId };
      port.postMessage(sub);
    };
    connect();
    return () => {
      disposed = true;
      if (timer) clearTimeout(timer);
      try {
        port?.disconnect();
      } catch {
        // 已断开
      }
    };
  }, [tabId]);
  return view;
}


function useViewSignal(): Signal<TabView | null> {
  const ref = useRef<Signal<TabView | null> | undefined>(undefined);
  if (!ref.current) ref.current = signal<TabView | null>(null);
  return ref.current;
}
