import type { ComponentChildren, JSX } from "preact";
import { Icon, type UiIconName } from "./Icon";
import "./components.css";

interface EmptyStateProps {
  icon?: UiIconName;
  title: string;
  hint?: string;
  children?: ComponentChildren;
}

export function EmptyState(props: EmptyStateProps): JSX.Element {
  return (
    <div class="fd-empty">
      {props.icon ? <Icon name={props.icon} size={28} /> : null}
      <div class="fd-empty-title">{props.title}</div>
      {props.hint ? <div class="fd-empty-hint">{props.hint}</div> : null}
      {props.children}
    </div>
  );
}
