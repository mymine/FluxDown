//! 本机 agent RPC 端点发现：显式覆盖优先，每次连接重读已发布地址。

use std::borrow::Cow;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::Path;

use fluxdown_protocol::local_service::{GATEWAY_ENDPOINT_FILE, LocalEndpoint};
use tokio_tungstenite::tungstenite::http::Uri;

pub(crate) const DEFAULT_RPC_URL: &str = "ws://127.0.0.1:17800/rpc";

pub(crate) async fn discover_rpc_url<'a>(
    override_url: Option<&'a str>,
    bearer_path: &Path,
) -> Result<Cow<'a, str>, EndpointError> {
    if let Some(url) = override_url {
        socket_target(url)?;
        return Ok(Cow::Borrowed(url));
    }
    let path = bearer_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(GATEWAY_ENDPOINT_FILE);
    let bytes = match tokio::fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Cow::Borrowed(DEFAULT_RPC_URL));
        }
        Err(source) => return Err(EndpointError::Read { path, source }),
    };
    let endpoint: LocalEndpoint =
        serde_json::from_slice(&bytes).map_err(|source| EndpointError::Parse {
            path: path.clone(),
            source,
        })?;
    socket_target(&endpoint.rpc_url)
        .map_err(|error| EndpointError::Invalid(format!("{}: {error}", path.display())))?;
    Ok(Cow::Owned(endpoint.rpc_url))
}

pub(crate) fn socket_target(rpc_url: &str) -> Result<SocketAddr, EndpointError> {
    let uri = rpc_url
        .parse::<Uri>()
        .map_err(|error| EndpointError::Invalid(error.to_string()))?;
    if rpc_url.contains('#')
        || uri.scheme_str() != Some("ws")
        || uri.path_and_query().map(|path| path.as_str()) != Some("/rpc")
    {
        return Err(EndpointError::Invalid(
            "agent URL must use ws and the exact /rpc path".to_owned(),
        ));
    }
    let authority = uri
        .authority()
        .ok_or_else(|| EndpointError::Invalid("agent URL has no authority".to_owned()))?;
    if authority.as_str().contains('@') {
        return Err(EndpointError::Invalid(
            "agent URL must not contain credentials".to_owned(),
        ));
    }
    let port = authority
        .port_u16()
        .filter(|port| *port != 0)
        .ok_or_else(|| EndpointError::Invalid("agent URL must have a valid port".to_owned()))?;
    let host = authority.host();
    // localhost 不走 DNS：即使 hosts/DNS 被改写，也不会把本机 bearer 发往远程主机。
    let ip = if host.eq_ignore_ascii_case("localhost") {
        IpAddr::V4(Ipv4Addr::LOCALHOST)
    } else {
        host.trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>()
            .map_err(|_| EndpointError::Invalid("agent URL must be loopback".to_owned()))?
    };
    if !ip.is_loopback() {
        return Err(EndpointError::Invalid(
            "agent URL must be loopback".to_owned(),
        ));
    }
    Ok(SocketAddr::new(ip, port))
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum EndpointError {
    #[error("invalid local agent endpoint: {0}")]
    Invalid(String),
    #[error("could not read local agent endpoint {}: {source}", .path.display())]
    Read {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    #[error("could not parse local agent endpoint {}: {source}", .path.display())]
    Parse {
        path: std::path::PathBuf,
        source: serde_json::Error,
    },
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use fluxdown_protocol::local_service::GATEWAY_ENDPOINT_FILE;

    use super::{DEFAULT_RPC_URL, EndpointError, discover_rpc_url, socket_target};
    use crate::service_bootstrap::{ListenerProbe, PROBE_TIMEOUT, connect_listener};

    async fn probe(endpoint: &str) -> ListenerProbe {
        connect_listener(endpoint, PROBE_TIMEOUT)
            .await
            .expect("probe endpoint")
    }

    struct Fixture(PathBuf);

    impl Fixture {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("fluxdown-endpoint-{label}-{}", std::process::id()));
            std::fs::create_dir(&path).expect("create endpoint fixture");
            Self(path)
        }

        fn bearer(&self) -> PathBuf {
            self.0.join("agent.token")
        }

        fn publish(&self, url: &str) {
            let bytes = serde_json::to_vec(&serde_json::json!({ "rpcUrl": url }))
                .expect("serialize endpoint");
            std::fs::write(self.0.join(GATEWAY_ENDPOINT_FILE), bytes).expect("publish endpoint");
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            if let Err(error) = std::fs::remove_dir_all(&self.0) {
                eprintln!("could not remove endpoint fixture: {error}");
            }
        }
    }

    #[tokio::test]
    async fn discovery_rereads_newly_published_and_restarted_agent_ports() {
        let fixture = Fixture::new("reread");
        assert_eq!(
            discover_rpc_url(None, &fixture.bearer())
                .await
                .expect("legacy default"),
            DEFAULT_RPC_URL
        );
        let current = std::net::TcpListener::bind("127.0.0.1:0").expect("bind current agent");
        let current_address = current.local_addr().expect("current address");
        fixture.publish(&format!("ws://{current_address}/rpc"));
        let endpoint = discover_rpc_url(None, &fixture.bearer())
            .await
            .expect("cold start endpoint");
        let ListenerProbe::Listening(stream) = probe(&endpoint).await else {
            panic!("current agent must be listening");
        };
        assert_eq!(stream.peer_addr().expect("current peer"), current_address);
        drop(stream);

        let restarted = std::net::TcpListener::bind("127.0.0.1:0").expect("bind restarted agent");
        let restarted_address = restarted.local_addr().expect("restarted address");
        drop(current);
        assert!(
            matches!(probe(&endpoint).await, ListenerProbe::Refused),
            "obsolete endpoint must be refused"
        );
        fixture.publish(&format!("ws://{restarted_address}/rpc"));
        let endpoint = discover_rpc_url(None, &fixture.bearer())
            .await
            .expect("restart endpoint");
        let ListenerProbe::Listening(stream) = probe(&endpoint).await else {
            panic!("restarted agent must be listening");
        };
        assert_eq!(
            stream.peer_addr().expect("restarted peer"),
            restarted_address
        );
    }

    #[tokio::test]
    async fn explicit_url_precedes_even_a_corrupt_endpoint_but_still_requires_loopback() {
        let fixture = Fixture::new("priority");
        std::fs::write(fixture.0.join(GATEWAY_ENDPOINT_FILE), "{broken").expect("corrupt endpoint");
        assert_eq!(
            discover_rpc_url(Some("ws://[::1]:19300/rpc"), &fixture.bearer())
                .await
                .expect("explicit override"),
            "ws://[::1]:19300/rpc"
        );
        assert!(
            discover_rpc_url(Some("ws://192.0.2.1:19300/rpc"), &fixture.bearer())
                .await
                .is_err()
        );
        assert!(matches!(
            discover_rpc_url(None, &fixture.bearer()).await,
            Err(EndpointError::Parse { .. })
        ));
    }

    #[tokio::test]
    async fn unreadable_or_malicious_endpoint_never_falls_back_to_another_agent() {
        let fixture = Fixture::new("invalid");
        fixture.publish("ws://attacker.example:17800/rpc");
        assert!(matches!(
            discover_rpc_url(None, &fixture.bearer()).await,
            Err(EndpointError::Invalid(_))
        ));
        std::fs::remove_file(fixture.0.join(GATEWAY_ENDPOINT_FILE)).expect("remove bad endpoint");
        std::fs::create_dir(fixture.0.join(GATEWAY_ENDPOINT_FILE))
            .expect("make endpoint unreadable");
        assert!(matches!(
            discover_rpc_url(None, &fixture.bearer()).await,
            Err(EndpointError::Read { .. })
        ));
    }

    #[test]
    fn only_loopback_websocket_rpc_urls_can_receive_the_local_bearer() {
        assert_eq!(
            socket_target("ws://localhost:19100/rpc").expect("localhost target"),
            "127.0.0.1:19100"
                .parse::<std::net::SocketAddr>()
                .expect("IPv4 address")
        );
        assert_eq!(
            socket_target("ws://[::1]:19100/rpc").expect("IPv6 target"),
            "[::1]:19100"
                .parse::<std::net::SocketAddr>()
                .expect("IPv6 address")
        );
        for url in [
            "ws://192.0.2.1:19100/rpc",
            "ws://localhost.attacker.example:19100/rpc",
            "ws://127.0.0.1.attacker.example:19100/rpc",
            "ws://localhost@192.0.2.1:19100/rpc",
            "ws://user@127.0.0.1:19100/rpc",
            "http://127.0.0.1:19100/rpc",
            "wss://127.0.0.1:19100/rpc",
            "ws://127.0.0.1:19100/download",
            "ws://127.0.0.1:19100/rpc?forward=remote",
            "ws://127.0.0.1:19100/rpc#remote",
            "ws://127.0.0.1/rpc",
            "ws://127.0.0.1:0/rpc",
            "ws://127.0.0.1:65536/rpc",
        ] {
            assert!(socket_target(url).is_err(), "accepted untrusted URL {url}");
        }
    }
}
