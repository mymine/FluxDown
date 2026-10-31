import { useState } from "react";
import { LoaderCircle } from "lucide-react";
import type { Lang } from "@/i18n/config";
import { pricingVote } from "@/i18n/messages/pricingVote";
import { BrandIcon } from "@/components/icons/brand";
import { formatDate } from "@/components/pay/money";
import { withBase } from "@/lib/base";

export interface PollComment {
  login: string;
  avatar: string;
  message: string;
  date: string;
}

export interface Viewer {
  login: string;
  avatar: string;
}

type Status = { text: string; ok: boolean } | null;

const MESSAGE_MAX = 500;

interface Props {
  lang: Lang;
  viewer: Viewer | null;
  comments: PollComment[];
  issueUrl: string;
  loginUrl: string;
  onPosted: (comment: PollComment) => void;
}

/** Discussion thread: GitHub-authenticated posting + public comment list. */
export default function Discussion({ lang, viewer, comments, issueUrl, loginUrl, onPosted }: Props) {
  const t = pricingVote[lang];
  const d = t.discuss;
  const [message, setMessage] = useState("");
  const [posting, setPosting] = useState(false);
  const [status, setStatus] = useState<Status>(null);

  async function submit() {
    const trimmed = message.trim();
    if (posting) return;
    if (!trimmed) {
      setStatus({ text: d.required, ok: false });
      return;
    }
    setPosting(true);
    setStatus(null);
    try {
      const res = await fetch(withBase("/api/pricing-vote"), {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ action: "comment", message: trimmed }),
      });
      if (res.status === 401) {
        window.location.href = loginUrl;
        return;
      }
      if (!res.ok) {
        setStatus({ text: res.status === 429 ? t.rateLimited : t.error, ok: false });
        return;
      }
      const result = (await res.json()) as { comment: PollComment };
      onPosted(result.comment);
      setMessage("");
      setStatus({ text: d.success, ok: true });
    } catch {
      setStatus({ text: t.error, ok: false });
    } finally {
      setPosting(false);
    }
  }

  return (
    <div className="grid grid-cols-1 gap-10 lg:grid-cols-[minmax(0,5fr)_minmax(0,7fr)] lg:gap-14">
      <div className="flex flex-col gap-5">
        <span className="eyebrow">{d.eyebrow}</span>
        <h2
          id="discussion-heading"
          className="h2 text-[clamp(1.8rem,3.2vw,2.6rem)]"
          dangerouslySetInnerHTML={{ __html: d.title.replace(/<em>(.*?)<\/em>/g, '<span class="serif">$1</span>') }}
        />
        <p className="lede">{d.lede}</p>
        <a href={issueUrl} target="_blank" rel="noopener noreferrer" className="link inline-flex w-fit items-center gap-2 text-sm">
          <BrandIcon name="github" size={14} />
          {d.viewOnGitHub}
        </a>
      </div>

      <div className="min-w-0">
        {viewer ? (
          <form
            className="fig p-4 sm:p-5"
            onSubmit={(e) => {
              e.preventDefault();
              void submit();
            }}
          >
            <label htmlFor="pricing-comment" className="flex items-center gap-2.5 text-sm">
              <Avatar src={viewer.avatar} size={22} />
              <span className="font-semibold">{viewer.login}</span>
              <span className="sr-only">— {d.label}</span>
            </label>
            <textarea
              id="pricing-comment"
              value={message}
              onChange={(e) => setMessage(e.target.value)}
              maxLength={MESSAGE_MAX}
              rows={4}
              placeholder={d.placeholder}
              className="field mt-3 resize-y"
            />
            <div className="mt-3 flex items-center justify-between gap-3">
              <span className="num font-mono text-xs text-subtle">
                {message.length}/{MESSAGE_MAX}
              </span>
              <button type="submit" disabled={posting} className="btn btn-primary btn-sm disabled:opacity-50">
                {posting && <LoaderCircle size={14} className="animate-spin" aria-hidden />}
                {posting ? d.submitting : d.submit}
              </button>
            </div>
            <p role="status" className={`text-sm ${status ? "mt-3" : ""} ${status?.ok ? "text-ok" : "text-danger"}`}>
              {status?.text}
            </p>
          </form>
        ) : (
          <div className="flex flex-col items-center gap-4 border border-dashed border-line-strong px-6 py-10 text-center">
            <p className="text-sm text-muted">{d.signInPrompt}</p>
            <a href={loginUrl} className="btn btn-secondary">
              <BrandIcon name="github" size={15} />
              {d.signIn}
            </a>
          </div>
        )}

        <p className="eyebrow-plain mb-3 mt-10">{d.count(comments.length)}</p>
        {comments.length === 0 ? (
          <p className="border-y border-line py-10 text-center text-sm text-muted">{d.empty}</p>
        ) : (
          <ol className="divide-y divide-line border-y border-line">
            {comments.map((c, i) => (
              <li key={`${c.date}-${i}`} className="py-4">
                <div className="flex items-center justify-between gap-3">
                  <a
                    href={`https://github.com/${c.login}`}
                    target="_blank"
                    rel="noopener noreferrer"
                    className="flex min-w-0 items-center gap-2 text-sm font-medium transition-colors hover:text-accent-ink"
                  >
                    <Avatar src={c.avatar} size={20} />
                    <span className="truncate">{c.login}</span>
                  </a>
                  <time dateTime={c.date} className="num shrink-0 font-mono text-xs text-subtle">
                    {formatDate(c.date, lang)}
                  </time>
                </div>
                <p className="mt-2 whitespace-pre-wrap break-words text-sm leading-relaxed text-muted">{c.message}</p>
              </li>
            ))}
          </ol>
        )}
      </div>
    </div>
  );
}

export function Avatar({ src, size }: { src: string; size: number }) {
  return src ? (
    <img src={src} alt="" width={size} height={size} loading="lazy" className="shrink-0 rounded-full bg-inset" />
  ) : (
    <BrandIcon name="github" size={size - 2} className="shrink-0" />
  );
}
