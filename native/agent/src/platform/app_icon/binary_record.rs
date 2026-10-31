//! 「程序本体图标」的落实记录（`<icons>/binary-icon.json`）与比较前写入的纯判定。
//!
//! 记录的是 agent 自己改过的条目：路径（macOS 的 bundle 目录 / Windows 的桌面程序）→ 当时落实的
//! 偏好值（`bolt` / `custom:<revision>`）。默认图标没有记录——默认 = 安装包原样，agent 不碰；只有
//! 记录里有、而当前选择回到默认时才需要还原。记录读不出来（缺失 / 损坏）等同空，宁可多做一次
//! 比较也不信任坏数据。

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// 记录文件名（位于图标目录）。
pub(super) const RECORD_FILE: &str = "binary-icon.json";

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct BinaryRecord {
    #[serde(default)]
    entries: BTreeMap<String, String>,
}

impl BinaryRecord {
    /// 读取记录；文件缺失或内容无效返回空记录。
    pub(super) fn load(icons_dir: &Path) -> Self {
        let path = icons_dir.join(RECORD_FILE);
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                tracing::debug!(path = %path.display(), %error, "ignoring invalid binary icon record");
                Self::default()
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(error) => {
                tracing::debug!(path = %path.display(), %error, "could not read binary icon record");
                Self::default()
            }
        }
    }

    /// 内容有变化才原子写盘；空记录删除文件。
    pub(super) fn save(&self, icons_dir: &Path) -> std::io::Result<()> {
        let path = icons_dir.join(RECORD_FILE);
        if self.entries.is_empty() {
            return match std::fs::remove_file(&path) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
                _ => Ok(()),
            };
        }
        let bytes = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        if std::fs::read(&path).is_ok_and(|existing| existing == bytes) {
            return Ok(());
        }
        crate::app_icon::write_atomic(&path, &bytes)
    }

    pub(super) fn get(&self, key: &str) -> Option<&str> {
        self.entries.get(key).map(String::as_str)
    }

    /// 返回记录是否因此改变。
    pub(super) fn set(&mut self, key: &str, preference: &str) -> bool {
        if self.get(key) == Some(preference) {
            return false;
        }
        self.entries.insert(key.to_owned(), preference.to_owned());
        true
    }

    /// 返回记录是否因此改变。
    pub(super) fn remove(&mut self, key: &str) -> bool {
        self.entries.remove(key).is_some()
    }
}

/// macOS bundle 对账动作。
#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BundleAction {
    /// 已经是目标状态。
    Skip,
    /// 设置自定义图标。
    Apply,
    /// 移除我们设置过的自定义图标（还原为安装包原样）。
    Remove,
    /// 图标已不在（如更新后换了新 bundle），只需清掉过期记录。
    Forget,
}

/// 比较后决定对一个 bundle 做什么。
///
/// - `wanted`：非默认选择的偏好值；`None` = 默认；
/// - `recorded`：记录里该 bundle 上次落实的偏好值；
/// - `has_icon`：bundle 根目录当前是否存在 Finder 自定义图标（`Icon\r`）。
///
/// 默认时只移除记录过的图标：没有记录的 `Icon\r` 是用户自己设的，不动。
#[cfg(any(target_os = "macos", test))]
pub(super) fn bundle_action(
    wanted: Option<&str>,
    recorded: Option<&str>,
    has_icon: bool,
) -> BundleAction {
    match wanted {
        Some(wanted) => {
            if has_icon && recorded == Some(wanted) {
                BundleAction::Skip
            } else {
                BundleAction::Apply
            }
        }
        None => match (recorded, has_icon) {
            (None, _) => BundleAction::Skip,
            (Some(_), true) => BundleAction::Remove,
            (Some(_), false) => BundleAction::Forget,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "fluxdown-binary-record-{name}-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn record_round_trips_and_empty_record_removes_file() {
        let dir = temp_dir("round-trip");
        let mut record = BinaryRecord::default();
        assert!(record.set("/Applications/FluxDown.app", "bolt"));
        assert!(!record.set("/Applications/FluxDown.app", "bolt"));
        record.save(&dir).expect("save");
        let mut loaded = BinaryRecord::load(&dir);
        assert_eq!(loaded.get("/Applications/FluxDown.app"), Some("bolt"));

        assert!(loaded.remove("/Applications/FluxDown.app"));
        assert!(!loaded.remove("/Applications/FluxDown.app"));
        loaded.save(&dir).expect("save empty");
        assert!(!dir.join(RECORD_FILE).exists());
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn invalid_record_loads_as_empty() {
        let dir = temp_dir("invalid");
        std::fs::write(dir.join(RECORD_FILE), b"{not json").expect("write");
        assert_eq!(BinaryRecord::load(&dir), BinaryRecord::default());
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn bundle_action_is_compare_before_write() {
        use BundleAction::{Apply, Forget, Remove, Skip};
        // 非默认：已设且记录一致才跳过；更新后新 bundle 没有图标 → 重新应用。
        assert_eq!(bundle_action(Some("bolt"), Some("bolt"), true), Skip);
        assert_eq!(bundle_action(Some("bolt"), Some("bolt"), false), Apply);
        assert_eq!(bundle_action(Some("bolt"), Some("custom:a"), true), Apply);
        assert_eq!(bundle_action(Some("bolt"), None, true), Apply);
        assert_eq!(bundle_action(Some("bolt"), None, false), Apply);
        // 默认：只还原记录过的图标；用户自己设的（无记录）不动。
        assert_eq!(bundle_action(None, None, false), Skip);
        assert_eq!(bundle_action(None, None, true), Skip);
        assert_eq!(bundle_action(None, Some("bolt"), true), Remove);
        assert_eq!(bundle_action(None, Some("bolt"), false), Forget);
    }
}
