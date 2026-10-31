/** 时间线上的单个版本:版本号 + 渠道徽标 + 日期、分类更新说明、复制按钮、可折叠的下载列表。 */
import { useState } from "react";
import { Check, ChevronDown, Download, FileCode, FileText, Link2 } from "lucide-react";
import type { Lang } from "@/i18n/config";
import { changelog } from "@/i18n/messages/changelog";
import { download } from "@/i18n/messages/download";
import { assetDownloadUrl } from "@/lib/download-source";
import { formatSize, versionAnchor, type ChangelogRelease } from "@/lib/release-format";
import { groupAssets, pickLocaleBody, renderMarkdown, toPlainText } from "./release-body";

interface Props {
  release: ChangelogRelease;
  lang: Lang;
  dateLabel: string;
  relativeLabel: string;
}

function CopyButton({ label, short, text, copiedLabel, icon }: {
  label: string;
  short: string;
  text: () => string;
  copiedLabel: string;
  icon: React.ReactNode;
}) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      type="button"
      title={label}
      aria-label={label}
      className="inline-flex h-7 items-center gap-1.5 rounded-md px-2 font-mono text-[11px] tracking-[0.06em] text-muted uppercase transition-colors hover:bg-inset hover:text-fg"
      onClick={async () => {
        try {
          await navigator.clipboard.writeText(text());
        } catch {
          return;
        }
        setCopied(true);
        window.setTimeout(() => setCopied(false), 2000);
      }}
    >
      {copied ? <Check size={12} strokeWidth={2} className="text-ok" aria-hidden="true" /> : icon}
      <span className={copied ? "text-ok" : undefined}>{copied ? copiedLabel : short}</span>
    </button>
  );
}

export default function ReleaseEntry({ release, lang, dateLabel, relativeLabel }: Props) {
  const t = changelog[lang];
  const anchor = versionAnchor(release.tag);
  const body = pickLocaleBody(release.body, lang);
  const groups = groupAssets(release.assets, t);
  const groupName = (g: string) => (g === "extension" ? t.asset.extension : g === "other" ? t.asset.other : g);

  return (
    <article id={anchor} className="cl-entry" aria-labelledby={`${anchor}-title`} data-entry={anchor}>
      <span className="cl-node" aria-hidden="true" data-rc={release.prerelease || undefined} />
      <header className="flex flex-wrap items-center gap-x-3 gap-y-2">
        <h2 id={`${anchor}-title`} className="font-mono text-[22px] font-medium tracking-[-0.02em]">
          <a href={`#${anchor}`} className="group inline-flex items-center gap-2" title={t.permalink(release.tag)}>
            {release.tag}
            <Link2
              size={14}
              strokeWidth={2}
              aria-hidden="true"
              className="text-subtle opacity-0 transition-opacity group-hover:opacity-100 group-focus-visible:opacity-100"
            />
          </a>
        </h2>
        <span className={`chip ${release.prerelease ? "cl-chip-rc" : "cl-chip-stable"}`}>
          {release.prerelease ? t.rc : t.stable}
        </span>
        <span className="flex items-baseline gap-2 text-[13px] text-muted">
          <time dateTime={release.published_at} className="num">
            {dateLabel}
          </time>
          <span className="text-subtle">· {relativeLabel}</span>
        </span>
        <span className="ml-auto flex items-center gap-0.5">
          <CopyButton
            label={t.copyMd}
            short={t.md}
            copiedLabel={t.copied}
            text={() => body}
            icon={<FileCode size={12} strokeWidth={1.75} aria-hidden="true" />}
          />
          <CopyButton
            label={t.copyText}
            short={t.text}
            copiedLabel={t.copied}
            text={() => toPlainText(release, lang, t, dateLabel)}
            icon={<FileText size={12} strokeWidth={1.75} aria-hidden="true" />}
          />
        </span>
      </header>

      <div className="prose cl-body mt-5" dangerouslySetInnerHTML={{ __html: renderMarkdown(body) }} />

      {release.assets.length > 0 && (
        <details className="cl-assets mt-6">
          <summary className="flex cursor-pointer list-none items-center gap-2 px-4 py-3 text-[13px] text-muted transition-colors hover:text-fg">
            <Download size={14} strokeWidth={1.75} aria-hidden="true" />
            <span>{t.downloads(release.assets.length)}</span>
            <ChevronDown size={14} strokeWidth={1.75} className="cl-caret ml-auto" aria-hidden="true" />
          </summary>
          <div className="flex flex-col gap-5 border-t border-line px-4 pt-4 pb-5">
            {groups.map(({ group, items }) => (
              <div key={group}>
                <p className="eyebrow-plain mb-2">{groupName(group)}</p>
                <ul className="grid grid-cols-1 gap-x-6 sm:grid-cols-2">
                  {items.map(({ asset, sub }) => (
                    <li key={asset.name}>
                      <a
                        href={assetDownloadUrl(asset, "auto")}
                        target="_blank"
                        rel="noopener nofollow"
                        title={asset.name}
                        className="flex items-center gap-3 border-b border-line py-2 text-[13.5px] transition-colors hover:text-accent-ink"
                      >
                        <span className="min-w-0 flex-1 truncate">{sub}</span>
                        <span className="num font-mono text-[12px] text-subtle">{formatSize(asset.size)}</span>
                        <Download size={13} strokeWidth={1.75} className="text-subtle" aria-hidden="true" />
                      </a>
                      <a
                        href={assetDownloadUrl(asset, "github")}
                        target="_blank"
                        rel="noopener nofollow"
                        title={`${download[lang].source.backup}: ${asset.name}`}
                        className="link text-[12px]"
                      >
                        {download[lang].source.backup}
                      </a>
                    </li>
                  ))}
                </ul>
              </div>
            ))}
            <p className="text-[12px] leading-relaxed text-subtle">{t.assetsNote}</p>
          </div>
        </details>
      )}
    </article>
  );
}
