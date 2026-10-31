/**
 * 本地规则的增删改、导入导出与「提交到社区」。表格行动作并入主规则表（RuleTable），
 * 这里只产出动作函数与需要渲染的工具栏 / 导入报告 / 对话框。
 */
import type { JSX } from "preact";
import { useRef, useState } from "preact/hooks";
import { browser } from "wxt/browser";
import type { RuleOverrides, SniffRuleV1 } from "@/rules/schema";
import { MAX_RULES_PER_PACK } from "@/rules/validate";
import { Button } from "@/ui/components/Button";
import { showToast } from "@/ui/components/Toast";
import { useT } from "@/ui/i18n";
import { downloadJson } from "../../lib/download";
import {
  type RuleIssueRef,
  type RuleTestCase,
  buildLocalPack,
  communitySubmission,
  mergeImported,
  parseLocalPack,
  planImport,
} from "../../lib/local-rules";
import { generateRuleId } from "../../lib/rule-form";
import { issueText } from "../../lib/rule-issue";
import { ConfirmModal } from "../../parts";
import { RuleEditorModal } from "./RuleEditorModal";
import type { TesterBinding } from "./RuleTester";
import { logError } from "@/utils/error-log";

interface UseLocalRulesOptions {
  rules: SniffRuleV1[];
  overrides: RuleOverrides;
  onRulesChange: (next: SniffRuleV1[]) => void;
  onOverridesChange: (next: RuleOverrides) => void;
  binding: TesterBinding;
  /** 测试台里附加的测试用例，随提交一起预填。 */
  testCases: RuleTestCase[];
  /** 最近一次在测试台测试的页面地址，作为提交的示例页面。 */
  testPage: string | undefined;
}

interface ImportReport {
  added: number;
  replaced: number;
  rejected: RuleIssueRef[];
}

interface PendingImport {
  accepted: SniffRuleV1[];
  rejected: RuleIssueRef[];
  replacing: string[];
}

type EditorTarget = { rule: SniffRuleV1 | null; seed?: SniffRuleV1 } | null;

export interface LocalRulesApi {
  rules: SniffRuleV1[];
  edit: (rule: SniffRuleV1) => void;
  remove: (rule: SniffRuleV1) => void;
  submit: (rule: SniffRuleV1) => void;
  /** 以任意规则为起点新建本地规则（官方 / 社区 / 内置 → 本地副本）。 */
  copyToLocal: (rule: SniffRuleV1) => void;
  toolbar: JSX.Element;
  report: JSX.Element | null;
  dialogs: JSX.Element | null;
}

export function useLocalRules(opts: UseLocalRulesOptions): LocalRulesApi {
  const t = useT();
  const fileInput = useRef<HTMLInputElement>(null);
  const [editor, setEditor] = useState<EditorTarget>(null);
  const [removing, setRemoving] = useState<SniffRuleV1 | null>(null);
  const [report, setReport] = useState<ImportReport | null>(null);
  const [pending, setPending] = useState<PendingImport | null>(null);

  const saveRule = (rule: SniffRuleV1) => {
    const exists = opts.rules.some((r) => r.id === rule.id);
    // 超过单包上限时整个本地包会被引擎拒收，必须在写入前拦住。
    if (!exists && opts.rules.length >= MAX_RULES_PER_PACK) {
      showToast(t("rules.local.tooMany", { max: String(MAX_RULES_PER_PACK) }), "error");
      return;
    }
    opts.onRulesChange(exists ? opts.rules.map((r) => (r.id === rule.id ? rule : r)) : [...opts.rules, rule]);
    setEditor(null);
    showToast(t("rules.local.saved"), "success");
  };

  const removeRule = (rule: SniffRuleV1) => {
    opts.onRulesChange(opts.rules.filter((r) => r.id !== rule.id));
    if (rule.id in opts.overrides) {
      const { [rule.id]: _removed, ...rest } = opts.overrides;
      opts.onOverridesChange(rest);
    }
    setRemoving(null);
  };

  const exportRules = () => {
    downloadJson(`fluxdown-local-rules-${new Date().toISOString().slice(0, 10)}.json`, buildLocalPack(opts.rules, Date.now()));
  };

  const submitRule = async (rule: SniffRuleV1) => {
    const submission = communitySubmission(rule, { pageUrl: opts.testPage, tests: opts.testCases });
    if (!submission.ok) {
      showToast(
        t("rules.local.submitInvalid", {
          reason: issueText(t, { code: submission.code, param: submission.param, detail: submission.reason }),
        }),
        "error",
      );
      return;
    }
    if (!submission.prefilled) {
      try {
        await navigator.clipboard.writeText(submission.json);
        showToast(t("rules.local.submitCopied"), "success");
      } catch {
        showToast(t("rules.local.submitCopyFailed"), "error");
        return;
      }
    }
    try {
      await browser.tabs.create({ url: submission.url });
    } catch (e) {
      showToast(t("rules.local.submitOpenFailed", { error: e instanceof Error ? e.message : String(e) }), "error");
    }
  };

  const commitImport = (accepted: SniffRuleV1[], rejected: RuleIssueRef[]) => {
    const merged = mergeImported(opts.rules, accepted);
    if (merged.rules.length > MAX_RULES_PER_PACK) {
      showToast(t("rules.local.tooMany", { max: String(MAX_RULES_PER_PACK) }), "error");
      return;
    }
    opts.onRulesChange(merged.rules);
    setReport({ added: merged.added, replaced: merged.replaced, rejected });
  };

  const importFile = async (file: File | undefined) => {
    if (!file) return;
    try {
      const parsed = parseLocalPack(await file.text());
      if (!parsed.ok) {
        showToast(t("rules.local.importInvalid", { message: parsed.error }), "error");
        return;
      }
      const plan = planImport(opts.rules, parsed.accepted);
      // 同 id 会覆盖已有规则：先让用户确认。
      if (plan.replaced.length > 0) {
        setPending({ accepted: parsed.accepted, rejected: parsed.rejected, replacing: plan.replaced });
        return;
      }
      commitImport(parsed.accepted, parsed.rejected);
    } catch (e) {
      logError("[FluxDown Options] import local rules failed:", e);
      showToast(t("rules.local.importInvalid", { message: e instanceof Error ? e.message : String(e) }), "error");
    }
  };

  const copyToLocal = (rule: SniffRuleV1) => {
    const title = rule.title || rule.id;
    setEditor({
      rule: null,
      seed: { ...rule, id: generateRuleId(title, Date.now()), tier: "local", version: 1, title, enabled: undefined },
    });
  };

  const toolbar = (
    <>
      <input
        ref={fileInput}
        type="file"
        accept="application/json,.json"
        class="fd-hidden"
        aria-label={t("rules.local.import")}
        onChange={(e) => {
          const input = e.currentTarget;
          void importFile(input.files?.[0]).finally(() => {
            input.value = "";
          });
        }}
      />
      <Button size="sm" onClick={() => fileInput.current?.click()}>
        {t("rules.local.import")}
      </Button>
      <Button size="sm" disabled={opts.rules.length === 0} onClick={exportRules}>
        {t("rules.local.export")}
      </Button>
      <Button size="sm" variant="primary" onClick={() => setEditor({ rule: null })}>
        {t("rules.local.new")}
      </Button>
    </>
  );

  const reportBox = report ? (
    <div class={report.rejected.length > 0 ? "opt-warn" : "opt-ok"} role="status">
      <div>{t("rules.local.importDone", { added: String(report.added), replaced: String(report.replaced) })}</div>
      {report.rejected.length > 0 ? (
        <>
          <strong>{t("rules.local.importRejected", { count: String(report.rejected.length) })}</strong>
          <ul>
            {report.rejected.map((r) => (
              <li key={r.id + r.reason}>
                <code>{r.id || "?"}</code> {issueText(t, { code: r.code, param: r.param, detail: r.reason })}
              </li>
            ))}
          </ul>
        </>
      ) : null}
      <Button size="sm" variant="ghost" onClick={() => setReport(null)}>
        {t("rules.local.dismiss")}
      </Button>
    </div>
  ) : null;

  const dialogs =
    editor || removing || pending ? (
      <>
        {editor ? (
          <RuleEditorModal
            editing={editor.rule}
            seed={editor.seed}
            binding={opts.binding}
            takenIds={new Set(opts.rules.map((r) => r.id))}
            onSave={saveRule}
            onClose={() => setEditor(null)}
          />
        ) : null}
        {removing ? (
          <ConfirmModal
            title={t("rules.local.remove")}
            message={t("rules.local.removeConfirm", { name: removing.title || removing.id })}
            confirmLabel={t("rules.local.remove")}
            danger
            onCancel={() => setRemoving(null)}
            onConfirm={() => removeRule(removing)}
          />
        ) : null}
        {pending ? (
          <ConfirmModal
            title={t("rules.local.importPreviewTitle")}
            message={
              <>
                <p>{t("rules.local.importPreview", { count: String(pending.replacing.length) })}</p>
                <ul class="opt-import-preview">
                  {pending.replacing.map((id) => (
                    <li key={id}>
                      <code>{id}</code>
                    </li>
                  ))}
                </ul>
              </>
            }
            confirmLabel={t("rules.local.importConfirm")}
            danger
            onCancel={() => setPending(null)}
            onConfirm={() => {
              commitImport(pending.accepted, pending.rejected);
              setPending(null);
            }}
          />
        ) : null}
      </>
    ) : null;

  return {
    rules: opts.rules,
    edit: (rule) => setEditor({ rule }),
    remove: setRemoving,
    submit: (rule) => void submitRule(rule),
    copyToLocal,
    toolbar,
    report: reportBox,
    dialogs,
  };
}
