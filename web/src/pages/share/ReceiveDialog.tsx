// 接收确认框：取件码 / 分享链接 resolve 后显示分享方设备名、文件清单、大小与当前可用来源，
// 确认后 `agent.share.receive` 建出普通下载任务。两个入口：
//   - 本页发起（取件码框 / 新建下载框粘贴）：先 resolve，取消不需要通知 agent；
//   - agent 已解析的 `pendingReceive`（系统打开 / 剪贴板 / NMH）：确认即 receive{resolveId}，取消调 dismissPending。
// agent 永不自动建任务。

import { useEffect, useMemo, useState } from "react";
import { useT } from "../../i18n";
import type { TFunction } from "../../i18n";
import { RpcError, rpc, useConfigValue } from "../../lib/rpc";
import type { ShareResolveDto } from "../../lib/rpc";
import { toastRpcError } from "../../lib/rpcToast";
import {
  Button,
  CheckRow,
  ConfirmFooter,
  FieldError,
  FieldHint,
  FieldLabel,
  Input,
  Spinner,
  toast,
} from "../../ui";
import { accountErrorKey } from "../settings/sections/account/errorText";
import { formatBytes } from "../downloads/dialogs/utils";
import { dateTimeLabel } from "./model";
import { ReportDialog } from "./ReportDialog";
import { ShareSurface } from "./ShareSurface";

type Phase =
  | { kind: "resolving" }
  | { kind: "ready"; preview: ShareResolveDto }
  | { kind: "failed"; message: string };

/** 失败文案：需要人机验证时说明应用内无法完成（Web 同样无法渲染 Turnstile）。 */
function failureText(error: unknown, t: (key: string) => string): string {
  if (error instanceof RpcError && error.reason === "shareTurnstileRequired")
    return t("shareTurnstileHint");
  return t(accountErrorKey(error));
}

/** 本页发起的接收：先 resolve 再确认。 */
export function ReceiveByInput({
  codeOrLink,
  onClose,
}: {
  codeOrLink: string;
  onClose: () => void;
}) {
  const t = useT();
  const [phase, setPhase] = useState<Phase>({ kind: "resolving" });
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let cancelled = false;
    setPhase({ kind: "resolving" });
    rpc.agent.share
      .resolve({ codeOrLink })
      .then((preview) => {
        if (!cancelled) setPhase({ kind: "ready", preview });
      })
      .catch((error: unknown) => {
        if (!cancelled)
          setPhase({ kind: "failed", message: failureText(error, t) });
      });
    return () => {
      cancelled = true;
    };
  }, [codeOrLink, attempt, t]);

  if (phase.kind === "ready") {
    return (
      <ReceiveConfirm
        preview={phase.preview}
        onDone={onClose}
        onCancel={onClose}
      />
    );
  }
  return (
    <ShareSurface
      open
      onOpenChange={(open) => !open && onClose()}
      title={t("shareReceiveTitle")}
      footer={
        <div className="flex justify-end gap-2 narrow:[&>*]:flex-1">
          <Button onClick={onClose}>{t("close")}</Button>
          {phase.kind === "failed" ? (
            <Button
              variant="primary"
              onClick={() => setAttempt((value) => value + 1)}
            >
              {t("shareRetry")}
            </Button>
          ) : null}
        </div>
      }
    >
      {phase.kind === "resolving" ? (
        <div className="flex items-center justify-center gap-2 py-8 text-sm text-muted-foreground">
          <Spinner size="md" />
          {t("shareResolving")}
        </div>
      ) : (
        <div role="alert" className="break-words py-4 text-sm text-destructive">
          {phase.message}
        </div>
      )}
    </ShareSurface>
  );
}

/** agent 推来的待确认接收：确认完成后 agent 清空 `pendingReceive`；取消调 `dismissPending`。 */
export function ReceivePending({ preview }: { preview: ShareResolveDto }) {
  const dismiss = () => {
    rpc.agent.share.dismissPending(preview.resolveId).catch(toastRpcError);
  };
  return (
    <ReceiveConfirm
      key={preview.resolveId}
      preview={preview}
      onDone={() => undefined}
      onCancel={dismiss}
    />
  );
}

function sourceChips(
  t: TFunction,
  sources: ShareResolveDto["sources"],
): { key: string; ok: boolean; label: string }[] {
  const sender =
    sources.senderOnline === null
      ? { key: "sender", ok: false, label: t("shareSourceSenderUnknown") }
      : {
          key: "sender",
          ok: sources.senderOnline,
          label: t(
            sources.senderOnline
              ? "shareSourceSenderOnline"
              : "shareSourceSenderOffline",
          ),
        };
  const chips = [
    sender,
    { key: "cloud", ok: sources.cloudCopy, label: t("shareSourceCloud") },
    { key: "bridge", ok: sources.bridge, label: t("shareSourceBridge") },
  ];
  const swarmPeers = sources.swarmPeers ?? 0;
  if (swarmPeers > 0)
    chips.push({
      key: "swarm",
      ok: true,
      label: t("shareSourceSwarm", { count: swarmPeers }),
    });
  return chips;
}

function ReceiveConfirm({
  preview,
  onDone,
  onCancel,
}: {
  preview: ShareResolveDto;
  onDone: () => void;
  onCancel: () => void;
}) {
  const t = useT();
  const defaultSaveDir = useConfigValue("default_save_dir") ?? "";
  const [selected, setSelected] = useState<ReadonlySet<number>>(
    () => new Set(preview.files.map((_, index) => index)),
  );
  const [saveDir, setSaveDir] = useState(defaultSaveDir);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [reporting, setReporting] = useState(false);

  const allSelected = selected.size === preview.files.length;
  const selectedBytes = useMemo(
    () =>
      preview.files.reduce(
        (sum, file, index) => (selected.has(index) ? sum + file.size : sum),
        0,
      ),
    [preview.files, selected],
  );
  const sender = preview.senderDeviceName.trim();
  const expires = dateTimeLabel(preview.expiresAt);

  const toggle = (index: number) => {
    setSelected((current) => {
      const next = new Set(current);
      if (!next.delete(index)) next.add(index);
      return next;
    });
  };

  const accept = async () => {
    setBusy(true);
    setError(null);
    try {
      const dir = saveDir.trim();
      const result = await rpc.agent.share.receive({
        resolveId: preview.resolveId,
        ...(dir === "" ? {} : { saveDir: dir }),
        ...(allSelected
          ? {}
          : { fileIndexes: [...selected].sort((a, b) => a - b) }),
      });
      toast.success(t("shareReceiveStarted", { count: result.taskIds.length }));
      onDone();
    } catch (failure) {
      setError(failureText(failure, t));
      setBusy(false);
    }
  };

  return (
    <ShareSurface
      open
      onOpenChange={(open) => !open && !busy && onCancel()}
      title={t("shareReceiveTitle")}
      description={t("shareFromDevice", {
        device: sender === "" ? t("shareSenderUnknown") : sender,
      })}
      persistent
      footer={
        <ConfirmFooter
          cancelLabel={t("cancel")}
          okLabel={t("shareStartReceive")}
          onCancel={onCancel}
          onOk={() => void accept()}
          okDisabled={selected.size === 0}
          loading={busy}
          extra={
            <button
              type="button"
              className="text-xs text-text-tertiary underline-offset-2 hover:text-foreground hover:underline"
              onClick={() => setReporting(true)}
            >
              {t("shareReport")}
            </button>
          }
        />
      }
    >
      {reporting ? (
        <ReportDialog
          shareId={preview.shareId}
          onClose={() => setReporting(false)}
        />
      ) : null}
      <div
        className="min-w-0 break-words text-sm font-medium text-foreground"
        title={preview.title}
      >
        {preview.title}
      </div>

      <div className="flex flex-col gap-1.5">
        <div className="flex items-center justify-between gap-2">
          <FieldLabel>
            {t("shareFilesCount", { count: preview.files.length })}
          </FieldLabel>
          <FieldHint className="tabular">
            {t("shareSelectedSummary", {
              count: selected.size,
              size: formatBytes(selectedBytes),
            })}
          </FieldHint>
        </div>
        <ul className="flex max-h-56 min-w-0 flex-col overflow-y-auto rounded-md border border-hairline">
          {preview.files.length > 1 ? (
            <li className="flex min-w-0 items-center border-b border-hairline px-2 py-0.5">
              <CheckRow
                checked={allSelected}
                onCheckedChange={(next) =>
                  setSelected(
                    next
                      ? new Set(preview.files.map((_, index) => index))
                      : new Set(),
                  )
                }
                className="min-w-0 flex-1"
              >
                <span className="block truncate text-sm">
                  {t("shareSelectAll")}
                </span>
              </CheckRow>
            </li>
          ) : null}
          {preview.files.map((file, index) => (
            <li
              key={`${index}:${file.name}`}
              className="flex min-w-0 items-center gap-2 border-b border-hairline px-2 py-0.5 last:border-b-0"
            >
              <CheckRow
                checked={selected.has(index)}
                onCheckedChange={() => toggle(index)}
                className="min-w-0 flex-1"
              >
                <span className="block truncate text-sm" title={file.name}>
                  {file.name}
                </span>
              </CheckRow>
              <span className="tabular shrink-0 text-xs text-text-tertiary">
                {formatBytes(file.size)}
              </span>
            </li>
          ))}
        </ul>
      </div>

      <div className="flex flex-col gap-1.5">
        <FieldLabel>{t("shareSourcesTitle")}</FieldLabel>
        <div className="flex flex-wrap gap-1.5">
          {sourceChips(t, preview.sources).map((source) => (
            <span
              key={source.key}
              className={`inline-flex items-center rounded-full px-2 py-0.5 text-caption ${source.ok ? "bg-success/15 text-success" : "bg-muted text-muted-foreground"}`}
            >
              {source.label}
            </span>
          ))}
        </div>
        {expires === "" ? null : (
          <FieldHint>{t("shareExpiresAt", { time: expires })}</FieldHint>
        )}
      </div>

      <div className="flex flex-col gap-1.5">
        <FieldLabel htmlFor="share-receive-dir">{t("shareSaveTo")}</FieldLabel>
        <Input
          id="share-receive-dir"
          value={saveDir}
          placeholder={t("shareSaveDefault")}
          spellCheck={false}
          onChange={(event) => setSaveDir(event.target.value)}
        />
      </div>

      {error ? <FieldError className="break-words">{error}</FieldError> : null}
    </ShareSurface>
  );
}
