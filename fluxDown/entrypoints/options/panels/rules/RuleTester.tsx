import type { JSX } from "preact";
import { useState } from "preact/hooks";
import type { RulesTestResult } from "@/background/protocol";
import type { ResponseInput, RuleRef } from "@/rules/engine";
import type { SniffRuleV1 } from "@/rules/schema";
import { Button } from "@/ui/components/Button";
import { Select } from "@/ui/components/Select";
import { showToast } from "@/ui/components/Toast";
import { TextInput } from "@/ui/components/TextInput";
import { Toggle } from "@/ui/components/Toggle";
import { useT } from "@/ui/i18n";
import { sendAction } from "@/utils/messaging";
import { type RuleTestCase, buildTestCase } from "../../lib/local-rules";
import { REQUEST_TYPE_OPTIONS } from "../../lib/rule-form";
import { issueText } from "../../lib/rule-issue";
import { logError } from "@/utils/error-log";

const MB = 1024 * 1024;

/** 测试台与「提交到社区」之间的共享状态：最近测试的页面地址与附带的测试用例。 */
export interface TesterBinding {
  caseCount: number;
  onTested: (pageUrl: string) => void;
  onAddCase: (testCase: RuleTestCase) => void;
  onClearCases: () => void;
}

interface RuleTesterProps {
  binding: TesterBinding;
  /** 编辑器内使用：返回通过校验的草稿规则（连同已保存规则一起编译后测试）；null = 未通过，由编辑器展示错误。 */
  getDraft?: () => SniffRuleV1 | null;
}

interface TestRun {
  input: ResponseInput;
  result: RulesTestResult;
}

const describeRule = (rule: RuleRef): string => `${rule.id} (v${rule.version})`;

export function RuleTester(props: RuleTesterProps): JSX.Element {
  const t = useT();
  const embedded = props.getDraft !== undefined;
  const [url, setUrl] = useState("");
  const [pageUrl, setPageUrl] = useState("");
  const [mime, setMime] = useState("");
  const [sizeMB, setSizeMB] = useState("");
  const [resourceType, setResourceType] = useState<string>("");
  const [attachment, setAttachment] = useState(false);
  const [running, setRunning] = useState(false);
  const [run, setRun] = useState<TestRun | null>(null);

  const execute = async () => {
    const target = url.trim();
    if (!target) {
      showToast(t("rules.test.urlRequired"), "error");
      return;
    }
    let draft: SniffRuleV1 | undefined;
    if (props.getDraft) {
      const checked = props.getDraft();
      if (!checked) {
        showToast(t("rules.editor.testDraftInvalid"), "error");
        return;
      }
      draft = checked;
    }
    const mb = Number(sizeMB);
    const input: ResponseInput = {
      url: target,
      pageUrl: pageUrl.trim() || target,
      resourceType: resourceType || undefined,
      mime: mime.trim() || undefined,
      // 留空 = 大小未知（-1）。
      size: sizeMB.trim() !== "" && Number.isFinite(mb) && mb >= 0 ? Math.round(mb * MB) : -1,
      attachment,
    };
    setRunning(true);
    try {
      const result = await sendAction<"rulesTest">({ action: "rulesTest", ...input, ...(draft ? { draft } : {}) });
      setRun({ input, result });
      props.binding.onTested(input.pageUrl);
    } catch (e) {
      logError("[FluxDown Options] rulesTest failed:", e);
      showToast(t("rules.test.failed"), "error");
    } finally {
      setRunning(false);
    }
  };

  const addCase = () => {
    if (!run) return;
    let host = run.input.url;
    try {
      const u = new URL(run.input.url);
      host = `${u.host}${u.pathname}`;
    } catch {
      // 非法地址原样作名字
    }
    props.binding.onAddCase(buildTestCase(run.input, run.result.response, host));
    showToast(t("rules.test.caseAdded", { count: String(props.binding.caseCount + 1) }), "success");
  };

  const result = run?.result;
  let decision = "";
  let capture = "";
  if (result) {
    const response = result.response;
    if (response.decision === "resource") {
      decision = t("rules.test.decisionResource", { type: response.type, rule: describeRule(response.rule) });
    } else if (response.decision === "ignore") {
      decision = t("rules.test.decisionIgnore", { rule: describeRule(response.rule) });
    } else {
      decision = t("rules.test.decisionNone");
    }
    if (result.wouldIngest) {
      capture = t("rules.test.captureYes", { type: result.finalType ?? "other" });
    } else if (!result.sniffAllowed) {
      capture = t("rules.test.captureNoSniffOff");
    } else if (result.ignoredBy) {
      capture = t("rules.test.captureNoIgnored", { rule: describeRule(result.ignoredBy) });
    } else {
      capture = t("rules.test.captureNoRule");
    }
  }

  return (
    <div class={embedded ? "opt-test-embedded" : "opt-card"}>
      {embedded ? (
        <h3 class="opt-subtitle">{t("rules.editor.testTitle")}</h3>
      ) : (
        <>
          <div class="opt-card-head">
            <h2 class="opt-card-title">{t("rules.test.title")}</h2>
          </div>
          <p class="opt-row-desc">{t("rules.test.desc")}</p>
        </>
      )}

      <div class="opt-form">
        <label class="opt-field">
          <span class="opt-field-label">{t("rules.test.url")}</span>
          <TextInput value={url} placeholder="https://cdn.example.com/video/clip.m3u8" onInput={setUrl} />
        </label>
        <label class="opt-field">
          <span class="opt-field-label">{t("rules.test.pageUrl")}</span>
          <TextInput value={pageUrl} placeholder={t("rules.test.pageUrlPlaceholder")} onInput={setPageUrl} />
        </label>
        <div class="opt-field-row">
          <label class="opt-field">
            <span class="opt-field-label">{t("rules.test.mime")}</span>
            <TextInput value={mime} placeholder="video/mp4" onInput={setMime} />
          </label>
          <label class="opt-field">
            <span class="opt-field-label">{t("rules.test.size")}</span>
            <TextInput type="number" min={0} value={sizeMB} placeholder={t("rules.test.sizePlaceholder")} onInput={setSizeMB} />
          </label>
        </div>
        <div class="opt-field-row">
          <div class="opt-field">
            <span class="opt-field-label">{t("rules.test.resourceType")}</span>
            <Select<string>
              value={resourceType}
              onChange={setResourceType}
              label={t("rules.test.resourceType")}
              options={[
                { value: "", label: t("rules.test.resourceTypeAny") },
                ...REQUEST_TYPE_OPTIONS.map((value) => ({ value, label: value })),
              ]}
            />
          </div>
          <div class="opt-field opt-field-inline">
            <span class="opt-field-label">{t("rules.test.attachment")}</span>
            <Toggle checked={attachment} label={t("rules.test.attachment")} onChange={setAttachment} />
          </div>
        </div>
        <div class="opt-filters">
          <Button variant="primary" disabled={running} onClick={() => void execute()}>
            {t("rules.test.run")}
          </Button>
          {run ? (
            <Button onClick={addCase}>{t("rules.test.addCase")}</Button>
          ) : null}
          {props.binding.caseCount > 0 ? (
            <>
              <span class="opt-row-desc">{t("rules.test.caseCount", { count: String(props.binding.caseCount) })}</span>
              <Button size="sm" variant="ghost" onClick={props.binding.onClearCases}>
                {t("rules.test.casesClear")}
              </Button>
            </>
          ) : null}
        </div>
      </div>

      <div role="status" aria-live="polite">
        {result ? (
          <dl class="opt-kv opt-test-result-box">
            <dt>{t("rules.test.resDecision")}</dt>
            <dd>{decision}</dd>
            <dt>{t("rules.test.resCapture")}</dt>
            <dd>{capture}</dd>
            {result.draft ? (
              <>
                <dt>{t("rules.test.resDraft")}</dt>
                <dd>
                  {result.draft.status === "active"
                    ? t("rules.test.draftActive")
                    : t("rules.test.draftInactive", {
                        reason: issueText(t, {
                          code: result.draft.code,
                          param: result.draft.param,
                          detail: result.draft.detail ?? result.draft.status,
                        }),
                      })}
                </dd>
              </>
            ) : null}
            <dt>{t("rules.test.resMatches")}</dt>
            <dd>
              {result.matches.length === 0 ? (
                "—"
              ) : (
                <ol class="opt-match-list">
                  {result.matches.map((m) => (
                    <li key={m.ref.id}>
                      <code>{m.ref.id}</code> · {m.decision === "resource" ? t("rules.editor.actResource") : t("rules.editor.actIgnore")} ·{" "}
                      {m.winner ? <strong>{t("rules.test.matchWinner")}</strong> : t("rules.test.matchShadowed")}
                    </li>
                  ))}
                </ol>
              )}
            </dd>
            <dt>{t("rules.test.resTrack")}</dt>
            <dd>{result.track ?? "—"}</dd>
            <dt>{t("rules.test.resDedupe")}</dt>
            <dd class="opt-cell-mono opt-cell-wrap">{result.dedupeKey || "—"}</dd>
            <dt>{t("rules.test.resExt")}</dt>
            <dd class="opt-cell-mono">{result.fileExtension || "—"}</dd>
          </dl>
        ) : null}
      </div>
    </div>
  );
}
