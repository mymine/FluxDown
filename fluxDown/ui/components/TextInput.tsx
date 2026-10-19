import type { JSX } from "preact";
import "./components.css";

interface TextInputProps {
  value: string;
  onInput?: (value: string) => void;
  /** 失焦或回车时提交（适合保存目录等「编辑完再保存」的字段）。 */
  onCommit?: (value: string) => void;
  placeholder?: string;
  type?: "text" | "password" | "number" | "url" | "search";
  disabled?: boolean;
  class?: string;
  label?: string;
  min?: number;
  max?: number;
  spellcheck?: boolean;
}

export function TextInput(props: TextInputProps): JSX.Element {
  return (
    <input
      class={`fd-input ${props.class ?? ""}`.trim()}
      type={(props.type ?? "text") as "text"}
      value={props.value}
      placeholder={props.placeholder}
      disabled={props.disabled}
      aria-label={props.label}
      min={props.min}
      max={props.max}
      spellcheck={props.spellcheck ?? false}
      autocomplete="off"
      onInput={(e) => props.onInput?.((e.currentTarget as HTMLInputElement).value)}
      onChange={(e) => props.onCommit?.((e.currentTarget as HTMLInputElement).value)}
      onKeyDown={(e) => {
        if (e.key === "Enter" && props.onCommit) props.onCommit((e.currentTarget as HTMLInputElement).value);
      }}
    />
  );
}
