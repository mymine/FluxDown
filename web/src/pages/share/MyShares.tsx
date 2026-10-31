// 「我的分享」：文件（副行 文件数 · 大小 · 到期）、领取次数、状态；行内只留 复制链接 / 二维码 / 停止或删除，
// 云端暂存、申诉、重新开始分享收进「更多操作」菜单。支持多选批量开始 / 停止 / 删除（逐项调用单项 RPC，汇总失败）。桌面为表格，窄屏为卡片。

import {
  Copy,
  Gavel,
  MoreHorizontal,
  Play,
  QrCode,
  Share2,
  Square,
  Trash2,
} from "lucide-react";
import { useState } from "react";
import { useT } from "../../i18n";
import type { TFunction } from "../../i18n";
import { copyText } from "../../lib/copy";
import { rpc } from "../../lib/rpc";
import type { ShareDto } from "../../lib/rpc";
import {
  ActionMenu,
  Badge,
  Button,
  Checkbox,
  EmptyState,
  Tooltip,
  confirmDialog,
  toast,
  useIsMobile,
} from "../../ui";
import type { MenuEntry } from "../../ui";
import { accountErrorText } from "../settings/sections/account/errorText";
import { useNowSeconds } from "../rss/useNow";
import {
  SHARE_STATUS_KEYS,
  appealState,
  shareActionApplies,
  canStartShare,
  canStopShare,
  isSourceMissing,
  planShareBatch,
  pruneSelection,
  selectAllState,
  shareDisplayStatus,
  shareMetaLine,
  shareTitleText,
  sortShares,
} from "./model";
import type { ShareDisplayStatus } from "./model";
import { CloudStateBadge } from "./CloudStorage";
import { useCloudMenuEntries } from "./cloudActions";
import { AppealDialog } from "./AppealDialog";
import { useShares } from "./state";
import { openShareDetail } from "./store";

type ShareOp = "start" | "stop" | "delete";

const SHARE_OPS: Readonly<Record<ShareOp, (id: string) => Promise<unknown>>> = {
  start: (id) => rpc.agent.share.resume({ id }),
  stop: (id) => rpc.agent.share.stop({ id }),
  delete: (id) => rpc.agent.share.delete({ id }),
};

/** 逐项（串行）执行单项 RPC；有失败时 toast 提示：单项直接给原因，多项汇总失败数与首个原因。 */
async function runShareOp(
  t: TFunction,
  op: ShareOp,
  shares: readonly ShareDto[],
): Promise<void> {
  let failed = 0;
  let firstError: unknown;
  for (const share of shares) {
    try {
      await SHARE_OPS[op](share.id);
    } catch (error) {
      if (failed === 0) firstError = error;
      failed += 1;
    }
  }
  if (failed === 0) return;
  const reason = accountErrorText(t, firstError);
  toast.error(
    shares.length === 1
      ? reason
      : t("shareBatchPartialFailed", { failed, reason }),
  );
}

/** 删除前确认（单项与批量共用）。 */
function confirmDelete(t: TFunction, count: number): Promise<boolean> {
  return confirmDialog({
    title: t("shareDeleteConfirmTitle"),
    description: t("shareDeleteConfirmBody", { n: count }),
    okLabel: t("shareActionDelete"),
    intent: "destructive",
  });
}

export function MyShares() {
  const t = useT();
  const mobile = useIsMobile();
  const shares = useShares();
  const nowMs = useNowSeconds() * 1000;
  const ordered = sortShares(shares, nowMs);
  const [selectedIds, setSelectedIds] = useState<ReadonlySet<string>>(
    new Set(),
  );
  const [batchBusy, setBatchBusy] = useState(false);

  if (ordered.length === 0) {
    return <EmptyState icon={Share2} title={t("shareMineEmpty")} />;
  }

  // 列表经 shareChanged 刷新后，已不存在的分享不再计入选中。
  const alive = pruneSelection(selectedIds, ordered);
  const selected = ordered.filter((share) => alive.has(share.id));
  const anySelected = selected.length > 0;
  const allState = selectAllState(selected.length, ordered.length);

  const toggle = (id: string) => {
    const next = new Set(alive);
    if (!next.delete(id)) next.add(id);
    setSelectedIds(next);
  };
  const toggleAll = () =>
    setSelectedIds(
      allState === true ? new Set() : new Set(ordered.map((share) => share.id)),
    );

  const runBatch = async (op: ShareOp, targets: readonly ShareDto[]) => {
    if (targets.length === 0) return;
    if (op === "delete" && !(await confirmDelete(t, targets.length))) return;
    setBatchBusy(true);
    try {
      await runShareOp(t, op, targets);
    } finally {
      setBatchBusy(false);
    }
    if (op === "delete") setSelectedIds(new Set());
  };

  const bar = anySelected ? (
    <BatchBar
      count={selected.length}
      selected={selected}
      busy={batchBusy}
      selectAll={mobile ? { state: allState, onToggle: toggleAll } : undefined}
      onRun={(op, targets) => void runBatch(op, targets)}
    />
  ) : null;

  return mobile ? (
    <div className="flex flex-col gap-2">
      {bar}
      <ul className="flex flex-col gap-2">
        {ordered.map((share) => (
          <ShareCard
            key={share.id}
            share={share}
            nowMs={nowMs}
            selected={alive.has(share.id)}
            onToggle={() => toggle(share.id)}
          />
        ))}
      </ul>
    </div>
  ) : (
    <div className="flex min-w-0 flex-col gap-2">
      {bar}
      <div className="min-w-0 overflow-x-auto rounded-md border border-hairline">
        <table className="w-full min-w-[640px] text-left text-sm">
          <thead className="bg-chrome text-xs text-text-tertiary">
            <tr className="group/head">
              <th className="w-9 px-2 py-2 font-normal">
                <div
                  className={
                    anySelected
                      ? "flex"
                      : "flex opacity-0 focus-within:opacity-100 group-hover/head:opacity-100"
                  }
                >
                  <Checkbox
                    checked={allState}
                    onCheckedChange={toggleAll}
                    aria-label={t("shareSelectAll")}
                  />
                </div>
              </th>
              <th className="px-3 py-2 font-normal">{t("shareColFile")}</th>
              <th className="px-3 py-2 font-normal">{t("shareColClaims")}</th>
              <th className="px-3 py-2 font-normal">{t("shareColStatus")}</th>
              <th className="px-3 py-2" />
            </tr>
          </thead>
          <tbody>
            {ordered.map((share) => (
              <ShareTableRow
                key={share.id}
                share={share}
                nowMs={nowMs}
                selected={alive.has(share.id)}
                anySelected={anySelected}
                onToggle={() => toggle(share.id)}
              />
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}

/** 批量操作条：「已选 n 项｜开始分享｜停止分享｜删除」；开始 / 停止只对适用的项执行。 */
function BatchBar({
  count,
  selected,
  busy,
  selectAll,
  onRun,
}: {
  count: number;
  selected: readonly ShareDto[];
  busy: boolean;
  selectAll?: { state: boolean | "indeterminate"; onToggle: () => void };
  onRun: (op: ShareOp, targets: readonly ShareDto[]) => void;
}) {
  const t = useT();
  const plan = planShareBatch(selected);
  return (
    <div className="flex min-w-0 flex-wrap items-center gap-2 rounded-md border border-hairline bg-chrome px-3 py-1.5 text-sm">
      {selectAll ? (
        <Checkbox
          checked={selectAll.state}
          onCheckedChange={selectAll.onToggle}
          aria-label={t("shareSelectAll")}
        />
      ) : null}
      <span className="tabular mr-auto text-muted-foreground">
        {t("shareSelectedCount", { n: count })}
      </span>
      <Button
        variant="outline"
        icon={Play}
        disabled={busy || plan.start.length === 0}
        onClick={() => onRun("start", plan.start)}
      >
        {t("shareBatchStart")}
      </Button>
      <Button
        variant="outline"
        icon={Square}
        disabled={busy || plan.stop.length === 0}
        onClick={() => onRun("stop", plan.stop)}
      >
        {t("shareBatchStop")}
      </Button>
      <Button
        variant="outline"
        icon={Trash2}
        className="text-destructive"
        disabled={busy}
        onClick={() => onRun("delete", plan.remove)}
      >
        {t("shareBatchDelete")}
      </Button>
    </div>
  );
}

function claimsText(t: TFunction, share: ShareDto): string {
  return share.maxClaims > 0
    ? t("shareClaimedOf", { count: share.claimCount, max: share.maxClaims })
    : t("shareClaimedCount", { count: share.claimCount });
}

/** 状态徽标：本机源文件缺失 / 被修改（且无就绪云端副本）时追加「文件缺失」。 */
function StatusBadges({
  share,
  status,
}: {
  share: ShareDto;
  status: ShareDisplayStatus;
}) {
  const t = useT();
  return (
    <>
      {status === "takedown" ? (
        <Tooltip
          content={t("shareTakedownReason", {
            reason: share.takedownReason ?? "",
          })}
        >
          <Badge tone="destructive">{t(SHARE_STATUS_KEYS[status])}</Badge>
        </Tooltip>
      ) : (
        <Badge tone={status === "active" ? "success" : "neutral"}>
          {t(SHARE_STATUS_KEYS[status])}
        </Badge>
      )}
      {isSourceMissing(share) ? (
        <Badge tone="destructive">{t("shareStatusSourceMissing")}</Badge>
      ) : null}
      <CloudStateBadge share={share} />
    </>
  );
}

/** 行内：复制 / 二维码 + 停止（有效）或 删除（已停止 / 已过期）；云端暂存 / 申诉 / 重新开始进「更多操作」。 */
function ShareActions({
  share,
  status,
  nowMs,
}: {
  share: ShareDto;
  status: ShareDisplayStatus;
  nowMs: number;
}) {
  const t = useT();
  const [pending, setPending] = useState<ShareOp | null>(null);
  const [appealing, setAppealing] = useState(false);
  const cloud = useCloudMenuEntries(share, nowMs);
  const appeal = appealState(share);
  const live = status === "active";
  const ended = !live && !canStopShare(share);

  const run = async (op: ShareOp) => {
    if (op === "stop") {
      const ok = await confirmDialog({
        title: t("shareStop"),
        description: t("shareStopConfirm"),
        okLabel: t("shareStop"),
        intent: "destructive",
      });
      if (!ok) return;
    } else if (op === "delete" && !(await confirmDelete(t, 1))) {
      return;
    }
    setPending(op);
    try {
      await runShareOp(t, op, [share]);
    } finally {
      setPending(null);
    }
  };

  const startable = canStartShare(share);
  const more: MenuEntry[] = [...cloud.entries];
  if (ended && shareActionApplies("start", share)) {
    more.push({
      type: "item",
      key: "start",
      label: t("shareActionStart"),
      icon: Play,
      disabled: !startable || pending !== null,
      ...(startable ? {} : { description: t("shareStartDisabledMissing") }),
      onSelect: () => void run("start"),
    });
  }
  if (appeal === "canAppeal") {
    more.push({
      type: "item",
      key: "appeal",
      label: t("shareAppeal"),
      icon: Gavel,
      onSelect: () => setAppealing(true),
    });
  } else if (appeal === "pending" || appeal === "rejected") {
    more.push({
      type: "item",
      key: "appeal-status",
      label: t(
        appeal === "pending" ? "shareAppealPending" : "shareAppealRejected",
      ),
      icon: Gavel,
      disabled: true,
      onSelect: () => undefined,
    });
  }

  return (
    <div className="flex shrink-0 items-center justify-end gap-0.5">
      <Tooltip content={t("shareCopyLink")}>
        <Button
          variant="ghost"
          iconOnly
          aria-label={t("shareCopyLink")}
          disabled={!live}
          onClick={() => {
            copyText(share.url);
            toast.success(t("shareLinkCopied"));
          }}
        >
          <Copy className="size-4" />
        </Button>
      </Tooltip>
      <Tooltip content={t("shareShowQr")}>
        <Button
          variant="ghost"
          iconOnly
          aria-label={t("shareShowQr")}
          onClick={() => openShareDetail(share.id)}
        >
          <QrCode className="size-4" />
        </Button>
      </Tooltip>
      {live ? (
        <Tooltip content={t("shareStop")}>
          <Button
            variant="ghost"
            iconOnly
            aria-label={t("shareStop")}
            className="text-destructive"
            loading={pending === "stop"}
            onClick={() => void run("stop")}
          >
            <Square className="size-4" />
          </Button>
        </Tooltip>
      ) : null}
      {ended ? (
        <Tooltip content={t("shareActionDelete")}>
          <Button
            variant="ghost"
            iconOnly
            aria-label={t("shareActionDelete")}
            className="text-destructive"
            disabled={pending !== null}
            loading={pending === "delete"}
            onClick={() => void run("delete")}
          >
            <Trash2 className="size-4" />
          </Button>
        </Tooltip>
      ) : null}
      {more.length > 0 ? (
        <ActionMenu
          title={t("moreActions")}
          entries={more}
          trigger={
            <Button
              variant="ghost"
              iconOnly
              aria-label={t("moreActions")}
              title={t("moreActions")}
              loading={cloud.busy}
            >
              <MoreHorizontal className="size-4" />
            </Button>
          }
        />
      ) : null}
      {appealing ? (
        <AppealDialog shareId={share.id} onClose={() => setAppealing(false)} />
      ) : null}
    </div>
  );
}

function ShareTableRow({
  share,
  nowMs,
  selected,
  anySelected,
  onToggle,
}: {
  share: ShareDto;
  nowMs: number;
  selected: boolean;
  anySelected: boolean;
  onToggle: () => void;
}) {
  const t = useT();
  const status = shareDisplayStatus(share, nowMs);
  return (
    <tr className="group/row border-t border-hairline align-middle">
      <td className="w-9 px-2 py-2">
        <div
          className={
            anySelected
              ? "flex"
              : "flex opacity-0 focus-within:opacity-100 group-hover/row:opacity-100"
          }
        >
          <Checkbox
            checked={selected}
            onCheckedChange={onToggle}
            aria-label={shareTitleText(t, share)}
          />
        </div>
      </td>
      <td className="max-w-[360px] px-3 py-2">
        <div className="truncate text-foreground" title={share.title}>
          {shareTitleText(t, share)}
        </div>
        <div className="tabular truncate text-xs text-text-tertiary">
          {shareMetaLine(t, share)}
        </div>
      </td>
      <td className="tabular px-3 py-2 text-xs text-muted-foreground">
        {claimsText(t, share)}
      </td>
      <td className="px-3 py-2">
        <div className="flex flex-wrap items-center gap-1">
          <StatusBadges share={share} status={status} />
        </div>
      </td>
      <td className="px-2 py-1">
        <ShareActions share={share} status={status} nowMs={nowMs} />
      </td>
    </tr>
  );
}

function ShareCard({
  share,
  nowMs,
  selected,
  onToggle,
}: {
  share: ShareDto;
  nowMs: number;
  selected: boolean;
  onToggle: () => void;
}) {
  const t = useT();
  const status = shareDisplayStatus(share, nowMs);
  return (
    <li className="flex min-w-0 flex-col gap-1.5 rounded-md border border-hairline bg-surface p-3">
      <div className="flex min-w-0 items-start gap-2">
        <div className="flex h-control items-center coarse:min-h-touch">
          <Checkbox
            checked={selected}
            onCheckedChange={onToggle}
            aria-label={shareTitleText(t, share)}
          />
        </div>
        <div className="min-w-0 flex-1">
          <div
            className="break-words text-sm text-foreground"
            title={share.title}
          >
            {shareTitleText(t, share)}
          </div>
          <div className="tabular text-xs text-text-tertiary">
            {shareMetaLine(t, share)}
          </div>
        </div>
        <div className="flex shrink-0 flex-col items-end gap-1">
          <StatusBadges share={share} status={status} />
        </div>
      </div>
      <div className="tabular text-xs text-muted-foreground">
        {claimsText(t, share)}
      </div>
      <ShareActions share={share} status={status} nowMs={nowMs} />
    </li>
  );
}
