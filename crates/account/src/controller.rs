//! 账户能力状态：登录会话、云设备、局域网配对与配置同步的快照/事件应用，
//! 以及需要一次性提示的状态转换（会话被结束、新的入站配对请求）。

use std::collections::BTreeMap;
use std::sync::Arc;

use fluxdown_protocol::{
    AgentEvent, AgentSessionDto, AgentSnapshot, CloudConnectionDto, CloudConnectionState,
    CloudDevice, ErrorReason, LinkDeviceInfo, LinkDiscoveredPeer, LinkPairingRequestDto,
    ServiceEvent, SyncStatusDto,
};

use crate::AccountPort;

/// 一次快照/事件应用产生的一次性通知。
#[derive(Debug, Default, Eq, PartialEq)]
pub(crate) struct Transition {
    /// 首次出现的入站配对请求（`session_id`）。
    pub new_pairing_requests: Vec<String>,
    /// agent 通知会话被非用户主动结束（`deviceUntrusted` / `sessionExpired` / `accountDisabled`）。
    pub session_revoked: Option<ErrorReason>,
    /// 账户相关状态是否发生变化；无关的服务事件（下载进度等）为 `false`，宿主据此跳过重渲染。
    pub changed: bool,
}

pub struct AccountController {
    pub(crate) port: Arc<dyn AccountPort>,
    session: Option<AgentSessionDto>,
    devices: Vec<CloudDevice>,
    cloud_connection: CloudConnectionDto,
    sync: SyncStatusDto,
    linked: Vec<LinkDeviceInfo>,
    discovered: Vec<LinkDiscoveredPeer>,
    pairing_requests: Vec<LinkPairingRequestDto>,
    lan_enabled: bool,
    lan_active: bool,
    stale: bool,
    /// 已经提示过的入站配对请求，避免重连快照重复弹窗。
    seen_requests: BTreeMap<String, i64>,
}

impl AccountController {
    #[must_use]
    pub fn new(port: Arc<dyn AccountPort>) -> Self {
        Self {
            port,
            session: None,
            devices: Vec::new(),
            cloud_connection: CloudConnectionDto::default(),
            sync: SyncStatusDto::default(),
            linked: Vec::new(),
            discovered: Vec::new(),
            pairing_requests: Vec::new(),
            lan_enabled: false,
            lan_active: false,
            stale: true,
            seen_requests: BTreeMap::new(),
        }
    }

    pub(crate) fn replace_snapshot(&mut self, snapshot: &AgentSnapshot) -> Transition {
        self.session.clone_from(&snapshot.session);
        self.devices.clone_from(&snapshot.cloud_devices);
        self.cloud_connection = if self.session.is_some() {
            snapshot.cloud_connection.clone()
        } else {
            self.devices.clear();
            CloudConnectionDto::default()
        };
        self.sync = snapshot.sync.clone();
        self.linked.clone_from(&snapshot.linked_devices);
        self.discovered.clone_from(&snapshot.link_discovered);
        self.pairing_requests
            .clone_from(&snapshot.link_pairing_requests);
        self.lan_enabled = snapshot.gateway.lan_enabled;
        self.lan_active = snapshot.gateway.lan_active;
        self.stale = false;
        Transition {
            new_pairing_requests: self.take_new_requests(),
            session_revoked: None,
            changed: true,
        }
    }

    pub(crate) fn apply_event(&mut self, event: &ServiceEvent) -> Transition {
        let ServiceEvent::Agent(event) = event else {
            return Transition::default();
        };
        let mut transition = Transition::default();
        match event {
            AgentEvent::SessionChanged(session) => {
                transition.changed = true;
                self.session.clone_from(session.as_ref());
                if self.session.is_none() {
                    // 会话结束后账号维度的数据随之失效，不留旧账号的设备。
                    self.devices.clear();
                    self.cloud_connection = CloudConnectionDto::default();
                }
            }
            AgentEvent::CloudConnectionChanged(connection) => {
                transition.changed = true;
                if self.session.is_some() {
                    self.cloud_connection.clone_from(connection);
                }
            }
            // agent 只在非用户主动结束时发送（登出 / 删除本机设备不发）。
            AgentEvent::SessionRevoked(reason) => {
                transition.changed = true;
                transition.session_revoked = Some(*reason);
            }
            AgentEvent::CloudDevicesChanged(devices) => {
                transition.changed = true;
                self.devices.clone_from(devices);
            }
            AgentEvent::SyncChanged(status) => {
                transition.changed = true;
                self.sync = status.clone();
            }
            AgentEvent::LinkedDevicesChanged(devices) => {
                transition.changed = true;
                self.linked.clone_from(devices);
            }
            AgentEvent::LinkDiscoveredChanged(peers) => {
                transition.changed = true;
                self.discovered.clone_from(peers);
            }
            AgentEvent::LinkPairingRequestsChanged(requests) => {
                transition.changed = true;
                self.pairing_requests.clone_from(requests);
                transition.new_pairing_requests = self.take_new_requests();
            }
            AgentEvent::GatewayChanged(gateway) => {
                transition.changed = true;
                self.lan_enabled = gateway.lan_enabled;
                self.lan_active = gateway.lan_active;
            }
            _ => {}
        }
        transition
    }

    /// 已存在的入站请求集合里首次出现的 ID；同时丢弃已消失请求的记录。
    /// 首次看到时记下本机截止时刻（收到时刻 + `ttlMs`），之后不再随快照重放而变。
    fn take_new_requests(&mut self) -> Vec<String> {
        self.seen_requests.retain(|id, _| {
            self.pairing_requests
                .iter()
                .any(|request| &request.session_id == id)
        });
        let now = crate::link::now_unix_ms();
        let mut fresh = Vec::new();
        for request in &self.pairing_requests {
            if !self.seen_requests.contains_key(&request.session_id) {
                let deadline =
                    crate::link::local_deadline_ms(request.ttl_ms, request.expires_at_unix_ms, now);
                self.seen_requests
                    .insert(request.session_id.clone(), deadline);
                fresh.push(request.session_id.clone());
            }
        }
        fresh
    }

    /// 入站请求的本机截止时刻（Unix 毫秒）；请求不存在时为 `None`。
    #[must_use]
    pub fn pairing_deadline_ms(&self, session_id: &str) -> Option<i64> {
        self.seen_requests.get(session_id).copied()
    }

    pub fn mark_stale(&mut self) {
        self.stale = true;
    }

    #[must_use]
    pub fn session(&self) -> Option<&AgentSessionDto> {
        self.session.as_ref()
    }

    #[must_use]
    pub fn devices(&self) -> &[CloudDevice] {
        &self.devices
    }

    pub(crate) fn cloud_connection(&self) -> &CloudConnectionDto {
        &self.cloud_connection
    }

    pub(crate) fn cloud_connected(&self) -> bool {
        self.session.is_some() && self.cloud_connection.state == CloudConnectionState::Connected
    }

    pub(crate) fn device_presence(&self, device: &CloudDevice) -> Option<bool> {
        (!self.stale && self.cloud_connected()).then_some(device.is_online)
    }

    /// 因同时在线设备数已达上限而在等待名额（连接未建立，原因为 `OnlineDeviceLimit`）。
    pub(crate) fn online_limit_waiting(&self) -> bool {
        !self.stale
            && self.session.is_some()
            && !self.cloud_connected()
            && self.cloud_connection.last_error_reason == Some(ErrorReason::OnlineDeviceLimit)
    }

    /// 能否让该设备下线：非本机、列表标记在线，且在线状态可信（已连接）或正在等待名额。
    pub(crate) fn can_disconnect_device(&self, device: &CloudDevice) -> bool {
        !device.is_current
            && device.is_online
            && (self.device_presence(device) == Some(true) || self.online_limit_waiting())
    }

    /// 套餐的同时在线设备上限（`maxOnlineDevices`）；缺失或 0（不限）为 `None`。
    pub(crate) fn max_online_devices(&self) -> Option<u64> {
        self.session
            .as_ref()
            .and_then(|session| crate::device_list::max_online_devices(&session.entitlements))
    }

    pub(crate) fn device_presence_key(&self, device: &CloudDevice) -> &'static str {
        match self.device_presence(device) {
            Some(true) => "deviceOnline",
            Some(false) => "deviceOffline",
            None => "devicePresenceUnknown",
        }
    }

    #[must_use]
    pub fn sync_status(&self) -> &SyncStatusDto {
        &self.sync
    }

    #[must_use]
    pub fn linked_devices(&self) -> &[LinkDeviceInfo] {
        &self.linked
    }

    #[must_use]
    pub fn discovered(&self) -> &[LinkDiscoveredPeer] {
        &self.discovered
    }

    #[must_use]
    pub fn pairing_requests(&self) -> &[LinkPairingRequestDto] {
        &self.pairing_requests
    }

    /// 本机网关是否对局域网开放（关闭时配对码没有可供对端输入的地址）。
    #[must_use]
    pub fn lan_enabled(&self) -> bool {
        self.lan_enabled
    }

    /// 本机网关本次运行是否实际监听非回环地址（`lan_enabled` 只是下次启动生效的持久化配置）。
    #[must_use]
    pub fn lan_active(&self) -> bool {
        self.lan_active
    }

    #[must_use]
    pub fn is_stale(&self) -> bool {
        self.stale
    }

    /// 供页面渲染函数发起额外命令（设备刷新、登出、同步开关……）。
    #[must_use]
    pub(crate) fn port(&self) -> Arc<dyn AccountPort> {
        self.port.clone()
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use std::sync::Arc;

    use fluxdown_protocol::{
        AgentSessionDto, CloudUser, Entitlements, GatewayStatusDto, LinkPairingRequestDto,
    };

    use super::*;
    use crate::{AccountCommand, PortFuture};

    struct NoopPort;

    impl AccountPort for NoopPort {
        fn execute(&self, _command: AccountCommand) -> PortFuture<serde_json::Value> {
            Box::pin(async { Ok(serde_json::Value::Null) })
        }
    }

    fn device(id: &str, current: bool) -> CloudDevice {
        CloudDevice {
            id: id.to_owned(),
            device_id: id.to_owned(),
            name: id.to_owned(),
            platform: None,
            created_at: String::new(),
            last_seen_at: String::new(),
            last_ip: None,
            app_version: None,
            is_online: false,
            is_current: current,
            default_save_dir: None,
            path_style: None,
        }
    }

    fn session() -> AgentSessionDto {
        AgentSessionDto {
            user: CloudUser {
                id: "u".to_owned(),
                email: "a@b.c".to_owned(),
                nickname: String::new(),
                plan: String::new(),
                status: fluxdown_protocol::CloudUserStatus::Active,
                created_at: String::new(),
                last_login_at: None,
                origin_id: None,
                origin_id_changed: false,
                membership_ordinal: None,
                has_password: Some(true),
            },
            entitlements: Entitlements::default(),
            current_plan: None,
            active_subscription: None,
            device: device("me", true),
        }
    }

    fn request(id: &str) -> LinkPairingRequestDto {
        LinkPairingRequestDto {
            session_id: id.to_owned(),
            peer_name: "Peer".to_owned(),
            peer_fingerprint: "fp".to_owned(),
            peer_platform: None,
            sas: "123456".to_owned(),
            requests_control: false,
            ttl_ms: 0,
            expires_at_unix_ms: 0,
        }
    }

    fn snapshot_with_session() -> AgentSnapshot {
        AgentSnapshot {
            session: Some(session()),
            cloud_devices: vec![device("me", true), device("other", false)],
            gateway: GatewayStatusDto::default(),
            ..AgentSnapshot::default()
        }
    }

    fn controller() -> AccountController {
        AccountController::new(Arc::new(NoopPort))
    }

    fn session_changed(session: Option<AgentSessionDto>) -> ServiceEvent {
        ServiceEvent::Agent(AgentEvent::SessionChanged(Box::new(session)))
    }

    #[test]
    fn presence_requires_cloud_connection_and_fresh_local_service() {
        let mut controller = controller();
        let mut snapshot = snapshot_with_session();
        snapshot.cloud_devices[1].is_online = true;
        controller.replace_snapshot(&snapshot);
        assert_eq!(controller.device_presence(&snapshot.cloud_devices[1]), None);
        for state in [
            CloudConnectionState::Connecting,
            CloudConnectionState::Reconnecting,
            CloudConnectionState::Disconnected,
        ] {
            let transition = controller.apply_event(&ServiceEvent::Agent(
                AgentEvent::CloudConnectionChanged(CloudConnectionDto {
                    state,
                    ..CloudConnectionDto::default()
                }),
            ));
            assert!(transition.changed);
            assert_eq!(
                controller.device_presence_key(&snapshot.cloud_devices[1]),
                "devicePresenceUnknown"
            );
        }
        controller.apply_event(&ServiceEvent::Agent(AgentEvent::CloudConnectionChanged(
            CloudConnectionDto {
                state: CloudConnectionState::Connected,
                ..CloudConnectionDto::default()
            },
        )));
        assert_eq!(
            controller.device_presence(&snapshot.cloud_devices[1]),
            Some(true)
        );
        // 本机标签不是在线证据。
        assert_eq!(
            controller.device_presence(&snapshot.cloud_devices[0]),
            Some(false)
        );
        controller.mark_stale();
        assert_eq!(controller.device_presence(&snapshot.cloud_devices[1]), None);
        assert_eq!(
            controller.cloud_connection().state,
            CloudConnectionState::Connected
        );
        snapshot.cloud_connection.state = CloudConnectionState::Connected;
        controller.replace_snapshot(&snapshot);
        assert_eq!(
            controller.device_presence(&snapshot.cloud_devices[1]),
            Some(true)
        );
    }

    #[test]
    fn sign_out_clears_cloud_connection_and_ignores_late_connection_events() {
        let mut controller = controller();
        let mut snapshot = snapshot_with_session();
        snapshot.cloud_connection = CloudConnectionDto {
            state: CloudConnectionState::Reconnecting,
            last_error: Some("private diagnostic".to_owned()),
            last_error_reason: Some(ErrorReason::CloudUnreachable),
        };
        controller.replace_snapshot(&snapshot);
        assert_eq!(
            controller.cloud_connection().last_error_reason,
            Some(ErrorReason::CloudUnreachable)
        );
        controller.apply_event(&session_changed(None));
        assert_eq!(
            controller.cloud_connection().state,
            CloudConnectionState::Disconnected
        );
        assert!(controller.cloud_connection().last_error.is_none());
        controller.apply_event(&ServiceEvent::Agent(AgentEvent::CloudConnectionChanged(
            snapshot.cloud_connection.clone(),
        )));
        assert!(controller.cloud_connection().last_error.is_none());
        snapshot.session = None;
        controller.replace_snapshot(&snapshot);
        assert!(controller.devices().is_empty());
        assert!(!controller.cloud_connected());
    }

    #[test]
    fn session_end_clears_devices_and_only_the_revoked_event_notifies() {
        let mut controller = controller();
        controller.replace_snapshot(&snapshot_with_session());
        assert_eq!(controller.devices().len(), 2);

        // 主动登出 / 删除本机设备：agent 只发 SessionChanged(None)，不提示。
        let transition = controller.apply_event(&session_changed(None));
        assert_eq!(transition.session_revoked, None);
        assert!(controller.devices().is_empty());
        assert!(controller.session().is_none());
    }

    #[test]
    fn revoked_event_carries_the_reason_through() {
        let mut controller = controller();
        controller.replace_snapshot(&snapshot_with_session());
        for reason in [
            ErrorReason::DeviceUntrusted,
            ErrorReason::SessionExpired,
            ErrorReason::AccountDisabled,
        ] {
            let transition =
                controller.apply_event(&ServiceEvent::Agent(AgentEvent::SessionRevoked(reason)));
            assert_eq!(transition.session_revoked, Some(reason));
        }
        // 事件本身不改会话；随后的 SessionChanged(None) 才清空。
        assert!(controller.session().is_some());
        controller.apply_event(&session_changed(None));
        assert!(controller.session().is_none());
    }

    #[test]
    fn reconnect_snapshot_never_fabricates_a_revocation() {
        let mut controller = controller();
        controller.replace_snapshot(&snapshot_with_session());
        // 重连快照里会话消失（可能是任何窗口主动登出）：没有 SessionRevoked 就不提示。
        let transition = controller.replace_snapshot(&AgentSnapshot::default());
        assert_eq!(transition.session_revoked, None);
        assert!(controller.session().is_none());
    }

    #[test]
    fn pairing_requests_are_reported_once_until_they_disappear() {
        let mut controller = controller();
        let snapshot = AgentSnapshot {
            link_pairing_requests: vec![request("a")],
            ..AgentSnapshot::default()
        };
        assert_eq!(
            controller.replace_snapshot(&snapshot).new_pairing_requests,
            vec!["a".to_owned()]
        );
        // 重连快照里仍是同一请求：不重复弹窗。
        assert!(
            controller
                .replace_snapshot(&snapshot)
                .new_pairing_requests
                .is_empty()
        );

        let event = |requests: Vec<LinkPairingRequestDto>| {
            ServiceEvent::Agent(AgentEvent::LinkPairingRequestsChanged(requests))
        };
        let transition = controller.apply_event(&event(vec![request("a"), request("b")]));
        assert_eq!(transition.new_pairing_requests, vec!["b".to_owned()]);

        // 请求消失后再次出现同一 ID 视为新请求。
        controller.apply_event(&event(Vec::new()));
        let transition = controller.apply_event(&event(vec![request("a")]));
        assert_eq!(transition.new_pairing_requests, vec!["a".to_owned()]);
    }

    #[test]
    fn gateway_event_tracks_lan_access_for_the_pairing_hint() {
        let mut controller = controller();
        assert!(!controller.lan_enabled());
        controller.apply_event(&ServiceEvent::Agent(AgentEvent::GatewayChanged(
            GatewayStatusDto {
                lan_enabled: true,
                ..GatewayStatusDto::default()
            },
        )));
        assert!(controller.lan_enabled());
    }

    #[test]
    fn only_account_events_report_change() {
        let mut controller = controller();
        assert!(controller.apply_event(&session_changed(None)).changed);
        // 下载进度等与账户无关的事件不应唤醒账户视图。
        let unrelated = ServiceEvent::Daemon(fluxdown_protocol::DaemonEvent::TaskDeleted {
            task_id: "t".to_owned(),
        });
        assert!(!controller.apply_event(&unrelated).changed);
        let unrelated = ServiceEvent::Agent(AgentEvent::DaemonConnectionChanged(true));
        assert!(!controller.apply_event(&unrelated).changed);
    }
}
