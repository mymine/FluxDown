/**
 * 反馈提交表单 → POST /api/feedback。
 * 功能建议只需标题;Bug / 其他 需要描述与应用版本(与服务端校验一致)。
 * 运行环境(系统 / 浏览器 / 语言)挂载后自动检测并明示,随反馈上报。
 */
import { useEffect, useState, type ReactNode } from "react";
import { Bug, Lightbulb, MessageCircle, MonitorSmartphone, Send, type LucideIcon } from "lucide-react";
import type { Lang } from "@/i18n/config";
import { feedback } from "@/i18n/messages/feedback";
import { cn } from "@/lib/utils";
import { Notice, Spinner } from "./shared";
import { withBase } from "@/lib/base";

type FeedbackType = "feature" | "bug" | "other";

const TYPES: { type: FeedbackType; icon: LucideIcon }[] = [
  { type: "feature", icon: Lightbulb },
  { type: "bug", icon: Bug },
  { type: "other", icon: MessageCircle },
];

const EMPTY = { type: "feature" as FeedbackType, title: "", description: "", appVersion: "", contact: "" };

/** 仅含通用设备信息(系统 + 浏览器 + 页面语言),不含个人身份数据。 */
function detectEnvironment(): string {
  const ua = navigator.userAgent;
  let m: RegExpMatchArray | null;
  let os = "Unknown OS";
  if ((m = ua.match(/Windows NT ([\d.]+)/))) os = `Windows NT ${m[1]}`;
  else if (/iPhone|iPad/.test(ua)) os = "iOS";
  else if ((m = ua.match(/Mac OS X ([\d_.]+)/))) os = `macOS ${m[1].replace(/_/g, ".")}`;
  else if ((m = ua.match(/Android ([\d.]+)/))) os = `Android ${m[1]}`;
  else if (/Linux/.test(ua)) os = "Linux";

  let browser = "Unknown browser";
  if ((m = ua.match(/Edg\/([\d.]+)/))) browser = `Edge ${m[1]}`;
  else if ((m = ua.match(/OPR\/([\d.]+)/))) browser = `Opera ${m[1]}`;
  else if ((m = ua.match(/Firefox\/([\d.]+)/))) browser = `Firefox ${m[1]}`;
  else if ((m = ua.match(/Chrome\/([\d.]+)/))) browser = `Chrome ${m[1]}`;
  else if ((m = ua.match(/Version\/([\d.]+).*Safari/))) browser = `Safari ${m[1]}`;

  return `${os}, ${browser}, locale ${navigator.language}`;
}

function Field({
  id,
  label,
  tag,
  hint,
  counter,
  children,
}: {
  id?: string;
  label: string;
  tag: string;
  hint?: string;
  counter?: string;
  children: ReactNode;
}) {
  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-baseline gap-2">
        <label htmlFor={id} className="text-sm font-medium text-fg">
          {label}
        </label>
        <span className="eyebrow-plain text-[10px]">{tag}</span>
        {counter && <span className="mono num ml-auto text-[11px] text-subtle">{counter}</span>}
      </div>
      {children}
      {hint && (
        <p id={id ? `${id}-hint` : undefined} className="text-xs leading-relaxed text-subtle">
          {hint}
        </p>
      )}
    </div>
  );
}

export default function FeedbackForm({ lang, onSuccess }: { lang: Lang; onSuccess: () => void }) {
  const t = feedback[lang].form;
  const [form, setForm] = useState(EMPTY);
  const [environment, setEnvironment] = useState("");
  const [status, setStatus] = useState<"idle" | "submitting" | "error">("idle");
  const [errorMsg, setErrorMsg] = useState("");

  useEffect(() => setEnvironment(detectEnvironment()), []);

  const requiresDetail = form.type !== "feature";
  const detailTag = requiresDetail ? t.required : t.optional;
  const canSubmit =
    form.title.trim().length > 0 &&
    (!requiresDetail || (form.description.trim().length > 0 && form.appVersion.trim().length > 0)) &&
    status !== "submitting";

  const set = (key: keyof typeof EMPTY) => (e: { target: { value: string } }) =>
    setForm((f) => ({ ...f, [key]: e.target.value }));

  const submit = async () => {
    if (!canSubmit) return;
    setStatus("submitting");
    setErrorMsg("");
    try {
      const res = await fetch(withBase("/api/feedback"), {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          type: form.type,
          title: form.title.trim(),
          description: form.description.trim() || undefined,
          appVersion: form.appVersion.trim() || undefined,
          contact: form.contact.trim() || undefined,
          environment: environment || undefined,
        }),
      });
      if (res.status === 429) {
        setStatus("error");
        setErrorMsg(t.rateLimited);
        return;
      }
      if (!res.ok) {
        const data = (await res.json().catch(() => ({}))) as { error?: string };
        setStatus("error");
        setErrorMsg(data.error || t.submitError);
        return;
      }
      setForm(EMPTY);
      setStatus("idle");
      onSuccess();
    } catch {
      setStatus("error");
      setErrorMsg(t.submitError);
    }
  };

  return (
    <form
      className="fig mx-auto w-full max-w-2xl"
      noValidate
      onSubmit={(e) => {
        e.preventDefault();
        submit();
      }}
    >
      <div className="fig-bar">
        <span>fig. 01</span>
        <span>{t.heading}</span>
      </div>

      <div className="flex flex-col gap-6 p-[clamp(20px,3vw,32px)]">
        <fieldset className="flex flex-col gap-2.5">
          <legend className="mb-2.5 text-sm font-medium text-fg">{t.typeLabel}</legend>
          <div className="cells grid-cols-3 border border-line">
            {TYPES.map(({ type, icon: Icon }) => (
              <label key={type} className={cn("cm-choice", form.type === type && "is-checked")}>
                <input
                  type="radio"
                  name="fb-type"
                  value={type}
                  checked={form.type === type}
                  onChange={() => setForm((f) => ({ ...f, type }))}
                  className="sr-only"
                />
                <Icon aria-hidden className="size-4" />
                <span className="text-xs font-medium sm:text-[13px]">{t.type[type]}</span>
              </label>
            ))}
          </div>
          <p className="text-xs text-subtle">{t.typeHint[form.type]}</p>
        </fieldset>

        <Field id="fb-title" label={t.titleLabel} tag={t.required} counter={t.count(form.title.length, 200)}>
          <input
            id="fb-title"
            type="text"
            required
            maxLength={200}
            value={form.title}
            onChange={set("title")}
            placeholder={t.titlePlaceholder}
            className="field"
          />
        </Field>

        <Field id="fb-desc" label={t.descLabel} tag={detailTag} counter={t.count(form.description.length, 5000)}>
          <textarea
            id="fb-desc"
            rows={6}
            required={requiresDetail}
            maxLength={5000}
            value={form.description}
            onChange={set("description")}
            placeholder={t.descPlaceholder}
            className="field resize-y"
          />
        </Field>

        <div className="grid grid-cols-1 gap-6 sm:grid-cols-2">
          <Field id="fb-version" label={t.versionLabel} tag={detailTag} hint={t.versionHint}>
            <input
              id="fb-version"
              type="text"
              required={requiresDetail}
              maxLength={50}
              value={form.appVersion}
              onChange={set("appVersion")}
              placeholder={t.versionPlaceholder}
              aria-describedby="fb-version-hint"
              className="field mono"
            />
          </Field>
          <Field id="fb-contact" label={t.contactLabel} tag={t.optional} hint={t.contactHint}>
            <input
              id="fb-contact"
              type="text"
              autoComplete="email"
              value={form.contact}
              onChange={set("contact")}
              placeholder={t.contactPlaceholder}
              aria-describedby="fb-contact-hint"
              className="field"
            />
          </Field>
        </div>

        {environment && (
          <Field label={t.envLabel} tag={t.envAuto} hint={t.envHint}>
            <div className="mono flex items-center gap-2 rounded-lg bg-sunken px-3 py-2.5 text-xs text-muted shadow-[inset_0_0_0_1px_var(--line)]">
              <MonitorSmartphone aria-hidden className="size-4 shrink-0 text-subtle" />
              <span className="truncate">{environment}</span>
            </div>
          </Field>
        )}
      </div>

      <div className="flex flex-wrap items-center gap-4 border-t border-line bg-sunken px-[clamp(20px,3vw,32px)] py-4">
        <button type="submit" className="btn btn-primary" disabled={!canSubmit}>
          {status === "submitting" ? <Spinner /> : <Send aria-hidden className="size-4" />}
          {status === "submitting" ? t.submitting : t.submit}
        </button>
        {status === "error" && <Notice tone="error">{errorMsg}</Notice>}
      </div>
    </form>
  );
}
