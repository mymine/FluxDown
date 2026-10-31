//! 状态投影、云端策略 / 公钥、端点生命周期与云端同步。

use std::time::{Duration, Instant};

use fluxdown_protocol::{
    AgentEvent, RpcErrorData, ShareCloudState, ShareDto, ShareHomeRelayDto, ShareNatType,
    ShareNetStatusDto, SharePathKind, ShareReceiverDto, ShareStateDto, ShareStatus,
    ShareTaskFactsDto,
};
use fluxdown_share::{
    NetSnapshot, Observers, ShareEndpoint, ShareEndpointParams, ShareEntry, ShareState,
};
use serde_json::Value;

use super::cloud::{CloudKeys, CloudShare, NetResponse, SharesList, share_error};
use super::convert;
use super::service::{CONFIG_TTL, CachedConfig, Inner, ShareService, unavailable};
use super::store::StoredShare;
use super::time::parse_rfc3339_secs;
use super::upload_job::sources_intact;

/// 接收方连上来后到对账之间的合并窗口。
const SHARE_SYNC_COALESCE: Duration = Duration::from_secs(1);
/// 一次对账因本机并发修改作废后最多重试的轮数。
const SYNC_ATTEMPTS: usize = 3;

impl Inner {
    /// 由内存状态拼出完整的 UI 状态（纯函数，不访问网络）。本机开关关闭时只带本机设置，其余为空。
    pub(super) fn state_dto(&self, has_account: bool) -> ShareStateDto {
        if !self.local.enabled {
            return ShareStateDto {
                local_config: self.local.clone(),
                ..ShareStateDto::default()
            };
        }
        let (enabled, can_send, policy_summary) = match &self.config {
            Some(cached) => (
                cached.cfg.enabled,
                cached.cfg.enabled && (has_account || cached.cfg.guest_share),
                cached.cfg.summary(),
            ),
            None => (
                false,
                false,
                fluxdown_protocol::SharePolicySummaryDto::default(),
            ),
        };
        let shares = self
            .shares
            .iter()
            .map(|stored| {
                let mut dto: ShareDto = stored.dto.clone();
                dto.active_receivers = self
                    .sessions
                    .iter()
                    .filter(|(_, m)| {
                        m.role == fluxdown_share::SessionRole::Sender
                            && !m.ended
                            && m.share_id == dto.id
                    })
                    .map(|(id, m)| ShareReceiverDto {
                        session_id: id.clone(),
                        device_name: m.device_name.clone(),
                        path_kind: convert::path_kind(m.path),
                        rate_bps: m.rate_bps,
                        bytes: m.bytes,
                        progress: if m.total_bytes == 0 {
                            0.0
                        } else {
                            (m.bytes as f64 / m.total_bytes as f64).clamp(0.0, 1.0)
                        },
                        started_at: m.started_at.clone(),
                    })
                    .collect();
                self.overlay_upload(&mut dto);
                dto
            })
            .collect();
        let mut task_facts: std::collections::BTreeMap<String, ShareTaskFactsDto> = self
            .receive_tasks
            .iter()
            .map(|(task_id, task)| {
                // 同一分享最新的接收会话（按开始时间）承载该任务。
                let session = self
                    .sessions
                    .iter()
                    .filter(|(_, m)| {
                        m.role == fluxdown_share::SessionRole::Receiver
                            && m.share_id == task.share_id
                    })
                    .max_by(|a, b| a.1.started_at.cmp(&b.1.started_at));
                // 最近交付块来自云端副本 / 桥接时，来源徽标以它为准（此时直连路径的状态与该任务无关）。
                let source = self
                    .receive_sessions
                    .get(&task.token)
                    .and_then(fluxdown_share::ReceiveSession::active_source);
                let swarm_peers = self
                    .receive_sessions
                    .get(&task.token)
                    .map_or(0, fluxdown_share::ReceiveSession::swarm_peers);
                let facts = ShareTaskFactsDto {
                    is_share_receive: true,
                    path_kind: match source {
                        Some(fluxdown_share::SourceKind::Cdn) => SharePathKind::Cdn,
                        Some(fluxdown_share::SourceKind::Bridge) => SharePathKind::Bridge,
                        // 分享方的会话已结束 / 不存在，但仍有共享对端在供块。
                        None => match session {
                            Some((_, m)) if !m.ended => convert::path_kind(m.path),
                            _ if swarm_peers > 0 => SharePathKind::Swarm,
                            Some((_, m)) => convert::path_kind(m.path),
                            None => Default::default(),
                        },
                    },
                    peer_nat_type: ShareNatType::Unknown,
                    peer_isp: None,
                    share_id: Some(task.share_id.clone()),
                    session_id: session.map(|(id, _)| id.clone()),
                    relay_stuck: source.is_none()
                        && session.is_some_and(|(_, m)| m.relay_stuck && !m.ended),
                    swarm_peers,
                    swarm_block: self.swarm_blocks.get(&task.share_id).copied(),
                    record_id: Some(task.record_id.clone()),
                    sender_device_name: self
                        .records
                        .iter()
                        .find(|r| r.id == task.record_id)
                        .map(|r| r.sender_device_name.clone())
                        .unwrap_or_default(),
                    provide_state: self.provide_state_of(&task.record_id),
                };
                (task_id.clone(), facts)
            })
            .collect();
        // 已完成后被移除的接收任务：只要其记录仍在共享就保留 facts（遍历全部记录，共享中的数量有界）。
        for record in &self.records {
            if self.provide_state_of(&record.id) != fluxdown_protocol::ShareProvideState::Providing
            {
                continue;
            }
            for task_id in &record.task_ids {
                if task_facts.contains_key(task_id) {
                    continue;
                }
                task_facts.insert(
                    task_id.clone(),
                    ShareTaskFactsDto {
                        is_share_receive: true,
                        share_id: record.share_id.clone(),
                        record_id: Some(record.id.clone()),
                        sender_device_name: record.sender_device_name.clone(),
                        provide_state: fluxdown_protocol::ShareProvideState::Providing,
                        ..Default::default()
                    },
                );
            }
        }
        let receive_records = self
            .records
            .iter()
            .rev()
            .take(fluxdown_protocol::SHARE_STATE_RECEIVE_RECORDS)
            .map(|record| self.record_dto(record))
            .collect();
        ShareStateDto {
            enabled,
            available: enabled,
            can_send,
            local_config: self.local.clone(),
            policy_summary,
            shares,
            net_status: self.net.clone(),
            task_facts,
            receive_records,
            pending_receive: self
                .pending_receive
                .as_ref()
                .and_then(|id| self.resolves.get(id))
                .map(|p| p.dto.clone()),
            hashing: self.hashing.values().cloned().collect(),
            usage: self.usage.clone().map(Box::new),
            usage_error: self.usage_error,
        }
    }
}

impl ShareService {
    /// 状态变化才发：与上次发布相同则静默。
    pub(super) async fn publish(&self) {
        let has_account = self.cloud.has_account().await;
        let dto = self.with(|i| {
            let dto = i.state_dto(has_account);
            if i.last_published.as_ref() == Some(&dto) {
                None
            } else {
                i.last_published = Some(dto.clone());
                Some(dto)
            }
        });
        if let Some(dto) = dto {
            self.events.publish(AgentEvent::ShareChanged(dto));
        }
    }

    // ───────────── 云端策略 / 公钥 ─────────────

    /// 取云端策略（5 分钟内用缓存，之后 ETag 条件请求；`force` 跳过新鲜期）。离线时沿用旧缓存。
    /// 本机开关关闭时不发请求，直接 `ShareUnavailable`（端点构建、分享、接收都经这里，等于总闸）。
    pub(super) async fn config(
        &self,
        force: bool,
    ) -> Result<super::cloud::CloudConfig, RpcErrorData> {
        if !self.is_enabled() {
            return Err(unavailable());
        }
        let (cached, etag) = self.with(|i| {
            (
                i.config
                    .as_ref()
                    .filter(|c| !force && c.fetched.elapsed() < CONFIG_TTL)
                    .map(|c| c.cfg.clone()),
                i.config.as_ref().and_then(|c| c.etag.clone()),
            )
        });
        if let Some(cfg) = cached {
            return Ok(cfg);
        }
        match self
            .cloud
            .share_config::<super::cloud::CloudConfig>(etag.as_deref())
            .await
        {
            Ok(None) => {
                self.with(|i| {
                    if let Some(c) = &mut i.config {
                        c.fetched = Instant::now();
                    }
                });
            }
            Ok(Some((cfg, new_etag))) => {
                let changed = self.with(|i| {
                    let changed = i
                        .config
                        .as_ref()
                        .is_none_or(|old| old.cfg.network != cfg.network);
                    if changed && i.endpoint.is_some() {
                        i.endpoint_dirty = true;
                    }
                    i.config = Some(CachedConfig {
                        cfg,
                        etag: new_etag,
                        fetched: Instant::now(),
                    });
                    changed
                });
                if changed {
                    self.with(|i| i.keys_fetched = None);
                }
                self.publish().await;
            }
            Err(error) => {
                let cached = self.with(|i| i.config.as_ref().map(|c| c.cfg.clone()));
                return match cached {
                    Some(cfg) => {
                        tracing::debug!(error = %error, "share policy refresh failed; using cache");
                        Ok(cfg)
                    }
                    None => Err(share_error(&error)),
                };
            }
        }
        let cfg = self
            .with(|i| i.config.as_ref().map(|c| c.cfg.clone()))
            .ok_or_else(unavailable)?;
        self.apply_swarm_policy();
        Ok(cfg)
    }

    /// 用户发起分享 / 接收时取最新策略（ETag 条件请求，未变时 304）：网络策略（中继、pkarr、DNS）
    /// 在后台改过的话，先按新策略重建端点，否则端点会一直向旧地址发布、对端查不到。
    pub(super) async fn fresh_config(&self) -> Result<super::cloud::CloudConfig, RpcErrorData> {
        let cfg = self.config(true).await?;
        if self.with(|i| i.endpoint_dirty) {
            self.refresh_endpoint().await;
        }
        Ok(cfg)
    }

    /// 把「云端下发的保留槽位 / 缺省每日上限」与本机设置合成后交给端点（无端点时什么也不做）。
    pub(super) fn apply_swarm_policy(&self) {
        let applied = self.with(|i| {
            let endpoint = i.endpoint.clone()?;
            let policy = i.config.as_ref().map(|c| c.cfg.swarm).unwrap_or_default();
            let cap = i
                .local
                .provide_daily_cap_bytes
                .unwrap_or(policy.provide_daily_cap_default_bytes);
            let offset = i.local.provide_day_offset_secs.unwrap_or(0);
            Some((endpoint, policy.priority_reserved_slots, cap, offset))
        });
        if let Some((endpoint, slots, cap, offset)) = applied {
            endpoint.set_swarm_policy(
                usize::try_from(slots).unwrap_or(usize::MAX),
                cap,
                i64::from(offset),
            );
        }
    }

    /// 把当日共享上传量落盘（仅在值变化时写；由登记周期与端点关闭前调用）。
    pub(super) async fn persist_daily(&self) {
        if let Some(endpoint) = self.with(|i| i.endpoint.clone()) {
            self.persist_daily_of(&endpoint).await;
        }
    }

    pub(super) async fn persist_daily_of(&self, endpoint: &ShareEndpoint) {
        let (day, used) = endpoint.swarm_daily_usage();
        let current = super::store::ProvideDaily { day, used };
        let changed = self.with(|i| match i.daily_saved {
            Some(saved) => saved != current,
            None => used > 0,
        });
        if !changed {
            return;
        }
        match self.store.save_daily(&current).await {
            Ok(()) => self.with(|i| i.daily_saved = Some(current)),
            Err(error) => {
                tracing::warn!(error = %error, "could not persist the daily provide usage");
            }
        }
    }

    /// 票据公钥环：为空或策略变化后拉取（`GET /share/keys`，公开）。
    pub(super) async fn ensure_keys(&self) {
        if !self.keys.is_empty() && self.with(|i| i.keys_fetched.is_some()) {
            return;
        }
        match self.cloud.share_keys::<CloudKeys>().await {
            Ok(response) => match self.keys.replace(&response.keys) {
                Ok(count) => {
                    tracing::debug!(count, "share ticket keys loaded");
                    self.with(|i| i.keys_fetched = Some(Instant::now()));
                }
                Err(error) => tracing::warn!(error = %error, "share ticket keys rejected"),
            },
            Err(error) => tracing::warn!(error = %error, "could not fetch share ticket keys"),
        }
    }

    // ───────────── 端点 ─────────────

    /// 取（必要时构建）端点对象；不绑定。
    pub(super) async fn endpoint(&self) -> Result<ShareEndpoint, RpcErrorData> {
        if let Some(endpoint) = self.with(|i| i.endpoint.clone()) {
            return Ok(endpoint);
        }
        let cfg = self.config(false).await?;
        if !cfg.enabled {
            return Err(unavailable());
        }
        self.ensure_keys().await;
        let local = self.with(|i| i.local.clone());
        let endpoint = ShareEndpoint::new(ShareEndpointParams {
            secret_key: self.secret.clone(),
            policy: cfg.network,
            local: convert::local_endpoint_config(&local),
            client_version: fluxdown_protocol::APP_VERSION.to_owned(),
            keys: self.keys.clone(),
            registry: self.registry.clone(),
            observers: Observers {
                events: Some(self.session_tx.clone()),
                telemetry: Some(self.telemetry.clone()),
            },
            content: self.content.clone(),
        })
        .map_err(|error| {
            tracing::warn!(error = %format!("{error:#}"), "share endpoint rejected the policy");
            unavailable()
        })?;
        endpoint
            .sender()
            .set_global_rate_limit(local.send_rate_limit_bytes_per_sec.unwrap_or(0));
        let (current, inserted) = self.with(|i| match &i.endpoint {
            Some(existing) => (existing.clone(), false),
            None => {
                i.endpoint = Some(endpoint.clone());
                i.endpoint_dirty = false;
                (endpoint.clone(), true)
            }
        });
        if inserted {
            self.apply_swarm_policy();
            if let Some(daily) = self.store.load_daily().await {
                current.restore_swarm_daily(daily.day, daily.used);
                self.with(|i| i.daily_saved = Some(daily));
            }
            self.spawn_net_watch(&current);
        }
        Ok(current)
    }

    /// 分享被停止 / 过期后不立即关端点：留一段宽限让还连着的接收方拿到「分享已结束」而不是「分享方离线」，
    /// 之后再按空闲规则收尾。一次性计时，没有周期性工作。
    pub(super) fn refresh_endpoint_later(&self) {
        // 停止 / 到期的内容立刻不再提供；端点本身留宽限期。
        self.sync_content();
        let weak = self.weak.clone();
        tokio::spawn(async move {
            tokio::time::sleep(super::service::ENDED_GRACE).await;
            if let Some(service) = weak.upgrade() {
                service.refresh_endpoint().await;
            }
        });
    }

    /// 有接收方连上来（发送方会话建立）时，领取次数 / 最近访问已在云端变化：稍后对账一次。
    /// 游客分享收不到云端推送，只能靠这里；短时间内的多个会话合并成一次请求。一次性计时。
    pub(super) fn schedule_share_sync(&self) {
        if self.with(|i| std::mem::replace(&mut i.sync_pending, true)) {
            return;
        }
        let weak = self.weak.clone();
        tokio::spawn(async move {
            tokio::time::sleep(SHARE_SYNC_COALESCE).await;
            if let Some(service) = weak.upgrade() {
                service.with(|i| i.sync_pending = false);
                service.sync_shares().await;
            }
        });
    }

    /// 登录用户的 endpoint 必须先登记到云端（自建 DNS 发布、中继准入都按登记放行）；游客在 resolve 时登记。
    pub(super) async fn ensure_registered(
        &self,
        endpoint: &ShareEndpoint,
    ) -> Result<(), RpcErrorData> {
        if !self.cloud.has_account().await {
            return Ok(());
        }
        let user = self.cloud.current_user_id().await;
        if self.with(|i| i.registered_for.is_some() && i.registered_for == user) {
            return Ok(());
        }
        self.cloud
            .share_register_endpoint::<Value>(&endpoint.id_z32())
            .await
            .map_err(|error| share_error(&error))?;
        self.with(|i| i.registered_for = user);
        Ok(())
    }

    /// 空闲静默：有有效分享或进行中的接收任务才绑定端点，否则关闭；本机开关关闭时恒不绑定。
    pub(super) async fn refresh_endpoint(&self) {
        let _guard = self.lifecycle.lock().await;
        self.sync_content();
        let enabled = self.is_enabled();
        let (needed, dirty_endpoint, receiving) = self.with(|i| {
            let receiving = i
                .receive_tasks
                .values()
                .any(super::service::ReceiveTask::is_active);
            (
                enabled
                    && (self.registry.active_count() > 0 || receiving || !self.content.is_empty()),
                i.endpoint_dirty.then(|| i.endpoint.clone()).flatten(),
                receiving,
            )
        });
        // 配置变化需要重建端点：没有进行中的接收会话时立即重建，否则等它们结束。
        if let Some(old) = dirty_endpoint
            && !receiving
        {
            self.persist_daily().await;
            old.shutdown().await;
            self.with(|i| {
                i.endpoint = None;
                i.endpoint_dirty = false;
            });
        }
        if needed {
            match self.endpoint().await {
                Ok(endpoint) => {
                    if let Err(error) = self.ensure_registered(&endpoint).await {
                        tracing::warn!(reason = ?error.reason, "share endpoint registration failed");
                    }
                    if let Err(error) = endpoint.ensure_bound().await {
                        tracing::warn!(error = %format!("{error:#}"), "share endpoint could not bind");
                    }
                }
                Err(error) => {
                    tracing::warn!(reason = ?error.reason, "share endpoint unavailable");
                }
            }
        } else if let Some(endpoint) = self.with(|i| i.endpoint.clone())
            && endpoint.is_bound()
        {
            self.persist_daily().await;
            endpoint.shutdown().await;
        }
    }

    fn spawn_net_watch(&self, endpoint: &ShareEndpoint) {
        let mut rx = endpoint.subscribe_snapshots();
        let weak = self.weak.clone();
        tokio::spawn(async move {
            while rx.changed().await.is_ok() {
                let snapshot = rx.borrow_and_update().clone();
                let Some(service) = weak.upgrade() else {
                    break;
                };
                service.on_net_snapshot(snapshot.as_deref()).await;
            }
        });
    }

    async fn on_net_snapshot(&self, snapshot: Option<&NetSnapshot>) {
        self.with(|i| {
            let (isp, province) = (i.net.isp.take(), i.net.province.take());
            i.net = match snapshot {
                None => ShareNetStatusDto {
                    isp,
                    province,
                    ..ShareNetStatusDto::default()
                },
                Some(s) => {
                    let report = s.net_report.as_ref();
                    ShareNetStatusDto {
                        bound: true,
                        online: s.home_relay.as_ref().is_some_and(|r| r.connected)
                            || report
                                .is_some_and(|r| r.global_v4.is_some() || r.global_v6.is_some()),
                        endpoint_id: Some(s.endpoint_id.clone()),
                        home_relay: s.home_relay.as_ref().map(|r| ShareHomeRelayDto {
                            url: r.url.clone(),
                            connected: r.connected,
                        }),
                        nat_type: report
                            .map_or(ShareNatType::Unknown, |r| convert::nat_type(r.nat_type)),
                        isp,
                        province,
                        ipv6: report.is_some_and(|r| r.udp_v6),
                        relay_latency_ms: report
                            .map(|r| r.relay_latency_ms.clone())
                            .unwrap_or_default(),
                    }
                }
            };
        });
        self.publish().await;
    }

    /// 云端按全局地址回传的运营商 / 省份折进网络状态。
    pub(super) async fn apply_geo(&self, response: &NetResponse) {
        let geo = response
            .global_v4_geo
            .as_ref()
            .or(response.global_v6_geo.as_ref());
        let Some(geo) = geo else {
            return;
        };
        let changed = self.with(|i| {
            let changed = i.net.isp != geo.isp || i.net.province != geo.province;
            i.net.isp.clone_from(&geo.isp);
            i.net.province.clone_from(&geo.province);
            changed
        });
        if changed {
            self.publish().await;
        }
    }

    /// 强制中断本机的会话（发送方或共享提供方），云端 `killed` 回应与 `share.sessionKill` 事件共用。
    pub(super) async fn kill_session(&self, session_id: &str) {
        if let Some(endpoint) = self.with(|i| i.endpoint.clone())
            && endpoint.sender().kill_session(session_id)
        {
            tracing::info!("share session killed by FluxCloud");
        }
    }

    pub(super) async fn on_account_changed(&self) {
        self.with(|i| i.registered_for = None);
        self.publish().await;
        if self.cloud.has_account().await {
            // 登录后：配置（权益 / 游客开关随账号变化）与分享列表重新对账。
            if let Err(error) = self.config(true).await {
                tracing::debug!(reason = ?error.reason, "share policy refresh after login failed");
            }
            self.sync_shares().await;
            self.wake_uploads();
            self.resume_login_lookups();
        }
        self.refresh_usage().await;
    }

    // ───────────── 分享登记与云端同步 ─────────────

    pub(super) fn register_local(&self, stored: &StoredShare) -> Result<(), String> {
        let state = match stored.dto.status {
            ShareStatus::Active => ShareState::Active,
            ShareStatus::Expired => ShareState::Expired,
            _ => ShareState::Stopped,
        };
        self.registry
            .insert(ShareEntry {
                manifest: stored.manifest.clone(),
                paths: stored.paths.clone(),
                state,
                expires_at: parse_rfc3339_secs(&stored.dto.expires_at),
                rate_limit_bytes_per_sec: None,
            })
            .map_err(|error| format!("{error:#}"))
    }

    /// 重启后：把仍有效的分享重新登记进发送方（文件仍在才登记），再与云端对账。
    pub(super) async fn restore_shares(&self, persisted_active: &[String]) {
        let shares = self.with(|i| i.shares.clone());
        for stored in &shares {
            if let Err(error) = self.register_local(stored) {
                tracing::warn!(error = %format!("{error:#}"), "could not restore a share");
            }
        }
        self.sync_shares().await;
        self.check_active_sources(persisted_active).await;
        self.resume_uploads().await;
        self.refresh_endpoint().await;
    }

    /// 启动时对上次运行留下、对账后仍在分享中的分享做一次源文件检查（一次性；之后只在接收方取数失败时再发现）。
    async fn check_active_sources(&self, persisted_active: &[String]) {
        let candidates: Vec<StoredShare> = self.with(|i| {
            i.shares
                .iter()
                .filter(|s| {
                    s.dto.status == ShareStatus::Active && persisted_active.contains(&s.dto.id)
                })
                .cloned()
                .collect()
        });
        for stored in candidates {
            if !sources_intact(&stored.manifest.files, &stored.paths).await {
                self.source_lost(&stored.dto.id).await;
            }
        }
    }

    /// 发送方读源文件时发现文件被删除 / 被改（`SessionEvent::SourceChanged`）：重新核对磁盘后按丢失处理。
    pub(super) async fn on_source_changed(&self, share_id: &str) {
        let Some(stored) = self.with(|i| i.shares.iter().find(|s| s.dto.id == share_id).cloned())
        else {
            return;
        };
        if stored.dto.status != ShareStatus::Active {
            return;
        }
        if sources_intact(&stored.manifest.files, &stored.paths).await {
            return;
        }
        self.source_lost(share_id).await;
    }

    /// 分享中的源文件已丢失：标记 `sourceMissing`；没有就绪的云端副本（接收方只能走本机）就按停止流程结束分享。
    /// 云端停止失败只记录警告，分享保持分享中 + `sourceMissing`，下一次云端对账时重试。
    async fn source_lost(&self, id: &str) {
        self.flag_source_missing(id);
        let has_cloud_copy = self.with(|i| {
            i.shares
                .iter()
                .find(|s| s.dto.id == id)
                .is_some_and(|s| s.dto.cloud_state == ShareCloudState::Ready)
        });
        if !has_cloud_copy {
            match self.stop(id).await {
                // 停止流程自己已落盘并发布。
                Ok(_) => return,
                Err(error) => tracing::warn!(
                    share = id,
                    reason = ?error.reason,
                    "could not stop a share whose source file is gone; retrying on the next sync"
                ),
            }
        }
        self.persist_shares().await;
        self.publish().await;
    }

    /// 把分享标记为源文件缺失；返回是否有变化。
    pub(super) fn flag_source_missing(&self, id: &str) -> bool {
        self.with(|i| {
            let Some(stored) = i.shares.iter_mut().find(|s| s.dto.id == id) else {
                return false;
            };
            !std::mem::replace(&mut stored.dto.source_missing, true)
        })
    }

    /// 上次因云端失败没能停掉的「源文件缺失」分享，在对账后再试一次。
    async fn retry_source_loss_stops(&self) {
        let ids: Vec<String> = self.with(|i| {
            i.shares
                .iter()
                .filter(|s| {
                    s.dto.source_missing
                        && s.dto.status == ShareStatus::Active
                        && s.dto.cloud_state != ShareCloudState::Ready
                })
                .map(|s| s.dto.id.clone())
                .collect()
        });
        for id in ids {
            self.source_lost(&id).await;
        }
    }

    /// `GET /share/shares` 对账：状态 / 到期 / 领取次数以云端为准；云端已不认识的本机分享视为已停止。
    /// 账号分享与游客分享分属不同归属方，各自用对应身份列出、只对账自己那一类。
    /// 请求期间本机改过分享表（`share_epoch` 变了）就作废这次列表重新请求，最多 [`SYNC_ATTEMPTS`] 次。
    pub(super) async fn sync_shares(&self) {
        if !self.is_enabled() {
            return;
        }
        self.reconcile_shares().await;
        self.retry_source_loss_stops().await;
    }

    async fn reconcile_shares(&self) {
        for _ in 0..SYNC_ATTEMPTS {
            let epoch = self.with(|i| i.share_epoch);
            let lists = self.fetch_share_lists().await;
            if lists.is_empty() {
                return;
            }
            if let Some(outcome) =
                self.with(|i| (i.share_epoch == epoch).then(|| self.apply_share_lists(i, &lists)))
            {
                self.after_share_sync(outcome).await;
                return;
            }
        }
        tracing::debug!("share list kept changing locally; reconciliation skipped");
    }

    /// 按归属列出云端分享：账号分享（已登录）与有效的游客分享各一份，失败的一份跳过。
    async fn fetch_share_lists(&self) -> Vec<(bool, Vec<CloudShare>)> {
        let mut lists: Vec<(bool, Vec<CloudShare>)> = Vec::new();
        if self.cloud.has_account().await {
            match self.cloud.share_list::<SharesList>(None).await {
                Ok(list) => lists.push((false, list.shares)),
                Err(error) => tracing::debug!(error = %error, "share list refresh failed"),
            }
        }
        let guest_live = self.with(|i| {
            i.shares
                .iter()
                .any(|s| s.guest_owned && s.dto.status == ShareStatus::Active)
        });
        if guest_live {
            match self.guest_sender_token().await {
                Ok(token) => match self.cloud.share_list::<SharesList>(Some(&token)).await {
                    Ok(list) => lists.push((true, list.shares)),
                    Err(error) => {
                        tracing::debug!(error = %error, "guest share list refresh failed");
                    }
                },
                Err(error) => {
                    tracing::debug!(reason = ?error.reason, "guest share session unavailable");
                }
            }
        }
        lists
    }

    /// 把云端列表落到本机分享表与发送方登记表；返回（是否有变化，已不再有效的分享）。
    fn apply_share_lists(
        &self,
        i: &mut Inner,
        lists: &[(bool, Vec<CloudShare>)],
    ) -> (bool, Vec<String>) {
        let mut killed = Vec::new();
        let mut changed = false;
        for stored in &mut i.shares {
            let Some((_, list)) = lists.iter().find(|(guest, _)| *guest == stored.guest_owned)
            else {
                continue;
            };
            let before = stored.dto.clone();
            match list.iter().find(|c| c.id == stored.dto.id) {
                Some(cloud) => {
                    let local = stored.dto.cloud_state;
                    apply_cloud(&mut stored.dto, cloud);
                    // 本机上传进行中：云端状态可能还没翻成 uploading，以本机为准；云端已就绪则以云端为准。
                    if i.uploads.contains_key(&stored.dto.id)
                        && stored.dto.cloud_state != ShareCloudState::Ready
                    {
                        stored.dto.cloud_state = local;
                    }
                }
                None if stored.dto.status == ShareStatus::Active => {
                    stored.dto.status = ShareStatus::Stopped;
                }
                None => {}
            }
            if stored.dto != before {
                changed = true;
            }
            let state = match stored.dto.status {
                ShareStatus::Active => ShareState::Active,
                ShareStatus::Expired => ShareState::Expired,
                _ => ShareState::Stopped,
            };
            self.registry.set_state(&stored.dto.id, state);
            self.registry
                .set_expiry(&stored.dto.id, parse_rfc3339_secs(&stored.dto.expires_at));
            if state != ShareState::Active {
                killed.push(stored.dto.id.clone());
            }
        }
        // 落地本身也算一次修改：更早发出、更晚返回的另一轮对账据此作废，不会用旧列表盖掉新结果。
        i.share_epoch += 1;
        (changed, killed)
    }

    async fn after_share_sync(&self, (changed, killed): (bool, Vec<String>)) {
        self.cancel_stale_uploads().await;
        let ended = !killed.is_empty();
        if let Some(endpoint) = self.with(|i| i.endpoint.clone()) {
            for id in killed {
                endpoint.sender().kill_share_sessions(&id);
            }
        }
        if changed {
            self.persist_shares().await;
            self.wake.notify_one();
            if ended {
                self.refresh_endpoint_later();
            } else {
                self.refresh_endpoint().await;
            }
            self.publish().await;
        }
    }

    pub(super) async fn handle_cloud_event(&self, event: &Value) {
        let kind = event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match kind {
            "share.claimed" => {
                let receiver = event.get("receiver");
                let endpoint = receiver
                    .and_then(|r| r.get("endpointId"))
                    .and_then(Value::as_str);
                let name = receiver
                    .and_then(|r| r.get("deviceName"))
                    .and_then(Value::as_str);
                if let (Some(endpoint), Some(name)) = (endpoint, name) {
                    self.with(|i| {
                        i.device_names.insert(endpoint.to_owned(), name.to_owned());
                        for meta in i.sessions.values_mut().filter(|m| m.peer == endpoint) {
                            meta.device_name = name.to_owned();
                        }
                    });
                    self.telemetry
                        .diag
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .set_peer_name(endpoint, name);
                }
                self.sync_shares().await;
                self.publish().await;
            }
            "share.sessionKill" => {
                if let Some(id) = event.get("sessionId").and_then(Value::as_str) {
                    self.kill_session(id).await;
                }
            }
            "share.changed" => self.sync_shares().await,
            "share.usage" => self.refresh_usage().await,
            _ => {}
        }
    }

    // ───────────── 持久化（写入只在状态变化时） ─────────────

    pub(super) async fn persist_shares(&self) {
        let _guard = self.persist.lock().await;
        let stored: Vec<StoredShare> = self.with(|i| {
            let mut inactive_kept = 0;
            i.shares
                .iter()
                .rev()
                .filter(|s| {
                    s.dto.status == ShareStatus::Active || {
                        inactive_kept += 1;
                        inactive_kept <= 20
                    }
                })
                .cloned()
                .map(|mut s| {
                    s.dto.active_receivers.clear();
                    s
                })
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect()
        });
        if let Err(error) = self.store.save_shares(&stored).await {
            tracing::warn!(error = %format!("{error:#}"), "could not persist shares");
        }
    }

    pub(super) async fn persist_records(&self) {
        let _guard = self.persist.lock().await;
        let records = self.with(|i| {
            let excess = i.records.len().saturating_sub(super::store::RECORD_LIMIT);
            i.records.drain(..excess);
            i.records.clone()
        });
        if let Err(error) = self.store.save_records(&records).await {
            tracing::warn!(error = %format!("{error:#}"), "could not persist receive records");
        }
    }
}

/// 云端 DTO 的可变字段合并进本机 DTO（文件清单 / 任务 ID 只在本机）。
pub(super) fn apply_cloud(dto: &mut ShareDto, cloud: &CloudShare) {
    dto.slug.clone_from(&cloud.slug);
    dto.url.clone_from(&cloud.url);
    if let Some(code) = &cloud.pickup_code {
        dto.pickup_code.clone_from(code);
    } else if cloud.status() != ShareStatus::Active {
        dto.pickup_code.clear();
    }
    dto.title.clone_from(&cloud.title);
    dto.mode = cloud.mode();
    dto.tier_id.clone_from(&cloud.tier_id);
    if let Some(expires) = &cloud.expires_at {
        dto.expires_at.clone_from(expires);
    }
    dto.max_claims = cloud
        .max_claims
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(0);
    dto.claim_count = u32::try_from(cloud.claim_count).unwrap_or(0);
    dto.notify_on_claim = cloud.notify_on_claim;
    dto.status = cloud.status();
    dto.last_accessed_at.clone_from(&cloud.last_accessed_at);
    dto.cloud_state = cloud.cloud_state();
    dto.cloud_bytes = u64::try_from(cloud.cloud_bytes).unwrap_or(0);
    dto.swarm = cloud.swarm;
    dto.swarm_locked = cloud.swarm_locked;
    dto.takedown_reason = if cloud.status() == ShareStatus::Takedown {
        cloud.takedown_reason.clone()
    } else {
        None
    };
    dto.appeal.clone_from(&cloud.appeal);
}
