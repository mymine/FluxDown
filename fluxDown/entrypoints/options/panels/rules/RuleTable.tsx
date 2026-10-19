import type { JSX } from "preact";
import { useMemo, useState } from "preact/hooks";
import type { RulesListEntry } from "@/background/protocol";
import { type ListedRuleStatus, matchesQuery } from "@/background/rule-view";
import type { RuleStatusReason } from "@/rules/engine";
import type { RuleOverrides, RuleTier, SniffRuleV1 } from "@/rules/schema";
import { Badge } from "@/ui/components/Badge";
import { Button } from "@/ui/components/Button";
import { EmptyState } from "@/ui/components/EmptyState";
import { IconButton } from "@/ui/components/IconButton";
import { Modal } from "@/ui/components/Modal";
import { Select } from "@/ui/components/Select";
import { TextInput } from "@/ui/components/TextInput";
import { Toggle } from "@/ui/components/Toggle";
import { useT } from "@/ui/i18n";
import { sendAction } from "@/utils/messaging";
import { BUILTIN_RULE_PREVIEW } from "@/utils/rule-preview";
import { applyOverride } from "../../lib/local-rules";
import { DEFAULT_PAGE_SIZE, type PageSize, paginate } from "../../lib/pagination";
import { issueText } from "../../lib/rule-issue";
import { ConfirmModal, Pagination } from "../../parts";
import type { LocalRulesApi } from "./useLocalRules";
import { stageLabels } from "./stage-labels";

interface RuleTableProps {
  entries: RulesListEntry[];
  overrides: RuleOverrides;
  onOverridesChange: (next: RuleOverrides) => void;
  local: LocalRulesApi;
}

type TierFilter = RuleTier | "all";
type SortMode = "default" | "eval" | "hits";

const TIER_TONES: Record<RuleTier, "neutral" | "accent" | "warning" | "success"> = {
  builtin: "neutral",
  official: "accent",
  plugin: "accent",
  community: "warning",
  local: "success",
};

const STATUS_TONES: Record<ListedRuleStatus, "neutral" | "success" | "warning" | "danger"> = {
  active: "success",
  disabled: "neutral",
  paused: "neutral",
  expired: "warning",
  "engine-too-new": "warning",
  rejected: "danger",
};

/** 这些状态下开关不可操作（由包本身或层级开关决定）。 */
const LOCKED_STATUSES: Record<ListedRuleStatus, boolean> = {
  active: false,
  disabled: false,
  paused: true,
  expired: true,
  "engine-too-new": true,
  rejected: true,
};

/** 排序：default = 列表原序；eval = 求值顺序（未参与求值的排最后）；hits = 命中次数降序。 */
function sortEntries(list: RulesListEntry[], mode: SortMode): RulesListEntry[] {
  if (mode === "default") return list;
  const keyed = list.map((entry, index) => ({ entry, index }));
  keyed.sort((a, b) => {
    if (mode === "eval") {
      const x = a.entry.evalOrder ?? Number.MAX_SAFE_INTEGER;
      const y = b.entry.evalOrder ?? Number.MAX_SAFE_INTEGER;
      return x - y || a.index - b.index;
    }
    return (b.entry.hits?.count ?? 0) - (a.entry.hits?.count ?? 0) || a.index - b.index;
  });
  return keyed.map((k) => k.entry);
}

interface PreviewState {
  id: string;
  title: string;
  tier: RuleTier;
  /** undefined = 加载中；null = 不可读取。 */
  rule: SniffRuleV1 | null | undefined;
}

export function RuleTable(props: RuleTableProps): JSX.Element {
  const t = useT();
  const { local } = props;
  const [tier, setTier] = useState<TierFilter>("all");
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<SortMode>("default");
  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState<PageSize>(DEFAULT_PAGE_SIZE);
  const [preview, setPreview] = useState<PreviewState | null>(null);
  const [disabling, setDisabling] = useState<RulesListEntry | null>(null);
  const [resettingAll, setResettingAll] = useState(false);

  const tierLabels: Record<RuleTier, string> = {
    builtin: t("rules.tier.builtin"),
    official: t("rules.tier.official"),
    plugin: t("rules.tier.plugin"),
    community: t("rules.tier.community"),
    local: t("rules.tier.local"),
  };
  const statusLabels: Record<ListedRuleStatus, string> = {
    active: t("rules.status.active"),
    disabled: t("rules.status.disabled"),
    paused: t("rules.status.paused"),
    expired: t("rules.status.expired"),
    "engine-too-new": t("rules.status.engineTooNew"),
    rejected: t("rules.status.rejected"),
  };
  const statusHints: Record<ListedRuleStatus, string | undefined> = {
    active: undefined,
    disabled: undefined,
    paused: t("rules.statusHint.paused"),
    expired: undefined,
    "engine-too-new": t("rules.statusHint.engineTooNew"),
    rejected: undefined,
  };
  // 吊销列表缺失 / 过期同样表现为社区包过期，不向用户暴露吊销机制。
  const reasonHints: Record<RuleStatusReason, string> = {
    "pack-expired": t("rules.statusHint.packExpired"),
    "revocations-stale": t("rules.statusHint.packExpired"),
  };
  const stageNames: Record<string, string> = stageLabels();

  const needle = query.trim().toLowerCase();
  const filtered = useMemo(
    () =>
      sortEntries(
        props.entries.filter(({ rule, searchText }) => {
          if (tier !== "all" && rule.tier !== tier) return false;
          return !needle || matchesQuery(searchText, needle);
        }),
        sort,
      ),
    [props.entries, tier, needle, sort],
  );
  // 规则列表重取 / 筛选变化后页码可能越界，渲染时钳到最后一页。
  const slice = paginate(filtered.length, page, pageSize);
  const visible = filtered.slice(slice.start, slice.end);

  const overrideCount = Object.keys(props.overrides).length;

  const writeEnabled = (entry: RulesListEntry, enabled: boolean) => {
    props.onOverridesChange(applyOverride(props.overrides, entry.rule.id, enabled, entry.rule.enabled !== false));
  };

  const setEnabled = (entry: RulesListEntry, enabled: boolean) => {
    // 关闭内置规则会让整类资源不再被识别（或放进大量噪声请求），先确认。
    if (!enabled && entry.rule.tier === "builtin") {
      setDisabling(entry);
      return;
    }
    writeEnabled(entry, enabled);
  };

  const resetOverride = (id: string) => {
    const { [id]: _removed, ...rest } = props.overrides;
    props.onOverridesChange(rest);
  };

  const openPreview = (entry: RulesListEntry) => {
    const { id, title, tier: ruleTier } = entry.rule;
    setPreview({ id, title: title || id, tier: ruleTier, rule: undefined });
    sendAction<"ruleGet">({ action: "ruleGet", id })
      .then((rule) => setPreview((current) => (current?.id === id ? { ...current, rule } : current)))
      .catch(() => setPreview((current) => (current?.id === id ? { ...current, rule: null } : current)));
  };

  return (
    <div class="opt-card">
      <div class="opt-card-head">
        <h2 class="opt-card-title">{t("rules.table.title")}</h2>
        <div class="opt-filters">
          <Select<TierFilter>
            value={tier}
            onChange={(v) => {
              setTier(v);
              setPage(1);
            }}
            label={t("rules.table.filterTier")}
            options={[
              { value: "all", label: t("rules.table.allTiers") },
              { value: "builtin", label: tierLabels.builtin },
              { value: "official", label: tierLabels.official },
              { value: "plugin", label: tierLabels.plugin },
              { value: "community", label: tierLabels.community },
              { value: "local", label: tierLabels.local },
            ]}
          />
          <Select<SortMode>
            value={sort}
            onChange={(v) => {
              setSort(v);
              setPage(1);
            }}
            label={t("rules.table.sort")}
            options={[
              { value: "default", label: t("rules.table.sortDefault") },
              { value: "eval", label: t("rules.table.sortEval") },
              { value: "hits", label: t("rules.table.sortHits") },
            ]}
          />
          <TextInput
            type="search"
            value={query}
            label={t("rules.table.searchLabel")}
            placeholder={t("rules.table.search")}
            onInput={(v) => {
              setQuery(v);
              setPage(1);
            }}
          />
          {overrideCount > 0 ? (
            <Button size="sm" onClick={() => setResettingAll(true)}>
              {t("rules.table.resetAll")}
            </Button>
          ) : null}
          {local.toolbar}
        </div>
      </div>
      <p class="opt-row-desc">{t("rules.local.desc")}</p>
      <p class="opt-row-desc">{t("rules.local.submitHint")}</p>
      {local.report}

      {filtered.length === 0 ? (
        <EmptyState icon="search" title={t("rules.table.empty")} />
      ) : (
        <div class="opt-table-wrap">
          <table class="opt-table">
            <thead>
              <tr>
                <th class="opt-col-toggle">{t("rules.table.colEnabled")}</th>
                <th>{t("rules.table.colRule")}</th>
                <th>{t("rules.table.colTier")}</th>
                <th>{t("rules.table.colStage")}</th>
                <th title={t("rules.table.colPriorityHint")}>{t("rules.table.colPriority")}</th>
                <th>{t("rules.table.colHits")}</th>
                <th>{t("rules.table.colStatus")}</th>
                <th class="opt-col-actions">{t("options.sites.colActions")}</th>
              </tr>
            </thead>
            <tbody>
              {visible.map((entry, i) => {
                const { rule, status, reason, detail, inert } = entry;
                const hint =
                  status === "rejected"
                    ? issueText(t, { code: entry.code, param: entry.param, detail: detail ?? "" })
                    : (detail ?? (reason ? reasonHints[reason] : statusHints[status]));
                const inertHint =
                  inert === "requires-setting"
                    ? t("rules.inert.requiresSettingHint")
                    : inert === "classify-only"
                      ? t("rules.inert.classifyOnlyHint")
                      : undefined;
                const localRule = rule.tier === "local" ? local.rules.find((r) => r.id === rule.id) : undefined;
                const canView = rule.tier !== "builtin" || BUILTIN_RULE_PREVIEW;
                return (
                  <tr key={`${slice.start + i}:${rule.id}`}>
                    <td class="opt-col-toggle">
                      <Toggle
                        checked={status === "active"}
                        disabled={LOCKED_STATUSES[status]}
                        label={rule.title || rule.id}
                        onChange={(v) => setEnabled(entry, v)}
                      />
                    </td>
                    <td>
                      <div class="opt-rule-title">{rule.title || rule.id}</div>
                      {rule.title ? <div class="opt-cell-mono opt-rule-id">{rule.id}</div> : null}
                    </td>
                    <td>
                      <Badge tone={TIER_TONES[rule.tier]}>{tierLabels[rule.tier]}</Badge>
                    </td>
                    <td class="opt-col-nowrap">{stageNames[rule.stage] ?? rule.stage}</td>
                    <td class="opt-cell-mono">
                      <div>{rule.priority ?? 0}</div>
                      {entry.evalOrder !== undefined ? (
                        <div class="opt-rule-id" title={t("rules.table.evalOrder", { n: String(entry.evalOrder + 1) })}>
                          #{entry.evalOrder + 1}
                        </div>
                      ) : null}
                    </td>
                    <td class="opt-cell-mono">
                      {entry.hits ? (
                        <span title={t("rules.table.hitsTitle", { time: new Date(entry.hits.lastAt).toLocaleString() })}>
                          {entry.hits.count}
                        </span>
                      ) : (
                        "—"
                      )}
                    </td>
                    <td>
                      <div class="opt-status-cell">
                        {/* 依赖设置未开启：只显示「未生效」，不与「生效」并列互相矛盾。 */}
                        {status === "active" && inert === "requires-setting" ? null : (
                          <Badge tone={STATUS_TONES[status]} title={hint}>
                            {statusLabels[status]}
                          </Badge>
                        )}
                        {status === "active" && inert ? (
                          <Badge tone="warning" title={inertHint}>
                            {inert === "requires-setting" ? t("rules.inert.requiresSetting") : t("rules.inert.classifyOnly")}
                          </Badge>
                        ) : null}
                      </div>
                      {hint || (status === "active" && inertHint) ? (
                        <div class="opt-reason-line">{hint || inertHint}</div>
                      ) : null}
                    </td>
                    <td class="opt-col-actions">
                      {rule.id in props.overrides ? (
                        <IconButton icon="refresh" label={t("rules.table.reset")} onClick={() => resetOverride(rule.id)} />
                      ) : null}
                      {canView ? <IconButton icon="eye" label={t("rules.table.viewRule")} onClick={() => openPreview(entry)} /> : null}
                      {localRule ? (
                        <>
                          <IconButton icon="upload" label={t("rules.local.submit")} onClick={() => local.submit(localRule)} />
                          <IconButton icon="edit" label={t("rules.local.edit")} onClick={() => local.edit(localRule)} />
                          <IconButton icon="trash" label={t("rules.local.remove")} onClick={() => local.remove(localRule)} />
                        </>
                      ) : null}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}

      <Pagination
        total={filtered.length}
        slice={slice}
        pageSize={pageSize}
        onPageChange={setPage}
        onPageSizeChange={(size) => {
          setPageSize(size);
          setPage(1);
        }}
      />

      {preview ? (
        <Modal
          title={preview.title}
          width={640}
          onClose={() => setPreview(null)}
          footer={
            <>
              {preview.rule && preview.tier !== "local" ? (
                <Button
                  onClick={() => {
                    const rule = preview.rule;
                    setPreview(null);
                    if (rule) local.copyToLocal(rule);
                  }}
                >
                  {t("rules.preview.copyToLocal")}
                </Button>
              ) : null}
              <Button onClick={() => setPreview(null)}>{t("rules.local.dismiss")}</Button>
            </>
          }
        >
          {preview.rule === undefined ? (
            <div role="status">{t("rules.preview.loading")}</div>
          ) : preview.rule === null ? (
            <div class="opt-warn" role="alert">
              {t("rules.preview.unavailable")}
            </div>
          ) : (
            <>
              <p class="opt-row-desc">{t("rules.preview.version", { version: String(preview.rule.version) })}</p>
              <pre class="opt-json opt-json-view">{JSON.stringify(preview.rule, null, 2)}</pre>
            </>
          )}
        </Modal>
      ) : null}

      {disabling ? (
        <ConfirmModal
          title={t("rules.table.disableBuiltinTitle")}
          message={t("rules.table.disableBuiltinConfirm", { name: disabling.rule.title || disabling.rule.id })}
          confirmLabel={t("rules.table.disable")}
          danger
          onCancel={() => setDisabling(null)}
          onConfirm={() => {
            writeEnabled(disabling, false);
            setDisabling(null);
          }}
        />
      ) : null}

      {resettingAll ? (
        <ConfirmModal
          title={t("rules.table.resetAll")}
          message={t("rules.table.resetAllConfirm", { count: String(overrideCount) })}
          confirmLabel={t("rules.table.resetAll")}
          danger
          onCancel={() => setResettingAll(false)}
          onConfirm={() => {
            props.onOverridesChange({});
            setResettingAll(false);
          }}
        />
      ) : null}

      {local.dialogs}
    </div>
  );
}
