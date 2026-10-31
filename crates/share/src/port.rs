//! FluxMesh能力的 agent 端口：宿主只需实现一个 `call`，类型化的 `agent.share.*` 封装在 [`ShareApi`]。

use std::{future::Future, pin::Pin, sync::Arc};

use fluxdown_protocol::{
    ApplicationErrorCode, RpcErrorData, ShareAppealParams, ShareConfigDto,
    ShareConvertToCloudParams, ShareCreateParams, ShareDiagnosticsDto, ShareDiagnosticsParams,
    ShareDismissPendingParams, ShareDto, ShareIdParams, ShareLocalConfigDto, ShareLogsDto,
    ShareLogsParams, ShareReceiveParams, ShareReceiveRecordDto, ShareReceiveResultDto,
    ShareReportParams, ShareResolveDto, ShareResolveParams, ShareSessionKillParams,
    ShareStopProvidingParams, ShareSwitchToBridgeParams, ShareTaskFactsDto, ShareUpdateParams,
    ShareUsageDto, method,
};
use serde::{Serialize, de::DeserializeOwned};

pub type PortFuture<T> = Pin<Box<dyn Future<Output = Result<T, RpcErrorData>> + Send + 'static>>;

/// 宿主注入的 agent 通道。
pub trait SharePort: Send + Sync {
    fn call(
        &self,
        method: &'static str,
        params: serde_json::Value,
    ) -> PortFuture<serde_json::Value>;
}

/// 类型化的 `agent.share.*` 调用。克隆廉价（共享同一端口）。
#[derive(Clone)]
pub struct ShareApi {
    port: Arc<dyn SharePort>,
}

/// 本地编解码失败：agent 与 UI 的 wire 不一致，按内部错误上报。
fn wire_error() -> RpcErrorData {
    RpcErrorData::new(ApplicationErrorCode::Internal, false)
}

impl ShareApi {
    pub fn new(port: Arc<dyn SharePort>) -> Self {
        Self { port }
    }

    fn invoke<P, R>(&self, method: &'static str, params: &P) -> PortFuture<R>
    where
        P: Serialize,
        R: DeserializeOwned + Send + 'static,
    {
        let params = match serde_json::to_value(params) {
            Ok(params) => params,
            Err(_) => return Box::pin(std::future::ready(Err(wire_error()))),
        };
        let call = self.port.call(method, params);
        Box::pin(async move {
            let value = call.await?;
            serde_json::from_value(value).map_err(|_| wire_error())
        })
    }

    fn invoke_empty<R>(&self, method: &'static str) -> PortFuture<R>
    where
        R: DeserializeOwned + Send + 'static,
    {
        self.invoke(method, &serde_json::json!({}))
    }

    pub fn config(&self) -> PortFuture<ShareConfigDto> {
        self.invoke_empty(method::AGENT_SHARE_CONFIG)
    }

    /// 整体替换本机FluxMesh设置。
    pub fn set_local_config(
        &self,
        config: &ShareLocalConfigDto,
    ) -> PortFuture<ShareLocalConfigDto> {
        self.invoke(method::AGENT_SHARE_SET_LOCAL_CONFIG, config)
    }

    pub fn create(&self, params: &ShareCreateParams) -> PortFuture<ShareDto> {
        self.invoke(method::AGENT_SHARE_CREATE, params)
    }

    pub fn list(&self) -> PortFuture<Vec<ShareDto>> {
        self.invoke_empty(method::AGENT_SHARE_LIST)
    }

    pub fn update(&self, params: &ShareUpdateParams) -> PortFuture<ShareDto> {
        self.invoke(method::AGENT_SHARE_UPDATE, params)
    }

    pub fn stop(&self, id: &str) -> PortFuture<ShareDto> {
        self.invoke(
            method::AGENT_SHARE_STOP,
            &ShareIdParams { id: id.to_owned() },
        )
    }

    /// 恢复已停止 / 已过期的分享（同一分享，重新分配取件码）。
    pub fn resume(&self, id: &str) -> PortFuture<ShareDto> {
        self.invoke(
            method::AGENT_SHARE_RESUME,
            &ShareIdParams { id: id.to_owned() },
        )
    }

    /// 删除分享记录；分享中的先停止。
    pub fn delete(&self, id: &str) -> PortFuture<serde_json::Value> {
        self.invoke(
            method::AGENT_SHARE_DELETE,
            &ShareIdParams { id: id.to_owned() },
        )
    }

    pub fn resolve(&self, params: &ShareResolveParams) -> PortFuture<ShareResolveDto> {
        self.invoke(method::AGENT_SHARE_RESOLVE, params)
    }

    pub fn receive(&self, params: &ShareReceiveParams) -> PortFuture<ShareReceiveResultDto> {
        self.invoke(method::AGENT_SHARE_RECEIVE, params)
    }

    /// 取消 agent 持有的待确认接收（`pendingReceive`）。
    pub fn dismiss_pending(&self, resolve_id: &str) -> PortFuture<serde_json::Value> {
        self.invoke(
            method::AGENT_SHARE_DISMISS_PENDING,
            &ShareDismissPendingParams {
                resolve_id: resolve_id.to_owned(),
            },
        )
    }

    pub fn switch_to_bridge(&self, task_id: &str) -> PortFuture<ShareTaskFactsDto> {
        self.invoke(
            method::AGENT_SHARE_SWITCH_TO_BRIDGE,
            &ShareSwitchToBridgeParams {
                task_id: task_id.to_owned(),
            },
        )
    }

    /// 停止该次接收的共享（共享提供）；幂等，返回带最新 `provideState` 的记录。
    pub fn stop_providing(&self, record_id: &str) -> PortFuture<ShareReceiveRecordDto> {
        self.invoke(
            method::AGENT_SHARE_STOP_PROVIDING,
            &ShareStopProvidingParams {
                record_id: record_id.to_owned(),
            },
        )
    }

    /// 把实时分享转为云端暂存并开始上传；返回时 `cloudState` 已是上传中，进度经 `ShareChanged` 回流。
    /// `tier_id` 取自策略的 `retentionTiers`，`None` 取云端默认档。
    pub fn convert_to_cloud(&self, id: &str, tier_id: Option<&str>) -> PortFuture<ShareDto> {
        self.invoke(
            method::AGENT_SHARE_CONVERT_TO_CLOUD,
            &ShareConvertToCloudParams {
                id: id.to_owned(),
                tier_id: tier_id.map(str::to_owned),
            },
        )
    }

    /// 删除云端副本；上传进行中则取消上传。
    pub fn delete_cloud_copy(&self, id: &str) -> PortFuture<ShareDto> {
        self.invoke(
            method::AGENT_SHARE_DELETE_CLOUD_COPY,
            &ShareIdParams { id: id.to_owned() },
        )
    }

    pub fn diagnostics(&self, params: &ShareDiagnosticsParams) -> PortFuture<ShareDiagnosticsDto> {
        self.invoke(method::AGENT_SHARE_DIAGNOSTICS, params)
    }

    pub fn logs(&self, params: &ShareLogsParams) -> PortFuture<ShareLogsDto> {
        self.invoke(method::AGENT_SHARE_LOGS, params)
    }

    pub fn session_kill(&self, session_id: &str) -> PortFuture<serde_json::Value> {
        self.invoke(
            method::AGENT_SHARE_SESSION_KILL,
            &ShareSessionKillParams {
                session_id: session_id.to_owned(),
            },
        )
    }

    /// 立即向云端取最新额度与贡献并写入 `ShareStateDto.usage`；未登录返回 `None`。
    pub fn usage_refresh(&self) -> PortFuture<Option<ShareUsageDto>> {
        self.invoke_empty(method::AGENT_SHARE_USAGE_REFRESH)
    }

    /// 举报一份已领取的分享；重复举报静默成功。
    pub fn report(&self, params: &ShareReportParams) -> PortFuture<serde_json::Value> {
        self.invoke(method::AGENT_SHARE_REPORT, params)
    }

    /// 对已下架的分享申诉；返回带申诉记录的分享。
    pub fn appeal(&self, params: &ShareAppealParams) -> PortFuture<ShareDto> {
        self.invoke(method::AGENT_SHARE_APPEAL, params)
    }
}
