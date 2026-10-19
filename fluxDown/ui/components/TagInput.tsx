import type { JSX } from "preact";
import { useState } from "preact/hooks";
import "./components.css";

interface TagInputProps {
  tags: string[];
  onChange: (tags: string[]) => void;
  placeholder?: string;
  /** 归一化输入（如转小写、去点）；返回空串表示丢弃。 */
  normalize?: (raw: string) => string;
  disabled?: boolean;
}

export function TagInput(props: TagInputProps): JSX.Element {
  const [draft, setDraft] = useState("");
  const add = () => {
    const raw = draft.trim();
    if (!raw) return;
    const v = props.normalize ? props.normalize(raw) : raw;
    setDraft("");
    if (!v || props.tags.includes(v)) return;
    props.onChange([...props.tags, v]);
  };
  return (
    <div class="fd-taginput">
      {props.tags.map((tag) => (
        <span class="fd-tag" key={tag}>
          {tag}
          <button
            type="button"
            class="fd-tag-x"
            aria-label="remove"
            disabled={props.disabled}
            onClick={() => props.onChange(props.tags.filter((x) => x !== tag))}
          >
            ×
          </button>
        </span>
      ))}
      <input
        class="fd-taginput-field"
        value={draft}
        placeholder={props.placeholder}
        disabled={props.disabled}
        spellcheck={false}
        onInput={(e) => setDraft((e.currentTarget as HTMLInputElement).value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            add();
          } else if (e.key === "Backspace" && !draft && props.tags.length) {
            props.onChange(props.tags.slice(0, -1));
          }
        }}
        onBlur={add}
      />
    </div>
  );
}
