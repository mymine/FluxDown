//! 站点代理规则（daemon 配置键 `proxy_rules`）的客户端侧 wire 模型。
//!
//! 值是 JSON 数组，数组顺序即优先级。解析宽松：空串 / 非法 JSON / 非数组 → 空列表，
//! 非对象元素跳过，字段缺失或类型错误回退默认值。与引擎 `fluxdown_engine::proxy_rules`、
//! Web `web/src/lib/proxyRules.ts`、Android `ProxyRules.kt`、iOS `ProxyRules.swift`
//! 的宽松解析逐条一致。匹配只在引擎实现，客户端经 `daemon.config.proxyRoute` 询问。

use serde::Serialize;
use serde_json::Value;

/// daemon 配置键。
pub const PROXY_RULES_KEY: &str = "proxy_rules";

/// 一条站点代理规则；编码器总是写出全部 7 个字段。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyRuleSpec {
    pub id: String,
    pub enabled: bool,
    pub name: String,
    /// `domain` / `regex`。
    #[serde(rename = "match")]
    pub match_kind: String,
    pub pattern: String,
    /// `direct` / `system` / `manual` / `custom`。
    pub action: String,
    /// 仅 `custom` 使用的代理 URL。
    pub proxy: String,
}

/// 新规则：启用、按域名匹配、直连。
pub fn new_proxy_rule(id: String) -> ProxyRuleSpec {
    ProxyRuleSpec {
        id,
        enabled: true,
        name: String::new(),
        match_kind: "domain".to_string(),
        pattern: String::new(),
        action: "direct".to_string(),
        proxy: String::new(),
    }
}

fn str_field(obj: &serde_json::Map<String, Value>, key: &str) -> String {
    obj.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_default()
}

fn enum_field(
    obj: &serde_json::Map<String, Value>,
    key: &str,
    allowed: &[&str],
    fallback: &str,
) -> String {
    obj.get(key)
        .and_then(Value::as_str)
        .filter(|v| allowed.contains(v))
        .unwrap_or(fallback)
        .to_string()
}

/// 宽松解析；非对象元素跳过。
pub fn parse_proxy_rules(raw: &str) -> Vec<ProxyRuleSpec> {
    let Ok(Value::Array(items)) = serde_json::from_str::<Value>(raw) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let obj = item.as_object()?;
            Some(ProxyRuleSpec {
                id: str_field(obj, "id"),
                enabled: obj.get("enabled").and_then(Value::as_bool).unwrap_or(true),
                name: str_field(obj, "name"),
                match_kind: enum_field(obj, "match", &["domain", "regex"], "domain"),
                pattern: str_field(obj, "pattern"),
                action: enum_field(
                    obj,
                    "action",
                    &["direct", "system", "manual", "custom"],
                    "direct",
                ),
                proxy: str_field(obj, "proxy"),
            })
        })
        .collect()
}

/// 编码为配置值。
pub fn encode_proxy_rules(rules: &[ProxyRuleSpec]) -> Result<String, serde_json::Error> {
    serde_json::to_string(rules)
}

/// 启用规则条数（状态栏 / 模式段提示）。
pub fn enabled_proxy_rule_count(raw: &str) -> usize {
    parse_proxy_rules(raw).iter().filter(|r| r.enabled).count()
}

/// 展示用代理端点：去掉凭据与末尾 `/`，如 `socks5://u:p@h:1/` → `socks5://h:1`。
pub fn proxy_display_endpoint(url: &str) -> String {
    let url = url.trim();
    let (scheme, rest) = match url.split_once("://") {
        Some((scheme, rest)) => (Some(scheme), rest),
        None => (None, url),
    };
    let host = rest.rsplit_once('@').map_or(rest, |(_, host)| host);
    let host = host.trim_end_matches('/');
    match scheme {
        Some(scheme) => format!("{scheme}://{host}"),
        None => host.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_values_parse_to_empty() {
        for raw in ["", "nope", "{}", "42", "\"[]\""] {
            assert!(parse_proxy_rules(raw).is_empty(), "{raw}");
        }
    }

    #[test]
    fn lenient_defaults_and_non_objects_skipped() {
        let raw = r#"[1, {"id":"a","enabled":"yes","name":3,"match":"glob","pattern":"x.com","action":"tunnel","proxy":false}, null, {"id":"b","enabled":false,"match":"regex","action":"custom","proxy":"socks5://h:1"}]"#;
        let rules = parse_proxy_rules(raw);
        assert_eq!(rules.len(), 2);
        assert_eq!(
            rules[0],
            ProxyRuleSpec {
                id: "a".into(),
                enabled: true,
                name: String::new(),
                match_kind: "domain".into(),
                pattern: "x.com".into(),
                action: "direct".into(),
                proxy: String::new(),
            }
        );
        assert!(!rules[1].enabled);
        assert_eq!(rules[1].match_kind, "regex");
        assert_eq!(rules[1].action, "custom");
        assert_eq!(rules[1].proxy, "socks5://h:1");
        assert_eq!(rules[1].pattern, "");
        assert_eq!(rules[1].name, "");
    }

    #[test]
    fn missing_fields_take_defaults() {
        let rules = parse_proxy_rules("[{}]");
        assert_eq!(rules, vec![new_proxy_rule(String::new())]);
    }

    #[test]
    fn encode_round_trips_parsed_values() {
        let mut rule = new_proxy_rule("pr_1".into());
        rule.name = "GitHub".into();
        rule.pattern = "github.com".into();
        rule.action = "custom".into();
        rule.proxy = "socks5://u:p@127.0.0.1:7891".into();
        let encoded = encode_proxy_rules(std::slice::from_ref(&rule)).unwrap();
        let value: Value = serde_json::from_str(&encoded).unwrap();
        let obj = value[0].as_object().unwrap();
        assert_eq!(obj.len(), 7);
        assert_eq!(obj["match"], "domain");
        assert_eq!(parse_proxy_rules(&encoded), vec![rule]);
    }

    #[test]
    fn enabled_count_ignores_disabled() {
        let raw = r#"[{"id":"a"},{"id":"b","enabled":false},{"id":"c","enabled":true},7]"#;
        assert_eq!(enabled_proxy_rule_count(raw), 2);
        assert_eq!(enabled_proxy_rule_count(""), 0);
    }

    #[test]
    fn display_endpoint_strips_credentials() {
        assert_eq!(
            proxy_display_endpoint("socks5://user:p@ss@127.0.0.1:7891/"),
            "socks5://127.0.0.1:7891"
        );
        assert_eq!(
            proxy_display_endpoint("http://h.example:8080"),
            "http://h.example:8080"
        );
        assert_eq!(proxy_display_endpoint("u@h:1"), "h:1");
    }
}
