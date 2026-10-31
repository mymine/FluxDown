//! FluxMesh额度与贡献（`GET /share/usage`）的投影：只在事件到来时拉取，不设定时器。

use std::time::{Duration, Instant};

use fluxdown_protocol::{ErrorReason, RpcErrorData, ShareUsageDto};

use super::cloud::share_error;
use super::service::ShareService;

/// `usageRefresh` 在这段时间内复用缓存，与云端推送的 `CACHE_TTL` 口径一致。
const USAGE_TTL: Duration = Duration::from_secs(30);
/// 接收完成后的合并窗口：一批文件完成只拉一次。
const USAGE_DEBOUNCE: Duration = Duration::from_secs(5);

impl ShareService {
    /// 拉取并发布额度。未登录清空；失败只写 `usage_error`、保留旧值。本机开关关闭时不请求。
    pub(super) async fn refresh_usage(&self) {
        if !self.is_enabled() {
            return;
        }
        if !self.cloud.has_account().await {
            self.with(|i| {
                i.usage = None;
                i.usage_error = None;
                i.usage_fetched_at = None;
            });
            self.publish().await;
            return;
        }
        match self.cloud.share_usage::<ShareUsageDto>().await {
            Ok(usage) => self.with(|i| {
                i.usage = Some(usage);
                i.usage_error = None;
                i.usage_fetched_at = Some(Instant::now());
            }),
            Err(error) => {
                // 没有细分原因的失败（如 5xx）也必须置位，UI 据此提示加载失败。
                let reason = share_error(&error)
                    .reason
                    .unwrap_or(ErrorReason::ShareUnavailable);
                tracing::debug!(?reason, "share usage refresh failed");
                self.with(|i| i.usage_error = Some(reason));
            }
        }
        self.publish().await;
    }

    /// `agent.share.usageRefresh`：30 s 内返回缓存，否则先拉取。
    pub(super) async fn usage_for_rpc(&self) -> Result<Option<ShareUsageDto>, RpcErrorData> {
        let fresh = self.with(|i| {
            i.usage_fetched_at
                .is_some_and(|at| at.elapsed() < USAGE_TTL)
                .then(|| i.usage.clone())
        });
        if let Some(cached) = fresh {
            return Ok(cached);
        }
        self.refresh_usage().await;
        Ok(self.with(|i| i.usage.clone()))
    }

    /// 5 秒防抖的刷新：接收记录完成后调用。
    pub(super) fn schedule_usage_refresh(&self) {
        if self.with(|i| std::mem::replace(&mut i.usage_debounce, true)) {
            return;
        }
        let weak = self.weak.clone();
        tokio::spawn(async move {
            tokio::time::sleep(USAGE_DEBOUNCE).await;
            let Some(service) = weak.upgrade() else {
                return;
            };
            service.with(|i| i.usage_debounce = false);
            service.refresh_usage().await;
        });
    }

    /// 订单支付成功后由网关调用：立即刷新（不等待防抖）。
    pub async fn on_order_paid(&self) {
        self.refresh_usage().await;
    }
}

#[cfg(test)]
mod tests {
    use fluxdown_protocol::{ErrorReason, method};
    use serde_json::{Value, json};

    use super::super::tests::{call, node, start_mock};

    fn body(remaining: u64) -> Value {
        json!({
            "month": "2026-10",
            "resetAt": "2026-11-01T00:00:00Z",
            "enforced": { "traffic": false, "swarm": false },
            "kinds": [{
                "kind": "swarm", "unlimited": false, "monthlyAllowance": 100,
                "usedThisMonth": 1, "remaining": remaining, "reserved": 0
            }],
            "grants": [],
            "contribution": {}
        })
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn usage_projects_refreshes_and_keeps_old_value_on_failure() {
        let (base, mock) = start_mock().await;

        // 未登录：没有额度，也不请求云端。
        let guest = node(&base, "usage_guest", false, vec![]).await;
        let value: Value = call(&guest, method::AGENT_SHARE_USAGE_REFRESH, json!({}))
            .await
            .unwrap();
        assert!(value.is_null());
        guest.cancel.cancel();

        // 登录：拉取成功后写入状态。
        *mock.usage.lock().unwrap() = Some(body(99));
        let user = node(&base, "usage_user", true, vec![]).await;
        let value: Value = call(&user, method::AGENT_SHARE_USAGE_REFRESH, json!({}))
            .await
            .unwrap();
        assert_eq!(value["kinds"][0]["remaining"], json!(99));
        let state = user.service.with(|i| i.state_dto(true));
        assert_eq!(
            state.usage.as_ref().map(|u| u.month.as_str()),
            Some("2026-10")
        );
        assert_eq!(state.usage_error, None);

        // 30 s 内复用缓存：云端改了也不变。
        *mock.usage.lock().unwrap() = Some(body(5));
        let cached: Value = call(&user, method::AGENT_SHARE_USAGE_REFRESH, json!({}))
            .await
            .unwrap();
        assert_eq!(cached["kinds"][0]["remaining"], json!(99));

        // 拉取失败：保留旧值并写 usage_error。
        *mock.usage.lock().unwrap() = None;
        user.service.refresh_usage().await;
        let state = user.service.with(|i| i.state_dto(true));
        assert_eq!(state.usage.as_ref().map(|u| u.kinds[0].remaining), Some(99));
        assert!(state.usage_error.is_some());
        assert_ne!(state.usage_error, Some(ErrorReason::ShareNotFound));

        // 恢复后清空错误。
        *mock.usage.lock().unwrap() = Some(body(5));
        user.service.refresh_usage().await;
        let state = user.service.with(|i| i.state_dto(true));
        assert_eq!(state.usage_error, None);
        assert_eq!(state.usage.map(|u| u.kinds[0].remaining), Some(5));
        user.cancel.cancel();
    }
}
