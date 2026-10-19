import type { JSX } from "preact";
import "./components.css";

export interface SelectOption<V extends string> {
  value: V;
  label: string;
}

interface SelectProps<V extends string> {
  value: V;
  options: SelectOption<V>[];
  onChange: (value: V) => void;
  disabled?: boolean;
  class?: string;
  label?: string;
}

export function Select<V extends string>(props: SelectProps<V>): JSX.Element {
  return (
    <select
      class={`fd-select ${props.class ?? ""}`.trim()}
      value={props.value}
      disabled={props.disabled}
      aria-label={props.label}
      onChange={(e) => props.onChange((e.currentTarget as HTMLSelectElement).value as V)}
    >
      {props.options.map((o) => (
        <option key={o.value} value={o.value} selected={o.value === props.value}>
          {o.label}
        </option>
      ))}
    </select>
  );
}
