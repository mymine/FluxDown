import type { JSX } from "preact";
import { useEffect, useMemo, useState } from "preact/hooks";
import { useT } from "@/ui/i18n";
import {
  LOCAL_RULES_KEY,
  RULE_OVERRIDES_KEY,
  type RuleTestCase,
  parseLocalRules,
  parseOverrides,
  pruneOverrides,
} from "../../lib/local-rules";
import { useLocalValue } from "../../lib/store";
import { PanelShell } from "../../parts";
import { RuleTable } from "./RuleTable";
import { RuleTester, type TesterBinding } from "./RuleTester";
import { SyncCard } from "./SyncCard";
import { useLocalRules } from "./useLocalRules";
import { useRulesData } from "./useRulesData";

export function RulesPanel(): JSX.Element {
  const t = useT();
  const data = useRulesData();
  const [overrides, writeOverrides] = useLocalValue(RULE_OVERRIDES_KEY, parseOverrides);
  const [localRules, writeLocalRules] = useLocalValue(LOCAL_RULES_KEY, parseLocalRules);
  // 测试台产出：最近测试的页面（作为提交的示例页面）与附加的测试用例，随「提交到社区」预填。
  const [testPage, setTestPage] = useState<string | undefined>(undefined);
  const [testCases, setTestCases] = useState<RuleTestCase[]>([]);

  const binding = useMemo<TesterBinding>(
    () => ({
      caseCount: testCases.length,
      onTested: setTestPage,
      onAddCase: (testCase) => setTestCases((cases) => [...cases, testCase]),
      onClearCases: () => setTestCases([]),
    }),
    [testCases.length],
  );

  // 指向已不存在规则的覆盖（上游移除的社区规则等）没有意义，列表加载后清掉。
  useEffect(() => {
    if (!data.loaded) return;
    const pruned = pruneOverrides(overrides, new Set(data.entries.map((e) => e.rule.id)));
    if (pruned !== overrides) void writeOverrides(pruned);
  }, [data.loaded, data.entries, overrides]);

  const local = useLocalRules({
    rules: localRules,
    overrides,
    onRulesChange: (next) => void writeLocalRules(next),
    onOverridesChange: (next) => void writeOverrides(next),
    binding,
    testCases,
    testPage,
  });

  return (
    <PanelShell title={t("options.nav.rules")} desc={t("rules.desc")}>
      {data.loadError ? (
        <div class="opt-warn" role="alert">
          {t("rules.loadError", { message: data.loadError })}
        </div>
      ) : null}
      <SyncCard sync={data.sync} onSynced={data.setSync} />
      <RuleTable
        entries={data.entries}
        overrides={overrides}
        onOverridesChange={(next) => void writeOverrides(next)}
        local={local}
      />
      <RuleTester binding={binding} />
    </PanelShell>
  );
}
