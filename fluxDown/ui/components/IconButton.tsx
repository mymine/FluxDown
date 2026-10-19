import type { JSX } from "preact";
import { Icon, type UiIconName } from "./Icon";
import "./components.css";

interface IconButtonProps {
  icon: UiIconName;
  /** 同时作为 title 与 aria-label。 */
  label: string;
  size?: number;
  active?: boolean;
  disabled?: boolean;
  class?: string;
  onClick?: (e: MouseEvent) => void;
}

export function IconButton(props: IconButtonProps): JSX.Element {
  return (
    <button
      type="button"
      class={`fd-icon-btn${props.active ? " fd-active" : ""} ${props.class ?? ""}`.trim()}
      title={props.label}
      aria-label={props.label}
      disabled={props.disabled}
      onClick={props.onClick}
    >
      <Icon name={props.icon} size={props.size ?? 15} />
    </button>
  );
}
