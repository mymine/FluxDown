import type { ComponentChildren, JSX } from "preact";
import "./components.css";

interface BadgeProps {
  tone?: "neutral" | "accent" | "success" | "warning" | "danger";
  title?: string;
  children?: ComponentChildren;
}

export function Badge(props: BadgeProps): JSX.Element {
  return (
    <span class={`fd-badge fd-badge-${props.tone ?? "neutral"}`} title={props.title}>
      {props.children}
    </span>
  );
}
