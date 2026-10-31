//! `agent.share.*` 的业务操作与 RPC 分发。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use fluxdown_protocol::{
    ApplicationErrorCode, CreateTaskRequest, ErrorReason, RpcErrorData, ShareConfigDto,
    ShareCreateParams, ShareDiagnosticsDto, ShareDiagnosticsParams, ShareDismissPendingParams,
    ShareDto, ShareFileDto, ShareHashingDto, ShareIdParams, ShareLocalConfigDto, ShareLogLevel,
    ShareLogsDto, ShareLogsParams, ShareReceiveParams, ShareReceiveRecordDto,
    ShareReceiveRecordsPage, ShareReceiveRecordsParams, ShareReceiveResultDto, ShareReceiveStatus,
    ShareResolveDto, ShareResolveParams, ShareSessionKillParams, ShareSourcesDto, ShareStatus,
    ShareStopProvidingParams, ShareSwitchToBridgeParams, ShareTaskFactsDto, ShareUpdateParams,
    method,
};
use fluxdown_share::{
    EndpointId, LocalAdapter, ReceiveParams, ReceiveSession, ShareFile, ShareManifest, SourceKind,
    SourcePreference,
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::cloud::{
    CloudShare, CreateBody, ResolveResponse, parse_code_or_link, safe_file_name, share_error,
};
use super::lifecycle::apply_cloud;
use super::service::{
    Pending, RESOLVE_CAP, RESOLVE_TTL, ReceiveTask, ShareService, internal, invalid, unavailable,
};
use super::source::{BridgeSource, CloudCopySource, bridge_switch_error};
use super::store::{StoredProvide, StoredShare};
use super::swarm::{FIRST_LOOKUP_TIMEOUT, LOOKUP_RETRY, Lookup};
use super::upload_job::sources_intact;
use super::{convert, logs};
use crate::share_link::ShareLinkSink;

fn parse<T: DeserializeOwned>(params: Option<Value>) -> Result<T, RpcErrorData> {
    serde_json::from_value(params.unwrap_or_else(|| json!({})))
        .map_err(|_| RpcErrorData::new(ApplicationErrorCode::InvalidArgument, false))
}

fn to_value<T: serde::Serialize>(value: &T) -> Result<Value, RpcErrorData> {
    serde_json::to_value(value).map_err(|error| internal(&error))
}

pub(super) fn reason(
    code: ApplicationErrorCode,
    retryable: bool,
    reason: ErrorReason,
) -> RpcErrorData {
    RpcErrorData::new(code, retryable).with_reason(reason)
}

fn not_shareable() -> RpcErrorData {
    reason(
        ApplicationErrorCode::InvalidArgument,
        false,
        ErrorReason::ShareTaskNotShareable,
    )
}

impl ShareService {
    /// `agent.share.*` 分发。本机开关关闭时只放行 `config`（不访问云端，策略摘要为空）与
    /// `setLocalConfig`（开启开关的唯一入口），其余一律 `ShareUnavailable`。
    pub async fn rpc(
        self: &Arc<Self>,
        method_name: &str,
        params: Option<Value>,
    ) -> Result<Value, RpcErrorData> {
        if !self.is_enabled()
            && method_name != method::AGENT_SHARE_CONFIG
            && method_name != method::AGENT_SHARE_SET_LOCAL_CONFIG
        {
            return Err(unavailable());
        }
        match method_name {
            method::AGENT_SHARE_USAGE_REFRESH => to_value(&self.usage_for_rpc().await?),
            method::AGENT_SHARE_REPORT => to_value(&self.report(parse(params)?).await?),
            method::AGENT_SHARE_APPEAL => to_value(&self.appeal(parse(params)?).await?),
            method::AGENT_SHARE_CONFIG => {
                let local = self.with(|i| i.local.clone());
                let policy = if local.enabled {
                    self.config(true).await?.summary()
                } else {
                    fluxdown_protocol::SharePolicySummaryDto::default()
                };
                to_value(&ShareConfigDto {
                    policy,
                    local_config: local,
                })
            }
            method::AGENT_SHARE_SET_LOCAL_CONFIG => {
                to_value(&self.set_local_config(parse(params)?).await?)
            }
            method::AGENT_SHARE_CREATE => to_value(&self.create(parse(params)?).await?),
            method::AGENT_SHARE_LIST => {
                self.sync_shares().await;
                let has_account = self.cloud.has_account().await;
                let state = self.with(|i| i.state_dto(has_account));
                to_value(&state.shares)
            }
            method::AGENT_SHARE_UPDATE => to_value(&self.update(parse(params)?).await?),
            method::AGENT_SHARE_STOP => {
                let ShareIdParams { id } = parse(params)?;
                to_value(&self.stop(&id).await?)
            }
            method::AGENT_SHARE_RESUME => {
                let ShareIdParams { id } = parse(params)?;
                to_value(&self.resume(&id).await?)
            }
            method::AGENT_SHARE_DELETE => {
                let ShareIdParams { id } = parse(params)?;
                self.delete(&id).await?;
                Ok(json!({ "ok": true }))
            }
            method::AGENT_SHARE_RESOLVE => {
                let params: ShareResolveParams = parse(params)?;
                to_value(
                    &self
                        .resolve_input(&params.code_or_link, params.turnstile_token.as_deref())
                        .await?,
                )
            }
            method::AGENT_SHARE_RECEIVE => to_value(&self.receive(parse(params)?).await?),
            method::AGENT_SHARE_DISMISS_PENDING => {
                let ShareDismissPendingParams { resolve_id } = parse(params)?;
                self.dismiss_pending(&resolve_id).await;
                Ok(json!({ "ok": true }))
            }
            method::AGENT_SHARE_RECEIVE_RECORDS => to_value(&self.receive_records(parse(params)?)),
            method::AGENT_SHARE_SWITCH_TO_BRIDGE => {
                let params: ShareSwitchToBridgeParams = parse(params)?;
                to_value(&self.switch_to_bridge(&params.task_id).await?)
            }
            method::AGENT_SHARE_CONVERT_TO_CLOUD => {
                to_value(&self.convert_to_cloud(parse(params)?).await?)
            }
            method::AGENT_SHARE_DELETE_CLOUD_COPY => {
                let ShareIdParams { id } = parse(params)?;
                to_value(&self.delete_cloud_copy(&id).await?)
            }
            method::AGENT_SHARE_STOP_PROVIDING => {
                let ShareStopProvidingParams { record_id } = parse(params)?;
                to_value(&self.stop_providing(&record_id).await?)
            }
            method::AGENT_SHARE_DIAGNOSTICS => to_value(&self.diagnostics(&parse(params)?)),
            method::AGENT_SHARE_LOGS => to_value(&Self::logs(&parse(params)?)?),
            method::AGENT_SHARE_SESSION_KILL => {
                let ShareSessionKillParams { session_id } = parse(params)?;
                let endpoint = self.with(|i| i.endpoint.clone());
                if endpoint.is_some_and(|e| e.sender().kill_session(&session_id)) {
                    Ok(json!({ "ok": true }))
                } else {
                    Err(RpcErrorData::new(ApplicationErrorCode::NotFound, false))
                }
            }
            _ => Err(RpcErrorData::new(ApplicationErrorCode::Unsupported, false)),
        }
    }

    // ───────────── 本机设置 ─────────────

    async fn set_local_config(
        &self,
        mut next: ShareLocalConfigDto,
    ) -> Result<ShareLocalConfigDto, RpcErrorData> {
        next.udp_port_v4 = next.udp_port_v4.filter(|p| *p != 0);
        next.udp_port_v6 = next.udp_port_v6.filter(|p| *p != 0);
        next.send_rate_limit_bytes_per_sec = next.send_rate_limit_bytes_per_sec.filter(|r| *r != 0);
        next.default_swarm = next.default_swarm.filter(|v| !*v);
        next.auto_provide = next.auto_provide.filter(|v| !*v);
        next.pinned_relay = next
            .pinned_relay
            .map(|r| r.trim().to_owned())
            .filter(|r| !r.is_empty());
        next.default_tier_id = next
            .default_tier_id
            .map(|r| r.trim().to_owned())
            .filter(|r| !r.is_empty());
        let mut addrs = Vec::new();
        for addr in &next.extra_addrs {
            let addr = addr.trim();
            if addr.is_empty() {
                continue;
            }
            if addr.parse::<std::net::SocketAddr>().is_err() {
                return Err(invalid("extraAddrs"));
            }
            if !addrs.iter().any(|a| a == addr) {
                addrs.push(addr.to_owned());
            }
        }
        next.extra_addrs = addrs;
        let summary = self.with(|i| i.config.as_ref().map(|c| c.cfg.summary()));
        if let Some(summary) = summary {
            if let Some(pinned) = &next.pinned_relay
                && !summary.relays.is_empty()
                && !summary
                    .relays
                    .iter()
                    .any(|r| r.url.trim_end_matches('/') == pinned.trim_end_matches('/'))
            {
                return Err(invalid("pinnedRelay"));
            }
            if let Some(tier) = &next.default_tier_id
                && !summary.live_tiers.is_empty()
                && !summary.live_tiers.iter().any(|t| &t.id == tier)
            {
                return Err(invalid("defaultTierId"));
            }
        }
        let (old, endpoint) = self.with(|i| (i.local.clone(), i.endpoint.clone()));
        let rebuild = old.udp_port_v4 != next.udp_port_v4
            || old.udp_port_v6 != next.udp_port_v6
            || old.portmapper != next.portmapper
            || old.extra_addrs != next.extra_addrs
            || old.mode != next.mode
            || old.pinned_relay != next.pinned_relay;
        if old != next {
            let _guard = self.persist.lock().await;
            self.store
                .save_config(&next)
                .await
                .map_err(|error| internal(&format!("{error:#}")))?;
        }
        logs::set_level(next.iroh_log_level);
        if let Some(endpoint) = endpoint {
            endpoint
                .sender()
                .set_global_rate_limit(next.send_rate_limit_bytes_per_sec.unwrap_or(0));
        }
        self.with(|i| {
            i.local = next.clone();
            if rebuild && i.endpoint.is_some() {
                i.endpoint_dirty = true;
            }
        });
        // 开关切换交给事件循环：开启按启动流程恢复，关闭收掉端点与全部运行期状态。
        if old.enabled != next.enabled {
            self.wake.notify_one();
            self.publish().await;
            return Ok(next);
        }
        self.apply_swarm_policy();
        // 关闭「接收的文件参与共享」：立即停止全部共享。
        if old.auto_provide.unwrap_or(true) && !next.auto_provide.unwrap_or(true) {
            self.stop_all_provides().await;
        }
        if rebuild {
            self.refresh_endpoint().await;
        }
        self.publish().await;
        Ok(next)
    }

    // ───────────── 发送 ─────────────

    /// 本机已完成任务 → `(任务 ID, 文件路径, 文件名)`。
    async fn locate_tasks(
        &self,
        ids: &[String],
    ) -> Result<Vec<(String, PathBuf, String)>, RpcErrorData> {
        if ids.is_empty() {
            return Err(invalid("taskIds"));
        }
        let mut found = Vec::with_capacity(ids.len());
        for id in ids {
            if found
                .iter()
                .any(|(seen, _, _): &(String, PathBuf, String)| seen == id)
            {
                continue;
            }
            let task = self
                .events
                .inspect(|s| s.daemon.tasks.iter().find(|t| &t.task_id == id).cloned())
                .ok_or_else(not_shareable)?;
            if task.status != 3 || task.file_missing || task.file_name.is_empty() {
                return Err(not_shareable());
            }
            let path = PathBuf::from(&task.save_dir).join(&task.file_name);
            match tokio::fs::metadata(&path).await {
                Ok(meta) if meta.is_file() => {}
                _ => return Err(not_shareable()),
            }
            found.push((id.clone(), path, task.file_name));
        }
        Ok(found)
    }

    async fn hash_files(
        &self,
        located: &[(String, PathBuf, String)],
        hashing_id: &str,
        title: &str,
    ) -> Result<Vec<ShareFile>, RpcErrorData> {
        let mut total = 0u64;
        for (_, path, _) in located {
            total += tokio::fs::metadata(path)
                .await
                .map_err(|_| not_shareable())?
                .len();
        }
        let cancel = CancellationToken::new();
        // 调用方放弃（连接断开）时 RPC 未来被丢弃：取消后台哈希。
        let _guard = cancel.clone().drop_guard();
        let mut files = Vec::with_capacity(located.len());
        let mut base = 0u64;
        let mut last_publish = Instant::now();
        for (_, path, name) in located {
            let (tx, mut rx) = mpsc::unbounded_channel::<u64>();
            let hashing = fluxdown_share::hash_file(
                path.clone(),
                Some(name.clone()),
                move |progress| {
                    if tx.send(progress.done).is_err() {
                        tracing::trace!("hash progress receiver is gone");
                    }
                },
                cancel.clone(),
            );
            tokio::pin!(hashing);
            let file = loop {
                tokio::select! {
                    result = &mut hashing => break result,
                    Some(done) = rx.recv() => {
                        self.with(|i| {
                            i.hashing.insert(hashing_id.to_owned(), ShareHashingDto {
                                id: hashing_id.to_owned(),
                                title: title.to_owned(),
                                done_bytes: base + done,
                                total_bytes: total,
                            });
                        });
                        if last_publish.elapsed() >= std::time::Duration::from_millis(250) {
                            last_publish = Instant::now();
                            self.publish().await;
                        }
                    }
                }
            };
            let file = file.map_err(|error| {
                tracing::warn!(error = %format!("{error:#}"), "hashing a shared file failed");
                not_shareable()
            })?;
            base += file.size;
            files.push(file);
        }
        Ok(files)
    }

    async fn create(&self, params: ShareCreateParams) -> Result<ShareDto, RpcErrorData> {
        let cfg = self.fresh_config().await?;
        let has_account = self.cloud.has_account().await;
        if !cfg.enabled || !(has_account || cfg.guest_share) {
            return Err(unavailable());
        }
        let located = self.locate_tasks(&params.task_ids).await?;
        let local = self.with(|i| i.local.clone());
        let tier = cfg
            .default_tier(
                params
                    .tier_id
                    .as_deref()
                    .or(local.default_tier_id.as_deref()),
            )
            .ok_or_else(unavailable)?;
        let max_claims = params.max_claims.unwrap_or(local.default_max_claims);
        let notify = params
            .notify_on_claim
            .unwrap_or(local.default_notify_on_claim);
        let title = match located.as_slice() {
            [(_, _, name)] => name.clone(),
            [(_, _, first), rest @ ..] => format!("{first} (+{})", rest.len()),
            [] => return Err(invalid("taskIds")),
        };
        let hashing_id = uuid::Uuid::new_v4().to_string();
        let hashed = self.hash_files(&located, &hashing_id, &title).await;
        self.with(|i| i.hashing.remove(&hashing_id));
        self.publish().await;
        let files = hashed?;
        let endpoint = self.endpoint().await?;
        self.ensure_registered(&endpoint).await?;
        // 未登录：以游客发送方身份创建（令牌绑定本机 endpoint）。
        let guest_owned = !has_account;
        let guest_token = self.owner_token(guest_owned).await?;
        let body = CreateBody {
            title: &title,
            files: &files,
            tier_id: &tier,
            max_claims: (max_claims > 0).then_some(max_claims),
            notify_on_claim: notify,
            endpoint_id: &endpoint.id_z32(),
            swarm: params.swarm.unwrap_or(local.default_swarm.unwrap_or(true)),
        };
        let cloud: CloudShare = self
            .cloud
            .share_create(&body, guest_token.as_deref())
            .await
            .map_err(|error| share_error(&error))?;
        if let Some(token) = &guest_token {
            self.remember_guest_token(std::slice::from_ref(&cloud.id), token);
        }
        let mut dto = ShareDto {
            id: cloud.id.clone(),
            files: files
                .iter()
                .map(|f| ShareFileDto {
                    name: f.name.clone(),
                    size: f.size,
                })
                .collect(),
            total_bytes: files.iter().map(|f| f.size).sum(),
            task_ids: located.iter().map(|(id, _, _)| id.clone()).collect(),
            ..ShareDto::default()
        };
        apply_cloud(&mut dto, &cloud);
        let stored = StoredShare {
            dto: dto.clone(),
            manifest: ShareManifest {
                share_id: cloud.id.clone(),
                files,
            },
            paths: located.into_iter().map(|(_, path, _)| path).collect(),
            guest_owned,
        };
        self.register_local(&stored)
            .map_err(|error| internal(&format!("{error:#}")))?;
        self.with(|i| {
            i.shares.push(stored);
            i.share_epoch += 1;
        });
        self.persist_shares().await;
        self.wake.notify_one();
        self.refresh_endpoint().await;
        self.publish().await;
        Ok(dto)
    }

    pub(super) fn find_share(&self, id: &str) -> Result<StoredShare, RpcErrorData> {
        self.with(|i| i.shares.iter().find(|s| s.dto.id == id).cloned())
            .ok_or_else(|| {
                reason(
                    ApplicationErrorCode::NotFound,
                    false,
                    ErrorReason::ShareNotFound,
                )
            })
    }

    async fn update(&self, params: ShareUpdateParams) -> Result<ShareDto, RpcErrorData> {
        let stored = self.find_share(&params.id)?;
        let mut body = serde_json::Map::new();
        if let Some(tier) = &params.tier_id {
            body.insert("tierId".into(), json!(tier));
        }
        if let Some(max) = params.max_claims {
            body.insert(
                "maxClaims".into(),
                if max == 0 { Value::Null } else { json!(max) },
            );
        }
        if let Some(notify) = params.notify_on_claim {
            body.insert("notifyOnClaim".into(), json!(notify));
        }
        if let Some(swarm) = params.swarm {
            body.insert("swarm".into(), json!(swarm));
        }
        if body.is_empty() {
            return Ok(stored.dto);
        }
        let guest_token = self.owner_token(stored.guest_owned).await?;
        let cloud: CloudShare = self
            .cloud
            .share_patch(&params.id, &Value::Object(body), guest_token.as_deref())
            .await
            .map_err(|error| share_error(&error))?;
        let dto = self.with(|i| {
            i.share_epoch += 1;
            i.shares
                .iter_mut()
                .find(|s| s.dto.id == params.id)
                .map(|s| {
                    apply_cloud(&mut s.dto, &cloud);
                    s.dto.clone()
                })
        });
        let dto = dto.ok_or_else(|| {
            reason(
                ApplicationErrorCode::NotFound,
                false,
                ErrorReason::ShareNotFound,
            )
        })?;
        self.registry
            .set_expiry(&dto.id, super::time::parse_rfc3339_secs(&dto.expires_at));
        self.persist_shares().await;
        self.wake.notify_one();
        self.publish().await;
        Ok(dto)
    }

    /// 停止分享：云端停止（取件码释放）后本机置为已停止。云端失败时本机状态不变。
    pub(super) async fn stop(&self, id: &str) -> Result<ShareDto, RpcErrorData> {
        let stored = self.find_share(id)?;
        let guest_token = self.owner_token(stored.guest_owned).await?;
        match self.cloud.share_stop(id, guest_token.as_deref()).await {
            Ok(_) => {}
            // 云端已经不认识它：本机同样视为已停止。
            Err(error) if error.status == Some(404) => {}
            Err(error) => return Err(share_error(&error)),
        }
        self.registry
            .set_state(id, fluxdown_share::ShareState::Stopped);
        self.cancel_upload(id).await;
        if let Some(endpoint) = self.with(|i| i.endpoint.clone()) {
            endpoint.sender().kill_share_sessions(id);
        }
        let dto = self.with(|i| {
            i.share_epoch += 1;
            i.shares.iter_mut().find(|s| s.dto.id == id).map(|s| {
                s.dto.status = ShareStatus::Stopped;
                s.dto.pickup_code.clear();
                s.dto.clone()
            })
        });
        let dto = dto.ok_or_else(|| {
            reason(
                ApplicationErrorCode::NotFound,
                false,
                ErrorReason::ShareNotFound,
            )
        })?;
        self.persist_shares().await;
        self.wake.notify_one();
        self.refresh_endpoint_later();
        self.publish().await;
        Ok(dto)
    }

    /// `agent.share.resume`：重新开始已停止 / 已过期的分享，同 id、同链接，只重新分配取件码；
    /// 云端会清掉旧的云端副本，恢复后一律是实时分享。
    ///
    /// 已在分享中则原样返回。开始前检查源文件（大小 + 修改时间与清单一致）：不一致则标记
    /// `sourceMissing` 并以 `ShareSourceMissing` 拒绝。
    async fn resume(&self, id: &str) -> Result<ShareDto, RpcErrorData> {
        let stored = self.find_share(id)?;
        if stored.dto.status == ShareStatus::Takedown {
            return Err(reason(
                ApplicationErrorCode::Conflict,
                false,
                ErrorReason::ShareStopped,
            ));
        }
        if stored.dto.status == ShareStatus::Active {
            return Ok(stored.dto);
        }
        if !sources_intact(&stored.manifest.files, &stored.paths).await {
            if self.flag_source_missing(id) {
                self.persist_shares().await;
                self.publish().await;
            }
            return Err(reason(
                ApplicationErrorCode::Conflict,
                false,
                ErrorReason::ShareSourceMissing,
            ));
        }
        let guest_token = self.owner_token(stored.guest_owned).await?;
        let cloud: CloudShare = self
            .cloud
            .share_resume(id, guest_token.as_deref())
            .await
            .map_err(|error| share_error(&error))?;
        let resumed = self.with(|i| {
            i.share_epoch += 1;
            i.shares.iter_mut().find(|s| s.dto.id == id).map(|s| {
                apply_cloud(&mut s.dto, &cloud);
                s.dto.source_missing = false;
                s.clone()
            })
        });
        let resumed = resumed.ok_or_else(|| {
            reason(
                ApplicationErrorCode::NotFound,
                false,
                ErrorReason::ShareNotFound,
            )
        })?;
        self.register_local(&resumed)
            .map_err(|error| internal(&error))?;
        self.persist_shares().await;
        self.wake.notify_one();
        self.refresh_endpoint().await;
        self.publish().await;
        Ok(resumed.dto)
    }

    /// `agent.share.delete`：删除分享记录。云端记录先删（分享中的由云端按停止处理；已不存在视为成功），
    /// 云端失败则保留本机记录；成功后取消上传、中断会话、移出发送方登记表并删除本机记录。
    async fn delete(&self, id: &str) -> Result<(), RpcErrorData> {
        let stored = self.find_share(id)?;
        let guest_token = self.owner_token(stored.guest_owned).await?;
        match self
            .cloud
            .share_delete_record(id, guest_token.as_deref())
            .await
        {
            Ok(_) => {}
            Err(error) if error.status == Some(404) => {}
            Err(error) => return Err(share_error(&error)),
        }
        self.registry.remove(id);
        self.cancel_upload(id).await;
        if let Some(endpoint) = self.with(|i| i.endpoint.clone()) {
            endpoint.sender().kill_share_sessions(id);
        }
        self.with(|i| {
            i.share_epoch += 1;
            i.shares.retain(|s| s.dto.id != id);
        });
        self.persist_shares().await;
        self.wake.notify_one();
        self.refresh_endpoint_later();
        self.publish().await;
        Ok(())
    }

    // ───────────── 接收 ─────────────

    /// resolve：取件码 / 链接 → 预览；解析句柄（含票据）留在内存。
    pub(super) async fn resolve_input(
        &self,
        input: &str,
        turnstile: Option<&str>,
    ) -> Result<ShareResolveDto, RpcErrorData> {
        let (code, slug) = parse_code_or_link(input).ok_or_else(|| {
            reason(
                ApplicationErrorCode::InvalidArgument,
                false,
                ErrorReason::ShareCodeInvalid,
            )
        })?;
        let cfg = self.fresh_config().await?;
        let has_account = self.cloud.has_account().await;
        if !cfg.enabled || !(has_account || cfg.guest_receive) {
            return Err(unavailable());
        }
        let endpoint = self.endpoint().await?;
        // 登录设备的 resolve 要求接收方 endpoint 已登记到本设备（服务端 400）；游客由 resolve 自己登记。
        self.ensure_registered(&endpoint).await?;
        let device_name = self.state.lock().await.device_name.clone();
        let mut body = serde_json::Map::new();
        body.insert("receiverEndpointId".into(), json!(endpoint.id_z32()));
        if let Some(code) = code {
            body.insert("code".into(), json!(code));
        }
        if let Some(slug) = slug {
            body.insert("slug".into(), json!(slug));
        }
        if !device_name.is_empty() {
            body.insert("deviceName".into(), json!(device_name));
        }
        if let Some(token) = turnstile {
            body.insert("turnstileToken".into(), json!(token));
        }
        let resp: ResolveResponse = self
            .cloud
            .share_resolve(&Value::Object(body))
            .await
            .map_err(|error| match error.status {
                Some(400 | 422) if error.code.as_deref() != Some("turnstile_required") => reason(
                    ApplicationErrorCode::InvalidArgument,
                    false,
                    ErrorReason::ShareCodeInvalid,
                ),
                _ => share_error(&error),
            })?;
        // iroh 端点不能连接自身：本机发起的分享在这里就拒绝，而不是建出注定 503 的下载任务。
        if resp.share.sender_endpoint_id == endpoint.id_z32() {
            return Err(reason(
                ApplicationErrorCode::InvalidArgument,
                false,
                ErrorReason::ShareOwnShare,
            ));
        }
        if let Some(token) = &resp.guest_token {
            self.telemetry
                .guests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(&resp.share.id, token.clone());
        }
        let dto = ShareResolveDto {
            resolve_id: uuid::Uuid::new_v4().to_string(),
            share_id: resp.share.id.clone(),
            title: resp.share.title.clone(),
            sender_device_name: resp.share.sender_device_name.clone(),
            files: resp
                .share
                .files
                .iter()
                .map(super::cloud::ResolveFile::dto)
                .collect(),
            total_bytes: resp
                .share
                .files
                .iter()
                .map(|f| u64::try_from(f.size).unwrap_or(0))
                .sum(),
            expires_at: resp.share.expires_at.clone().unwrap_or_default(),
            sources: ShareSourcesDto {
                swarm_peers: resp.share.swarm_peers.unwrap_or(0),
                sender_online: resp.share.sender_online,
                cloud_copy: resp.share.cloud_state.as_deref() == Some("ready"),
                // 桥接需要分享方在线与可用的网关；这里只能按身份与已知的在线状态判断，
                // 真正换不到桥接时 `switchToBridge` 会给出带类型的错误。
                bridge: (has_account || cfg.guest_bridge)
                    && resp.share.sender_online != Some(false)
                    && resp.share.cloud_state.as_deref() != Some("ready"),
            },
        };
        self.with(|i| {
            i.resolves.retain(|_, p| p.created.elapsed() < RESOLVE_TTL);
            while i.resolves.len() >= RESOLVE_CAP {
                let oldest = i
                    .resolves
                    .iter()
                    .min_by_key(|(_, p)| p.created)
                    .map(|(id, _)| id.clone());
                match oldest {
                    Some(id) => {
                        i.resolves.remove(&id);
                    }
                    None => break,
                }
            }
            i.resolves.insert(
                dto.resolve_id.clone(),
                Pending {
                    resp,
                    dto: dto.clone(),
                    created: Instant::now(),
                },
            );
        });
        Ok(dto)
    }

    async fn dismiss_pending(&self, resolve_id: &str) {
        self.with(|i| {
            i.resolves.remove(resolve_id);
            if i.pending_receive.as_deref() == Some(resolve_id) {
                i.pending_receive = None;
            }
        });
        self.publish().await;
    }

    async fn receive(
        &self,
        params: ShareReceiveParams,
    ) -> Result<ShareReceiveResultDto, RpcErrorData> {
        let resolve_id = match (&params.resolve_id, &params.code_or_link) {
            (Some(id), None) => id.clone(),
            (None, Some(input)) => self.resolve_input(input, None).await?.resolve_id,
            _ => return Err(invalid("resolveId")),
        };
        self.with(|i| i.receiving += 1);
        let result = self.receive_inner(&resolve_id, &params).await;
        self.with(|i| i.receiving = i.receiving.saturating_sub(1));
        if result.is_err() {
            // 失败时清掉可能残留的空会话 / 适配器。
            self.reconcile_tasks().await;
        }
        result
    }

    async fn receive_inner(
        &self,
        resolve_id: &str,
        params: &ShareReceiveParams,
    ) -> Result<ShareReceiveResultDto, RpcErrorData> {
        let (resp, dto) = self
            .with(|i| {
                i.resolves
                    .get(resolve_id)
                    .filter(|p| p.created.elapsed() < RESOLVE_TTL)
                    .map(|p| (p.resp.clone(), p.dto.clone()))
            })
            .ok_or_else(|| {
                reason(
                    ApplicationErrorCode::NotFound,
                    false,
                    ErrorReason::ShareNotFound,
                )
            })?;
        let files = &resp.share.files;
        let indexes: Vec<u32> = match &params.file_indexes {
            Some(list) => {
                let mut out: Vec<u32> = Vec::new();
                for index in list {
                    if usize::try_from(*index).map_or(true, |i| i >= files.len()) {
                        return Err(invalid("fileIndexes"));
                    }
                    if !out.contains(index) {
                        out.push(*index);
                    }
                }
                out
            }
            None => (0..u32::try_from(files.len()).map_err(|_| invalid("fileIndexes"))?).collect(),
        };
        if indexes.is_empty() {
            return Err(invalid("fileIndexes"));
        }
        let manifest = ShareManifest {
            share_id: resp.share.id.clone(),
            files: files
                .iter()
                .map(super::cloud::ResolveFile::to_share_file)
                .collect(),
        };
        if !manifest
            .files
            .iter()
            .all(fluxdown_share::hashing::manifest_root_is_consistent)
        {
            tracing::warn!("share manifest has inconsistent hashes; rejected");
            return Err(internal(&"inconsistent share manifest"));
        }
        let sender = EndpointId::from_z32(&resp.share.sender_endpoint_id)
            .map_err(|error| internal(&error))?;
        let endpoint = self.endpoint().await?;
        self.ensure_registered(&endpoint).await?;
        let mut receive_params = ReceiveParams::new(manifest, resp.ticket.clone(), sender);
        receive_params.addr_hint = self.addr_hint_for(&resp.share.sender_endpoint_id);
        let session = ReceiveSession::new(endpoint, receive_params);
        self.attach_sources(&session, &dto.share_id, &resp.ticket, &dto.sources);
        self.watch_sources(&session);
        self.watch_swarm_peers(&session);
        // 首次对端查询在建任务前同步做一次（限时）：任务一开始取块就能用上共享对端。
        let first_lookup = tokio::time::timeout(
            FIRST_LOOKUP_TIMEOUT,
            self.swarm_lookup(&session, &dto.share_id, &resp.ticket, &indexes),
        )
        .await
        .unwrap_or(Lookup::Refresh(LOOKUP_RETRY));
        if self.with(|i| i.adapter.is_none()) {
            let adapter = LocalAdapter::start()
                .await
                .map_err(|error| internal(&format!("{error:#}")))?;
            // 并发的另一次接收已经起了适配器：丢弃多余的（丢弃即停止监听）。
            self.with(|i| {
                if i.adapter.is_none() {
                    i.adapter = Some(adapter);
                }
            });
        }
        let (token, urls) = self
            .with(|i| {
                let adapter = i.adapter.as_ref()?;
                let token = adapter.register(session.clone());
                i.receive_sessions.insert(token.clone(), session.clone());
                let urls: Vec<String> = indexes
                    .iter()
                    .map(|idx| adapter.url(&token, *idx))
                    .collect();
                Some((token, urls))
            })
            .ok_or_else(unavailable)?;
        self.continue_lookup(first_lookup, &token, &dto.share_id, &resp.ticket, &indexes);
        let record_id = uuid::Uuid::new_v4().to_string();
        let save_dir = params.save_dir.clone().unwrap_or_default();
        let mut task_ids = Vec::with_capacity(indexes.len());
        let mut failure = None;
        for (index, url) in indexes.iter().zip(urls) {
            let name = files
                .get(usize::try_from(*index).unwrap_or(usize::MAX))
                .map(|f| safe_file_name(&f.name))
                .unwrap_or_default();
            let request: CreateTaskRequest = serde_json::from_value(json!({
                "url": url,
                "fileName": name,
                "saveDir": save_dir,
                // 回环适配器必须直连：系统 / 手动代理不得接管 127.0.0.1。
                "proxyUrl": "direct://",
            }))
            .map_err(|error| internal(&error))?;
            match self.tasks.create_task(request).await {
                Ok(task_id) => {
                    self.with(|i| {
                        i.receive_tasks.insert(
                            task_id.clone(),
                            ReceiveTask {
                                share_id: dto.share_id.clone(),
                                record_id: record_id.clone(),
                                token: token.clone(),
                                status: 0,
                                created: Instant::now(),
                                file_index: *index,
                            },
                        );
                    });
                    task_ids.push(task_id);
                }
                Err(error) => {
                    tracing::warn!(error = %error, "could not create a share receive task");
                    failure = Some(RpcErrorData::new(ApplicationErrorCode::Unavailable, true));
                    break;
                }
            }
        }
        if !task_ids.is_empty() {
            let record = ShareReceiveRecordDto {
                provide_state: fluxdown_protocol::ShareProvideState::Off,
                id: record_id.clone(),
                share_id: Some(dto.share_id.clone()),
                title: dto.title.clone(),
                sender_device_name: dto.sender_device_name.clone(),
                file_count: u32::try_from(task_ids.len()).unwrap_or(u32::MAX),
                total_bytes: indexes
                    .iter()
                    .filter_map(|idx| files.get(usize::try_from(*idx).ok()?))
                    .map(|f| u64::try_from(f.size).unwrap_or(0))
                    .sum(),
                task_ids: task_ids.clone(),
                started_at: fluxdown_share::timefmt::now_rfc3339(),
                status: ShareReceiveStatus::Active,
            };
            let provideed = self.with(|i| {
                let provideed = i.auto_provide();
                if provideed {
                    i.provides.push(StoredProvide {
                        record_id: record_id.clone(),
                        share_id: dto.share_id.clone(),
                        ticket: resp.ticket.clone(),
                        share_expires_at: resp.share.expires_at.clone(),
                        files: Vec::new(),
                        state: fluxdown_protocol::ShareProvideState::Providing,
                    });
                }
                i.records.push(record);
                i.resolves.remove(resolve_id);
                if i.pending_receive.as_deref() == Some(resolve_id) {
                    i.pending_receive = None;
                }
                provideed
            });
            self.persist_records().await;
            if provideed {
                self.persist_provides().await;
                self.announce_soon();
            }
            self.reconcile_tasks().await;
            self.refresh_endpoint().await;
            self.publish().await;
        }
        match failure {
            Some(error) => Err(error),
            None => Ok(ShareReceiveResultDto {
                record_id,
                task_ids,
            }),
        }
    }

    #[cfg(test)]
    pub(super) fn addr_hint_for(&self, sender_z32: &str) -> Option<fluxdown_share::EndpointAddr> {
        let hook = self
            .addr_hint
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()?;
        hook(sender_z32)
    }

    #[cfg(not(test))]
    #[allow(clippy::unused_self)]
    pub(super) fn addr_hint_for(&self, _sender_z32: &str) -> Option<fluxdown_share::EndpointAddr> {
        None
    }

    fn receive_records(&self, params: ShareReceiveRecordsParams) -> ShareReceiveRecordsPage {
        let limit = params.limit.map_or(50, |l| l.clamp(1, 200)) as usize;
        let start = params
            .cursor
            .as_deref()
            .and_then(|c| c.parse::<usize>().ok())
            .unwrap_or(0);
        self.with(|i| {
            let all: Vec<ShareReceiveRecordDto> =
                i.records.iter().rev().map(|r| i.record_dto(r)).collect();
            let page: Vec<ShareReceiveRecordDto> =
                all.iter().skip(start).take(limit).cloned().collect();
            let next = start + page.len();
            ShareReceiveRecordsPage {
                records: page,
                next_cursor: (next < all.len()).then(|| next.to_string()),
            }
        })
    }

    // ───────────── 来源（云端副本 / 桥接） ─────────────

    /// 给新建的接收会话挂云端副本来源：云端存储已配置时恒挂——副本随时可能在传输中途就绪，
    /// 未就绪的请求在来源内部退避，不会拖慢直连。分享方已知离线且副本就绪时直接来源优先。
    fn attach_sources(
        &self,
        session: &ReceiveSession,
        share_id: &str,
        ticket: &str,
        sources: &ShareSourcesDto,
    ) {
        let cloud_enabled = self.with(|i| i.config.as_ref().is_some_and(|c| c.cfg.cloud_enabled));
        if !(cloud_enabled || sources.cloud_copy) {
            return;
        }
        session.set_source(Arc::new(CloudCopySource::new(
            self.cloud.clone(),
            self.http.clone(),
            share_id.to_owned(),
            ticket.to_owned(),
        )));
        if sources.cloud_copy && sources.sender_online == Some(false) {
            session.set_preference(SourcePreference::Sources);
        }
    }

    /// 来源变化（直连 ↔ 云端 / 桥接）时刷新任务行徽标；会话销毁后订阅自然结束。
    fn watch_sources(&self, session: &ReceiveSession) {
        let mut changes = session.watch_source();
        let weak = self.weak.clone();
        tokio::spawn(async move {
            while changes.changed().await.is_ok() {
                let Some(service) = weak.upgrade() else {
                    break;
                };
                service.publish().await;
            }
        });
    }

    /// `agent.share.switchToBridge`：换取桥接授权并让会话来源优先；任务不中断，已校验的块不重取。
    async fn switch_to_bridge(&self, task_id: &str) -> Result<ShareTaskFactsDto, RpcErrorData> {
        let cfg = self.config(false).await?;
        let has_account = self.cloud.has_account().await;
        if !has_account && !cfg.guest_bridge {
            return Err(reason(
                ApplicationErrorCode::Unavailable,
                false,
                ErrorReason::ShareBridgeUnavailable,
            ));
        }
        let found = self.with(|i| {
            let task = i.receive_tasks.get(task_id)?;
            let session = i.receive_sessions.get(&task.token)?.clone();
            Some((task.share_id.clone(), session))
        });
        let (share_id, session) = found.ok_or_else(|| {
            reason(
                ApplicationErrorCode::NotFound,
                false,
                ErrorReason::ShareNotFound,
            )
        })?;
        if !session.source_kinds().contains(&SourceKind::Bridge) {
            let bridge = BridgeSource::connect(
                self.cloud.clone(),
                self.http.clone(),
                share_id,
                session.ticket().to_owned(),
            )
            .await
            .map_err(|error| bridge_switch_error(&error))?;
            session.set_source(bridge);
        }
        session.set_preference(SourcePreference::Sources);
        self.publish().await;
        self.with(|i| i.state_dto(has_account).task_facts.remove(task_id))
            .ok_or_else(|| {
                reason(
                    ApplicationErrorCode::NotFound,
                    false,
                    ErrorReason::ShareNotFound,
                )
            })
    }

    // ───────────── 诊断 ─────────────

    fn diagnostics(&self, params: &ShareDiagnosticsParams) -> ShareDiagnosticsDto {
        let snapshot = self
            .with(|i| i.endpoint.clone())
            .and_then(|endpoint| endpoint.snapshot());
        let snapshot_json = snapshot
            .as_ref()
            .and_then(|s| serde_json::to_string_pretty(s).ok())
            .unwrap_or_default();
        let mut sessions = self
            .telemetry
            .diag
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .details(params.session_id.as_deref());
        if sessions.iter().any(|s| !s.swarm_files.is_empty()) {
            let names = self.with(|i| i.root_names());
            for file in sessions.iter_mut().flat_map(|s| s.swarm_files.iter_mut()) {
                file.file_name = names.get(&file.root_hash).cloned();
            }
        }
        ShareDiagnosticsDto {
            snapshot: snapshot.as_ref().map(convert::snapshot),
            snapshot_json,
            sessions,
        }
    }

    fn logs(params: &ShareLogsParams) -> Result<ShareLogsDto, RpcErrorData> {
        let regex = params
            .regex
            .as_deref()
            .filter(|r| !r.is_empty())
            .map(regex::Regex::new)
            .transpose()
            .map_err(|_| invalid("regex"))?;
        let limit = params.limit.map_or(500, |l| l.clamp(1, 2000)) as usize;
        Ok(ShareLogsDto {
            lines: logs::read(
                params.level.unwrap_or(ShareLogLevel::Debug),
                regex.as_ref(),
                limit,
            ),
        })
    }
}

/// 本机开关关闭时分享链接一律 `ShareUnavailable`：不解析、不访问云端，也绝不落成下载任务。
#[async_trait]
impl ShareLinkSink for ShareService {
    async fn open_link(&self, link: &str) -> Result<(), RpcErrorData> {
        if !self.is_enabled() {
            return Err(unavailable());
        }
        let dto = self.resolve_input(link, None).await?;
        let replaced = self.with(|i| i.pending_receive.replace(dto.resolve_id.clone()));
        if let Some(old) = replaced {
            self.with(|i| i.resolves.remove(&old));
        }
        self.publish().await;
        Ok(())
    }

    async fn receive_link(&self, link: &str, save_dir: Option<&str>) -> Result<(), RpcErrorData> {
        if !self.is_enabled() {
            return Err(unavailable());
        }
        self.receive(ShareReceiveParams {
            code_or_link: Some(link.to_owned()),
            save_dir: save_dir.map(str::to_owned),
            ..ShareReceiveParams::default()
        })
        .await
        .map(|_| ())
    }
}
