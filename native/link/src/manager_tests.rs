#![allow(clippy::unwrap_used, clippy::expect_used)]

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::*;
use crate::crypto::{derive_link_aead_key, link_auth_tag, seal_link_body};
use crate::storage::memory::MemoryLinkStorage;
use crate::types::DiscoveryKind;

fn peer_fp() -> String {
    "ab".repeat(32)
}

fn other_fp() -> String {
    "cd".repeat(32)
}

fn options() -> LinkOptions {
    LinkOptions {
        api_port: 17800,
        reachable: true,
        advertise: false,
        advertise_addrs: Vec::new(),
    }
}

fn self_info() -> SelfInfo {
    SelfInfo {
        name: "me".into(),
        ..SelfInfo::default()
    }
}

fn direct(addr: &str) -> PeerCandidate {
    PeerCandidate {
        kind: TransportKind::Direct,
        address: addr.to_string(),
    }
}

fn paired_record(secret: Vec<u8>, candidates: Vec<PeerCandidate>) -> PeerRecord {
    PeerRecord {
        fingerprint: peer_fp(),
        identity_pub: vec![1u8; 32],
        name: "peer".into(),
        link_secret: secret,
        candidates,
        ..PeerRecord::default()
    }
}

async fn mgr_with_events(
    secret: Vec<u8>,
    storage: Arc<MemoryLinkStorage>,
    candidates: Vec<PeerCandidate>,
) -> (Arc<LinkManager>, String, mpsc::Receiver<LinkEngineEvent>) {
    let (tx, rx) = mpsc::channel(16);
    let mgr = LinkManager::load(storage, self_info(), options(), tx)
        .await
        .unwrap();
    mgr.store
        .upsert(&paired_record(secret, candidates))
        .await
        .unwrap();
    (mgr, peer_fp(), rx)
}

async fn mgr_with_storage(
    secret: Vec<u8>,
    storage: Arc<MemoryLinkStorage>,
) -> (Arc<LinkManager>, String) {
    let (mgr, fp, _rx) = mgr_with_events(secret, storage, vec![]).await;
    (mgr, fp)
}

async fn mgr_with_device(secret: Vec<u8>) -> (Arc<LinkManager>, String) {
    mgr_with_storage(secret, Arc::new(MemoryLinkStorage::default())).await
}

/// 按发起方 [`LinkManager::dispatch`] 同款 encrypt-then-MAC 流程构造发给 `mgr` 的密文 body + tag：
/// 先按请求 AAD 加密、再对**密文**算绑定收发双方指纹的 MAC。
fn seal_request(
    mgr: &LinkManager,
    secret: &[u8],
    sender_fp: &str,
    path: &str,
    ts: i64,
    nonce: &str,
    plaintext: &[u8],
) -> (Vec<u8>, String) {
    let exchange = LinkExchange {
        method: "POST",
        path,
        nonce,
        sender_fp,
        receiver_fp: mgr.fingerprint(),
    };
    let sealed = seal_link_body(
        &derive_link_aead_key(secret),
        &request_aad(&exchange),
        plaintext,
    );
    let tag = link_auth_tag(secret, &exchange, ts, &sealed);
    (sealed, tag)
}

async fn authorize_as(
    mgr: &LinkManager,
    secret: &[u8],
    fp: &str,
    path: &str,
    ts: i64,
    nonce: &str,
    plaintext: &[u8],
) -> LinkResult<LinkRequest> {
    let (sealed, tag) = seal_request(mgr, secret, fp, path, ts, nonce, plaintext);
    mgr.authorize("POST", path, fp, ts, nonce, &sealed, &tag, "v2")
        .await
}

// ── 假对端：只会回应 /ping 与数据面 POST 的最小 HTTP 服务 ────────────────────

#[derive(Clone, Debug)]
struct SeenRequest {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

type PostHandler = Arc<dyn Fn(&SeenRequest) -> Reply + Send + Sync>;

struct FakePeer {
    addr: std::net::SocketAddr,
    seen: Arc<Mutex<Vec<SeenRequest>>>,
}

impl FakePeer {
    async fn start(fingerprint: String, link_protocol: Option<u32>, post: PostHandler) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorded = seen.clone();
        tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    break;
                };
                let fingerprint = fingerprint.clone();
                let post = post.clone();
                let recorded = recorded.clone();
                tokio::spawn(async move {
                    serve_one(socket, &fingerprint, link_protocol, post, recorded).await;
                });
            }
        });
        Self { addr, seen }
    }

    fn candidate(&self) -> PeerCandidate {
        direct(&self.addr.to_string())
    }

    fn posts(&self) -> Vec<SeenRequest> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.method == "POST")
            .cloned()
            .collect()
    }
}

async fn read_request(socket: &mut TcpStream) -> Option<SeenRequest> {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        let read = socket.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..read]);
        if let Some(pos) = buf.windows(4).position(|window| window == b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let mut lines = head.lines();
    let mut request_line = lines.next()?.split_whitespace();
    let method = request_line.next()?.to_owned();
    let path = request_line.next()?.to_owned();
    let headers: HashMap<String, String> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    let content_length: usize = headers
        .get("content-length")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    while buf.len() < header_end + content_length {
        let read = socket.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..read]);
    }
    Some(SeenRequest {
        method,
        path,
        headers,
        body: buf[header_end..header_end + content_length].to_vec(),
    })
}

async fn write_reply(socket: &mut TcpStream, reply: Reply) {
    let mut head = format!("HTTP/1.1 {} X\r\n", reply.status);
    for (key, value) in &reply.headers {
        head.push_str(&format!("{key}: {value}\r\n"));
    }
    head.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n",
        reply.body.len()
    ));
    if let Err(error) = socket.write_all(head.as_bytes()).await {
        tracing::debug!(%error, "fake peer client closed before the reply head");
        return;
    }
    if let Err(error) = socket.write_all(&reply.body).await {
        tracing::debug!(%error, "fake peer client closed before the reply body");
        return;
    }
    if let Err(error) = socket.shutdown().await {
        tracing::debug!(%error, "fake peer shutdown failed");
    }
}

async fn serve_one(
    mut socket: TcpStream,
    fingerprint: &str,
    link_protocol: Option<u32>,
    post: PostHandler,
    seen: Arc<Mutex<Vec<SeenRequest>>>,
) {
    let Some(request) = read_request(&mut socket).await else {
        return;
    };
    seen.lock().unwrap().push(request.clone());
    let reply = if request.method == "GET" && request.path == "/ping" {
        let mut body = serde_json::json!({ "success": true, "app": "FluxDown", "linkFingerprint": fingerprint });
        if let Some(version) = link_protocol {
            body["linkProtocol"] = serde_json::json!(version);
        }
        Reply {
            status: 200,
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: body.to_string().into_bytes(),
        }
    } else {
        // 处理器可能故意阻塞（模拟对端用户迟迟不表态）：放到阻塞线程池，免得占住运行时工作线程，
        // 让同一线程 LIFO 槽里被唤醒的任务（如在途 confirm 的取消分支）无法被窃取执行。
        tokio::task::spawn_blocking(move || post(&request))
            .await
            .unwrap()
    };
    write_reply(&mut socket, reply).await;
}

fn exchange_of<'a>(request: &'a SeenRequest, responder_fp: &'a str) -> LinkExchange<'a> {
    LinkExchange {
        method: "POST",
        path: &request.path,
        nonce: &request.headers["x-fluxlink-nonce"],
        sender_fp: &request.headers["x-fluxlink-device"],
        receiver_fp: responder_fp,
    }
}

/// 假对端校验收到的请求确实遵守 v2 线格式：方案头、MAC、AEAD 都对；返回解密后的明文。
fn open_request(secret: &[u8], request: &SeenRequest, responder_fp: &str) -> Option<Vec<u8>> {
    if request.headers.get("x-fluxlink-enc").map(String::as_str) != Some("v2") {
        return None;
    }
    let exchange = exchange_of(request, responder_fp);
    let ts: i64 = request.headers["x-fluxlink-ts"].parse().ok()?;
    let tag = request.headers.get("x-fluxlink-tag")?;
    if !verify_link_auth_tag(secret, &exchange, ts, &request.body, tag) {
        return None;
    }
    open_link_body(
        &derive_link_aead_key(secret),
        &request_aad(&exchange),
        &request.body,
    )
}

fn sealed_reply(
    secret: &[u8],
    request: &SeenRequest,
    responder_fp: &str,
    plaintext: &[u8],
) -> Reply {
    let exchange = exchange_of(request, responder_fp);
    Reply {
        status: 200,
        headers: vec![
            ("X-FluxLink-Enc".into(), "v2".into()),
            ("Content-Type".into(), "application/octet-stream".into()),
        ],
        body: seal_link_body(
            &derive_link_aead_key(secret),
            &response_aad(&exchange),
            plaintext,
        ),
    }
}

fn status_reply(status: u16, body: &str) -> Reply {
    Reply {
        status,
        headers: vec![("Content-Type".into(), "application/json".into())],
        body: body.as_bytes().to_vec(),
    }
}

/// 一个会按 v2 规则校验请求并回封装 `{"taskId":"t1"}` 的合法对端处理器。
fn honest_task_handler(secret: Vec<u8>) -> PostHandler {
    Arc::new(move |request| {
        if open_request(&secret, request, &peer_fp()).is_some() {
            sealed_reply(&secret, request, &peer_fp(), br#"{"taskId":"t1"}"#)
        } else {
            status_reply(401, r#"{"success":false,"message":"unauthorized"}"#)
        }
    })
}

async fn closed_port() -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    listener.local_addr().unwrap()
}

// ── 入站鉴权 ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn api_port_update_refreshes_pairing_candidates_without_resetting_identity_or_auth() {
    let secret = vec![7u8; 32];
    let (mgr, fp) = mgr_with_device(secret.clone()).await;
    let identity = mgr.fingerprint().to_owned();
    let peer = PeerAddress::parse("127.0.0.1:17800").unwrap();
    let path = LINK_TASKS_PATH;
    let ts = now_unix();
    let plaintext = br#"{"url":"http://example.test/file"}"#;
    let (sealed, tag) = seal_request(
        &mgr,
        &secret,
        &fp,
        path,
        ts,
        "before-port-update",
        plaintext,
    );
    mgr.authorize(
        "POST",
        path,
        &fp,
        ts,
        "before-port-update",
        &sealed,
        &tag,
        "v2",
    )
    .await
    .unwrap();
    assert_eq!(mgr.self_addrs_towards(&peer).await, vec!["127.0.0.1:17800"]);

    mgr.update_api_port(17801).unwrap();
    assert_eq!(mgr.self_addrs_towards(&peer).await, vec!["127.0.0.1:17801"]);
    assert_eq!(mgr.fingerprint(), identity);
    assert!(
        mgr.advertiser
            .lock()
            .is_ok_and(|advertiser| advertiser.is_none()),
        "updating a port must not start advertising"
    );
    // 端口切换不能清除防重放状态，也不能使已有 link_secret 失效。
    assert!(matches!(
        mgr.authorize(
            "POST",
            path,
            &fp,
            ts,
            "before-port-update",
            &sealed,
            &tag,
            "v2"
        )
        .await,
        Err(LinkError::Unauthorized)
    ));
    let authorized = authorize_as(&mgr, &secret, &fp, path, ts, "after-port-update", plaintext)
        .await
        .unwrap();
    assert_eq!(authorized.device, fp);
    assert_eq!(authorized.body, plaintext);

    mgr.update_api_port(17800).unwrap();
    assert_eq!(mgr.self_addrs_towards(&peer).await, vec!["127.0.0.1:17800"]);
}

#[tokio::test]
async fn api_port_update_does_not_expand_reachability_or_revoke_pairing_codes() {
    let (mgr, _events) = responder_mgr().await;
    let code = mgr.generate_code();
    mgr.update_api_port(17801).unwrap();
    let peer = PeerAddress::parse("127.0.0.1:17800").unwrap();
    assert!(mgr.self_addrs_towards(&peer).await.is_empty());
    assert!(
        mgr.advertiser
            .lock()
            .is_ok_and(|advertiser| advertiser.is_none())
    );
    let mut initiator = PairingInitiator::new(LinkIdentity::generate());
    let hello = initiator.build_hello(&code, &mgr.responder.self_info(), Vec::new());
    let response = mgr.responder.handle_hello(&hello, None).unwrap();
    assert_eq!(
        crate::crypto::fingerprint(&response.responder_id_pub),
        mgr.fingerprint()
    );
}

#[tokio::test]
async fn authorize_accepts_valid_then_rejects_replay_tamper_skew_and_unknown_devices() {
    let secret = vec![7u8; 32];
    let (mgr, fp) = mgr_with_device(secret.clone()).await;
    let path = LINK_TASKS_PATH;
    let ts = now_unix();
    let plaintext = br#"{"url":"http://x/f"}"#;
    let (sealed, tag) = seal_request(&mgr, &secret, &fp, path, ts, "n1", plaintext);

    // 合法请求通过，且解密还原出原始明文。
    let authorized = mgr
        .authorize("POST", path, &fp, ts, "n1", &sealed, &tag, "v2")
        .await
        .unwrap();
    assert_eq!(authorized.device, fp);
    assert_eq!(authorized.body, plaintext);
    // 同 nonce 重放被拒（防重放）。
    assert!(matches!(
        mgr.authorize("POST", path, &fp, ts, "n1", &sealed, &tag, "v2")
            .await,
        Err(LinkError::Unauthorized)
    ));
    // 篡改密文（哪怕只翻一个字节）→ MAC 覆盖密文摘要，篡改后 tag 不再匹配 → 拒。
    let mut tampered = sealed.clone();
    let last = tampered.len() - 1;
    tampered[last] ^= 0xFF;
    assert!(matches!(
        mgr.authorize("POST", path, &fp, ts, "n2", &tampered, &tag, "v2")
            .await,
        Err(LinkError::Unauthorized)
    ));
    // 过期时间戳 → 时钟偏差（诊断用，仍按鉴权失败处理）。
    let old_ts = ts - LINK_AUTH_SKEW_SECS - 5;
    let (old_sealed, old_tag) = seal_request(&mgr, &secret, &fp, path, old_ts, "n3", plaintext);
    assert!(matches!(
        mgr.authorize("POST", path, &fp, old_ts, "n3", &old_sealed, &old_tag, "v2")
            .await,
        Err(LinkError::ClockSkew)
    ));
    // 未配对设备 → 未配对。
    let unknown = other_fp();
    let (sealed4, tag4) = seal_request(&mgr, &secret, &unknown, path, ts, "n4", plaintext);
    assert!(matches!(
        mgr.authorize("POST", path, &unknown, ts, "n4", &sealed4, &tag4, "v2")
            .await,
        Err(LinkError::NotPaired)
    ));
    // 头部形态非法（指纹 / nonce / tag）→ 廉价检查直接拒绝。
    let (sealed5, tag5) = seal_request(&mgr, &secret, &fp, path, ts, "n5", plaintext);
    for (device, nonce, tag) in [
        ("not-a-fingerprint", "n5", tag5.as_str()),
        (fp.as_str(), "", tag5.as_str()),
        (fp.as_str(), "bad nonce!", tag5.as_str()),
        (fp.as_str(), "n5", "short"),
    ] {
        assert!(matches!(
            mgr.authorize("POST", path, device, ts, nonce, &sealed5, tag, "v2")
                .await,
            Err(LinkError::Unauthorized)
        ));
    }
}

#[tokio::test]
async fn authorize_rejects_missing_or_non_v2_enc_header() {
    // 数据面不留降级路径：缺 `X-FluxLink-Enc` 头或头值不是 "v2"（含旧的 "v1"），
    // 哪怕 MAC/nonce/时间戳全部合法也一律拒绝。
    let secret = vec![7u8; 32];
    let (mgr, fp) = mgr_with_device(secret.clone()).await;
    let ts = now_unix();
    let (sealed, tag) = seal_request(&mgr, &secret, &fp, LINK_TASKS_PATH, ts, "n1", b"{}");
    for enc in ["", "v0", "v1", "V2"] {
        assert!(matches!(
            mgr.authorize("POST", LINK_TASKS_PATH, &fp, ts, "n1", &sealed, &tag, enc)
                .await,
            Err(LinkError::Unauthorized)
        ));
    }
}

#[tokio::test]
async fn authorize_rejects_a_request_reflected_back_to_its_sender() {
    // 本机发给对端的请求被嗅探后，把 Device 头改成对端指纹原样发回本机：MAC 绑定了收发双方指纹，
    // 反射必须被拒，不能在本机凭空造出一个「对端下发的任务」。
    let secret = vec![7u8; 32];
    let (mgr, fp) = mgr_with_device(secret.clone()).await;
    let ts = now_unix();
    let exchange = LinkExchange {
        method: "POST",
        path: LINK_TASKS_PATH,
        nonce: "reflect",
        sender_fp: mgr.fingerprint(),
        receiver_fp: &fp,
    };
    let sealed = seal_link_body(
        &derive_link_aead_key(&secret),
        &request_aad(&exchange),
        br#"{"url":"http://x/f"}"#,
    );
    let tag = link_auth_tag(&secret, &exchange, ts, &sealed);
    assert!(matches!(
        mgr.authorize(
            "POST",
            LINK_TASKS_PATH,
            &fp,
            ts,
            "reflect",
            &sealed,
            &tag,
            "v2"
        )
        .await,
        Err(LinkError::Unauthorized)
    ));
}

#[tokio::test]
async fn replay_nonce_is_kept_until_the_skewed_timestamp_window_closes() {
    // 发送方时钟超前 100 秒：请求在 [ts-120, ts+120] 内都通过时间窗，nonce 必须保留到 ts+120，
    // 而不是只保留到「收到时刻 + 120」。
    let secret = vec![7u8; 32];
    let (mgr, fp) = mgr_with_device(secret.clone()).await;
    // 本测试关注的是时间窗内的 nonce 保留：把「进程启动时刻」挪到时间窗之前，不触发重启重放防护。
    mgr.started_at
        .store(now_unix() - 1_000, std::sync::atomic::Ordering::Relaxed);
    let ts = now_unix() + 100;
    authorize_as(&mgr, &secret, &fp, LINK_TASKS_PATH, ts, "ahead", b"{}")
        .await
        .unwrap();
    let key = format!("{fp}:ahead");
    assert_eq!(
        mgr.seen_nonces.lock().unwrap().get(&key).copied(),
        Some(ts + LINK_AUTH_SKEW_SECS)
    );
    mgr.prune_expired();
    assert!(mgr.seen_nonces.lock().unwrap().contains_key(&key));

    // 时钟落后的请求保留到「当前时间 + 容忍窗」：之后的重放才会被时间窗拒绝。
    let now = now_unix();
    authorize_as(
        &mgr,
        &secret,
        &fp,
        LINK_TASKS_PATH,
        now - 100,
        "behind",
        b"{}",
    )
    .await
    .unwrap();
    assert!(
        mgr.seen_nonces
            .lock()
            .unwrap()
            .get(&format!("{fp}:behind"))
            .copied()
            .unwrap()
            >= now + LINK_AUTH_SKEW_SECS
    );
}

#[tokio::test]
async fn prune_expired_evicts_stale_seen_nonces() {
    // 超过过期时刻的防重放记录必须被清掉，否则该表在进程存活期内随数据面请求量无界增长。
    let (mgr, fp) = mgr_with_device(vec![7u8; 32]).await;
    mgr.seen_nonces
        .lock()
        .unwrap()
        .insert(format!("{fp}:stale"), now_unix() - 1);
    mgr.prune_expired();
    assert!(mgr.seen_nonces.lock().unwrap().is_empty());
}

// ── 身份 ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn unreadable_identity_seed_fails_loading_instead_of_minting_a_new_identity() {
    let storage = Arc::new(MemoryLinkStorage::default());
    storage
        .seed_unreadable
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let (tx, _rx) = mpsc::channel(4);
    let result = LinkManager::load(storage.clone(), self_info(), options(), tx).await;
    assert!(matches!(result, Err(LinkError::Store(_))));
    // 绝不能悄悄生成新身份覆盖：存储里仍然没有种子。
    assert!(storage.raw_seed().is_none());
}

#[tokio::test]
async fn identity_is_stable_across_reloads() {
    let storage = Arc::new(MemoryLinkStorage::default());
    let (tx, _rx) = mpsc::channel(4);
    let first = LinkManager::load(storage.clone(), self_info(), options(), tx)
        .await
        .unwrap();
    let (tx, _rx) = mpsc::channel(4);
    let second = LinkManager::load(storage, self_info(), options(), tx)
        .await
        .unwrap();
    assert_eq!(first.fingerprint(), second.fingerprint());
}

#[tokio::test]
async fn self_update_changes_name_and_cloud_id_and_reports_no_change_when_identical() {
    let (mgr, _events) = responder_mgr().await;
    assert!(!mgr.update_self("nas", None).unwrap());
    assert!(mgr.update_self("nas-renamed", Some("cloud-1")).unwrap());
    assert_eq!(mgr.self_name(), "nas-renamed");
    assert_eq!(mgr.self_info().cloud_device_id.as_deref(), Some("cloud-1"));
    assert!(!mgr.update_self("nas-renamed", Some("cloud-1")).unwrap());
    // 登出：云设备 ID 清除。形态非法的 ID 同样视为没有。
    assert!(mgr.update_self("nas-renamed", None).unwrap());
    assert_eq!(mgr.self_info().cloud_device_id, None);
    assert!(!mgr.update_self("nas-renamed", Some("bad id!")).unwrap());
    // 控制字符被剔除，空名回落到默认名。
    mgr.update_self("a\u{0}b", None).unwrap();
    assert_eq!(mgr.self_name(), "ab");
    mgr.update_self("\u{202E}", None).unwrap();
    assert_eq!(mgr.self_name(), "FluxDown");
}

#[tokio::test]
async fn renamed_device_answers_new_hellos_with_the_new_name() {
    let (mgr, _events) = responder_mgr().await;
    mgr.update_self("nas-2", None).unwrap();
    let code = mgr.generate_code();
    let mut initiator = PairingInitiator::new(LinkIdentity::generate());
    let hello = initiator.build_hello(
        &code,
        &SelfInfo {
            name: "laptop".into(),
            ..SelfInfo::default()
        },
        vec![],
    );
    let response = mgr.pair_hello_wire(wire_hello(&hello), None).unwrap();
    assert_eq!(response.name, "nas-2");
}

// ── 候选 / 拨号 ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn metadata_persistence_failure_keeps_verified_hint_until_retry_succeeds() -> LinkResult<()> {
    let storage = Arc::new(MemoryLinkStorage::default());
    let (manager, fingerprint) = mgr_with_storage(vec![7; 32], storage.clone()).await;
    let candidate = direct("10.0.0.9:17800");
    manager
        .hinted
        .lock()
        .map_err(|error| LinkError::Store(error.to_string()))?
        .insert(fingerprint.clone(), candidate.clone());
    let address = PeerAddress::parse(&candidate.address)?.base_url();
    storage
        .fail_metadata_writes
        .store(true, std::sync::atomic::Ordering::Relaxed);
    manager.promote_hinted(&fingerprint, &address).await;
    assert_eq!(
        manager
            .hinted
            .lock()
            .map_err(|error| LinkError::Store(error.to_string()))?
            .get(&fingerprint),
        Some(&candidate)
    );
    assert!(
        manager
            .store
            .get(&fingerprint)
            .await?
            .ok_or(LinkError::NotPaired)?
            .candidates
            .is_empty()
    );
    storage
        .fail_metadata_writes
        .store(false, std::sync::atomic::Ordering::Relaxed);
    manager.promote_hinted(&fingerprint, &address).await;
    assert!(
        !manager
            .hinted
            .lock()
            .map_err(|error| LinkError::Store(error.to_string()))?
            .contains_key(&fingerprint)
    );
    assert_eq!(
        manager
            .store
            .get(&fingerprint)
            .await?
            .ok_or(LinkError::NotPaired)?
            .candidates,
        vec![candidate]
    );
    Ok(())
}

#[tokio::test]
async fn unverified_hint_is_dialed_first_but_never_persisted() {
    let (mgr, fp) = mgr_with_device(vec![7u8; 32]).await;
    mgr.store
        .update_candidates(&fp, &[direct("10.0.0.2:17800")])
        .await
        .unwrap();
    mgr.hinted
        .lock()
        .unwrap()
        .insert(fp.clone(), direct("10.0.0.9:17800"));
    let record = mgr.store.get(&fp).await.unwrap().unwrap();
    let dial = mgr.with_hinted(record);
    // 换 IP 后新地址最可能有效：排在已存候选之前尝试；已存候选留作回退。
    assert_eq!(
        dial.candidates,
        vec![direct("10.0.0.9:17800"), direct("10.0.0.2:17800")]
    );
    // 线索只影响本次拨号，名册不动。
    let stored = mgr.store.get(&fp).await.unwrap().unwrap();
    assert_eq!(stored.candidates, vec![direct("10.0.0.2:17800")]);
}

#[tokio::test]
async fn hint_is_persisted_only_after_authenticated_success_on_its_address() {
    let (mgr, fp) = mgr_with_device(vec![7u8; 32]).await;
    mgr.store
        .update_candidates(&fp, &[direct("10.0.0.2:17800")])
        .await
        .unwrap();
    mgr.hinted
        .lock()
        .unwrap()
        .insert(fp.clone(), direct("10.0.0.9:17800"));
    let other = PeerAddress::parse("10.0.0.2:17800").unwrap().base_url();
    mgr.promote_hinted(&fp, &other).await;
    let stored = mgr.store.get(&fp).await.unwrap().unwrap();
    assert_eq!(stored.candidates, vec![direct("10.0.0.2:17800")]);

    let hinted = PeerAddress::parse("10.0.0.9:17800").unwrap().base_url();
    mgr.promote_hinted(&fp, &hinted).await;
    let stored = mgr.store.get(&fp).await.unwrap().unwrap();
    assert_eq!(
        stored.candidates,
        vec![direct("10.0.0.9:17800"), direct("10.0.0.2:17800")]
    );
}

#[tokio::test]
async fn dispatch_dials_the_hint_first_skips_a_squatter_and_promotes_after_a_verified_reply() {
    let secret = vec![7u8; 32];
    // 旧 IP 现在被另一台 FluxDown 设备占用：指纹不符，不能挡住后面的候选。
    let squatter = FakePeer::start(
        other_fp(),
        Some(2),
        Arc::new(|_| status_reply(500, r#"{"success":false,"message":"must not be called"}"#)),
    )
    .await;
    let peer = FakePeer::start(peer_fp(), Some(2), honest_task_handler(secret.clone())).await;
    let storage = Arc::new(MemoryLinkStorage::default());
    let (mgr, fp, _rx) = mgr_with_events(secret, storage.clone(), vec![squatter.candidate()]).await;
    mgr.hinted
        .lock()
        .unwrap()
        .insert(fp.clone(), peer.candidate());

    let task_id = mgr
        .dispatch(&fp, "https://example.com/f.bin", Some("/dl"), Some("f.bin"))
        .await
        .unwrap();
    assert_eq!(task_id, "t1");
    assert!(
        squatter.posts().is_empty(),
        "an impostor must never receive a task"
    );
    assert_eq!(peer.posts().len(), 1);
    assert_eq!(peer.posts()[0].path, LINK_TASKS_PATH);
    // 响应经链路密钥解封成功 → 线索写入名册并置首，旧候选保留作回退。
    let stored = mgr.store.get(&fp).await.unwrap().unwrap();
    assert_eq!(
        stored.candidates,
        vec![peer.candidate(), squatter.candidate()]
    );
    assert!(!mgr.hinted.lock().unwrap().contains_key(&fp));
}

#[tokio::test]
async fn a_reply_that_cannot_be_opened_is_an_auth_failure_and_never_promotes_the_hint() {
    let secret = vec![7u8; 32];
    let storage = Arc::new(MemoryLinkStorage::default());

    // 三种不可验证的「成功」响应：明文 JSON、缺方案头的密文、AAD 绑定了别的 nonce 的密文。
    let replies: Vec<(&str, PostHandler)> = vec![
        (
            "plaintext",
            Arc::new(|_| Reply {
                status: 200,
                headers: vec![("Content-Type".into(), "application/json".into())],
                body: br#"{"taskId":"forged"}"#.to_vec(),
            }),
        ),
        ("missing-enc", {
            let secret = secret.clone();
            Arc::new(move |request: &SeenRequest| {
                let mut reply = sealed_reply(&secret, request, &peer_fp(), br#"{"taskId":"x"}"#);
                reply.headers.retain(|(key, _)| key != "X-FluxLink-Enc");
                reply
            })
        }),
        ("wrong-nonce", {
            let secret = secret.clone();
            Arc::new(move |request: &SeenRequest| {
                let mut other = request.clone();
                other
                    .headers
                    .insert("x-fluxlink-nonce".into(), "some-other-request".into());
                sealed_reply(&secret, &other, &peer_fp(), br#"{"taskId":"replayed"}"#)
            })
        }),
    ];
    for (label, handler) in replies {
        let peer = FakePeer::start(peer_fp(), Some(2), handler).await;
        let (mgr, fp, _rx) =
            mgr_with_events(secret.clone(), storage.clone(), vec![direct("127.0.0.1:1")]).await;
        mgr.hinted
            .lock()
            .unwrap()
            .insert(fp.clone(), peer.candidate());
        let error = mgr
            .dispatch(&fp, "https://example.com/f.bin", None, None)
            .await
            .unwrap_err();
        assert!(
            matches!(error, LinkError::Unauthorized),
            "{label}: {error:?}"
        );
        // 攻击者靠冒用指纹得到的地址不能被写成首选候选。
        let stored = mgr.store.get(&fp).await.unwrap().unwrap();
        assert_eq!(stored.candidates, vec![direct("127.0.0.1:1")], "{label}");
        assert!(mgr.hinted.lock().unwrap().contains_key(&fp), "{label}");
    }
}

#[tokio::test]
async fn peers_with_an_older_data_plane_are_refused_before_any_request() {
    let secret = vec![7u8; 32];
    for link_protocol in [None, Some(1)] {
        let peer = FakePeer::start(
            peer_fp(),
            link_protocol,
            honest_task_handler(secret.clone()),
        )
        .await;
        let (mgr, fp, _rx) = mgr_with_events(
            secret.clone(),
            Arc::new(MemoryLinkStorage::default()),
            vec![peer.candidate()],
        )
        .await;
        assert!(matches!(
            mgr.dispatch(&fp, "https://example.com/f", None, None).await,
            Err(LinkError::PeerOutdated)
        ));
        assert!(matches!(
            mgr.exchange_peer_info(&fp, &PeerInfo::default()).await,
            Err(LinkError::PeerOutdated)
        ));
        // 在线判定只看 /ping：旧版设备依旧「在线」，只是不能下发。
        assert!(mgr.is_online(&fp).await);
        assert!(peer.posts().is_empty(), "no data-plane request may be sent");
    }
}

// ── 信息交换 ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn exchanging_info_updates_the_record_and_writes_only_when_something_changed() {
    let secret = vec![7u8; 32];
    let dead = closed_port().await;
    let captured: Arc<Mutex<Option<PeerInfo>>> = Arc::new(Mutex::new(None));
    let handler: PostHandler = {
        let secret = secret.clone();
        let captured = captured.clone();
        let dead = dead.to_string();
        Arc::new(move |request| {
            let Some(plaintext) = open_request(&secret, request, &peer_fp()) else {
                return status_reply(401, r#"{"success":false,"message":"unauthorized"}"#);
            };
            *captured.lock().unwrap() = decode_peer_info(&plaintext);
            let mine = serde_json::json!({
                "v": 2,
                "name": "Phone\u{0}",
                "platform": "android",
                "appVersion": "2.0.0",
                "addrs": [dead, "ftp://nope", "not an address"],
                "cloudDeviceId": "cloud-peer",
                "pathStyle": "posix",
            });
            sealed_reply(&secret, request, &peer_fp(), mine.to_string().as_bytes())
        })
    };
    let peer = FakePeer::start(peer_fp(), Some(2), handler).await;
    let storage = Arc::new(MemoryLinkStorage::default());
    let (mgr, fp, _rx) = mgr_with_events(secret, storage.clone(), vec![peer.candidate()]).await;
    mgr.update_self("me-renamed", Some("cloud-me")).unwrap();
    mgr.set_advertise_addrs(&["192.168.1.9:17800".to_owned()]);
    let local = PeerInfo {
        default_save_dir: Some("/downloads".into()),
        path_style: Some("posix".into()),
        ..PeerInfo::default()
    };

    let baseline = storage.write_count();
    assert!(mgr.exchange_peer_info(&fp, &local).await.unwrap());
    // 对端收到的是本机最新的自报信息（改名后的名字、云设备 ID、覆盖地址、目录与风格）。
    let sent = captured.lock().unwrap().clone().unwrap();
    assert_eq!(sent.name.as_deref(), Some("me-renamed"));
    assert_eq!(sent.cloud_device_id.as_deref(), Some("cloud-me"));
    assert_eq!(sent.addrs, vec!["192.168.1.9:17800".to_owned()]);
    assert_eq!(sent.default_save_dir.as_deref(), Some("/downloads"));
    // 本机名册记录被对端自报信息更新：名称（清洗后）、云设备 ID、自报地址放候选首位。
    let record = mgr.store.get(&fp).await.unwrap().unwrap();
    assert_eq!(record.name, "Phone");
    assert_eq!(record.platform.as_deref(), Some("android"));
    assert_eq!(record.app_version.as_deref(), Some("2.0.0"));
    assert_eq!(record.cloud_device_id.as_deref(), Some("cloud-peer"));
    assert_eq!(
        record.candidates,
        vec![direct(&dead.to_string()), peer.candidate()]
    );
    assert_eq!(storage.write_count(), baseline + 1);

    // 空闲时再次交换同样的信息：没有变化 → 不写盘。
    assert!(!mgr.exchange_peer_info(&fp, &local).await.unwrap());
    assert_eq!(storage.write_count(), baseline + 1);
}

#[tokio::test]
async fn answering_info_updates_the_requesters_record_only_when_it_changed() {
    let secret = vec![7u8; 32];
    let storage = Arc::new(MemoryLinkStorage::default());
    let (mgr, fp, _rx) = mgr_with_events(secret.clone(), storage.clone(), vec![]).await;
    let info = PeerInfo {
        name: Some("Laptop".into()),
        platform: Some("macos".into()),
        app_version: Some("1.0.0".into()),
        addrs: vec!["10.0.0.7:17800".into()],
        cloud_device_id: Some("cloud-a".into()),
        default_save_dir: Some("/Users/me/Downloads".into()),
        path_style: Some("posix".into()),
    };
    let baseline = storage.write_count();

    let request = authorize_as(
        &mgr,
        &secret,
        &fp,
        LINK_INFO_PATH,
        now_unix(),
        "i1",
        &encode_peer_info(&info),
    )
    .await
    .unwrap();
    let answer = mgr
        .answer_peer_info(&request, &PeerInfo::default())
        .await
        .unwrap();
    assert!(answer.changed);
    let record = mgr.store.get(&fp).await.unwrap().unwrap();
    assert_eq!(record.name, "Laptop");
    assert_eq!(record.cloud_device_id.as_deref(), Some("cloud-a"));
    assert_eq!(record.candidates, vec![direct("10.0.0.7:17800")]);
    assert_eq!(storage.write_count(), baseline + 1);
    // 响应是用请求 nonce 绑定的封装体：请求方能用响应 AAD 解开，本机自报信息在里面。
    let exchange = LinkExchange {
        method: "POST",
        path: LINK_INFO_PATH,
        nonce: "i1",
        sender_fp: &fp,
        receiver_fp: mgr.fingerprint(),
    };
    let plaintext = open_link_body(
        &derive_link_aead_key(&secret),
        &response_aad(&exchange),
        &answer.sealed,
    )
    .unwrap();
    assert_eq!(
        decode_peer_info(&plaintext).unwrap().name.as_deref(),
        Some("me")
    );

    // 同样的信息再来一次：不变 → 不写盘、不通知。
    let request = authorize_as(
        &mgr,
        &secret,
        &fp,
        LINK_INFO_PATH,
        now_unix(),
        "i2",
        &encode_peer_info(&info),
    )
    .await
    .unwrap();
    let answer = mgr
        .answer_peer_info(&request, &PeerInfo::default())
        .await
        .unwrap();
    assert!(!answer.changed);
    assert_eq!(storage.write_count(), baseline + 1);

    // 对端登出云账号：云设备 ID 被清除。
    let logged_out = PeerInfo {
        cloud_device_id: None,
        ..info
    };
    let request = authorize_as(
        &mgr,
        &secret,
        &fp,
        LINK_INFO_PATH,
        now_unix(),
        "i3",
        &encode_peer_info(&logged_out),
    )
    .await
    .unwrap();
    assert!(
        mgr.answer_peer_info(&request, &PeerInfo::default())
            .await
            .unwrap()
            .changed
    );
    assert_eq!(
        mgr.store.get(&fp).await.unwrap().unwrap().cloud_device_id,
        None
    );
}

#[tokio::test]
async fn peer_info_answer_does_not_report_success_before_metadata_is_persisted() -> LinkResult<()> {
    let secret = vec![7u8; 32];
    let storage = Arc::new(MemoryLinkStorage::default());
    let (manager, fingerprint) = mgr_with_storage(secret.clone(), storage.clone()).await;
    storage
        .fail_metadata_writes
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let info = PeerInfo {
        default_save_dir: Some("/downloads".into()),
        path_style: Some("posix".into()),
        ..PeerInfo::default()
    };
    let request = authorize_as(
        &manager,
        &secret,
        &fingerprint,
        LINK_INFO_PATH,
        now_unix(),
        "i1",
        &encode_peer_info(&info),
    )
    .await?;
    let result = manager
        .answer_peer_info(&request, &PeerInfo::default())
        .await;
    assert!(matches!(result, Err(LinkError::Store(_))));
    assert_eq!(
        manager
            .store
            .get(&fingerprint)
            .await?
            .ok_or(LinkError::NotPaired)?
            .default_save_dir,
        None
    );
    Ok(())
}

#[test]
fn peer_info_decoding_sanitizes_and_bounds_every_field() {
    let long_name = "x".repeat(200);
    let json = serde_json::json!({
        "v": 2,
        "name": format!("\u{202E}{long_name}"),
        "platform": "mac\nos",
        "appVersion": "1.0\u{7}",
        "addrs": [
            "10.0.0.1:17800", "http://10.0.0.1:17800", "ftp://x", "10.0.0.2:17800",
            "10.0.0.3:17800", "10.0.0.4:17800", "10.0.0.5:17800", "10.0.0.6:17800"
        ],
        "cloudDeviceId": "not a uuid!",
        "defaultSaveDir": "  /dl  ",
        "pathStyle": "amiga",
    });
    let info = decode_peer_info(json.to_string().as_bytes()).unwrap();
    assert_eq!(
        info.name.as_deref().map(|name| name.chars().count()),
        Some(MAX_NAME_CHARS)
    );
    assert_eq!(info.platform.as_deref(), Some("macos"));
    assert_eq!(info.app_version.as_deref(), Some("1.0"));
    // 去重（两种写法同一地址）、丢弃非法条目，至多 4 条。
    assert_eq!(
        info.addrs,
        vec![
            "10.0.0.1:17800".to_owned(),
            "10.0.0.2:17800".to_owned(),
            "10.0.0.3:17800".to_owned(),
            "10.0.0.4:17800".to_owned(),
        ]
    );
    assert_eq!(info.cloud_device_id, None);
    assert_eq!(info.default_save_dir.as_deref(), Some("/dl"));
    assert_eq!(info.path_style, None);
    // 不是 v2 信息包（含旧的 v1 与任意其它数据面明文）→ None。
    assert!(decode_peer_info(br#"{"v":1,"pathStyle":"posix"}"#).is_none());
    assert!(decode_peer_info(br#"{"url":"http://x/f"}"#).is_none());
    // 编码再解码保持一致。
    let encoded = encode_peer_info(&info);
    assert_eq!(decode_peer_info(&encoded).unwrap(), info);
}

// ── 解除配对 / 备注名 ────────────────────────────────────────────────────────

#[tokio::test]
async fn unpair_marks_the_peer_revoked_and_blocks_both_directions_until_repaired() {
    let secret = vec![7u8; 32];
    let storage = Arc::new(MemoryLinkStorage::default());
    let (mgr, fp, mut events) = mgr_with_events(secret.clone(), storage.clone(), vec![]).await;

    let request = authorize_as(
        &mgr,
        &secret,
        &fp,
        LINK_UNPAIR_PATH,
        now_unix(),
        "u1",
        br#"{"v":2}"#,
    )
    .await
    .unwrap();
    let sealed = mgr.answer_unpair(&request).await.unwrap();
    let exchange = LinkExchange {
        method: "POST",
        path: LINK_UNPAIR_PATH,
        nonce: "u1",
        sender_fp: &fp,
        receiver_fp: mgr.fingerprint(),
    };
    assert_eq!(
        open_link_body(
            &derive_link_aead_key(&secret),
            &response_aad(&exchange),
            &sealed
        )
        .unwrap(),
        br#"{"ok":true}"#
    );
    assert!(mgr.store.get(&fp).await.unwrap().unwrap().revoked);
    assert!(matches!(events.try_recv(), Ok(LinkEngineEvent::Revoked(revoked)) if revoked == fp));

    // 重复通知幂等：不再写盘、不再发事件。
    let writes = storage.write_count();
    let again = authorize_as(
        &mgr,
        &secret,
        &fp,
        LINK_UNPAIR_PATH,
        now_unix(),
        "u2",
        br#"{"v":2}"#,
    )
    .await;
    assert!(
        matches!(again, Err(LinkError::NotPaired)),
        "revoked peers are rejected inbound"
    );
    assert_eq!(storage.write_count(), writes);

    // 本机下发 / 交换信息 / 探活：直接 NotPaired（不发请求）或离线。
    assert!(matches!(
        mgr.dispatch(&fp, "https://example.com/f", None, None).await,
        Err(LinkError::NotPaired)
    ));
    assert!(matches!(
        mgr.exchange_peer_info(&fp, &PeerInfo::default()).await,
        Err(LinkError::NotPaired)
    ));
    assert!(!mgr.is_online(&fp).await);

    // 重新配对同一指纹：新记录覆盖旧记录，revoked 清除，入站请求恢复。
    mgr.store
        .upsert(&paired_record(secret.clone(), vec![]))
        .await
        .unwrap();
    assert!(!mgr.store.get(&fp).await.unwrap().unwrap().revoked);
    authorize_as(&mgr, &secret, &fp, LINK_TASKS_PATH, now_unix(), "u3", b"{}")
        .await
        .unwrap();
}

#[tokio::test]
async fn removing_a_device_notifies_the_online_peer_before_deleting_locally() {
    let secret = vec![7u8; 32];
    let handler: PostHandler = {
        let secret = secret.clone();
        Arc::new(move |request| {
            if open_request(&secret, request, &peer_fp()).is_some() {
                sealed_reply(&secret, request, &peer_fp(), br#"{"ok":true}"#)
            } else {
                status_reply(401, r#"{"success":false,"message":"unauthorized"}"#)
            }
        })
    };
    let peer = FakePeer::start(peer_fp(), Some(2), handler).await;
    let (mgr, fp, mut events) = mgr_with_events(
        secret,
        Arc::new(MemoryLinkStorage::default()),
        vec![peer.candidate()],
    )
    .await;
    assert!(mgr.remove_device(&fp).await.unwrap());
    let posts = peer.posts();
    assert_eq!(posts.len(), 1);
    assert_eq!(posts[0].path, LINK_UNPAIR_PATH);
    assert!(mgr.store.get(&fp).await.unwrap().is_none());
    assert!(matches!(events.try_recv(), Ok(LinkEngineEvent::Unpaired(removed)) if removed == fp));
}

#[tokio::test]
async fn removing_an_unreachable_device_still_deletes_it_promptly() {
    let dead = closed_port().await;
    let (mgr, fp, _rx) = mgr_with_events(
        vec![7u8; 32],
        Arc::new(MemoryLinkStorage::default()),
        vec![direct(&dead.to_string())],
    )
    .await;
    let started = std::time::Instant::now();
    assert!(mgr.remove_device(&fp).await.unwrap());
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(mgr.store.get(&fp).await.unwrap().is_none());
    // 不存在的设备：返回 false。
    assert!(!mgr.remove_device(&fp).await.unwrap());
}

#[tokio::test]
async fn rename_sets_clears_and_validates_the_custom_name() {
    let storage = Arc::new(MemoryLinkStorage::default());
    let (mgr, fp) = mgr_with_storage(vec![7u8; 32], storage.clone()).await;
    let record = mgr.rename_device(&fp, Some("  书房 PC  ")).await.unwrap();
    assert_eq!(record.custom_name.as_deref(), Some("书房 PC"));
    assert_eq!(record.effective_name(), "书房 PC");
    // 相同备注名再设一次：不写盘。
    let writes = storage.write_count();
    mgr.rename_device(&fp, Some("书房 PC")).await.unwrap();
    assert_eq!(storage.write_count(), writes);
    // 空 / 全空白 / 省略 = 清除，恢复对端自报名。
    for cleared in [None, Some(""), Some("   ")] {
        mgr.rename_device(&fp, Some("x")).await.unwrap();
        let record = mgr.rename_device(&fp, cleared).await.unwrap();
        assert_eq!(record.custom_name, None);
        assert_eq!(record.effective_name(), "peer");
    }
    // 超长 / 含控制字符 → 参数非法；未配对 → NotPaired。
    assert!(matches!(
        mgr.rename_device(&fp, Some(&"x".repeat(65))).await,
        Err(LinkError::BadPayload(_))
    ));
    assert!(matches!(
        mgr.rename_device(&fp, Some("a\nb")).await,
        Err(LinkError::BadPayload(_))
    ));
    assert!(mgr.rename_device(&fp, Some(&"x".repeat(64))).await.is_ok());
    assert!(matches!(
        mgr.rename_device(&other_fp(), Some("x")).await,
        Err(LinkError::NotPaired)
    ));
}

// ── 配对 / 发现 ─────────────────────────────────────────────────────────────

fn wire_hello(hello: &HelloRequest) -> WireHello {
    WireHello {
        protocol_version: hello.protocol_version,
        code: hello.code.clone(),
        initiator_id_pub: B64.encode(hello.initiator_id_pub),
        initiator_commit: B64.encode(hello.initiator_commit),
        initiator_sig: B64.encode(hello.initiator_sig),
        name: hello.name.clone(),
        platform: hello.platform.clone(),
        app_version: hello.app_version.clone(),
        initiator_addrs: hello.initiator_addrs.clone(),
        request_control: hello.request_control,
    }
}

async fn responder_mgr() -> (Arc<LinkManager>, mpsc::Receiver<LinkEngineEvent>) {
    let (tx, rx) = mpsc::channel(8);
    let mgr = LinkManager::load(
        Arc::new(MemoryLinkStorage::default()),
        SelfInfo {
            name: "nas".into(),
            platform: Some("linux".into()),
            ..SelfInfo::default()
        },
        LinkOptions {
            api_port: 17800,
            reachable: false,
            advertise: false,
            advertise_addrs: Vec::new(),
        },
        tx,
    )
    .await
    .unwrap();
    (mgr, rx)
}

#[tokio::test]
async fn incoming_pairing_is_announced_only_after_reveal_with_the_shared_sas() {
    // 响应方在 hello 阶段还看不到发起方的临时公钥，没有 SAS 可展示：入站配对请求
    // 只在 reveal 被受理之后才广播，且广播的 SAS 与发起方算出的一致。
    let (mgr, mut events) = responder_mgr().await;
    let code = mgr.generate_code();
    let initiator_id = LinkIdentity::generate();
    let mut initiator = PairingInitiator::new(initiator_id.clone());
    let hello = initiator.build_hello(
        &code,
        &SelfInfo {
            name: "laptop".into(),
            platform: Some("macos".into()),
            ..SelfInfo::default()
        },
        vec![],
    );

    let resp = mgr.pair_hello_wire(wire_hello(&hello), None).unwrap();
    tokio::task::yield_now().await;
    assert!(events.try_recv().is_err(), "hello alone must not announce");

    // 发起方按对端 JSON 的形状解析 hello 回复（与 begin_pairing 同一条解析路径）。
    let json = serde_json::json!({
        "protocolVersion": resp.protocol_version,
        "sessionId": resp.session_id,
        "responderEphPub": resp.responder_eph_pub,
        "responderNonce": resp.responder_nonce,
        "responderIdPub": resp.responder_id_pub,
        "name": resp.name,
        "platform": resp.platform,
        "appVersion": resp.app_version,
    });
    let hello_resp = parse_hello_response(&json).unwrap();
    let reveal = initiator
        .on_hello_response(&hello_resp, "10.0.0.1:17800", &[])
        .unwrap();
    let revealed = mgr
        .pair_reveal_wire(WireReveal {
            session_id: reveal.session_id.clone(),
            initiator_eph_pub: B64.encode(reveal.initiator_eph_pub),
            initiator_nonce: B64.encode(reveal.initiator_nonce),
        })
        .unwrap();
    let reveal_resp =
        parse_reveal_response(&serde_json::json!({ "responderSig": revealed.responder_sig }))
            .unwrap();
    let initiator_sas = initiator.on_reveal_response(&reveal_resp).unwrap();

    let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .unwrap()
        .unwrap();
    match event {
        LinkEngineEvent::IncomingPairing {
            session_id,
            sas,
            peer_name,
            peer_fingerprint,
            ..
        } => {
            assert_eq!(session_id, hello_resp.session_id);
            assert_eq!(sas, initiator_sas);
            assert_eq!(peer_name, "laptop");
            assert_eq!(peer_fingerprint, initiator_id.fingerprint());
        }
        other => panic!("expected IncomingPairing, got {other:?}"),
    }
}

#[tokio::test]
async fn hello_from_other_protocol_version_is_rejected_before_field_decoding() {
    // 旧版发起方的 hello 没有新字段（解码后是空串）：要得到明确的版本不兼容，
    // 而不是含糊的载荷非法。
    let (mgr, _events) = responder_mgr().await;
    let code = mgr.generate_code();
    let stale = WireHello {
        protocol_version: 0,
        code: code.clone(),
        initiator_id_pub: String::new(),
        initiator_commit: String::new(),
        initiator_sig: String::new(),
        name: "old".into(),
        platform: None,
        app_version: None,
        initiator_addrs: vec![],
        request_control: false,
    };
    assert!(matches!(
        mgr.pair_hello_wire(stale, None),
        Err(LinkError::UnsupportedVersion)
    ));
}

#[test]
fn legacy_peer_replies_are_reported_as_unsupported_version() {
    // 旧版响应方的 hello 回复：有 sessionId、明文 SAS 与自己的签名，没有 protocolVersion。
    let legacy = serde_json::json!({
        "sessionId": "abc",
        "responderEphPub": B64.encode([1u8; 32]),
        "responderIdPub": B64.encode([2u8; 32]),
        "responderSig": B64.encode([3u8; 64]),
        "name": "old-nas",
        "sas": "123456",
    });
    assert!(matches!(
        parse_hello_response(&legacy),
        Err(LinkError::UnsupportedVersion)
    ));
    // 根本不是 FluxDown 的 JSON 仍是 NotFluxDown，不被误判成版本问题。
    assert!(matches!(
        parse_hello_response(&serde_json::json!({ "ok": true })),
        Err(LinkError::NotFluxDown(_))
    ));

    // 旧版响应方会把新 hello 当作无法解析的载荷拒绝。
    let rejected = LinkError::BadPayload(
        "invalid link hello payload: missing field `initiatorEphPub` at line 1 column 80".into(),
    );
    assert!(matches!(
        hello_rejection(rejected),
        LinkError::UnsupportedVersion
    ));
    assert!(matches!(
        hello_rejection(LinkError::InvalidCode),
        LinkError::InvalidCode
    ));
    assert!(matches!(
        hello_rejection(LinkError::BadPayload("unrelated".into())),
        LinkError::BadPayload(_)
    ));
}

#[test]
fn new_pairing_errors_round_trip_through_the_wire_message_contract() {
    // 发起方只看得到对端的 HTTP message：新错误必须能被还原，而不是一律变成含糊的载荷错误。
    for error in [LinkError::UnsupportedVersion, LinkError::CommitmentMismatch] {
        let restored = LinkError::from_wire_message(&error.to_string());
        assert_eq!(
            restored.map(|e| std::mem::discriminant(&e)),
            Some(std::mem::discriminant(&error))
        );
    }
}

#[tokio::test]
async fn known_fingerprints_cover_probe_and_discovery_for_that_address_only() {
    let (mgr, _events) = responder_mgr().await;
    let addr = PeerAddress::parse("10.0.0.2:17800").unwrap();
    let elsewhere = PeerAddress::parse("10.0.0.3:17800").unwrap();
    assert!(mgr.known_fingerprints(&addr).is_empty());

    mgr.remember_probe(&addr, "fp-probed");
    let peer = |fingerprint: Option<&str>, host: &str| DiscoveredPeer {
        fingerprint: fingerprint.map(str::to_string),
        name: "peer".into(),
        platform: None,
        host: host.into(),
        port: 17800,
        app_version: None,
        kind: DiscoveryKind::Mdns,
    };
    {
        let mut snapshot = mgr.discovered.lock().unwrap();
        snapshot.push(peer(Some("fp-mdns"), "10.0.0.2"));
        // 没有指纹的条目与其它地址的条目都不提供这个地址的指纹。
        snapshot.push(peer(None, "10.0.0.2"));
        snapshot.push(peer(Some("fp-other"), "10.0.0.3"));
    }

    let mut known = mgr.known_fingerprints(&addr);
    known.sort();
    assert_eq!(known, vec!["fp-mdns".to_string(), "fp-probed".to_string()]);
    let mut elsewhere_known = mgr.known_fingerprints(&elsewhere);
    elsewhere_known.sort();
    assert_eq!(elsewhere_known, vec!["fp-other".to_string()]);
}

#[tokio::test]
async fn probed_fingerprints_are_bounded_and_expire() {
    let (mgr, _events) = responder_mgr().await;
    for i in 0..(MAX_PROBED_FINGERPRINTS + 8) {
        let addr = PeerAddress::parse(&format!("10.1.0.{}:17800", i + 1)).unwrap();
        mgr.remember_probe(&addr, &format!("fp-{i}"));
    }
    assert_eq!(mgr.probed.lock().unwrap().len(), MAX_PROBED_FINGERPRINTS);
    // 满员淘汰的是最旧的，最新的仍在。
    let newest =
        PeerAddress::parse(&format!("10.1.0.{}:17800", MAX_PROBED_FINGERPRINTS + 8)).unwrap();
    assert_eq!(
        mgr.known_fingerprints(&newest),
        vec![format!("fp-{}", MAX_PROBED_FINGERPRINTS + 7)]
    );

    // 过期的探测结果不再参与比对，并被后台剪枝回收。
    let Some(expired_at) =
        std::time::Instant::now().checked_sub(PROBED_FINGERPRINT_TTL + Duration::from_secs(1))
    else {
        return;
    };
    for (_, at) in mgr.probed.lock().unwrap().values_mut() {
        *at = expired_at;
    }
    assert!(mgr.known_fingerprints(&newest).is_empty());
    mgr.prune_expired();
    assert!(mgr.probed.lock().unwrap().is_empty());
}

#[tokio::test]
async fn advertise_override_replaces_the_detected_addresses_only_when_reachable() {
    let (tx, _rx) = mpsc::channel(4);
    let mgr = LinkManager::load(
        Arc::new(MemoryLinkStorage::default()),
        self_info(),
        LinkOptions {
            advertise_addrs: vec![
                "nas.example.com:8080".into(),
                "https://nas.example.com/fluxdown".into(),
                "ftp://nope".into(),
            ],
            ..options()
        },
        tx,
    )
    .await
    .unwrap();
    let lan = PeerAddress::parse("192.168.1.2:17800").unwrap();
    assert_eq!(
        mgr.self_addrs_towards(&lan).await,
        vec![
            "nas.example.com:8080".to_owned(),
            "https://nas.example.com/fluxdown".to_owned()
        ]
    );
    // 对端是 https 站点（多半是公网反代）：回连无意义，仍为空。
    let proxy = PeerAddress::parse("https://peer.example.com").unwrap();
    assert!(mgr.self_addrs_towards(&proxy).await.is_empty());
    // 运行期更新。
    assert!(mgr.set_advertise_addrs(&["10.0.0.9:17800".to_owned()]));
    assert!(!mgr.set_advertise_addrs(&["http://10.0.0.9:17800".to_owned()]));
    assert_eq!(mgr.self_addrs_towards(&lan).await, vec!["10.0.0.9:17800"]);

    // 本机不可达（只监听回环）：覆盖地址也不自报。
    let (unreachable, _events) = responder_mgr().await;
    unreachable.set_advertise_addrs(&["10.0.0.9:17800".to_owned()]);
    assert!(unreachable.self_addrs_towards(&lan).await.is_empty());
}

#[tokio::test]
async fn presence_advertising_follows_the_roster_and_survives_closing_the_pairing_window() {
    // `advertise=false`：只验证「是否需要广播」的状态机，不真的起 mDNS。
    let storage = Arc::new(MemoryLinkStorage::default());
    let (mgr, fp, _rx) = mgr_with_events(vec![7u8; 32], storage.clone(), vec![]).await;
    mgr.refresh_presence().await;
    assert!(
        mgr.roster_present
            .load(std::sync::atomic::Ordering::Relaxed)
    );
    assert!(mgr.advertising_needed());
    // 配对窗口关闭不影响名册带来的存在性广播。
    let _code = mgr.generate_code();
    mgr.stop_pairing();
    assert!(mgr.advertising_needed());

    // 对端解除配对（revoked）后名册里没有可用设备：不再需要常驻广播。
    mgr.store
        .update_record(
            &fp,
            Box::new(|record| {
                record.revoked = true;
                true
            }),
        )
        .await
        .unwrap();
    mgr.refresh_presence().await;
    assert!(
        !mgr.roster_present
            .load(std::sync::atomic::Ordering::Relaxed)
    );
    assert!(!mgr.advertising_needed());
    // 配对窗口开着时仍需要广播；窗口关闭后恢复不需要。
    let _code = mgr.generate_code();
    assert!(mgr.advertising_needed());
    mgr.stop_pairing();
    assert!(!mgr.advertising_needed());

    // 宿主显式要求长期广播（headless 常驻服务）：不被配对窗口关闭取消。
    mgr.start_advertising();
    mgr.stop_pairing();
    assert!(mgr.advertising_needed());
}

#[tokio::test]
async fn self_heal_browsing_is_a_no_op_when_mdns_is_disabled() {
    let (mgr, _rx) = responder_mgr().await;
    // `advertise=false`（FLUXDOWN_MDNS=off）：不起浏览器，立即返回。
    let started = std::time::Instant::now();
    mgr.browse_for(Duration::from_secs(30)).await.unwrap();
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(mgr.browser.lock().unwrap().is_none());
}

// ── 设备控制（requestControl / 凭据）────────────────────────────────────────

/// 经 wire 形态走完一次完整配对（hello → reveal → 批准 → confirm），返回发起方视角的名册记录。
/// `request_control` 同时核对响应方事件里的 `requests_control`。
async fn wire_pair(
    host: &Arc<LinkManager>,
    events: &mut mpsc::Receiver<LinkEngineEvent>,
    controller: &LinkIdentity,
    request_control: bool,
) -> PeerRecord {
    let code = host.generate_code();
    let mut initiator = PairingInitiator::new(controller.clone()).request_control(request_control);
    let hello = initiator.build_hello(
        &code,
        &SelfInfo {
            name: "phone".into(),
            platform: Some("android".into()),
            ..SelfInfo::default()
        },
        vec![],
    );
    let resp = host.pair_hello_wire(wire_hello(&hello), None).unwrap();
    let hello_resp = parse_hello_response(&serde_json::json!({
        "protocolVersion": resp.protocol_version,
        "sessionId": resp.session_id,
        "responderEphPub": resp.responder_eph_pub,
        "responderNonce": resp.responder_nonce,
        "responderIdPub": resp.responder_id_pub,
        "name": resp.name,
        "platform": resp.platform,
        "appVersion": resp.app_version,
    }))
    .unwrap();
    let reveal = initiator
        .on_hello_response(&hello_resp, "10.0.0.1:17800", &[])
        .unwrap();
    let revealed = host
        .pair_reveal_wire(WireReveal {
            session_id: reveal.session_id.clone(),
            initiator_eph_pub: B64.encode(reveal.initiator_eph_pub),
            initiator_nonce: B64.encode(reveal.initiator_nonce),
        })
        .unwrap();
    let reveal_resp =
        parse_reveal_response(&serde_json::json!({ "responderSig": revealed.responder_sig }))
            .unwrap();
    initiator.on_reveal_response(&reveal_resp).unwrap();

    let session_id = loop {
        let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .unwrap()
            .unwrap();
        if let LinkEngineEvent::IncomingPairing {
            session_id,
            requests_control,
            ..
        } = event
        {
            assert_eq!(requests_control, request_control);
            break session_id;
        }
    };
    host.approve_incoming(&session_id, true).unwrap();
    assert!(host.pair_confirm(&session_id, true).await.unwrap().paired());
    initiator.finalize().unwrap()
}

#[tokio::test]
async fn repairing_takes_the_new_control_grant_and_never_inherits_the_old_one() {
    let (host, mut events) = responder_mgr().await;
    let controller = LinkIdentity::generate();
    let fingerprint = controller.fingerprint().to_owned();

    // 第一次：请求并获批控制。
    let initiator_side = wire_pair(&host, &mut events, &controller, true).await;
    assert!(initiator_side.controllable && !initiator_side.grants_control);
    let record = host.get_device(&fingerprint).await.unwrap().unwrap();
    assert!(record.grants_control && !record.controllable);

    // 同一台设备重新配对且这次没有请求控制：授权随新记录消失。
    let initiator_side = wire_pair(&host, &mut events, &controller, false).await;
    assert!(!initiator_side.controllable);
    let record = host.get_device(&fingerprint).await.unwrap().unwrap();
    assert!(
        !record.grants_control,
        "a plain re-pairing must drop control"
    );

    // 再请求一次：重新授予。
    wire_pair(&host, &mut events, &controller, true).await;
    assert!(
        host.get_device(&fingerprint)
            .await
            .unwrap()
            .unwrap()
            .grants_control
    );
}

#[tokio::test]
async fn control_credential_is_issued_only_for_controllable_unrevoked_hosts() {
    let secret = vec![9u8; 32];
    let (mgr, fp) = mgr_with_device(secret.clone()).await;
    // 普通配对（没有 controllable 标志）：不签发；未知设备同样不签发。
    assert!(matches!(
        mgr.control_credential(&fp).await,
        Err(LinkError::NotPaired)
    ));
    assert!(matches!(
        mgr.control_credential(&other_fp()).await,
        Err(LinkError::NotPaired)
    ));

    let mut record = paired_record(secret.clone(), vec![]);
    record.controllable = true;
    mgr.store.upsert(&record).await.unwrap();
    let token = mgr.control_credential(&fp).await.unwrap();
    assert_eq!(
        token,
        crate::crypto::control_credential(&secret, mgr.fingerprint(), &fp).unwrap()
    );

    // 对端解除配对（revoked）后不再签发。
    record.revoked = true;
    mgr.store.upsert(&record).await.unwrap();
    assert!(matches!(
        mgr.control_credential(&fp).await,
        Err(LinkError::NotPaired)
    ));
}

#[tokio::test]
async fn control_credential_verification_follows_the_roster() {
    let secret = vec![9u8; 32];
    // 这里的 mgr 是被控主机：名册里的 `peer_fp()` 是控制端。
    let (host, controller_fp) = mgr_with_device(secret.clone()).await;
    let token =
        crate::crypto::control_credential(&secret, &controller_fp, host.fingerprint()).unwrap();

    // 记录存在但没有授权控制（普通配对）：拒绝。
    assert_eq!(host.verify_control_credential(&token).await, None);

    let mut record = paired_record(secret.clone(), vec![]);
    record.grants_control = true;
    host.store.upsert(&record).await.unwrap();
    assert_eq!(
        host.verify_control_credential(&token).await.as_deref(),
        Some(controller_fp.as_str())
    );

    // 伪造：同一指纹但密钥不同 / 为另一台主机签发的凭据 / 指纹不在名册里。
    let wrong_secret =
        crate::crypto::control_credential(&[1u8; 32], &controller_fp, host.fingerprint()).unwrap();
    let other_host =
        crate::crypto::control_credential(&secret, &controller_fp, &other_fp()).unwrap();
    let unknown =
        crate::crypto::control_credential(&secret, &other_fp(), host.fingerprint()).unwrap();
    for forged in [wrong_secret, other_host, unknown] {
        assert_eq!(host.verify_control_credential(&forged).await, None);
    }
    // 格式错误。
    let no_mac = format!("fdl1.{controller_fp}.");
    for malformed in ["", "fdl1", "fdl1.x.y", "Bearer", no_mac.as_str()] {
        assert_eq!(host.verify_control_credential(malformed).await, None);
    }

    // 重新配对产生新的 link_secret：旧凭据随之失效。
    let mut repaired = record.clone();
    repaired.link_secret = vec![5u8; 32];
    host.store.upsert(&repaired).await.unwrap();
    assert_eq!(host.verify_control_credential(&token).await, None);

    // 授权被撤销（grants_control 变化）/ 对端解除配对（revoked）：拒绝。
    let mut revoked = record.clone();
    revoked.revoked = true;
    host.store.upsert(&revoked).await.unwrap();
    assert_eq!(host.verify_control_credential(&token).await, None);
    let mut ungranted = record;
    ungranted.grants_control = false;
    host.store.upsert(&ungranted).await.unwrap();
    assert_eq!(host.verify_control_credential(&token).await, None);
    host.store.remove(&controller_fp).await.unwrap();
    assert_eq!(host.verify_control_credential(&token).await, None);
}

#[tokio::test]
async fn requesting_control_from_a_legacy_peer_fails_before_any_hello_is_sent() {
    // 旧版对端 `/ping` 没有 `linkControl`：不发 hello（旧版会忽略 requestControl 而静默配成
    // 普通设备，签名字节也对不上），返回版本不兼容。
    let legacy = FakePeer::start(
        peer_fp(),
        Some(2),
        Arc::new(|_| status_reply(400, r#"{"success":false,"message":"nope"}"#)),
    )
    .await;
    let (mgr, _rx) = responder_mgr().await;
    let address = PeerAddress::parse(&legacy.addr.to_string()).unwrap();

    assert!(matches!(
        mgr.begin_pairing(&address, "123456", true).await,
        Err(LinkError::UnsupportedVersion)
    ));
    assert!(
        legacy.posts().is_empty(),
        "no hello may reach the legacy peer"
    );

    // 不请求控制的配对不做这项预检：hello 照常发出（这里被假对端 400 拒绝）。
    assert!(mgr.begin_pairing(&address, "123456", false).await.is_err());
    assert_eq!(legacy.posts().len(), 1);
    assert_eq!(legacy.posts()[0].path, "/api/v1/link/pair/hello");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn declining_while_the_peer_decides_aborts_the_in_flight_confirm() {
    // 对端用户迟迟不表态：accept=true 的 confirm 在对端那里挂着。
    let peer = FakePeer::start(
        peer_fp(),
        Some(2),
        Arc::new(|request| {
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            if body["confirm"] == true {
                std::thread::sleep(Duration::from_secs(5));
            }
            status_reply(200, r#"{"success":true,"paired":false}"#)
        }),
    )
    .await;
    let (mgr, _rx) = responder_mgr().await;
    mgr.pending.lock().unwrap().insert(
        "token".to_owned(),
        PendingInit {
            initiator: PairingInitiator::new(LinkIdentity::generate()),
            session_id: "session-1".to_owned(),
            peer: PeerAddress::parse(&peer.addr.to_string()).unwrap(),
            created: std::time::Instant::now(),
            confirming: false,
            cancel: Arc::new(tokio::sync::Notify::new()),
        },
    );

    let in_flight = {
        let mgr = mgr.clone();
        tokio::spawn(async move { mgr.confirm_pairing("token", true).await })
    };
    for _ in 0..100 {
        if !peer.posts().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        peer.posts().len(),
        1,
        "the accept=true confirm must be on the wire"
    );
    // 重复确认没有意义。
    assert!(matches!(
        mgr.confirm_pairing("token", true).await,
        Err(LinkError::SessionExpired)
    ));

    // 用户取消：取消调用立即通知对端放弃并返回「未配对」，在途调用马上以会话过期结束。
    let started = std::time::Instant::now();
    assert!(mgr.confirm_pairing("token", false).await.unwrap().is_none());
    let result = tokio::time::timeout(Duration::from_secs(2), in_flight)
        .await
        .expect("the in-flight confirm must end promptly")
        .unwrap();
    assert!(
        matches!(result, Err(LinkError::SessionExpired)),
        "{result:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(mgr.pending.lock().unwrap().is_empty());
    let confirms: Vec<serde_json::Value> = peer
        .posts()
        .iter()
        .map(|request| serde_json::from_slice(&request.body).unwrap())
        .collect();
    assert_eq!(confirms.len(), 2, "abort posts exactly one confirm=false");
    assert_eq!(confirms[1]["confirm"], false);
    assert_eq!(confirms[1]["sessionId"], "session-1");
    // 已结束的待确认项不能再被使用。
    assert!(matches!(
        mgr.confirm_pairing("token", false).await,
        Err(LinkError::SessionExpired)
    ));
}

// ── 发现快照：mDNS 实例消失 ──────────────────────────────────────────────────

fn discovered(fingerprint: Option<&str>, kind: DiscoveryKind, port: u16) -> DiscoveredPeer {
    DiscoveredPeer {
        fingerprint: fingerprint.map(str::to_owned),
        name: "peer".into(),
        platform: None,
        host: "10.0.0.2".into(),
        port,
        app_version: None,
        kind,
    }
}

#[test]
fn lost_mdns_instances_leave_the_discovery_snapshot() {
    let alpha = "aa".repeat(32);
    let beta = "bb".repeat(32);
    let mut snapshot = vec![
        discovered(Some(&alpha), DiscoveryKind::Mdns, 1),
        discovered(Some(&beta), DiscoveryKind::Mdns, 2),
        // 手动探测加入的设备没有「下线」概念：同指纹也不动。
        discovered(Some(&alpha), DiscoveryKind::Manual, 3),
        discovered(None, DiscoveryKind::Mdns, 4),
    ];
    // 实例名 = 指纹前 12 位。
    assert!(remove_lost_instance(&mut snapshot, &alpha[..12]));
    let ports: Vec<u16> = snapshot.iter().map(|peer| peer.port).collect();
    assert_eq!(ports, vec![2, 3, 4]);
    // 再来一次 / 未知实例：没有变化，调用方据此不发事件。
    assert!(!remove_lost_instance(&mut snapshot, &alpha[..12]));
    assert!(!remove_lost_instance(&mut snapshot, "ffffffffffff"));
    assert_eq!(snapshot.len(), 3);
}

// ── 重启重放防护 ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn requests_older_than_the_process_start_are_refused_even_inside_the_skew_window() {
    let secret = vec![9u8; 32];
    let (mgr, fp) = mgr_with_device(secret.clone()).await;
    let started = mgr.started_at.load(std::sync::atomic::Ordering::Relaxed);
    // 重启之前发出的请求（`ts` 早于启动，但仍在 120s 时间窗内）：拒绝，不管 MAC 对不对。
    let stale = authorize_as(
        &mgr,
        &secret,
        &fp,
        LINK_INFO_PATH,
        started - 1,
        "n-stale",
        b"{}",
    )
    .await;
    assert!(matches!(stale, Err(LinkError::ClockSkew)), "{stale:?}");
    let long_ago = authorize_as(
        &mgr,
        &secret,
        &fp,
        LINK_INFO_PATH,
        started - 100,
        "n-old",
        b"{}",
    )
    .await;
    assert!(
        matches!(long_ago, Err(LinkError::ClockSkew)),
        "{long_ago:?}"
    );
    // 启动时刻及之后的请求照常受理。
    let fresh = authorize_as(
        &mgr,
        &secret,
        &fp,
        LINK_INFO_PATH,
        started,
        "n-fresh",
        b"{}",
    )
    .await;
    assert!(fresh.is_ok(), "{fresh:?}");
    let later = authorize_as(
        &mgr,
        &secret,
        &fp,
        LINK_INFO_PATH,
        now_unix(),
        "n-now",
        b"{}",
    )
    .await;
    assert!(later.is_ok(), "{later:?}");
}
