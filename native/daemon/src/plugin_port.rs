//! 常驻插件宿主的 `flux.tasks.*` 出口：把插件的任务操作转成对 daemon 服务的进程内调用。
//!
//! 只放行 `list / get / create / pause / resume / delete` 六个方法；插件拿不到别的
//! daemon 方法。调用走与外部 RPC 相同的 `call_inner`，因此参数校验、演示模式限制、
//! 事件发布与外部客户端完全一致。

use std::sync::Weak;

use fluxdown_engine::plugin::{PluginError, PluginTaskPort};
use fluxdown_protocol::method;
use serde_json::Value;

use crate::service::DaemonService;

/// `call_inner` 的连接标识：审计 / 日志里区分插件宿主发起的调用。
const CONNECTION_ID: &str = "plugin-host";

/// `flux.tasks.*` 允许调用的 daemon 方法。
const ALLOWED_METHODS: [&str; 6] = [
    method::DAEMON_TASK_LIST,
    method::DAEMON_TASK_GET,
    method::DAEMON_TASK_CREATE,
    method::DAEMON_TASK_PAUSE,
    method::DAEMON_TASK_RESUME,
    method::DAEMON_TASK_DELETE,
];

/// 弱引用 [`DaemonService`]：宿主由引擎持有、服务由 daemon 持有，避免互相强引用成环。
pub struct DaemonPluginPort {
    service: Weak<DaemonService>,
}

impl DaemonPluginPort {
    #[must_use]
    pub fn new(service: Weak<DaemonService>) -> Self {
        Self { service }
    }
}

#[async_trait::async_trait]
impl PluginTaskPort for DaemonPluginPort {
    async fn call(&self, method_name: &str, params: Value) -> Result<Value, PluginError> {
        if !ALLOWED_METHODS.contains(&method_name) {
            return Err(PluginError::Runtime(format!(
                "method not allowed for plugins: {method_name}"
            )));
        }
        let service = self
            .service
            .upgrade()
            .ok_or_else(|| PluginError::Runtime("daemon is shutting down".to_string()))?;
        service
            .call_inner(CONNECTION_ID, true, method_name, Some(params))
            .await
            .map_err(|error| PluginError::Runtime(error.message))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::sync::Weak;

    use fluxdown_engine::plugin::{PluginError, PluginTaskPort};
    use serde_json::json;

    use super::DaemonPluginPort;

    #[tokio::test]
    async fn rejects_methods_outside_the_task_whitelist() {
        let port = DaemonPluginPort::new(Weak::new());
        let error = port
            .call("daemon.plugin.uninstall", json!({ "identity": "a@b" }))
            .await
            .expect_err("non-task methods are rejected");
        assert!(
            matches!(&error, PluginError::Runtime(message) if message.contains("not allowed")),
            "unexpected error: {error:?}"
        );
    }

    #[tokio::test]
    async fn whitelisted_method_without_service_reports_shutdown() {
        let port = DaemonPluginPort::new(Weak::new());
        let error = port
            .call("daemon.task.list", json!({}))
            .await
            .expect_err("service is gone");
        assert!(
            matches!(&error, PluginError::Runtime(message) if message.contains("shutting down"))
        );
    }
}
