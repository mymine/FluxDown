//! 「在此设备同步的范围」：把同步目录键按前缀分组，并按 `local_only_keys` 计算每组开关状态。
//!
//! 分组顺序与 Web 端一致；组内键取自协议同步目录（`SYNC_SETTING_SPECS`），
//! 目录新增键会自动落入对应分组，无需改这里。

use std::time::Duration;

use fluxdown_protocol::{
    CUSTOM_CATEGORIES_PREF_KEY, ErrorReason, SYNC_SETTING_SPECS, SettingOwner, SyncStatusDto,
};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) enum SyncGroup {
    Appearance,
    General,
    Ui,
    Download,
    Bt,
    Ed2k,
    Categories,
    Other,
}

impl SyncGroup {
    pub(crate) const ALL: [Self; 8] = [
        Self::Appearance,
        Self::General,
        Self::Ui,
        Self::Download,
        Self::Bt,
        Self::Ed2k,
        Self::Categories,
        Self::Other,
    ];

    /// 按键前缀归组；分类列表键（自定义分类）单独一组。
    #[must_use]
    pub(crate) fn of_key(key: &str) -> Self {
        if key == CUSTOM_CATEGORIES_PREF_KEY {
            return Self::Categories;
        }
        match key.split_once('.').map_or(key, |(prefix, _)| prefix) {
            "appearance" => Self::Appearance,
            "general" => Self::General,
            "ui" => Self::Ui,
            "download" => Self::Download,
            "bt" => Self::Bt,
            "ed2k" => Self::Ed2k,
            "categories" => Self::Categories,
            _ => Self::Other,
        }
    }

    #[must_use]
    pub(crate) fn label_key(self) -> &'static str {
        match self {
            Self::Appearance => "syncScopeAppearance",
            Self::General => "syncScopeGeneral",
            Self::Ui => "syncScopeUi",
            Self::Download => "syncScopeDownload",
            Self::Bt => "syncScopeBt",
            Self::Ed2k => "syncScopeEd2k",
            Self::Categories => "syncScopeCategories",
            Self::Other => "syncScopeOther",
        }
    }
}

/// 一个分组及其在本设备上的同步状态。
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SyncGroupState {
    pub group: SyncGroup,
    pub keys: Vec<&'static str>,
    /// 组内已设为「仅本机」的键数。
    pub local_only: usize,
}

impl SyncGroupState {
    /// 组内全部键都参与同步（开关打开）。
    #[must_use]
    pub(crate) fn all_synced(&self) -> bool {
        self.local_only == 0
    }

    /// 部分键为本机专属（开关关闭并提示数量）。
    #[must_use]
    pub(crate) fn is_partial(&self) -> bool {
        self.local_only > 0 && self.local_only < self.keys.len()
    }

    /// 切到目标状态需要提交的键：目标 `local_only=true` 只提交仍在同步的键，
    /// 反之只提交已本机专属的键，避免重复写入。
    #[must_use]
    pub(crate) fn keys_to_change(
        &self,
        local_only_keys: &[String],
        make_local_only: bool,
    ) -> Vec<String> {
        self.keys
            .iter()
            .filter(|key| local_only_keys.iter().any(|local| local == *key) != make_local_only)
            .map(|key| (*key).to_owned())
            .collect()
    }
}

/// 配置同步卡片展示的整体阶段。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SyncPhase {
    /// 未登录或未启用。
    Disabled,
    /// 因设备超限 / 设备未受信任暂停自动重试，需处理后重新启用。
    Halted(Option<ErrorReason>),
    /// 最近一次同步失败（会自动重试）。
    Failed(Option<ErrorReason>),
    /// 同步通道尚未连通。
    Connecting,
    /// 有待推送的本地改动。
    Syncing,
    /// 已同步；带最近一次成功时间（Unix 毫秒）。
    Synced(Option<i64>),
}

#[must_use]
pub(crate) fn sync_phase(logged_in: bool, sync: &SyncStatusDto) -> SyncPhase {
    if !logged_in || !sync.enabled {
        return SyncPhase::Disabled;
    }
    if sync.halted {
        return SyncPhase::Halted(sync.last_error_reason);
    }
    let has_error = sync.last_error_reason.is_some()
        || sync
            .last_error
            .as_deref()
            .is_some_and(|error| !error.is_empty());
    if has_error {
        return SyncPhase::Failed(sync.last_error_reason);
    }
    if !sync.connected {
        return SyncPhase::Connecting;
    }
    if !sync.dirty_keys.is_empty() {
        return SyncPhase::Syncing;
    }
    SyncPhase::Synced(sync.last_synced_at_unix_ms)
}

/// 「立即同步」可用：已启用、未暂停（暂停需先重新启用）。
#[must_use]
pub(crate) fn can_sync_now(logged_in: bool, sync: &SyncStatusDto) -> bool {
    logged_in && sync.enabled && !sync.halted
}

/// 相对时间：`(文案键, 数量)`；不足 1 分钟为「刚刚」（数量 0）。
#[must_use]
pub(crate) fn relative_time(now_unix_ms: i64, then_unix_ms: i64) -> (&'static str, i64) {
    let minutes = now_unix_ms.saturating_sub(then_unix_ms).max(0) / 60_000;
    match minutes {
        0 => ("cloudSyncTimeJustNow", 0),
        1..=59 => ("cloudSyncTimeMinutesAgo", minutes),
        60..=1439 => ("cloudSyncTimeHoursAgo", minutes / 60),
        _ => ("cloudSyncTimeDaysAgo", minutes / 1440),
    }
}

/// [`relative_time`] 的显示距下一次变化（跨入下一个分钟 / 小时 / 天档）的时长。
///
/// 相对时间不在任何被追踪的状态里：retained 渲染下必须按它定时 notify，否则一直停在旧值。
#[must_use]
pub(crate) fn relative_time_changes_in(now_unix_ms: i64, then_unix_ms: i64) -> Duration {
    let age = now_unix_ms.saturating_sub(then_unix_ms).max(0);
    let unit: i64 = match age / 60_000 {
        0..=59 => 60_000,
        60..=1439 => 3_600_000,
        _ => 86_400_000,
    };
    Duration::from_millis(u64::try_from(unit - age % unit).unwrap_or(60_000))
}

/// 全部非空分组（固定顺序）及各自的本机专属键计数。
#[must_use]
pub(crate) fn sync_groups(local_only_keys: &[String]) -> Vec<SyncGroupState> {
    SyncGroup::ALL
        .into_iter()
        .filter_map(|group| {
            let keys: Vec<&'static str> = SYNC_SETTING_SPECS
                .iter()
                .filter(|spec| spec.owner != SettingOwner::Excluded)
                .map(|spec| spec.key)
                .filter(|key| SyncGroup::of_key(key) == group)
                .collect();
            if keys.is_empty() {
                return None;
            }
            let local_only = keys
                .iter()
                .filter(|key| local_only_keys.iter().any(|local| local == *key))
                .count();
            Some(SyncGroupState {
                group,
                keys,
                local_only,
            })
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    fn owned(keys: &[&str]) -> Vec<String> {
        keys.iter().map(|key| (*key).to_owned()).collect()
    }

    #[test]
    fn every_catalog_key_lands_in_exactly_one_group() {
        let groups = sync_groups(&[]);
        let grouped: usize = groups.iter().map(|group| group.keys.len()).sum();
        let catalog = SYNC_SETTING_SPECS
            .iter()
            .filter(|spec| spec.owner != SettingOwner::Excluded)
            .count();
        assert_eq!(grouped, catalog);
        assert!(groups.iter().all(|group| group.all_synced()));
        // 分组顺序固定，且不产生空组。
        let order: Vec<_> = groups.iter().map(|group| group.group).collect();
        let mut sorted = order.clone();
        sorted.sort();
        assert_eq!(order, sorted);
        assert!(groups.iter().all(|group| !group.keys.is_empty()));
    }

    #[test]
    fn keys_are_grouped_by_prefix() {
        assert_eq!(SyncGroup::of_key("bt.enable_dht"), SyncGroup::Bt);
        assert_eq!(SyncGroup::of_key("ed2k.enable_kad"), SyncGroup::Ed2k);
        assert_eq!(SyncGroup::of_key("ui.show_sidebar_rss"), SyncGroup::Ui);
        assert_eq!(
            SyncGroup::of_key("download.speed_limit_bytes"),
            SyncGroup::Download
        );
        assert_eq!(
            SyncGroup::of_key("appearance.theme_mode"),
            SyncGroup::Appearance
        );
        assert_eq!(
            SyncGroup::of_key(CUSTOM_CATEGORIES_PREF_KEY),
            SyncGroup::Categories
        );
        assert_eq!(
            SyncGroup::of_key("categories.custom"),
            SyncGroup::Categories
        );
        assert_eq!(SyncGroup::of_key("something.else"), SyncGroup::Other);
    }

    #[test]
    fn local_only_keys_drive_group_state() {
        let local = owned(&["bt.enable_dht", "bt.enable_upnp"]);
        let groups = sync_groups(&local);
        let bt = groups
            .iter()
            .find(|group| group.group == SyncGroup::Bt)
            .expect("bt group");
        assert_eq!(bt.local_only, 2);
        assert!(bt.is_partial());
        assert!(!bt.all_synced());
        let appearance = groups
            .iter()
            .find(|group| group.group == SyncGroup::Appearance)
            .expect("appearance group");
        assert!(appearance.all_synced());
    }

    #[test]
    fn toggling_only_submits_keys_that_actually_change() {
        let local = owned(&["bt.enable_dht"]);
        let groups = sync_groups(&local);
        let bt = groups
            .iter()
            .find(|group| group.group == SyncGroup::Bt)
            .expect("bt group");
        // 关闭整组同步：已是本机专属的键不重复提交。
        let to_local = bt.keys_to_change(&local, true);
        assert!(!to_local.contains(&"bt.enable_dht".to_owned()));
        assert_eq!(to_local.len(), bt.keys.len() - 1);
        // 恢复同步：只提交本机专属的键。
        assert_eq!(bt.keys_to_change(&local, false), owned(&["bt.enable_dht"]));
    }

    fn status(enabled: bool) -> SyncStatusDto {
        SyncStatusDto {
            enabled,
            connected: true,
            ..SyncStatusDto::default()
        }
    }

    #[test]
    fn phase_follows_priority_disabled_halted_failed_connecting_syncing_synced() {
        assert_eq!(sync_phase(false, &status(true)), SyncPhase::Disabled);
        assert_eq!(sync_phase(true, &status(false)), SyncPhase::Disabled);

        let mut halted = status(true);
        halted.halted = true;
        halted.last_error_reason = Some(ErrorReason::OnlineDeviceLimit);
        assert_eq!(
            sync_phase(true, &halted),
            SyncPhase::Halted(Some(ErrorReason::OnlineDeviceLimit))
        );

        let mut failed = status(true);
        failed.last_error = Some("boom".to_owned());
        assert_eq!(sync_phase(true, &failed), SyncPhase::Failed(None));
        failed.last_error_reason = Some(ErrorReason::CloudUnreachable);
        assert_eq!(
            sync_phase(true, &failed),
            SyncPhase::Failed(Some(ErrorReason::CloudUnreachable))
        );

        let mut connecting = status(true);
        connecting.connected = false;
        assert_eq!(sync_phase(true, &connecting), SyncPhase::Connecting);

        let mut syncing = status(true);
        syncing.dirty_keys = vec!["ui.show_sidebar_rss".to_owned()];
        assert_eq!(sync_phase(true, &syncing), SyncPhase::Syncing);

        let mut synced = status(true);
        synced.last_synced_at_unix_ms = Some(42);
        assert_eq!(sync_phase(true, &synced), SyncPhase::Synced(Some(42)));
    }

    #[test]
    fn empty_error_text_is_not_a_failure() {
        let mut synced = status(true);
        synced.last_error = Some(String::new());
        assert_eq!(sync_phase(true, &synced), SyncPhase::Synced(None));
    }

    #[test]
    fn sync_now_needs_login_enabled_and_not_halted() {
        assert!(can_sync_now(true, &status(true)));
        assert!(!can_sync_now(false, &status(true)));
        assert!(!can_sync_now(true, &status(false)));
        let mut halted = status(true);
        halted.halted = true;
        assert!(!can_sync_now(true, &halted));
    }

    #[test]
    fn relative_time_buckets() {
        let now = 10_000_000_000;
        assert_eq!(
            relative_time(now, now - 30_000),
            ("cloudSyncTimeJustNow", 0)
        );
        assert_eq!(
            relative_time(now, now - 5 * 60_000),
            ("cloudSyncTimeMinutesAgo", 5)
        );
        assert_eq!(
            relative_time(now, now - 3 * 3_600_000),
            ("cloudSyncTimeHoursAgo", 3)
        );
        assert_eq!(
            relative_time(now, now - 2 * 86_400_000),
            ("cloudSyncTimeDaysAgo", 2)
        );
        // 时钟回拨（未来时间）按刚刚处理。
        assert_eq!(relative_time(now, now + 5_000), ("cloudSyncTimeJustNow", 0));
    }

    #[test]
    fn relative_time_changes_exactly_when_the_label_changes() {
        let then = 10_000_000_000;
        for age in [
            0,
            59_999,
            60_000,
            90_000,
            59 * 60_000 + 1,
            3_600_000,
            5 * 3_600_000 + 7,
            1439 * 60_000 + 30_000,
            86_400_000,
            3 * 86_400_000 + 1,
        ] {
            let now = then + age;
            let delay =
                i64::try_from(relative_time_changes_in(now, then).as_millis()).expect("delay fits");
            assert!(delay > 0, "age {age}");
            assert_eq!(
                relative_time(now + delay - 1, then),
                relative_time(now, then),
                "age {age}: label must hold until the deadline"
            );
            assert_ne!(
                relative_time(now + delay, then),
                relative_time(now, then),
                "age {age}: label must change at the deadline"
            );
        }
    }

    #[test]
    fn local_only_key_outside_catalog_is_ignored() {
        let local = owned(&["not.in.catalog"]);
        assert!(sync_groups(&local).iter().all(|group| group.all_synced()));
    }
}
