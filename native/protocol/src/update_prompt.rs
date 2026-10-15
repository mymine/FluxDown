//! 桌面「发现新版本」提醒窗口的弹出判定。
//!
//! agent（托盘驻留且没有界面时拉起桌面程序）与 GPUI（界面在运行时直接开窗）共用这一判定，
//! 两边对同一份 `UpdateStatusDto` + 偏好给出一致结论。用户的选择存为设备本地 agent 偏好
//! （不进云同步）：
//!
//! - 「跳过此版本」→ [`SKIPPED_VERSION_KEY`] = 该版本，此后只有更高版本才再提醒；
//! - 「稍后提示」/ 关闭窗口 → [`REMIND_AFTER_KEY`] = 现在 + [`REMIND_LATER_MS`]，到期前不提醒。

use std::collections::BTreeMap;

use serde_json::Value;

use crate::agent::{UpdatePhase, UpdateStatusDto};

/// 设备本地偏好：被「跳过此版本」的版本号。
pub const SKIPPED_VERSION_KEY: &str = "desktop.update_skipped_version";
/// 设备本地偏好：「稍后提示」到期的 Unix 毫秒时间戳。
pub const REMIND_AFTER_KEY: &str = "desktop.update_remind_after_ms";
/// 「稍后提示」推迟的时长：24 小时。
pub const REMIND_LATER_MS: u64 = 24 * 60 * 60 * 1000;

/// 值得提醒的新版本：确有更新，且用户还有事可做（可下载 / 下载中 / 待重启）。
#[must_use]
pub fn prompt_version(status: &UpdateStatusDto) -> Option<&str> {
    let actionable = matches!(
        status.phase,
        UpdatePhase::Available | UpdatePhase::Downloading | UpdatePhase::Ready
    );
    (status.has_update && actionable && !status.latest_version.is_empty())
        .then_some(status.latest_version.as_str())
}

/// 被跳过的版本（未设置时为 `None`）。
#[must_use]
pub fn skipped_version(preferences: &BTreeMap<String, Value>) -> Option<&str> {
    preferences
        .get(SKIPPED_VERSION_KEY)
        .and_then(Value::as_str)
        .filter(|version| !version.is_empty())
}

/// 「稍后提示」到期时刻（未设置时为 `None`）。
#[must_use]
pub fn remind_after_ms(preferences: &BTreeMap<String, Value>) -> Option<u64> {
    preferences.get(REMIND_AFTER_KEY).and_then(Value::as_u64)
}

/// 现在是否应自动弹出更新提醒：有值得提醒的版本、未被跳过、不在「稍后提示」期内。
/// 用户主动「检查更新」不经此判定，有新版本即开窗。
#[must_use]
pub fn is_due(
    status: &UpdateStatusDto,
    preferences: &BTreeMap<String, Value>,
    now_ms: u64,
) -> bool {
    let Some(version) = prompt_version(status) else {
        return false;
    };
    skipped_version(preferences) != Some(version)
        && remind_after_ms(preferences).is_none_or(|after| now_ms >= after)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn status(phase: UpdatePhase) -> UpdateStatusDto {
        UpdateStatusDto {
            phase,
            latest_version: "2.0.0".to_owned(),
            has_update: true,
            ..UpdateStatusDto::default()
        }
    }

    fn prefs(values: &[(&str, Value)]) -> BTreeMap<String, Value> {
        values
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect()
    }

    #[test]
    fn only_actionable_phases_with_an_update_prompt() {
        let none = BTreeMap::new();
        for phase in [
            UpdatePhase::Available,
            UpdatePhase::Downloading,
            UpdatePhase::Ready,
        ] {
            assert!(is_due(&status(phase), &none, 0), "{phase:?}");
        }
        for phase in [
            UpdatePhase::Idle,
            UpdatePhase::Checking,
            UpdatePhase::UpToDate,
            UpdatePhase::Installing,
            UpdatePhase::Failed,
        ] {
            assert!(!is_due(&status(phase), &none, 0), "{phase:?}");
        }
        let mut no_update = status(UpdatePhase::Available);
        no_update.has_update = false;
        assert!(!is_due(&no_update, &none, 0));
    }

    #[test]
    fn skip_applies_to_that_version_only() {
        let skipped = prefs(&[(SKIPPED_VERSION_KEY, json!("2.0.0"))]);
        assert!(!is_due(&status(UpdatePhase::Ready), &skipped, 0));
        let mut newer = status(UpdatePhase::Ready);
        newer.latest_version = "2.0.1".to_owned();
        assert!(is_due(&newer, &skipped, 0));
    }

    #[test]
    fn remind_later_holds_until_the_deadline() {
        let later = prefs(&[(REMIND_AFTER_KEY, json!(1_000))]);
        let ready = status(UpdatePhase::Ready);
        assert!(!is_due(&ready, &later, 999));
        assert!(is_due(&ready, &later, 1_000));
        // 损坏的值不得让提醒永久失效。
        let garbage = prefs(&[(REMIND_AFTER_KEY, json!("soon"))]);
        assert!(is_due(&ready, &garbage, 0));
    }
}
