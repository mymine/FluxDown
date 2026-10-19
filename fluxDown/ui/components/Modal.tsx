import type { ComponentChildren, JSX } from "preact";
import { useEffect, useId, useRef } from "preact/hooks";
import { useT } from "@/ui/i18n";
import { IconButton } from "./IconButton";
import "./components.css";

interface ModalProps {
  title: string;
  onClose: () => void;
  children?: ComponentChildren;
  footer?: ComponentChildren;
  width?: number;
}

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]):not([type="hidden"]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

/** 已打开的对话框栈：Escape 与焦点陷阱只作用于最上层，叠加确认框时不会连环关闭。 */
const openModals: symbol[] = [];

export function Modal(props: ModalProps): JSX.Element {
  const t = useT();
  const titleId = useId();
  const dialog = useRef<HTMLDivElement>(null);
  const onClose = useRef(props.onClose);
  onClose.current = props.onClose;

  useEffect(() => {
    const self = Symbol("modal");
    openModals.push(self);
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const root = dialog.current;
    // 初始焦点：正文里的第一个控件，没有则落在对话框本身。
    const body = root?.querySelector<HTMLElement>(".fd-modal-body");
    (body?.querySelector<HTMLElement>(FOCUSABLE) ?? root)?.focus();

    const onKey = (e: KeyboardEvent) => {
      if (openModals[openModals.length - 1] !== self) return;
      if (e.key === "Escape") {
        onClose.current();
        return;
      }
      if (e.key !== "Tab" || !root) return;
      const items = Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE));
      if (items.length === 0) {
        e.preventDefault();
        return;
      }
      const first = items[0];
      const last = items[items.length - 1];
      const active = document.activeElement;
      if (e.shiftKey && (active === first || active === root)) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && active === last) {
        e.preventDefault();
        first.focus();
      } else if (!root.contains(active)) {
        e.preventDefault();
        first.focus();
      }
    };
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("keydown", onKey);
      const at = openModals.indexOf(self);
      if (at !== -1) openModals.splice(at, 1);
      // 关闭后焦点还给打开它的控件（它仍在文档中时）。
      if (opener?.isConnected) opener.focus();
    };
  }, []);

  return (
    <div
      class="fd-modal-overlay"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) props.onClose();
      }}
    >
      <div
        ref={dialog}
        class="fd-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={-1}
        style={props.width ? { width: `${props.width}px` } : undefined}
      >
        <div class="fd-modal-head">
          <span class="fd-modal-title" id={titleId}>
            {props.title}
          </span>
          <IconButton icon="close" label={t("ui.close")} onClick={props.onClose} />
        </div>
        <div class="fd-modal-body">{props.children}</div>
        {props.footer ? <div class="fd-modal-foot">{props.footer}</div> : null}
      </div>
    </div>
  );
}
