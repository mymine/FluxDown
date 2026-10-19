//! 订阅来源（provider）纯模型：可选来源、当前选择与 provider 配置校验。
//!
//! Web 镜像：`web/src/pages/rss/provider.ts`，两端用同一组用例（`provider.test.ts`），
//! 改动必须同步。

use fluxdown_protocol::PluginDto;

/// 内置 RSS / Atom / JSON Feed provider 的稳定 ID（与引擎 `RSS_PROVIDER_ID` 一致）。
pub(crate) const BUILTIN_PROVIDER_ID: &str = "rss";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderKind {
    /// 内置 RSS / Atom 抓取。
    Builtin,
    /// 已启用、已加载且已授权的插件 provider。
    Plugin,
    /// 订阅记录里的 provider 当前没有可用插件（未安装 / 已禁用 / 加载失败 / 待授权）。
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProviderChoice {
    pub(crate) id: String,
    /// 插件名；内置与 `Missing` 项为空（调用方按 `kind` 取本地化文案或 `id`）。
    pub(crate) name: String,
    pub(crate) kind: ProviderKind,
}

/// 把空值归一为内置 provider，与引擎 `RssSourceInfo::normalize` 一致。
pub(crate) fn normalize_provider_id(id: &str) -> &str {
    let id = id.trim();
    if id.is_empty() {
        BUILTIN_PROVIDER_ID
    } else {
        id
    }
}

/// 插件 provider 当前能否被引擎调用：已启用、已加载、权限与网络主机均已授权。
fn plugin_usable(plugin: &PluginDto) -> bool {
    plugin.enabled
        && plugin.load_status == "Loaded"
        && plugin.consent_pending.is_empty()
        && plugin.consent_pending_hosts.is_empty()
}

/// 来源下拉的选项：内置 RSS 固定在首位，随后是可用插件 provider（按插件 identity 升序，
/// 同一 provider ID 只取 identity 最小者——与引擎的冲突裁决一致），最后在 `current`
/// 不可用时追加一个 `Missing` 项，避免编辑既有订阅时悄悄切回 RSS。
pub(crate) fn provider_choices(plugins: &[PluginDto], current: &str) -> Vec<ProviderChoice> {
    let mut usable: Vec<&PluginDto> = plugins.iter().filter(|p| plugin_usable(p)).collect();
    usable.sort_by(|a, b| a.identity.cmp(&b.identity));
    let mut choices = vec![ProviderChoice {
        id: BUILTIN_PROVIDER_ID.to_owned(),
        name: String::new(),
        kind: ProviderKind::Builtin,
    }];
    for plugin in usable {
        for id in &plugin.subscription_provider_ids {
            if id.is_empty() || choices.iter().any(|choice| &choice.id == id) {
                continue;
            }
            choices.push(ProviderChoice {
                id: id.clone(),
                name: if plugin.name.is_empty() {
                    plugin.identity.clone()
                } else {
                    plugin.name.clone()
                },
                kind: ProviderKind::Plugin,
            });
        }
    }
    let current = normalize_provider_id(current);
    if !choices.iter().any(|choice| choice.id == current) {
        choices.push(ProviderChoice {
            id: current.to_owned(),
            name: String::new(),
            kind: ProviderKind::Missing,
        });
    }
    choices
}

/// provider 配置是不透明字符串；界面要求留空或为合法 JSON（插件通常 `JSON.parse`）。
/// 返回去掉首尾空白后的文本；非法返回 `None`。
pub(crate) fn validate_provider_config(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return Some(String::new());
    }
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .map(|_| text.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin(
        identity: &str,
        name: &str,
        providers: &[&str],
        patch: impl FnOnce(&mut PluginDto),
    ) -> PluginDto {
        let mut dto: PluginDto = serde_json::from_value(serde_json::json!({
            "identity": identity,
            "name": name,
            "version": "1.0.0",
            "enabled": true,
            "devMode": false,
            "disabledReason": "None",
            "settings": [],
            "settingsValues": {},
            "subscriptionProviderIds": providers,
        }))
        .expect("plugin dto fixture");
        patch(&mut dto);
        dto
    }

    fn ids(choices: &[ProviderChoice]) -> Vec<&str> {
        choices.iter().map(|c| c.id.as_str()).collect()
    }

    #[test]
    fn builtin_first_then_usable_plugins_in_identity_order() {
        let plugins = [
            plugin("z@x", "Zed", &["zed"], |_| {}),
            plugin("a@x", "Alpha", &["alpha"], |_| {}),
            plugin("m@x", "No provider", &[], |_| {}),
        ];
        let choices = provider_choices(&plugins, "rss");
        assert_eq!(ids(&choices), ["rss", "alpha", "zed"]);
        assert_eq!(choices[0].kind, ProviderKind::Builtin);
        assert_eq!(choices[1].kind, ProviderKind::Plugin);
        assert_eq!(choices[1].name, "Alpha");
    }

    #[test]
    fn unusable_plugins_are_not_offered() {
        let plugins = [
            plugin("a@x", "Disabled", &["a"], |p| p.enabled = false),
            plugin("b@x", "Failed", &["b"], |p| p.load_status = "Failed".into()),
            plugin("c@x", "Pending perms", &["c"], |p| {
                p.consent_pending = vec!["ffmpeg".into()];
            }),
            plugin("d@x", "Pending hosts", &["d"], |p| {
                p.consent_pending_hosts = vec!["example.com".into()];
            }),
            plugin("e@x", "Fine", &["e"], |_| {}),
        ];
        assert_eq!(ids(&provider_choices(&plugins, "")), ["rss", "e"]);
    }

    #[test]
    fn duplicate_provider_id_keeps_smallest_identity() {
        let plugins = [
            plugin("b@x", "Second", &["same"], |_| {}),
            plugin("a@x", "First", &["same"], |_| {}),
        ];
        let choices = provider_choices(&plugins, "same");
        assert_eq!(ids(&choices), ["rss", "same"]);
        assert_eq!(choices[1].name, "First");
    }

    #[test]
    fn plugin_name_falls_back_to_identity() {
        let plugins = [plugin("a@x", "", &["a"], |_| {})];
        assert_eq!(provider_choices(&plugins, "a")[1].name, "a@x");
    }

    #[test]
    fn missing_current_provider_is_kept_as_missing_choice() {
        let plugins = [plugin("a@x", "Alpha", &["alpha"], |p| p.enabled = false)];
        let choices = provider_choices(&plugins, " alpha ");
        assert_eq!(ids(&choices), ["rss", "alpha"]);
        assert_eq!(choices[1].kind, ProviderKind::Missing);
    }

    #[test]
    fn empty_current_means_builtin_and_adds_nothing() {
        assert_eq!(ids(&provider_choices(&[], "  ")), ["rss"]);
        assert_eq!(normalize_provider_id(""), "rss");
        assert_eq!(normalize_provider_id(" demo "), "demo");
    }

    #[test]
    fn provider_config_accepts_blank_or_json_only() {
        assert_eq!(validate_provider_config("  "), Some(String::new()));
        assert_eq!(
            validate_provider_config(" {\"q\":\"1080p\"} "),
            Some("{\"q\":\"1080p\"}".to_owned())
        );
        assert_eq!(validate_provider_config("[1,2]"), Some("[1,2]".to_owned()));
        assert_eq!(validate_provider_config("{oops"), None);
        assert_eq!(validate_provider_config("1080p"), None);
    }
}
