/** 悬浮球（拖动 / 贴边吸附 / 位置记忆）+ 资源面板（ResourceList compact）。 */
import type { JSX } from "preact";
import { useEffect, useRef, useState } from "preact/hooks";
import { browser } from "wxt/browser";
import type { TabView } from "@/background/protocol";
import type { PageConfig } from "@/page/bridge";
import { saveSiteProfile, DEFAULT_SITE_PROFILE } from "@/utils/site-profiles";
import { sendAction } from "@/utils/messaging";
import { Icon } from "@/ui/components/Icon";
import { IconButton } from "@/ui/components/IconButton";
import { ToastHost } from "@/ui/components/Toast";
import { ResourceList } from "@/ui/features/ResourceList";
import { useTabView } from "@/ui/hooks/useTabView";
import { useT } from "@/ui/i18n";
import { useThemeAttr } from "@/ui/theme";

const DOT_POS_KEY = "fluxdown_dot_pos";
const DOT_SIZE = 36;
const PANEL_W = 380;
const PANEL_H = 460;

type Side = "left" | "right";

/** 徽标数 = 候选可见行（variants 数，至少 1）+ 文件数，与 background 的徽标口径一致。 */
function badgeCount(view: TabView | null): number {
  if (!view) return 0;
  let n = view.files.length;
  for (const r of [...view.playing, ...view.otherElements, ...view.unbound]) {
    n += Math.max(r.candidate.variants.length, 1);
  }
  return n;
}

export function PanelApp(props: { host: HTMLElement }): JSX.Element | null {
  const t = useT();
  useThemeAttr(props.host);
  const view = useTabView("sender").value;

  const [config, setConfig] = useState<PageConfig | null>(null);
  const [open, setOpen] = useState(false);
  const [side, setSide] = useState<Side>("right");
  const [y, setY] = useState(() => Math.round(window.innerHeight * 0.4));
  const [dragLeft, setDragLeft] = useState<number | null>(null);
  const dotRef = useRef<HTMLButtonElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const drag = useRef({ active: false, moved: 0, startX: 0, startY: 0, startLeft: 0, startTop: 0, didDrag: false });

  // PageConfig：自己拉取并跟随 pageConfigChanged（不在内容脚本里加载完整 settings）
  useEffect(() => {
    let alive = true;
    const load = () =>
      void sendAction({ action: "getPageConfig", pageUrl: location.href })
        .then((c) => {
          const next = c as PageConfig | undefined;
          if (alive && next && typeof next.pageDot === "boolean" && typeof next.sniff === "boolean") setConfig(next);
        })
        .catch(() => {});
    load();
    const onMessage = (msg: unknown) => {
      if ((msg as { action?: string } | null)?.action === "pageConfigChanged") load();
    };
    browser.runtime.onMessage.addListener(onMessage);
    return () => {
      alive = false;
      browser.runtime.onMessage.removeListener(onMessage);
    };
  }, []);

  // 位置记忆
  useEffect(() => {
    browser.storage.local
      .get(DOT_POS_KEY)
      .then((r) => {
        const pos = r[DOT_POS_KEY] as { y?: unknown; side?: unknown } | undefined;
        if (!pos) return;
        if (typeof pos.y === "number" && pos.y > 0) setY(Math.min(pos.y, window.innerHeight - 56));
        if (pos.side === "left" || pos.side === "right") setSide(pos.side);
      })
      .catch(() => {});
  }, []);

  // background 转发的 openPanel
  useEffect(() => {
    const onMessage = (msg: unknown) => {
      if ((msg as { action?: string } | null)?.action === "openPanel") setOpen(true);
    };
    browser.runtime.onMessage.addListener(onMessage);
    return () => browser.runtime.onMessage.removeListener(onMessage);
  }, []);

  // 视口变化（旋转 / 地址栏收放 / 软键盘）：重算面板与圆点位置
  const [, setViewportTick] = useState(0);
  useEffect(() => {
    const bump = () => setViewportTick((n) => n + 1);
    window.addEventListener("resize", bump);
    window.visualViewport?.addEventListener("resize", bump);
    return () => {
      window.removeEventListener("resize", bump);
      window.visualViewport?.removeEventListener("resize", bump);
    };
  }, []);

  // Esc / 点击外部（composedPath 穿透 Shadow DOM）关闭
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    const onClick = (e: MouseEvent) => {
      const path = e.composedPath();
      if (path.includes(panelRef.current as EventTarget) || path.includes(dotRef.current as EventTarget)) return;
      // 预览弹层属于面板内部（fixed 渲染，不在 panelEl 的 DOM 子树外），composedPath 已覆盖
      setOpen(false);
    };
    document.addEventListener("keydown", onKey, true);
    document.addEventListener("click", onClick);
    return () => {
      document.removeEventListener("keydown", onKey, true);
      document.removeEventListener("click", onClick);
    };
  }, [open]);

  const profile = view?.site.profile ?? DEFAULT_SITE_PROFILE;
  // 悬浮球可见性：pageDot 已含 settings.showPageDot 与站点配置，sniff 已含 resourceSniffing 与站点配置
  const visible = !!config && config.pageDot && config.sniff;

  // openPanel（浮钮菜单）不受球可见性约束：只要有视图就允许打开面板
  if (!view) return null;

  const count = badgeCount(view);

  const onPointerDown = (e: PointerEvent) => {
    if (e.button !== 0 || !dotRef.current) return;
    const rect = dotRef.current.getBoundingClientRect();
    drag.current = {
      active: true,
      moved: 0,
      startX: e.clientX,
      startY: e.clientY,
      startLeft: rect.left,
      startTop: rect.top,
      didDrag: false,
    };
    dotRef.current.setPointerCapture(e.pointerId);
  };

  const onPointerMove = (e: PointerEvent) => {
    const d = drag.current;
    if (!d.active) return;
    d.moved++;
    if (d.moved < 3) return;
    if (!d.didDrag) {
      d.didDrag = true;
      setOpen(false); // 拖动是重新定位，面板碍事
    }
    // 允许拖到半隐藏范围
    setDragLeft(Math.max(-18, Math.min(window.innerWidth - 18, d.startLeft + (e.clientX - d.startX))));
    setY(Math.max(20, Math.min(window.innerHeight - 56, d.startTop + (e.clientY - d.startY))));
  };

  const endDrag = () => {
    const d = drag.current;
    if (!d.active) return;
    d.active = false;
    if (!d.didDrag) return;
    const rect = dotRef.current?.getBoundingClientRect();
    const nextSide: Side = (rect?.left ?? 0) + DOT_SIZE / 2 < window.innerWidth / 2 ? "left" : "right";
    const nextY = Math.round(rect?.top ?? y);
    setSide(nextSide);
    setY(nextY);
    setDragLeft(null);
    void browser.storage.local.set({ [DOT_POS_KEY]: { y: nextY, side: nextSide } });
  };

  const hideDot = () => {
    setOpen(false);
    // #862：只关当前站点的悬浮球，不动全局开关；写到命中的配置 key（无命中则新建当前 host）
    const key = view.site.profileKey ?? view.site.host;
    if (!key) return;
    void saveSiteProfile(key, { ...profile, pageDot: false, updatedAt: Date.now() });
  };

  // 面板高度不超过视口（矮视口 / 横屏）；窄屏的横向定位由 panel.css 的媒体查询覆盖
  const panelH = Math.min(PANEL_H, window.innerHeight - 20);
  let panelTop = y - 20;
  if (panelTop + panelH > window.innerHeight - 10) panelTop = window.innerHeight - panelH - 10;
  if (panelTop < 10) panelTop = 10;
  const panelStyle =
    side === "left"
      ? { top: `${panelTop}px`, left: "52px", maxHeight: `${panelH}px` }
      : { top: `${panelTop}px`, right: "52px", maxHeight: `${panelH}px` };
  // 旋转 / 缩放视口后圆点不能落到视口外
  const dotTop = Math.max(0, Math.min(y, window.innerHeight - 56));

  return (
    <>
      {visible ? <button
        ref={dotRef}
        type="button"
        class={`fluxdown-dot ${side}${open ? " active" : ""}${dragLeft !== null ? " dragging" : ""}`}
        style={{ top: `${dotTop}px`, left: dragLeft !== null ? `${dragLeft}px` : undefined }}
        title="FluxDown"
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={endDrag}
        onPointerCancel={endDrag}
        onClick={() => {
          if (!drag.current.didDrag) setOpen(!open);
          drag.current.didDrag = false;
        }}
      >
        <Icon name="logo" size={18} class="dot-icon" />
        <span class={`dot-badge${count > 0 ? " show" : ""}`}>{count > 99 ? "99+" : count}</span>
      </button> : null}

      <div ref={panelRef} class={`fluxdown-panel ${side}${open ? " visible" : ""}`} style={panelStyle} aria-hidden={!open}>
        <div class="panel-header">
          <Icon name="logo" size={16} class="logo" />
          <span class="title">FluxDown</span>
          <span class="resource-count">{count > 0 ? `${count} ${t("panel.resources")}` : ""}</span>
          <IconButton
            icon="trash"
            label={t("panel.clearResourcesTitle")}
            size={14}
            onClick={() => void sendAction<"clearResources">({ action: "clearResources" })}
          />
          <IconButton icon="eyeOff" label={t("panel.hideDot")} size={14} onClick={hideDot} />
          <IconButton icon="close" label={t("panel.collapse")} size={14} onClick={() => setOpen(false)} />
        </div>
        <div class="panel-body">{open ? <ResourceList view={view} density="compact" showSearch={false} /> : null}</div>
      </div>
      <ToastHost />
    </>
  );
}
