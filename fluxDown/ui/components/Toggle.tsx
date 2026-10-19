import type { JSX } from "preact";
import "./components.css";

interface ToggleProps {
  checked: boolean;
  onChange: (checked: boolean) => void;
  disabled?: boolean;
  label?: string;
}

export function Toggle(props: ToggleProps): JSX.Element {
  return (
    <label class={`fd-toggle${props.disabled ? " fd-disabled" : ""}`} title={props.label}>
      <input
        type="checkbox"
        role="switch"
        checked={props.checked}
        disabled={props.disabled}
        aria-label={props.label}
        onChange={(e) => props.onChange((e.currentTarget as HTMLInputElement).checked)}
      />
      <span class="fd-toggle-track" />
    </label>
  );
}
