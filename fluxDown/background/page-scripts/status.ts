/**
 * 给 UI（选项页 / popup）的页面脚本状态视图：索引元数据缓存 × 批准记录 × 浏览器可用性 × 同步结论。纯函数。
 */
import type {
  PageScriptPluginView,
  PageScriptsAvailability,
  PageScriptsStatus,
  PageScriptsSyncState,
  PluginApprovals,
  PluginViewState,
} from "./model";
import { registrationId } from "./registration";

function stateOf(
  approved: boolean,
  availability: PageScriptsAvailability,
  sync: PageScriptsSyncState,
  identity: string,
): PluginViewState {
  if (!approved) return "disabled";
  if (availability === "permission-missing") return "permission-missing";
  if (availability === "switch-off") return "switch-off";
  if (availability === "browser-unsupported") return "browser-unsupported";
  if (sync.errors[identity]) return "error";
  if (sync.outcome === "offline" || sync.outcome === "remote-mode" || sync.outcome === "host-unsupported") {
    return "desktop-offline";
  }
  return "enabled";
}

export function buildStatus(
  sync: PageScriptsSyncState,
  approvals: PluginApprovals,
  availability: PageScriptsAvailability,
): PageScriptsStatus {
  const plugins = new Map<string, PageScriptPluginView>();

  for (const entry of sync.index) {
    let plugin = plugins.get(entry.identity);
    if (!plugin) {
      const approved = entry.identity in approvals;
      plugin = {
        identity: entry.identity,
        name: entry.pluginName,
        version: entry.pluginVersion,
        approved,
        provided: true,
        state: stateOf(approved, availability, sync, entry.identity),
        error: sync.errors[entry.identity],
        scripts: [],
      };
      plugins.set(entry.identity, plugin);
    }
    plugin.scripts.push({
      id: entry.id,
      title: entry.title,
      matches: entry.matches,
      excludeMatches: entry.excludeMatches,
      world: entry.world,
      registered: registrationId(entry.identity, entry.id) in sync.registered,
    });
  }

  // 已批准但桌面端不再提供（卸载 / 停用 / 撤销授权）：仍列出，用户能看到并移除批准记录。
  for (const [identity, approval] of Object.entries(approvals)) {
    if (plugins.has(identity)) continue;
    plugins.set(identity, {
      identity,
      name: approval.pluginName,
      version: "",
      approved: true,
      provided: false,
      state: stateOf(true, availability, sync, identity),
      error: sync.errors[identity],
      scripts: [],
    });
  }

  const list = [...plugins.values()].sort((a, b) => a.name.localeCompare(b.name) || a.identity.localeCompare(b.identity));
  const blocked =
    sync.outcome === "remote-mode" || sync.outcome === "host-unsupported" ? sync.outcome : null;
  const pendingCount = list.filter((p) => !p.approved && p.provided).length;
  const needsSwitch = list.some((p) => p.state === "switch-off" || p.state === "permission-missing");

  let attention: PageScriptsStatus["attention"] = null;
  if (blocked === null && availability !== "browser-unsupported") {
    if (needsSwitch) attention = "switch";
    else if (pendingCount > 0) attention = "pending";
  }

  return {
    availability,
    blocked,
    outcome: sync.outcome ?? null,
    lastSyncAt: sync.lastSyncAt ?? null,
    plugins: list,
    attention,
    pendingCount,
  };
}
