import type { ComponentChildren, JSX } from "preact";
import "./components.css";

interface ButtonProps {
  variant?: "primary" | "secondary" | "ghost" | "danger";
  size?: "sm" | "md";
  disabled?: boolean;
  title?: string;
  type?: "button" | "submit";
  class?: string;
  onClick?: (e: MouseEvent) => void;
  children?: ComponentChildren;
}

export function Button(props: ButtonProps): JSX.Element {
  const { variant = "secondary", size = "md" } = props;
  return (
    <button
      type={props.type ?? "button"}
      class={`fd-btn fd-btn-${variant} fd-btn-${size} ${props.class ?? ""}`.trim()}
      disabled={props.disabled}
      title={props.title}
      onClick={props.onClick}
    >
      {props.children}
    </button>
  );
}
