/**
 * 订阅某 tab 的 TabView（C6 端口协议）。
 * subscribe 为 number（popup/侧边栏指定 tab）或 "sender"（content UI 用端口所在 tab）。
 * 端口断开 1 s 后重连；tabId 变化时重新订阅。页面进入 bfcache 时主动断开、恢复时立即重连：
 * 否则 Chrome 会以「page … moved into back/forward cache」强制关闭端口。
 */
import { useEffect, useRef } from "preact/hooks";
import { signal, type Signal } from "@preact/signals";
import { browser, type Browser } from "wxt/browser";
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
    let port: Browser.runtime.Port | null = null;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let disposed = false;

    const connect = () => {
      clearTimeout(timer);
      timer = undefined;
      if (disposed || port) return;
      let opened: Browser.runtime.Port;
      try {
        opened = browser.runtime.connect({ name: TAB_VIEW_PORT });
      } catch {
        timer = setTimeout(connect, RECONNECT_MS);
        return;
      }
      port = opened;
      opened.onMessage.addListener((msg: unknown) => {
        const push = msg as TabViewPush;
        if (push?.type === "tabView") view.value = push.view;
      });
      opened.onDisconnect.addListener(() => {
        // 被动断开（后台重启、扩展重载等）的原因在 runtime.lastError；不读会被记为 Unchecked 错误。
        void browser.runtime.lastError;
        if (port !== opened) return;
        port = null;
        if (!disposed) timer = setTimeout(connect, RECONNECT_MS);
      });
      const sub: TabViewSubscribe = { subscribe: tabId };
      opened.postMessage(sub);
    };
    const disconnect = () => {
      clearTimeout(timer);
      timer = undefined;
      const current = port;
      port = null;
      try {
        current?.disconnect();
      } catch {
        // 已断开
      }
    };
    const onPageHide = (event: PageTransitionEvent) => {
      if (event.persisted) disconnect();
    };
    const onPageShow = (event: PageTransitionEvent) => {
      if (event.persisted) connect();
    };
    window.addEventListener("pagehide", onPageHide);
    window.addEventListener("pageshow", onPageShow);
    connect();
    return () => {
      disposed = true;
      window.removeEventListener("pagehide", onPageHide);
      window.removeEventListener("pageshow", onPageShow);
      disconnect();
    };
  }, [tabId]);
  return view;
}


function useViewSignal(): Signal<TabView | null> {
  const ref = useRef<Signal<TabView | null> | undefined>(undefined);
  if (!ref.current) ref.current = signal<TabView | null>(null);
  return ref.current;
}
