//! 已装插件的市场撤回定期触发。
//!
//! daemon 在 `daemon.plugin.marketList` 拉到索引时会把被标记 vulnerable / malicious 的
//! 已装插件版本强制禁用；本模块只负责在后台低频触发一次这条链路，让用户即使不打开
//! 市场页也能收到撤回。

use std::sync::Arc;
use std::time::Duration;

use fluxdown_protocol::{ApplicationErrorCode, PluginDto, method};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::daemon_client::DaemonClient;

/// 启动后的首次等待：避开启动高峰。
const INITIAL_DELAY: Duration = Duration::from_secs(120);
/// 之后的检查间隔。
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// 启动后台撤回检查任务（桌面 / server 宿主；embedded 不调用）。
pub fn spawn(daemon: Arc<DaemonClient>, cancel: CancellationToken) {
    tokio::spawn(async move {
        if !sleep_or_cancel(INITIAL_DELAY, &cancel).await {
            return;
        }
        loop {
            if !check_once(&daemon).await {
                return;
            }
            if !sleep_or_cancel(CHECK_INTERVAL, &cancel).await {
                return;
            }
        }
    });
}

/// 返回 `false` 表示被取消。
async fn sleep_or_cancel(duration: Duration, cancel: &CancellationToken) -> bool {
    tokio::select! {
        () = cancel.cancelled() => false,
        () = tokio::time::sleep(duration) => true,
    }
}

/// 执行一轮检查；返回 `false` 表示 daemon 不支持插件，应结束循环。
async fn check_once(daemon: &DaemonClient) -> bool {
    let plugins = match daemon
        .call::<Value, Vec<PluginDto>>(method::DAEMON_PLUGIN_LIST, None)
        .await
    {
        Ok(plugins) => plugins,
        Err(error) if error.code == ApplicationErrorCode::Unsupported => return false,
        Err(error) => {
            tracing::debug!(code = ?error.code, "plugin revocation check: list failed");
            return true;
        }
    };
    if !has_marketable_plugin(&plugins) {
        return true;
    }
    if let Err(error) = daemon
        .call::<Value, Value>(method::DAEMON_PLUGIN_MARKET_LIST, None)
        .await
    {
        tracing::debug!(code = ?error.code, "plugin revocation check: market list failed");
    }
    true
}

/// 只有存在非 dev 的已装插件时才值得联网拉索引。
fn has_marketable_plugin(plugins: &[PluginDto]) -> bool {
    plugins.iter().any(|plugin| !plugin.dev_mode)
}

#[cfg(test)]
mod tests {
    use super::has_marketable_plugin;
    use fluxdown_protocol::PluginDto;

    fn dto(dev_mode: bool) -> PluginDto {
        PluginDto {
            identity: "a@b".into(),
            name: "a".into(),
            version: "1.0.0".into(),
            description: String::new(),
            homepage: String::new(),
            enabled: true,
            dev_mode,
            disabled_reason: "None".into(),
            settings: Vec::new(),
            settings_values: std::collections::HashMap::new(),
            permissions: Vec::new(),
            auth_supported: false,
            subscription_provider_ids: Vec::new(),
            resolver_urls: Vec::new(),
            load_status: "Loaded".into(),
            load_error: String::new(),
            contributions: fluxdown_protocol::PluginContributionsDto::default(),
            risk: "standard".to_owned(),
            consent_pending: Vec::new(),
            consent_pending_hosts: Vec::new(),
            network_hosts: Vec::new(),
            package_dir: None,
            package_revision: 0,
        }
    }

    #[test]
    fn only_dev_plugins_skip_market_fetch() {
        assert!(!has_marketable_plugin(&[]));
        assert!(!has_marketable_plugin(&[dto(true)]));
        assert!(has_marketable_plugin(&[dto(true), dto(false)]));
    }
}
