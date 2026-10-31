/**
 * FluxMesh分享落地页的举报入口：一个紧凑的「举报」按钮，展开后是小表单。
 *
 * 提交到同源代理 `POST /api/cloud/share/public/<slug>/report`（见 src/lib/share-proxy.ts，FluxCloud 每 IP 5 次 / 小时）。
 * 发给上游的 reason 是固定中文文本（`REPORT_REASON_VALUE`，后台审核员阅读），界面文案按语言显示。
 * 清单带 `turnstileSiteKey` 时表单内渲染 Turnstile，token 一次性。
 */
import { useRef, useState } from "react";
import { Flag, LoaderCircle } from "lucide-react";
import { withBase } from "@/lib/base";
import {
  downloadErrorKey,
  isRecord,
  MAX_REPORT_CONTACT,
  MAX_REPORT_DETAIL,
  REPORT_REASON_KEYS,
  REPORT_REASON_VALUE,
  type DownloadErrorKey,
  type ReportReasonKey,
} from "@/lib/share-landing";
import TurnstileWidget, { type TurnstileHandle } from "./TurnstileWidget";

export interface ShareReportLabels {
  open: string;
  title: string;
  intro: string;
  reasonLabel: string;
  reasonPlaceholder: string;
  reasons: Record<ReportReasonKey, string>;
  detailLabel: string;
  detailPlaceholder: string;
  contactLabel: string;
  contactPlaceholder: string;
  submit: string;
  sending: string;
  cancel: string;
  sent: string;
  reasonRequired: string;
  verify: string;
  errors: Record<DownloadErrorKey, string>;
}

export default function ShareReport({
  slug,
  siteKey,
  lang,
  labels,
}: {
  slug: string;
  siteKey?: string;
  lang: "en" | "zh";
  labels: ShareReportLabels;
}) {
  const [open, setOpen] = useState(false);
  const [reason, setReason] = useState<ReportReasonKey | "">("");
  const [detail, setDetail] = useState("");
  const [contact, setContact] = useState("");
  const [token, setToken] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [sent, setSent] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const widget = useRef<TurnstileHandle>(null);

  const submit = async (event: { preventDefault(): void }) => {
    event.preventDefault();
    if (reason === "") {
      setError(labels.reasonRequired);
      return;
    }
    if (siteKey && !token) {
      setError(labels.errors.turnstileRequired);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const response = await fetch(withBase(`/api/cloud/share/public/${slug}/report`), {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          reason: REPORT_REASON_VALUE[reason],
          ...(detail.trim() && { detail }),
          ...(contact.trim() && { contact }),
          ...(token && { turnstileToken: token }),
        }),
      });
      if (response.ok) {
        setSent(true);
        return;
      }
      let code: string | undefined;
      try {
        const data: unknown = await response.json();
        if (isRecord(data) && typeof data.code === "string") code = data.code;
      } catch {
        code = undefined;
      }
      setError(labels.errors[downloadErrorKey(response.status, code)]);
    } catch {
      setError(labels.errors.generic);
    } finally {
      setBusy(false);
      widget.current?.reset();
    }
  };

  if (sent) {
    return (
      <p className="text-sm text-ok" role="status">
        {labels.sent}
      </p>
    );
  }

  if (!open) {
    return (
      <button type="button" className="btn btn-ghost btn-sm" onClick={() => setOpen(true)}>
        <Flag aria-hidden className="size-3.5" />
        <span>{labels.open}</span>
      </button>
    );
  }

  return (
    <form className="flex max-w-[560px] flex-col gap-4" onSubmit={(event) => void submit(event)}>
      <div className="flex flex-col gap-1">
        <h2 className="font-medium text-fg">{labels.title}</h2>
        <p className="text-sm text-muted">{labels.intro}</p>
      </div>
      <label className="flex flex-col gap-1.5">
        <span className="eyebrow-plain">{labels.reasonLabel}</span>
        <select
          className="field"
          value={reason}
          onChange={(event) => setReason(event.target.value as ReportReasonKey | "")}
          required
        >
          <option value="" disabled>
            {labels.reasonPlaceholder}
          </option>
          {REPORT_REASON_KEYS.map((key) => (
            <option key={key} value={key}>
              {labels.reasons[key]}
            </option>
          ))}
        </select>
      </label>
      <label className="flex flex-col gap-1.5">
        <span className="eyebrow-plain">{labels.detailLabel}</span>
        <textarea
          className="field min-h-24"
          maxLength={MAX_REPORT_DETAIL}
          value={detail}
          placeholder={labels.detailPlaceholder}
          onChange={(event) => setDetail(event.target.value)}
        />
      </label>
      <label className="flex flex-col gap-1.5">
        <span className="eyebrow-plain">{labels.contactLabel}</span>
        <input
          className="field"
          type="text"
          maxLength={MAX_REPORT_CONTACT}
          autoComplete="off"
          value={contact}
          placeholder={labels.contactPlaceholder}
          onChange={(event) => setContact(event.target.value)}
        />
      </label>
      {siteKey && (
        <div className="flex flex-col gap-2">
          {!token && <p className="text-sm text-muted">{labels.verify}</p>}
          <TurnstileWidget
            ref={widget}
            siteKey={siteKey}
            lang={lang}
            onToken={setToken}
            onError={() => setError(labels.errors.turnstileFailed)}
          />
        </div>
      )}
      {error && (
        <p className="text-sm text-danger" role="alert">
          {error}
        </p>
      )}
      <div className="flex gap-3">
        <button type="submit" className="btn btn-secondary btn-sm" disabled={busy}>
          {busy && <LoaderCircle aria-hidden className="size-3.5 animate-spin" />}
          <span>{busy ? labels.sending : labels.submit}</span>
        </button>
        <button type="button" className="btn btn-ghost btn-sm" disabled={busy} onClick={() => setOpen(false)}>
          {labels.cancel}
        </button>
      </div>
    </form>
  );
}
