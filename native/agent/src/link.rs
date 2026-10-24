//! 局域网设备互联（L1）与 daemon legacy 数据的一次性导入。
//!
//! - [`LinkService`]（`link/service.rs`）：装配 `fluxdown_link::LinkManager`，实现响应端路由、
//!   发现 / 配对 / 下发 / 在线探测与全部 `agent.link.*` 方法。
//! - [`AgentLinkStorage`]（`link/storage.rs`）：本机身份与已配对名册落在 agent 私有状态文件。
//! - 本文件：旧宿主（daemon）里的身份 / 名册 / UI Gateway 设置只在首次启动时迁入一次。

mod service;
mod storage;
#[cfg(test)]
mod tests;

use std::sync::Arc;

use fluxdown_protocol::{
    AgentEvent, GatewayMigrationExport, LinkMigrationExport, MigrationAckParams,
};
use serde_json::Value;
use tokio::sync::Mutex;

use crate::daemon_client::DaemonClient;
use crate::event_hub::AgentEventHub;
use crate::state::{AgentState, StateStore};

pub(crate) use service::refresh_request_ttls;
pub use service::{
    DaemonTaskCreator, ErrorContext, LinkNetworkOptions, LinkOpError, LinkService,
    LinkServiceParts, LinkTaskCreator, lan_base_urls, resolve_receive_dir, rpc_error, rpc_value,
    valid_file_name,
};
pub use storage::{AgentLinkStorage, device_info, public_devices};

pub async fn migrate_legacy_state(
    daemon: &DaemonClient,
    state: &Arc<Mutex<AgentState>>,
    store: &StateStore,
    events: &AgentEventHub,
) -> Result<(), LinkMigrationError> {
    migrate_link(daemon, state, store, events).await?;
    migrate_gateway(daemon, state, store, events).await
}

/// 导入 daemon 时代的设备身份与已配对名册。daemon 里没有身份（全新安装）时什么都不写：
/// 身份由 [`LinkService::start`] 在迁移完成后首次生成，避免与迁移互相覆盖。
async fn migrate_link(
    daemon: &DaemonClient,
    state: &Arc<Mutex<AgentState>>,
    store: &StateStore,
    events: &AgentEventHub,
) -> Result<(), LinkMigrationError> {
    if state.lock().await.link_migration_revision.is_some() {
        return Ok(());
    }
    let export = daemon
        .call::<Value, LinkMigrationExport>(
            fluxdown_protocol::method::DAEMON_MIGRATION_LINK_EXPORT,
            None,
        )
        .await;
    let export = match export {
        Ok(export) => export,
        Err(error) if error.code == fluxdown_protocol::ApplicationErrorCode::NotFound => {
            return Ok(());
        }
        Err(error) => return Err(LinkMigrationError::Daemon(error)),
    };
    {
        let mut state = state.lock().await;
        state.link_identity = (!export.identity.is_null()).then_some(export.identity);
        state.linked_devices = export.roster;
        state.link_migration_revision = Some(export.revision);
        store.save(&state).await?;
        events.publish(AgentEvent::LinkedDevicesChanged(public_devices(&state)));
    }
    let _: Value = daemon
        .call(
            fluxdown_protocol::method::DAEMON_MIGRATION_LINK_ACK,
            Some(MigrationAckParams {
                revision: export.revision,
            }),
        )
        .await
        .map_err(LinkMigrationError::Daemon)?;
    Ok(())
}

async fn migrate_gateway(
    daemon: &DaemonClient,
    state: &Arc<Mutex<AgentState>>,
    store: &StateStore,
    events: &AgentEventHub,
) -> Result<(), LinkMigrationError> {
    if state.lock().await.gateway_migration_revision.is_some() {
        return Ok(());
    }
    let export = daemon
        .call::<Value, GatewayMigrationExport>(
            fluxdown_protocol::method::DAEMON_MIGRATION_GATEWAY_EXPORT,
            None,
        )
        .await;
    let export = match export {
        Ok(export) => export,
        Err(error) if error.code == fluxdown_protocol::ApplicationErrorCode::NotFound => {
            return Ok(());
        }
        Err(error) => return Err(LinkMigrationError::Daemon(error)),
    };
    {
        let mut state = state.lock().await;
        state.gateway.takeover_enabled = export.takeover_enabled;
        state.gateway.jsonrpc_enabled = export.jsonrpc_enabled;
        state.gateway.api_enabled = export.api_enabled;
        state.gateway.mcp_enabled = export.mcp_enabled;
        state.gateway.cors_enabled = export.cors_enabled;
        state.gateway.user_token_configured = export.user_token_configured;
        state.gateway_user_token.clone_from(&export.user_token);
        state.gateway_migration_revision = Some(export.revision);
        store.save(&state).await?;
        events.publish(AgentEvent::GatewayChanged(state.gateway.clone()));
    }
    let _: Value = daemon
        .call(
            fluxdown_protocol::method::DAEMON_MIGRATION_GATEWAY_ACK,
            Some(MigrationAckParams {
                revision: export.revision,
            }),
        )
        .await
        .map_err(LinkMigrationError::Daemon)?;
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum LinkMigrationError {
    #[error("daemon migration RPC failed: {0:?}")]
    Daemon(fluxdown_protocol::RpcErrorData),
    #[error(transparent)]
    State(#[from] crate::state::StateError),
}
