/**
 * FluxMesh分享落地页的取件码面板：6 位数字分格展示 + 一键复制。
 *
 * 剪贴板不可用（非安全上下文 / 被拒）时选中数字，提示手动复制。
 */
import { useEffect, useRef, useState } from "react";
import { Check, Copy } from "lucide-react";

export interface PickupCodeLabels {
  label: string;
  hint: string;
  copy: string;
  copied: string;
  copyAria: string;
  copyFailed: string;
}

export default function PickupCode({ code, labels }: { code: string; labels: PickupCodeLabels }) {
  const [state, setState] = useState<"idle" | "copied" | "failed">("idle");
  const digitsRef = useRef<HTMLSpanElement>(null);
  const timer = useRef<number | undefined>(undefined);

  useEffect(() => () => window.clearTimeout(timer.current), []);

  const copy = async () => {
    window.clearTimeout(timer.current);
    try {
      await navigator.clipboard.writeText(code);
    } catch {
      if (digitsRef.current) window.getSelection()?.selectAllChildren(digitsRef.current);
      setState("failed");
      return;
    }
    setState("copied");
    timer.current = window.setTimeout(() => setState("idle"), 1800);
  };

  return (
    <div className="fig">
      <div className="fig-bar">
        <span>{labels.label}</span>
      </div>
      <div className="flex flex-col gap-5 p-5 sm:p-6">
        <span className="sr-only">{`${labels.label} ${code.split("").join(" ")}`}</span>
        <span ref={digitsRef} aria-hidden className="flex select-all items-center justify-center gap-1.5 sm:gap-2">
          {code.split("").map((digit, index) => (
            <span
              key={index}
              className={`mono num grid h-14 w-10 place-items-center rounded-[8px] bg-sunken text-[1.65rem] font-medium text-fg shadow-[0_0_0_1px_var(--line-strong)_inset] sm:h-16 sm:w-12 sm:text-[2rem]${index === 3 ? " ml-2 sm:ml-3" : ""}`}
            >
              {digit}
            </span>
          ))}
        </span>
        <button
          type="button"
          className="btn btn-secondary w-full justify-center"
          aria-label={labels.copyAria}
          onClick={() => void copy()}
        >
          {state === "copied" ? (
            <Check aria-hidden className="size-4 text-ok" />
          ) : (
            <Copy aria-hidden className="size-4" />
          )}
          <span aria-live="polite">{state === "copied" ? labels.copied : labels.copy}</span>
        </button>
        <p
          className={`text-sm leading-relaxed ${state === "failed" ? "text-danger" : "text-muted"}`}
          role={state === "failed" ? "alert" : undefined}
        >
          {state === "failed" ? labels.copyFailed : labels.hint}
        </p>
      </div>
    </div>
  );
}
