//! 发现层：mDNS 局域网自动发现（广播 + 浏览）+ 手动地址 `/ping` 探测。
//!
//! # 两个职责（设计文档 §6.5）
//! 1. **发现设备以加入名册**（免账号本地配对入口）：浏览 `_fluxdown._tcp.local.`。
//! 2. **为已知设备找最快连接路径**：mDNS 得到的 `ip:port` 即 Direct 候选。
//!
//! **扩展点**：发现方式是可替换策略；未来可加其他发现源（如账户名册回填、二维码
//! 带地址），只要往 [`DiscoveredPeer`] 汇流即可，配对/传输层无感。mDNS 广播/浏览
//! 各自独立运行在 mdns-sd 自建线程上（不阻塞宿主的 async runtime）。
//!
//! # 隐私权衡（可发现性的固有取舍，非编码缺陷）
//! 广播的 TXT 记录明文携带 `fp`（长期身份指纹）/`name`（设备名）/`plat`/
//! `ver`（平台/版本）：局域网内任何主机都能被动监听或主动查询
//! `_fluxdown._tcp.local.` 枚举出这些信息，完全不需要任何配对码。这是
//! 「让本机可被发现」这一功能自身的代价，仅凭这些信息也拼不出配对所需的
//! 临时密钥/共享密钥，无法完成配对。广播**不含**任何配对标志，也不含云账号信息。
//! 配对窗口（配对码有效期）内会广播；名册里有未解除的已配对设备时保持常驻的存在性广播
//! （让对端在本机换 IP / 端口后重新找到本机，见 `LinkManager::refresh_presence`）；
//! 其余时候调用 `LinkManager::stop_advertising` 撤回，缩短暴露窗口。

use std::net::{IpAddr, Ipv4Addr, UdpSocket};
use std::time::Duration;

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use tokio::sync::mpsc;

use super::address::PeerAddress;
use super::error::{LinkError, LinkResult};
use super::text::{MAX_NAME_CHARS, MAX_PLATFORM_CHARS, MAX_VERSION_CHARS, sanitize_optional};
use super::types::{DiscoveredPeer, DiscoveryKind};
use super::wire::{classify_failure, read_json_object, send_failure};

/// FluxDown 局域网服务类型（DNS-SD）。
pub const SERVICE_TYPE: &str = "_fluxdown._tcp.local.";

/// TXT 记录键。
const TXT_FINGERPRINT: &str = "fp";
const TXT_NAME: &str = "name";
const TXT_PLATFORM: &str = "plat";
const TXT_VERSION: &str = "ver";

fn map_mdns_err(e: mdns_sd::Error) -> LinkError {
    LinkError::Io(e.to_string())
}

/// mDNS 广播器：向局域网通告本设备的 fluxdown 服务（供其他设备发现并配对）。
/// 持有 daemon 句柄，`Drop` 时优雅关闭。
pub struct MdnsAdvertiser {
    daemon: ServiceDaemon,
}

impl MdnsAdvertiser {
    /// 开始广播。`port` 为本机 fluxdown API 端口；TXT 携带身份指纹/名称/平台/版本。
    pub fn start(
        port: u16,
        fingerprint: &str,
        name: &str,
        platform: Option<&str>,
        app_version: Option<&str>,
    ) -> LinkResult<Self> {
        let daemon = ServiceDaemon::new().map_err(map_mdns_err)?;
        let info = Self::service_info(port, fingerprint, name, platform, app_version)?;
        daemon.register(info).map_err(map_mdns_err)?;
        Ok(Self { daemon })
    }

    /// 将已有广告重新通告为新的端口 / 设备名，沿用同一 daemon、服务名与设备身份。
    pub(crate) fn update(
        &self,
        port: u16,
        fingerprint: &str,
        name: &str,
        platform: Option<&str>,
        app_version: Option<&str>,
    ) -> LinkResult<()> {
        let info = Self::service_info(port, fingerprint, name, platform, app_version)?;
        // mdns-sd 用相同 fullname 原地替换服务，无需 unregister（避免广告空窗）。
        self.daemon.register(info).map_err(map_mdns_err)
    }

    fn service_info(
        port: u16,
        fingerprint: &str,
        name: &str,
        platform: Option<&str>,
        app_version: Option<&str>,
    ) -> LinkResult<ServiceInfo> {
        // 实例名用短指纹保证唯一（同名设备不冲突）；host_name 走 <fp>.local.。
        let short_fp: String = fingerprint.chars().take(12).collect();
        let host_name = format!("{short_fp}.local.");
        let props = [
            (TXT_FINGERPRINT, fingerprint),
            (TXT_NAME, name),
            (TXT_PLATFORM, platform.unwrap_or("")),
            (TXT_VERSION, app_version.unwrap_or("")),
        ];
        // ip 传空 + enable_addr_auto()：由 mdns-sd 自动探测并跟踪本机接口地址。
        ServiceInfo::new(SERVICE_TYPE, &short_fp, &host_name, "", port, &props[..])
            .map(ServiceInfo::enable_addr_auto)
            .map_err(map_mdns_err)
    }
}

impl Drop for MdnsAdvertiser {
    fn drop(&mut self) {
        if let Err(error) = self.daemon.shutdown() {
            tracing::warn!(%error, "mDNS shutdown failed");
        }
    }
}

/// 浏览器汇出的事件：解析出一台设备，或一个服务实例消失（对端下线 / 记录 TTL 过期）。
#[derive(Debug, Clone)]
pub enum BrowseEvent {
    Found(DiscoveredPeer),
    /// 服务实例名：`<短指纹>._fluxdown._tcp.local.` 的第一段，即设备指纹的前 12 位
    /// （见 `MdnsAdvertiser::service_info`）。
    Lost {
        instance: String,
    },
}

/// mDNS 服务实例全名 → 实例名（第一段，非空）。
fn instance_of(fullname: &str) -> Option<String> {
    let first = fullname.split('.').next()?;
    (!first.is_empty()).then(|| first.to_owned())
}

/// mDNS 浏览事件 → [`BrowseEvent`]（与网络无关的纯映射，便于单测）。
fn browse_event(event: ServiceEvent) -> Option<BrowseEvent> {
    match event {
        ServiceEvent::ServiceResolved(info) => resolved_to_peer(&info).map(BrowseEvent::Found),
        ServiceEvent::ServiceRemoved(_, fullname) => {
            instance_of(&fullname).map(|instance| BrowseEvent::Lost { instance })
        }
        _ => None,
    }
}

/// mDNS 浏览器：发现局域网内的 fluxdown 设备，解析后经 `sink` 汇出
/// [`BrowseEvent`]。持有 daemon 句柄，`Drop` 时优雅关闭。
pub struct MdnsBrowser {
    daemon: ServiceDaemon,
}

impl MdnsBrowser {
    /// 开始浏览，把解析出的设备 / 消失的实例推送到 `sink`（满时丢弃，关闭后退出，不阻塞）。
    pub fn start(sink: mpsc::Sender<BrowseEvent>) -> LinkResult<Self> {
        let daemon = ServiceDaemon::new().map_err(map_mdns_err)?;
        let receiver = daemon.browse(SERVICE_TYPE).map_err(map_mdns_err)?;
        tokio::spawn(async move {
            let mut full_reported = false;
            while let Ok(event) = receiver.recv_async().await {
                if let Some(event) = browse_event(event) {
                    match sink.try_send(event) {
                        Ok(()) => full_reported = false,
                        Err(mpsc::error::TrySendError::Full(_)) => {
                            if !full_reported {
                                tracing::debug!(
                                    "mDNS discovery sink full; dropping discoveries until drained"
                                );
                                full_reported = true;
                            }
                        }
                        Err(mpsc::error::TrySendError::Closed(_)) => {
                            tracing::debug!("mDNS discovery sink closed");
                            break;
                        }
                    }
                }
            }
        });
        Ok(Self { daemon })
    }
}

impl Drop for MdnsBrowser {
    fn drop(&mut self) {
        if let Err(error) = self.daemon.shutdown() {
            tracing::warn!(%error, "mDNS shutdown failed");
        }
    }
}

/// 把解析出的 mDNS 服务映射为 [`DiscoveredPeer`]（IPv4 地址经 `pick_best_v4`
/// 按可达性优先级挑选，而非不加区分地取第一个）。广播内容来自局域网任意主机，一律不信任：
/// 指纹必须是 64 位小写 hex，名称 / 平台 / 版本剔除控制与双向控制字符并限长。
fn resolved_to_peer(info: &mdns_sd::ResolvedService) -> Option<DiscoveredPeer> {
    let candidates: Vec<Ipv4Addr> = info.get_addresses_v4().into_iter().collect();
    let addr = pick_best_v4(&candidates)?;
    let fingerprint = info
        .get_property_val_str(TXT_FINGERPRINT)
        .filter(|s| {
            s.len() == 64
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
        .map(str::to_string);
    let name = sanitize_optional(info.get_property_val_str(TXT_NAME), MAX_NAME_CHARS)
        .unwrap_or_else(|| addr.to_string());
    let platform = sanitize_optional(info.get_property_val_str(TXT_PLATFORM), MAX_PLATFORM_CHARS);
    let app_version = sanitize_optional(info.get_property_val_str(TXT_VERSION), MAX_VERSION_CHARS);
    Some(DiscoveredPeer {
        fingerprint,
        name,
        platform,
        host: addr.to_string(),
        port: info.get_port(),
        app_version,
        kind: DiscoveryKind::Mdns,
    })
}

/// 从候选 IPv4 地址中选出「最可能可达」的一个。
///
/// 广播端用 `enable_addr_auto()` 会把本机所有网卡地址都塞进服务记录；若对端
/// 同时开着 Docker/WSL/VPN 等虚拟网卡，不加区分地取第一个很可能选中一个从
/// 浏览方根本连不通的地址，表现为「发现到了但怎么都连不上」。
///
/// 排除：回环 `127.0.0.0/8`、link-local `169.254.0.0/16`、Docker 默认桥接
/// 网段 `172.17.0.0/16`（几乎总是不可达的容器内部地址）。
/// 优先级（数字越小越优先）：`192.168.0.0/16` 家庭/办公网常见网段 >
/// `10.0.0.0/8` 路由器下发/企业网常见网段 > 其余 `172.16.0.0/12` 私网 >
/// 其他（含公网）地址。都不命中优先私网段时，回退到第一个未被排除的地址
/// （`Iterator::min_by_key` 对并列最小值保留原始顺序中的第一个）；全部被
/// 排除则返回 `None`。
///
/// **启发式，可能选错**：这只是「多个网段里挑一个最可能通」的经验排序，
/// 从未实际探测任何一个候选地址的可达性（不发包、不比对指纹）。调用方
/// ——尤其是刷新**已配对设备**回连候选的路径（见
/// `crate::manager::LinkManager::start_discovery`）——必须把这里
/// 选出的地址当作「值得优先一试」而非「确认可达」，保留原有候选作为
/// 回退，不能直接覆盖掉配对时验证过的旧地址。
#[must_use]
fn pick_best_v4(addrs: &[Ipv4Addr]) -> Option<Ipv4Addr> {
    fn is_excluded(ip: &Ipv4Addr) -> bool {
        let o = ip.octets();
        ip.is_loopback() || ip.is_link_local() || (o[0] == 172 && o[1] == 17)
    }
    fn priority(ip: &Ipv4Addr) -> u8 {
        let o = ip.octets();
        if o[0] == 192 && o[1] == 168 {
            0
        } else if o[0] == 10 {
            1
        } else if o[0] == 172 && (16..=31).contains(&o[1]) {
            2
        } else {
            3
        }
    }
    addrs
        .iter()
        .filter(|ip| !is_excluded(ip))
        .min_by_key(|ip| priority(ip))
        .copied()
}

/// GET `{base}/ping` 并确认应答来自 FluxDown，返回应答 JSON 对象。
///
/// 错误语义：网络失败 → [`LinkError::Io`]（带完整根因）；对端（或反代）返回的不是
/// FluxDown `/ping` 应答 → [`LinkError::NotFluxDown`]。
async fn fetch_ping(
    client: &reqwest::Client,
    address: &PeerAddress,
) -> LinkResult<serde_json::Value> {
    let resp = client
        .get(address.url("/ping"))
        .timeout(Duration::from_secs(3))
        .send()
        .await
        .map_err(|e| send_failure(&e))?;
    if !resp.status().is_success() {
        return Err(classify_failure(resp).await);
    }
    let json = read_json_object(resp).await?;
    if json.get("app").and_then(|v| v.as_str()) != Some("FluxDown") {
        return Err(LinkError::NotFluxDown(
            "/ping response is not from FluxDown".into(),
        ));
    }
    Ok(json)
}

/// 手动地址探测：GET `{base}/ping`，解析设备身份/名称/平台/版本。
/// 供「本地配对」的「手动输入地址」兜底路径（Docker bridge / AP 隔离等 mDNS 失效场景），
/// 地址可以是 https / 域名 / 带反代子路径（见 [`PeerAddress`]）。
///
/// 错误语义见 [`fetch_ping`]。
pub async fn probe(client: &reqwest::Client, address: &PeerAddress) -> LinkResult<DiscoveredPeer> {
    let json = fetch_ping(client, address).await?;
    let get = |k: &str| {
        json.get(k)
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    // `/ping` 的名称 / 平台 / 版本来自对端自报，剔除控制 / 双向控制字符并限长后才展示。
    let clean = |k: &str, max| sanitize_optional(json.get(k).and_then(|v| v.as_str()), max);
    Ok(DiscoveredPeer {
        fingerprint: get("linkFingerprint"),
        name: clean("linkName", MAX_NAME_CHARS).unwrap_or_else(|| address.host().to_string()),
        platform: clean("linkPlatform", MAX_PLATFORM_CHARS),
        host: address.host().to_string(),
        port: address.port(),
        app_version: clean("version", MAX_VERSION_CHARS),
        kind: DiscoveryKind::Manual,
    })
}

/// GET `{base}/ping` 读取对端自报的设备控制配对能力版本（`linkControl`；旧版对端没有该字段 = 0）。
/// 发起方请求「完全控制」前用它预检，错误语义见 [`fetch_ping`]。
pub async fn probe_link_control(
    client: &reqwest::Client,
    address: &PeerAddress,
) -> LinkResult<u32> {
    let json = fetch_ping(client, address).await?;
    Ok(json
        .get("linkControl")
        .and_then(serde_json::Value::as_u64)
        .and_then(|version| u32::try_from(version).ok())
        .unwrap_or(0))
}

/// 计算本机朝向 `peer_host` 的出站本地 IP（UDP connect 技巧，不真正发包），
/// 拼成 Direct 候选 `ip:port`（`api_port` = 本机 fluxdown API 端口）。
///
/// 供配对时向对端自报可达地址（对端存为回连候选）。探测失败返回空列表。
#[must_use]
pub fn local_direct_addrs(peer_host: &str, api_port: u16) -> Vec<String> {
    let Ok(peer_ip) = peer_host.parse::<IpAddr>() else {
        return Vec::new();
    };
    let bind = if peer_ip.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let Ok(sock) = UdpSocket::bind(bind) else {
        return Vec::new();
    };
    // connect 只设置默认目的地，不发包；随后 local_addr 给出朝该目的地的本地 IP。
    if sock.connect((peer_ip, api_port.max(1))).is_err() {
        return Vec::new();
    }
    match sock.local_addr() {
        Ok(local) => vec![match local.ip() {
            IpAddr::V4(ip) => format!("{ip}:{api_port}"),
            IpAddr::V6(ip) => format!("[{ip}]:{api_port}"),
        }],
        Err(_) => Vec::new(),
    }
}

/// 本机各网段的可回连地址（`ip:port`，至多 [`crate::types::MAX_DIRECT_CANDIDATES`] 条）：对几个
/// 常见目的地（公网 / 192.168 / 10 / 172.16 网段）各做一次出站网卡探测（UDP connect，不发包），
/// 滤掉回环 / 通配 / link-local 后去重。没有具体对端地址可参照时（响应方在数据面信息交换里
/// 自报地址）使用；只看得到各目的地的默认出站网卡，多网卡 / 容器 / VPN 环境下用
/// `FLUXDOWN_LINK_ADVERTISE_ADDR` 覆盖。
#[must_use]
pub fn default_route_addrs(api_port: u16) -> Vec<String> {
    let mut addrs: Vec<String> = Vec::new();
    for target in ["8.8.8.8", "192.168.0.1", "10.0.0.1", "172.16.0.1"] {
        for candidate in local_direct_addrs(target, api_port) {
            let usable =
                candidate
                    .parse::<std::net::SocketAddr>()
                    .is_ok_and(|addr| match addr.ip() {
                        IpAddr::V4(v4) => {
                            !v4.is_loopback() && !v4.is_unspecified() && !v4.is_link_local()
                        }
                        IpAddr::V6(v6) => !v6.is_loopback() && !v6.is_unspecified(),
                    });
            if usable && !addrs.contains(&candidate) {
                addrs.push(candidate);
            }
        }
    }
    addrs.truncate(crate::types::MAX_DIRECT_CANDIDATES);
    addrs
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn local_addr_towards_loopback_is_loopback() {
        let addrs = local_direct_addrs("127.0.0.1", 17800);
        assert_eq!(addrs.len(), 1);
        assert!(addrs[0].starts_with("127.0.0.1:") || addrs[0].starts_with("127."));
        assert!(addrs[0].ends_with(":17800"));
    }

    #[test]
    fn local_addr_towards_garbage_is_empty() {
        assert!(local_direct_addrs("not-an-ip", 17800).is_empty());
    }

    #[test]
    fn pick_best_v4_prefers_192_168_then_10_then_other_private() {
        // 全量四个地址：优先命中 192.168/16。
        let all = [
            Ipv4Addr::new(172, 20, 0, 5),
            Ipv4Addr::new(203, 0, 113, 9), // 公网地址，优先级最低
            Ipv4Addr::new(10, 0, 0, 5),
            Ipv4Addr::new(192, 168, 1, 5),
        ];
        assert_eq!(pick_best_v4(&all), Some(Ipv4Addr::new(192, 168, 1, 5)));
        // 去掉 192.168 后应退而求其次选中 10/8。
        assert_eq!(pick_best_v4(&all[..3]), Some(Ipv4Addr::new(10, 0, 0, 5)));
    }

    #[test]
    fn pick_best_v4_excludes_loopback_link_local_and_docker_bridge() {
        // 三个排除项之外仅剩一个公网地址，必须回退到它而不是径直选第一个
        // （第一个是被排除的回环地址）。
        let addrs = [
            Ipv4Addr::new(127, 0, 0, 1),
            Ipv4Addr::new(169, 254, 1, 1),
            Ipv4Addr::new(172, 17, 0, 2),
            Ipv4Addr::new(203, 0, 113, 9),
        ];
        assert_eq!(pick_best_v4(&addrs), Some(Ipv4Addr::new(203, 0, 113, 9)));
    }

    #[test]
    fn pick_best_v4_all_excluded_returns_none() {
        let addrs = [
            Ipv4Addr::new(127, 0, 0, 1),
            Ipv4Addr::new(169, 254, 1, 1),
            Ipv4Addr::new(172, 17, 0, 2),
        ];
        assert!(pick_best_v4(&addrs).is_none());
    }

    #[test]
    fn removed_service_instances_map_to_lost_events_keyed_by_the_short_fingerprint() {
        let fingerprint = "ab12cd34ef56".repeat(5) + "abcd";
        let short: String = fingerprint.chars().take(12).collect();
        let removed = ServiceEvent::ServiceRemoved(
            SERVICE_TYPE.to_owned(),
            format!("{short}.{SERVICE_TYPE}"),
        );
        match browse_event(removed) {
            Some(BrowseEvent::Lost { instance }) => assert_eq!(instance, short),
            other => panic!("expected a Lost event, got {other:?}"),
        }
        // 没有实例名的残缺事件被忽略；与发现无关的事件不产生任何输出。
        assert!(
            browse_event(ServiceEvent::ServiceRemoved(
                SERVICE_TYPE.to_owned(),
                String::new()
            ))
            .is_none()
        );
        assert!(browse_event(ServiceEvent::SearchStopped(SERVICE_TYPE.to_owned())).is_none());
    }
}
