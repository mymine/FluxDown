//! 插件授权（consent）记录：持久化形状与「待授权」集合的纯计算。
//!
//! 配置键 `plugin.<id>.consent`，JSON：
//! `{"permissions":[排序],"networkHosts":[排序],"version":"x.y.z","grantedAt":<unix ms>,"source":"user"|"migration"}`。

use serde::{Deserialize, Serialize};

use super::manifest::PluginManifest;

/// 迁移完成标记（全局键）。
pub const CONSENT_MIGRATED_KEY: &str = "plugin.consent_migrated";

pub fn consent_key(identity: &str) -> String {
    format!("plugin.{identity}.consent")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsentRecord {
    pub permissions: Vec<String>,
    pub network_hosts: Vec<String>,
    pub version: String,
    pub granted_at: i64,
    pub source: String,
}

impl ConsentRecord {
    /// 以 manifest 当前请求的能力构造一条记录（集合已排序去重）。
    pub fn for_manifest(manifest: &PluginManifest, source: &str) -> Self {
        Self {
            permissions: requested_permissions(manifest),
            network_hosts: manifest.network_hosts(),
            version: manifest.version.clone(),
            granted_at: crate::task_activity::timestamp_ms(),
            source: source.to_string(),
        }
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// 解析失败视为未授权（损坏的记录不能放行）。
    pub fn parse(raw: &str) -> Option<Self> {
        serde_json::from_str(raw).ok()
    }
}

/// manifest 请求的权限集合（排序去重）。
pub fn requested_permissions(manifest: &PluginManifest) -> Vec<String> {
    let mut v = manifest.permissions.clone();
    v.sort();
    v.dedup();
    v
}

/// `requested − granted`（`requested` 须已排序，结果保持有序）。
fn difference(requested: &[String], granted: &[String]) -> Vec<String> {
    requested
        .iter()
        .filter(|item| !granted.contains(item))
        .cloned()
        .collect()
}

/// 返回 `(consent_pending, consent_pending_hosts)`；无授权记录时全部请求项都待授权。
pub fn pending(
    manifest: &PluginManifest,
    consent: Option<&ConsentRecord>,
) -> (Vec<String>, Vec<String>) {
    let permissions = requested_permissions(manifest);
    let hosts = manifest.network_hosts();
    match consent {
        Some(c) => (
            difference(&permissions, &c.permissions),
            difference(&hosts, &c.network_hosts),
        ),
        None => (permissions, hosts),
    }
}

/// 排序去重后比较。
pub fn same_set(a: &[String], b: &[String]) -> bool {
    let norm = |v: &[String]| {
        let mut v = v.to_vec();
        v.sort();
        v.dedup();
        v
    };
    norm(a) == norm(b)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn manifest(perms: &[&str]) -> PluginManifest {
        let json = serde_json::json!({
            "identity": "a@b", "name": "N", "version": "1.0.0", "permissions": perms,
        });
        PluginManifest::parse(json.to_string().as_bytes()).unwrap()
    }

    #[test]
    fn pending_is_requested_minus_granted() {
        let m = manifest(&["tasks", "ytdlp"]);
        let (p, h) = pending(&m, None);
        assert_eq!(p, ["tasks", "ytdlp"]);
        assert!(h.is_empty());
        let c = ConsentRecord {
            permissions: vec!["tasks".into(), "auth".into()],
            network_hosts: vec![],
            version: "0.9.0".into(),
            granted_at: 0,
            source: "user".into(),
        };
        assert_eq!(pending(&m, Some(&c)).0, ["ytdlp"]);
    }

    #[test]
    fn record_roundtrip_and_corrupt_is_none() {
        let m = manifest(&["tasks"]);
        let rec = ConsentRecord::for_manifest(&m, "migration");
        let parsed = ConsentRecord::parse(&rec.to_json().unwrap()).unwrap();
        assert_eq!(parsed, rec);
        assert!(ConsentRecord::parse("{oops").is_none());
    }

    #[test]
    fn same_set_ignores_order_and_dups() {
        assert!(same_set(
            &["b".into(), "a".into(), "a".into()],
            &["a".into(), "b".into()]
        ));
        assert!(!same_set(&["a".into()], &["a".into(), "b".into()]));
    }
}
