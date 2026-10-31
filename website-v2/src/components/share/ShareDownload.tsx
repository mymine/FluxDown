/**
 * FluxMesh分享落地页的文件列表与浏览器下载（云端副本就绪时）。
 *
 * 点「下载」→ POST 同源代理 `/api/cloud/share/public/<slug>/download`（见 src/lib/share-proxy.ts）拿签名 CDN 链接，
 * 再用隐藏 <a> 导航触发浏览器下载。清单带 `turnstileSiteKey` 时先渲染 Cloudflare Turnstile 小部件，
 * 拿到 token 才请求，且 token 一次性：每次请求后重置小部件。
 */
import { useRef, useState } from "react";
import { Download, LoaderCircle } from "lucide-react";
import { withBase } from "@/lib/base";
import {
  downloadErrorKey,
  formatBytes,
  isRecord,
  parseGrant,
  type DownloadErrorKey,
  type ShareFile,
} from "@/lib/share-landing";
import TurnstileWidget, { type TurnstileHandle } from "./TurnstileWidget";

export interface ShareDownloadLabels {
  download: string;
  preparing: string;
  started: string;
  verify: string;
  errors: Record<DownloadErrorKey, string>;
}

export default function ShareDownload({
  slug,
  files,
  siteKey,
  lang,
  labels,
}: {
  slug: string;
  files: ShareFile[];
  siteKey?: string;
  lang: "en" | "zh";
  labels: ShareDownloadLabels;
}) {
  const [busy, setBusy] = useState<number | null>(null);
  const [started, setStarted] = useState<number | null>(null);
  const [error, setError] = useState<DownloadErrorKey | null>(null);
  const [token, setToken] = useState<string | null>(null);
  const widget = useRef<TurnstileHandle>(null);

  const download = async (file: ShareFile) => {
    if (siteKey && !token) {
      setError("turnstileRequired");
      return;
    }
    setBusy(file.index);
    setError(null);
    setStarted(null);
    try {
      const response = await fetch(withBase(`/api/cloud/share/public/${slug}/download`), {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(token ? { fileIndex: file.index, turnstileToken: token } : { fileIndex: file.index }),
      });
      let data: unknown = null;
      try {
        data = await response.json();
      } catch {
        data = null;
      }
      if (!response.ok) {
        setError(downloadErrorKey(response.status, isRecord(data) && typeof data.code === "string" ? data.code : undefined));
        return;
      }
      const grant = parseGrant(data);
      if (!grant) {
        setError("generic");
        return;
      }
      const anchor = document.createElement("a");
      anchor.href = grant.url;
      anchor.rel = "noopener noreferrer";
      document.body.appendChild(anchor);
      anchor.click();
      anchor.remove();
      setStarted(file.index);
    } catch {
      setError("generic");
    } finally {
      setBusy(null);
      // Turnstile token 一次性：无论成败都换新。
      widget.current?.reset();
    }
  };

  return (
    <div className="flex flex-col gap-5">
      <ul className="cells border border-line">
        {files.map((file) => (
          <li key={file.index} className="flex flex-wrap items-center justify-between gap-x-6 gap-y-3 px-5 py-4">
            <div className="min-w-0 flex-1">
              <p className="font-medium text-fg [overflow-wrap:anywhere]">{file.name}</p>
              <p className="mono num mt-1 text-sm text-muted">{formatBytes(file.size)}</p>
            </div>
            <div className="flex items-center gap-3">
              {started === file.index && (
                <span className="text-sm text-ok" role="status">
                  {labels.started}
                </span>
              )}
              <button
                type="button"
                className="btn btn-secondary btn-sm"
                disabled={busy !== null}
                onClick={() => void download(file)}
              >
                {busy === file.index ? (
                  <LoaderCircle aria-hidden className="size-3.5 animate-spin" />
                ) : (
                  <Download aria-hidden className="size-3.5" />
                )}
                <span>{busy === file.index ? labels.preparing : labels.download}</span>
              </button>
            </div>
          </li>
        ))}
      </ul>
      {siteKey && (
        <div className="flex flex-col gap-2">
          {!token && <p className="text-sm text-muted">{labels.verify}</p>}
          <TurnstileWidget
            ref={widget}
            siteKey={siteKey}
            lang={lang}
            onToken={(value) => {
              setToken(value);
              if (value) {
                setError((current) => (current === "turnstileRequired" || current === "turnstileFailed" ? null : current));
              }
            }}
            onError={() => setError("turnstileFailed")}
          />
        </div>
      )}
      {error && (
        <p className="text-sm text-danger" role="alert">
          {labels.errors[error]}
        </p>
      )}
    </div>
  );
}
