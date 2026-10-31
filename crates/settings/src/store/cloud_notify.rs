//! 云端推送：概览 / 上报偏好 / 投递记录第一页（均为 `AgentSnapshot.cloud_notify` 投影，由 agent 维护并
//! 实时推送）与投递记录的「加载更多」分页。
//!
//! 概览只经 `CloudNotifyChanged` 整体替换，本模块不自建缓存状态机；这些调用失败不写全局错误条
//! （`last_error`），原因由页面就地展示（`action_error` / `CloudDeliveryLog::error`）。

use fluxdown_protocol::{
    ApplicationErrorCode, CloudNotifyDeliveriesPage, CloudNotifyDeliveriesParams,
    CloudNotifyDeliveryDto, CloudNotifyPrivacyParams, CloudNotifyReportingParams,
    CloudNotifyStateDto, RpcErrorData, method,
};
use gpui::Context;
use serde_json::{Value, json};

use super::SettingsStore;
use crate::model::cloud_notify::{append_page, merge_deliveries};

const ACTION_REFRESH: &str = "cloudRefresh";
const ACTION_REPORTING: &str = "cloudReporting";
const ACTION_PRIVACY: &str = "cloudPrivacy";
const ACTION_DELIVERIES: &str = "cloudDeliveries";
/// 投递记录每页条数（协议缺省 50、上限 100）。
const LOG_PAGE_SIZE: u32 = 50;

/// 云端投递记录的「加载更多」状态（第一页不在这里：由 `cloud_notify().recent_deliveries` 提供）。
#[derive(Clone, Debug, Default)]
pub(crate) struct CloudDeliveryLog {
    /// 已追加的更早记录（按 id 去重、新的在前）。
    older: Vec<CloudNotifyDeliveryDto>,
    /// 最近一次「加载更多」返回的下一页游标：`None` = 还没翻过页（沿用快照里的游标），
    /// `Some(None)` = 已翻到底。
    older_cursor: Option<Option<String>>,
    /// 最近一次「加载更多」失败的原因；下一次成功后清空。
    pub(crate) error: Option<RpcErrorData>,
}

impl SettingsStore {
    /// 云端推送的本机状态（上报开关、隐私字段、云端概览）。
    #[must_use]
    pub(crate) fn cloud_notify(&self) -> &CloudNotifyStateDto {
        &self.cloud_notify
    }

    #[must_use]
    pub(crate) fn cloud_log(&self) -> &CloudDeliveryLog {
        &self.cloud_log
    }

    /// 最近一次上报开关 / 隐私设置失败的原因；下一次调用开始或成功时清空。
    #[must_use]
    pub(crate) fn cloud_action_error(&self) -> Option<&RpcErrorData> {
        self.cloud_action_error.as_ref()
    }

    /// 云端投递记录（列表顺序：新的在前）：agent 状态投影里的第一页快照 + 已加载的更早记录。
    #[must_use]
    pub(crate) fn cloud_deliveries(&self) -> Vec<CloudNotifyDeliveryDto> {
        merge_deliveries(&self.cloud_notify.recent_deliveries, &self.cloud_log.older)
    }

    /// 「加载更多」要用的游标：翻过页后取最近一页返回的（`None` = 已到底），没翻过取快照里的。
    #[must_use]
    pub(crate) fn cloud_deliveries_next_cursor(&self) -> Option<&str> {
        match &self.cloud_log.older_cursor {
            Some(cursor) => cursor.as_deref(),
            None => self.cloud_notify.recent_next_cursor.as_deref(),
        }
    }

    /// 会话结束：账号维度的概览、设备与投递记录一并失效（本机上报偏好与公开目录保留）。
    pub(super) fn clear_cloud_account(&mut self) {
        self.cloud_notify.overview = None;
        self.cloud_notify.recent_deliveries.clear();
        self.cloud_notify.recent_next_cursor = None;
        self.cloud_notify.loading = false;
        self.cloud_notify.last_error_reason = None;
        self.cloud_notify.updated_at_unix_ms = None;
        self.cloud_devices.clear();
        self.cloud_log = CloudDeliveryLog::default();
        self.cloud_action_error = None;
    }

    /// 发起云端推送 RPC：只维护 `busy`，不碰全局错误条。断线只读时静默跳过。
    fn cloud_call(
        &mut self,
        action: &'static str,
        method: &'static str,
        params: Value,
        cx: &mut Context<Self>,
        on_done: impl FnOnce(&mut Self, Result<Value, RpcErrorData>, &mut Context<Self>) + 'static,
    ) {
        if self.stale {
            return;
        }
        self.begin_busy(action, cx);
        cx.notify();
        let future = self.port.call(method, params);
        cx.spawn(async move |this, cx| {
            let result = future.await;

            let Ok(()) = this.update(cx, |this, cx| {
                this.busy.remove(action);
                on_done(this, result, cx);
                cx.notify();
            }) else {
                // 设置视图或窗口已释放，结束回调，不再更新状态。
                return;
            };
        })
        .detach();
    }

    /// 解析 agent 回执的 `CloudNotifyStateDto` 并整体替换；失败记入 `action_error`。
    fn absorb_cloud_state(&mut self, result: Result<Value, RpcErrorData>) {
        match result.and_then(|value| {
            serde_json::from_value::<CloudNotifyStateDto>(value)
                .map_err(|_| RpcErrorData::new(ApplicationErrorCode::Internal, false))
        }) {
            Ok(state) => {
                self.cloud_notify = state;
                self.cloud_action_error = None;
            }
            Err(error) => self.cloud_action_error = Some(error),
        }
    }

    /// 拉取概览：`force = false` 读 agent 缓存（过期时 agent 后台刷新并推送），`true` 立即向云端拉取。
    pub(crate) fn refresh_cloud_notify(&mut self, force: bool, cx: &mut Context<Self>) {
        if self.in_flight(ACTION_REFRESH) {
            return;
        }
        self.cloud_call(
            ACTION_REFRESH,
            if force {
                method::AGENT_CLOUD_NOTIFY_REFRESH
            } else {
                method::AGENT_CLOUD_NOTIFY_GET
            },
            json!({}),
            cx,
            |this, result, _| this.absorb_cloud_state(result),
        );
    }

    /// 本设备上报开关（设备本地偏好）。
    pub(crate) fn set_cloud_reporting(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.cloud_action_error = None;
        let params = serde_json::to_value(CloudNotifyReportingParams { enabled })
            .unwrap_or_else(|_| json!({}));
        self.cloud_call(
            ACTION_REPORTING,
            method::AGENT_CLOUD_NOTIFY_SET_REPORTING,
            params,
            cx,
            |this, result, _| this.absorb_cloud_state(result),
        );
    }

    /// 上报附带字段（下载地址 / 保存目录，设备本地偏好）。
    pub(crate) fn set_cloud_privacy(
        &mut self,
        include_url: bool,
        include_save_dir: bool,
        cx: &mut Context<Self>,
    ) {
        self.cloud_action_error = None;
        let params = serde_json::to_value(CloudNotifyPrivacyParams {
            include_url,
            include_save_dir,
        })
        .unwrap_or_else(|_| json!({}));
        self.cloud_call(
            ACTION_PRIVACY,
            method::AGENT_CLOUD_NOTIFY_SET_PRIVACY,
            params,
            cx,
            |this, result, _| this.absorb_cloud_state(result),
        );
    }

    /// 投递记录第一页以 agent 状态投影里的 `recent_deliveries` 为准（实时更新，不在这里拉取）；
    /// 这里只负责「加载更多」：按游标追加更早的记录，已出现在快照里的按 id 去重。
    pub(crate) fn load_more_cloud_deliveries(&mut self, cx: &mut Context<Self>) {
        if self.stale || self.in_flight(ACTION_DELIVERIES) {
            return;
        }
        let Some(cursor) = self.cloud_deliveries_next_cursor().map(str::to_owned) else {
            return;
        };
        let params = serde_json::to_value(CloudNotifyDeliveriesParams {
            limit: Some(LOG_PAGE_SIZE),
            before: Some(cursor),
        })
        .unwrap_or_else(|_| json!({}));
        self.cloud_call(
            ACTION_DELIVERIES,
            method::AGENT_CLOUD_NOTIFY_DELIVERIES,
            params,
            cx,
            |this, result, _| this.absorb_cloud_older_page(result),
        );
    }

    fn absorb_cloud_older_page(&mut self, result: Result<Value, RpcErrorData>) {
        match result.and_then(|value| {
            serde_json::from_value::<CloudNotifyDeliveriesPage>(value)
                .map_err(|_| RpcErrorData::new(ApplicationErrorCode::Internal, false))
        }) {
            Ok(page) => {
                append_page(&mut self.cloud_log.older, page.items);
                self.cloud_log.older_cursor = Some(page.next_cursor);
                self.cloud_log.error = None;
            }
            Err(error) => self.cloud_log.error = Some(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use fluxdown_protocol::{CloudNotifyOverviewDto, ErrorReason};

    use super::*;
    use crate::port::{PortFuture, SettingsPort};

    struct NullPort;

    impl SettingsPort for NullPort {
        fn call(&self, _method: &'static str, _params: Value) -> PortFuture<Value> {
            Box::pin(async { Ok(Value::Null) })
        }
    }

    fn delivery(id: &str) -> CloudNotifyDeliveryDto {
        CloudNotifyDeliveryDto {
            id: id.to_owned(),
            ..Default::default()
        }
    }

    fn ids(store: &SettingsStore) -> Vec<String> {
        store
            .cloud_deliveries()
            .into_iter()
            .map(|item| item.id)
            .collect()
    }

    #[test]
    fn session_end_clears_account_scope_but_keeps_local_preferences() {
        let mut store = SettingsStore::new(Arc::new(NullPort));
        store.cloud_notify = CloudNotifyStateDto {
            reporting: true,
            include_url: true,
            include_save_dir: false,
            catalog: Some(vec![fluxdown_protocol::CloudNotifyKindDto {
                kind: "email".to_owned(),
                available: true,
            }]),
            overview: Some(CloudNotifyOverviewDto::default()),
            loading: true,
            last_error_reason: Some(ErrorReason::CloudUnreachable),
            updated_at_unix_ms: Some(1),
            recent_deliveries: vec![delivery("r")],
            recent_next_cursor: Some("c".to_owned()),
        };
        store.cloud_log.older.push(delivery("a"));
        store.cloud_log.older_cursor = Some(None);
        store.cloud_action_error = Some(RpcErrorData::new(ApplicationErrorCode::Internal, false));

        store.clear_cloud_account();

        let state = store.cloud_notify();
        assert!(state.reporting && state.include_url && !state.include_save_dir);
        assert!(state.overview.is_none() && !state.loading);
        // 目录是公开信息，登出不清。
        assert_eq!(state.catalog.as_ref().map(Vec::len), Some(1));
        assert!(state.last_error_reason.is_none() && state.updated_at_unix_ms.is_none());
        assert!(state.recent_deliveries.is_empty() && state.recent_next_cursor.is_none());
        assert!(store.cloud_deliveries().is_empty());
        assert!(store.cloud_deliveries_next_cursor().is_none());
        assert!(store.cloud_action_error().is_none());
    }

    fn page(items: &[&str], cursor: Option<&str>) -> Value {
        serde_json::to_value(CloudNotifyDeliveriesPage {
            items: items.iter().map(|id| delivery(id)).collect(),
            next_cursor: cursor.map(str::to_owned),
        })
        .unwrap_or_default()
    }

    #[test]
    fn first_page_is_the_snapshot_and_cursor_comes_from_it() {
        let mut store = SettingsStore::new(Arc::new(NullPort));
        store.cloud_notify.recent_deliveries = vec![delivery("b"), delivery("a")];
        store.cloud_notify.recent_next_cursor = Some("c1".to_owned());
        assert_eq!(ids(&store), ["b", "a"]);
        assert_eq!(store.cloud_deliveries_next_cursor(), Some("c1"));

        // 快照实时更新：新记录出现在前面，列表立即跟随（没有自建首页缓存）。
        store
            .cloud_notify
            .recent_deliveries
            .insert(0, delivery("c"));
        assert_eq!(ids(&store), ["c", "b", "a"]);
    }

    #[test]
    fn load_more_appends_older_page_dedupes_and_advances_cursor() {
        let mut store = SettingsStore::new(Arc::new(NullPort));
        store.cloud_notify.recent_deliveries = vec![delivery("b"), delivery("a")];
        store.cloud_notify.recent_next_cursor = Some("c1".to_owned());

        store.absorb_cloud_older_page(Ok(page(&["a", "z"], Some("c2"))));
        assert_eq!(ids(&store), ["b", "a", "z"]);
        assert_eq!(store.cloud_deliveries_next_cursor(), Some("c2"));

        // 翻到底：游标为空后不再有「更多」，即使快照里还留着旧游标。
        store.absorb_cloud_older_page(Ok(page(&["y"], None)));
        assert_eq!(ids(&store), ["b", "a", "z", "y"]);
        assert_eq!(store.cloud_deliveries_next_cursor(), None);

        // 之后快照里出现了已加载过的旧记录：以快照为准、不重复。
        store.cloud_notify.recent_deliveries = vec![delivery("z"), delivery("b"), delivery("a")];
        assert_eq!(ids(&store), ["z", "b", "a", "y"]);
    }

    #[test]
    fn failed_load_more_keeps_items_and_records_error_until_next_success() {
        let mut store = SettingsStore::new(Arc::new(NullPort));
        store.cloud_notify.recent_deliveries = vec![delivery("a")];
        store.cloud_notify.recent_next_cursor = Some("c1".to_owned());
        let error = RpcErrorData::new(ApplicationErrorCode::Internal, true)
            .with_reason(ErrorReason::CloudUnreachable);
        store.absorb_cloud_older_page(Err(error.clone()));
        assert_eq!(ids(&store), ["a"]);
        assert_eq!(store.cloud_log().error, Some(error));
        // 失败不推进游标，可重试。
        assert_eq!(store.cloud_deliveries_next_cursor(), Some("c1"));
        store.absorb_cloud_older_page(Ok(page(&["z"], None)));
        assert!(store.cloud_log().error.is_none());
        assert_eq!(ids(&store), ["a", "z"]);
    }

    #[test]
    fn state_receipt_replaces_wholesale_and_garbage_is_an_error() {
        let mut store = SettingsStore::new(Arc::new(NullPort));
        store.absorb_cloud_state(Ok(
            json!({ "reporting": true, "includeUrl": true, "includeSaveDir": false }),
        ));
        assert!(store.cloud_notify().reporting && store.cloud_notify().include_url);
        assert!(store.cloud_action_error().is_none());

        store.absorb_cloud_state(Ok(json!("nope")));
        assert!(store.cloud_action_error().is_some());
        // 整体替换失败时保留旧状态。
        assert!(store.cloud_notify().reporting);
    }
}
