import type { ComponentChildren, JSX } from "preact";
import "./components.css";

export interface TabItem<K extends string> {
  key: K;
  label: ComponentChildren;
  /** 右上角计数（0 / undefined 不显示）。 */
  count?: number;
}

interface TabsProps<K extends string> {
  items: TabItem<K>[];
  active: K;
  onChange: (key: K) => void;
  class?: string;
}

export function Tabs<K extends string>(props: TabsProps<K>): JSX.Element {
  return (
    <div class={`fd-tabs ${props.class ?? ""}`.trim()} role="tablist">
      {props.items.map((it) => (
        <button
          key={it.key}
          type="button"
          role="tab"
          aria-selected={it.key === props.active}
          class={`fd-tab${it.key === props.active ? " fd-active" : ""}`}
          onClick={() => props.onChange(it.key)}
        >
          {it.label}
          {it.count ? <span class="fd-tab-count">{it.count}</span> : null}
        </button>
      ))}
    </div>
  );
}
