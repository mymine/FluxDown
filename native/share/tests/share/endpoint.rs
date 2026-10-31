//! 端点生命周期、策略装配、快照与上报、地址发布。

use std::time::Duration;

use fluxdown_share::{
    ConnectionMode, KeyRing, LocalEndpointConfig, Nameserver, NameserverProtocol, NatType,
    NetworkPolicy, Observers, RelayNode, SecretKey, ShareEndpoint, ShareEndpointParams,
    ShareRegistry, nat_type_of,
};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;

use crate::common::{offline_policy, peer, peer_with};

fn params(policy: NetworkPolicy, local: LocalEndpointConfig) -> ShareEndpointParams {
    ShareEndpointParams {
        secret_key: SecretKey::generate(),
        policy,
        local,
        client_version: "0.0.0-test".into(),
        keys: KeyRing::new(),
        registry: ShareRegistry::new(),
        content: fluxdown_share::ContentRegistry::new(),
        observers: Observers::default(),
    }
}

#[tokio::test]
async fn endpoint_is_lazy_and_rebindable() {
    let p = peer(KeyRing::new());
    // 构造不绑定，没有快照，id 来自私钥。
    assert!(!p.ep.is_bound());
    assert!(p.ep.snapshot().is_none());
    assert_eq!(p.ep.id_z32().len(), 52);
    p.ep.shutdown().await; // 未绑定时关闭是空操作。

    let first = p.ep.ensure_bound().await.unwrap();
    assert!(p.ep.is_bound());
    // 重复调用复用同一个端点。
    assert_eq!(p.ep.ensure_bound().await.unwrap().id(), first.id());
    assert_eq!(first.id(), p.ep.id());

    p.ep.shutdown().await;
    assert!(!p.ep.is_bound());
    assert!(first.is_closed());
    assert!(p.ep.snapshot().is_none());

    // 关闭后可再次绑定，ID 不变。
    let again = p.ep.ensure_bound().await.unwrap();
    assert!(!again.is_closed());
    assert_eq!(again.id(), p.ep.id());
}

#[tokio::test]
async fn snapshot_has_generic_metrics_and_is_deduplicated() {
    let p = peer_with(KeyRing::new(), 1, LocalEndpointConfig::default());
    p.ep.ensure_bound().await.unwrap();

    let mut rx = p.ep.subscribe_snapshots();
    tokio::time::timeout(Duration::from_secs(10), rx.wait_for(Option::is_some))
        .await
        .unwrap()
        .unwrap();

    let snap = p.ep.snapshot().unwrap();
    assert_eq!(snap.endpoint_id, p.ep.id_z32());
    assert_eq!(snap.client_version, "0.0.0-test");
    assert!(
        ["windows", "macos", "linux", "android", "ios"].contains(&snap.platform.as_str()),
        "{}",
        snap.platform
    );
    assert!(!snap.addrs.is_empty(), "bound sockets are advertised");
    assert!(snap.captured_at.ends_with('Z') && snap.captured_at.contains('T'));
    // 指标是遍历全部指标组得到的：键为 `<group>.<name>`，数量不是手挑的几个。
    assert!(snap.metrics.len() > 20, "{} metrics", snap.metrics.len());
    assert!(snap.metrics.keys().all(|k| k.contains('.')));
    assert!(snap.metrics.keys().any(|k| k.starts_with("socket.")));

    // 无中继策略：没有 home relay，也不会等不存在的 net report。
    assert!(snap.home_relay.is_none());

    // 去重：同样的状态只上报一次；等一会儿也不会重复。
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let nets = p.sink.nets.lock().unwrap().clone();
    assert_eq!(
        nets.len(),
        1,
        "identical network state is reported once, got {}",
        nets.len()
    );
    assert!(nets[0].same_state(&snap));
    assert!(p.ep.home_relay().is_none());

    // JSON 形状符合 §9.4。
    let json = serde_json::to_value(&snap).unwrap();
    for key in [
        "capturedAt",
        "endpointId",
        "clientVersion",
        "platform",
        "addrs",
        "metrics",
    ] {
        assert!(json.get(key).is_some(), "{key}");
    }
    p.ep.shutdown().await;
    assert!(rx.borrow().is_none(), "snapshot cleared on shutdown");
}

#[test]
fn nat_type_is_derived_from_mapping_variance() {
    assert_eq!(nat_type_of(Some(false)), NatType::Cone);
    assert_eq!(nat_type_of(Some(true)), NatType::Symmetric);
    assert_eq!(nat_type_of(None), NatType::Unknown);
    assert_eq!(
        serde_json::to_value(NatType::Symmetric).unwrap(),
        "symmetric"
    );
}

#[tokio::test]
async fn policy_and_local_config_are_validated() {
    // 钉住的中继不在策略里。
    let local = LocalEndpointConfig {
        pinned_relay: Some("https://nope.example/".into()),
        ..LocalEndpointConfig::default()
    };
    assert!(ShareEndpoint::new(params(offline_policy(5), local)).is_err());

    // 设置了 dnsOrigin 却没有 nameserver：必须报错，而不是静默回落到公共 DNS。
    let mut policy = offline_policy(5);
    policy.dns_origin = "dns.example.test".into();
    let err =
        ShareEndpoint::new(params(policy.clone(), LocalEndpointConfig::default())).unwrap_err();
    assert!(format!("{err:#}").contains("nameserver"), "{err:#}");
    policy.nameservers = vec![Nameserver {
        addr: "not-an-ip".into(),
        protocol: NameserverProtocol::Udp,
    }];
    assert!(ShareEndpoint::new(params(policy.clone(), LocalEndpointConfig::default())).is_err());
    policy.nameservers = vec![
        Nameserver {
            addr: "127.0.0.1:5353".into(),
            protocol: NameserverProtocol::Udp,
        },
        Nameserver {
            addr: "[::1]".into(),
            protocol: NameserverProtocol::Tcp,
        },
        Nameserver {
            addr: "127.0.0.1".into(),
            protocol: NameserverProtocol::Https,
        },
    ];
    policy.doh_url = Some("https://dns.example.test/dns-query".into());
    ShareEndpoint::new(params(policy, LocalEndpointConfig::default())).unwrap();

    // 非法额外地址 / 非法中继 URL。
    let local = LocalEndpointConfig {
        extra_addrs: vec!["nonsense".into()],
        ..LocalEndpointConfig::default()
    };
    assert!(ShareEndpoint::new(params(offline_policy(5), local)).is_err());
    let mut policy = offline_policy(5);
    policy.relays = vec![RelayNode {
        node_id: "n1".into(),
        url: "not a url".into(),
        region: "x".into(),
        quic_port: None,
        auth_token: None,
    }];
    assert!(ShareEndpoint::new(params(policy, LocalEndpointConfig::default())).is_err());

    // 仅中继模式但策略没有中继：绑定时报错。
    let local = LocalEndpointConfig {
        mode: ConnectionMode::RelayOnly,
        ..LocalEndpointConfig::default()
    };
    let ep = ShareEndpoint::new(params(offline_policy(5), local)).unwrap();
    assert!(ep.ensure_bound().await.is_err());
    assert!(!ep.is_bound());
}

#[tokio::test]
async fn network_policy_json_matches_the_contract() {
    let json = serde_json::json!({
        "relays": [{ "nodeId": "n1", "url": "https://relay1.example.test", "region": "cn-bj" }],
        "dnsOrigin": "dns.example.test",
        "pkarrUrl": "https://api.example.test/pkarr",
        "nameservers": [{ "addr": "203.0.113.5:53", "protocol": "udp" }, { "addr": "203.0.113.5", "protocol": "https" }],
        "dohUrl": "https://dns.example.test/dns-query",
        "pkarrTtlSecs": 30,
        "republishSecs": 300,
        "publishDirectAddrs": true,
        "portmapper": false,
        "holePunchHintSecs": 8,
        "reportIntervalSecs": 5
    });
    let policy: NetworkPolicy = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(policy.relays[0].url, "https://relay1.example.test");
    assert_eq!(policy.nameservers[1].protocol, NameserverProtocol::Https);
    assert!(policy.publish_direct_addrs);
    // 回写后保留契约字段名。
    let back = serde_json::to_value(&policy).unwrap();
    for key in [
        "dnsOrigin",
        "pkarrUrl",
        "pkarrTtlSecs",
        "republishSecs",
        "publishDirectAddrs",
        "holePunchHintSecs",
        "reportIntervalSecs",
    ] {
        assert!(back.get(key).is_some(), "{key}");
    }
    // 配置了中继 / pkarr / DNS 的完整策略可以装配（不触网）。
    ShareEndpoint::new(params(policy, LocalEndpointConfig::default())).unwrap();
}

#[tokio::test]
async fn pkarr_publisher_targets_only_the_policy_url() {
    // 本地假 pkarr 服务：记录请求行。
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let tx = tx.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let mut got = 0;
                while got < buf.len() {
                    let n = stream.read(&mut buf[got..]).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    got += n;
                    if buf[..got].windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let line = String::from_utf8_lossy(&buf[..got])
                    .lines()
                    .next()
                    .unwrap_or("")
                    .to_owned();
                tx.send(line).unwrap();
                let reply = b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";
                if let Err(e) = stream.write_all(reply).await {
                    eprintln!("fake pkarr reply failed: {e}");
                }
            });
        }
    });

    let mut policy = offline_policy(5);
    policy.pkarr_url = format!("http://127.0.0.1:{port}/pkarr");
    policy.publish_direct_addrs = true;
    policy.pkarr_ttl_secs = 17;
    let ep = ShareEndpoint::new(params(policy, LocalEndpointConfig::default())).unwrap();
    ep.ensure_bound().await.unwrap();

    let line = tokio::time::timeout(Duration::from_secs(15), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        line.starts_with(&format!("PUT /pkarr/{} ", ep.id_z32())),
        "unexpected request line: {line}"
    );
    ep.shutdown().await;
}

#[tokio::test]
async fn explicit_udp_ports_and_direct_only_mode_bind() {
    // 先让系统挑一个空闲 UDP 端口再释放，交给端点显式绑定。
    let probe = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);
    let local = LocalEndpointConfig {
        udp_port_v4: Some(port),
        mode: ConnectionMode::DirectOnly,
        portmapper: Some(false),
        ..LocalEndpointConfig::default()
    };
    let ep = ShareEndpoint::new(params(offline_policy(5), local)).unwrap();
    let bound = ep.ensure_bound().await.unwrap();
    assert!(
        bound
            .bound_sockets()
            .iter()
            .any(|a| a.is_ipv4() && a.port() == port),
        "{:?}",
        bound.bound_sockets()
    );
    ep.shutdown().await;
}

/// 策略里有（不可达的）中继：端点照常绑定，net report 在探测失败后仍会产出，
/// 快照里带中继延迟为空、NAT 类型未知；全程只会联系策略里的中继。
#[tokio::test]
async fn policy_relay_is_used_and_reported_even_when_unreachable() {
    let mut policy = offline_policy(5);
    policy.relays = vec![RelayNode {
        node_id: "n1".into(),
        url: "https://127.0.0.1:9".into(),
        region: "test".into(),
        quic_port: Some(9),
        auth_token: Some("t".into()),
    }];
    let ep = ShareEndpoint::new(params(policy.clone(), LocalEndpointConfig::default())).unwrap();
    ep.ensure_bound().await.unwrap();
    let mut rx = ep.subscribe_snapshots();
    let snap = tokio::time::timeout(Duration::from_secs(30), rx.wait_for(Option::is_some))
        .await
        .unwrap()
        .unwrap()
        .clone()
        .unwrap();
    let report = snap.net_report.clone().unwrap();
    assert!(!report.udp_v4 && !report.udp_v6);
    assert_eq!(report.nat_type, NatType::Unknown);
    assert!(report.relay_latency_ms.is_empty());
    assert!(report.preferred_relay.is_none());
    ep.shutdown().await;

    // 钉住中继 + 仅中继模式（不绑定 IP 传输）也能装配并绑定。
    let local = LocalEndpointConfig {
        mode: ConnectionMode::RelayOnly,
        pinned_relay: Some("https://127.0.0.1:9/".into()),
        ..LocalEndpointConfig::default()
    };
    let ep = ShareEndpoint::new(params(policy, local)).unwrap();
    let bound = ep.ensure_bound().await.unwrap();
    assert!(
        bound.bound_sockets().is_empty(),
        "{:?}",
        bound.bound_sockets()
    );
    ep.shutdown().await;
}
