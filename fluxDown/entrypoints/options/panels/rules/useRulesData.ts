/**
 * 规则面板的数据源：精简规则列表（rulesList，background 等当前编译完成后才返回）与规则同步状态（rulesSyncStatus）。
 * 规则相关存储键或影响编译的设置变化后短暂延迟重取；重取带序号，乱序返回的旧结果会被丢弃。
 */
import { useCallback, useEffect, useRef, useState } from "preact/hooks";
import { browser } from "wxt/browser";
import type { RuleSyncStatus, RulesListEntry } from "@/background/protocol";
import { sendAction } from "@/utils/messaging";
import { LOCAL_RULES_KEY, RULE_OVERRIDES_KEY } from "../../lib/local-rules";

const RULE_PACKS_KEY = "fluxdown_rule_packs";
const WATCHED_LOCAL_KEYS: Record<string, true> = {
  [LOCAL_RULES_KEY]: true,
  [RULE_OVERRIDES_KEY]: true,
  [RULE_PACKS_KEY]: true,
};
/** 这些设置会改变规则编译结果或同步行为；其它设置变化不必重取。 */
const RELEVANT_SETTINGS = ["sniffImages", "communityRulesEnabled", "ruleSyncEnabled"] as const;
const RELOAD_DELAY_MS = 300;

export interface RulesData {
  entries: RulesListEntry[];
  /** 至少成功取到过一次规则列表（据此才能判断覆盖是否指向不存在的规则）。 */
  loaded: boolean;
  sync: RuleSyncStatus | null;
  /** 最近一次取数失败的原因；成功后清空。 */
  loadError: string | null;
  setSync: (status: RuleSyncStatus) => void;
  reload: () => void;
}

function settingsChanged(change: unknown): boolean {
  if (change === null || typeof change !== "object") return true;
  const { oldValue, newValue } = change as { oldValue?: Record<string, unknown>; newValue?: Record<string, unknown> };
  if (!oldValue || !newValue) return true;
  return RELEVANT_SETTINGS.some((key) => oldValue[key] !== newValue[key]);
}

export function useRulesData(): RulesData {
  const [entries, setEntries] = useState<RulesListEntry[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [sync, setSync] = useState<RuleSyncStatus | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const timer = useRef<number | undefined>(undefined);
  const listSeq = useRef(0);
  const syncSeq = useRef(0);

  const reload = useCallback(() => {
    const listId = ++listSeq.current;
    const syncId = ++syncSeq.current;
    sendAction<"rulesList">({ action: "rulesList" })
      .then((list) => {
        if (listId !== listSeq.current) return;
        setEntries(list);
        setLoaded(true);
        setLoadError(null);
      })
      .catch((e) => {
        if (listId === listSeq.current) setLoadError(e instanceof Error ? e.message : String(e));
      });
    sendAction<"rulesSyncStatus">({ action: "rulesSyncStatus" })
      .then((status) => {
        if (syncId === syncSeq.current) setSync(status);
      })
      .catch((e) => {
        if (syncId === syncSeq.current) setLoadError(e instanceof Error ? e.message : String(e));
      });
  }, []);

  useEffect(() => {
    reload();
    const onChanged = (changes: Record<string, unknown>, area: string) => {
      const relevant =
        (area === "local" && Object.keys(changes).some((k) => WATCHED_LOCAL_KEYS[k])) ||
        (area === "sync" && "settings" in changes && settingsChanged(changes.settings));
      if (!relevant) return;
      clearTimeout(timer.current);
      timer.current = window.setTimeout(reload, RELOAD_DELAY_MS);
    };
    browser.storage.onChanged.addListener(onChanged);
    return () => {
      clearTimeout(timer.current);
      browser.storage.onChanged.removeListener(onChanged);
    };
  }, [reload]);

  const setSyncStatus = useCallback((status: RuleSyncStatus) => {
    // 手动同步的结果比任何在途的状态查询都新。
    syncSeq.current++;
    setSync(status);
  }, []);

  return { entries, loaded, sync, loadError, setSync: setSyncStatus, reload };
}
