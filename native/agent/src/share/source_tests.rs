//! 云端副本（CDN）/ 网关桥接来源端到端测试：mock FluxCloud（resolve / 下载票据 / 桥接票据 / 上报）
//! + mock CDN + mock 网关 + 真实 `ShareService`。分享方从未上线（端点不可达），所以全部字节都来自 HTTP 来源。

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use axum::Json;
use axum::Router;
use axum::extract::{Path as AxPath, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use fluxdown_api::service::ApiError;
use fluxdown_protocol::{
    AgentEvent, AgentSnapshot, CreateTaskRequest, DaemonEvent, ErrorReason, SharePathKind,
    ShareReceiveResultDto, ShareResolveDto, ShareTaskFactsDto, TaskDto, method,
};
use fluxdown_share::ticket::{encode_public_key, sign_ticket};
use fluxdown_share::timefmt::{now_unix_secs, rfc3339_ms};
use fluxdown_share::{SecretKey, ShareFile, TicketPayload};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{ShareService, ShareServiceParts};
use crate::cloud::{CloudApi, CloudClient};
use crate::egress::Egress;
use crate::event_hub::AgentEventHub;
use crate::link::LinkTaskCreator;
use crate::state::{AgentState, StateStore};

const MIB: usize = 1024 * 1024;

// ───────────── mock ─────────────

struct Mock {
    base: OnceLock<String>,
    signer: SecretKey,
    sender: String,
    file: ShareFile,
    data: Vec<u8>,
    /// resolve 报告的云端副本状态。
    cloud_state: Mutex<&'static str>,
    /// resolve 报告的分享方在线状态。
    sender_online: Mutex<Option<bool>>,
    bridge_exhausted: AtomicBool,
    download_tickets: Mutex<Vec<Value>>,
    cdn_ranges: Mutex<Vec<String>>,
    bridge_requests: Mutex<Vec<(String, String)>>,
    sessions: Mutex<Vec<Value>>,
}

type MockState = Arc<Mock>;

fn error(status: StatusCode, code: &str) -> Response {
    (status, Json(json!({ "code": code, "message": code }))).into_response()
}

async fn config() -> Response {
    let body = json!({
        "enabled": true, "guestReceive": true, "guestBridge": true, "guestShare": false,
        "pickupCodeLength": 6, "chunkBytes": 4_194_304,
        "liveTiers": [{ "id": "t1", "hours": 24, "enabled": true, "default": true }],
        "retentionTiers": [], "cloudEnabled": true,
        "network": {
            "relays": [], "dnsOrigin": "", "pkarrUrl": "", "nameservers": [], "dohUrl": null,
            "pkarrTtlSecs": 30, "republishSecs": 300, "publishDirectAddrs": false,
            "portmapper": false, "holePunchHintSecs": 8, "reportIntervalSecs": 1
        },
        "revision": 1
    });
    ([(header::ETAG, "\"v1\"")], Json(body)).into_response()
}

async fn keys(State(mock): State<MockState>) -> Json<Value> {
    Json(json!({ "keys": [{
        "kid": "k1", "publicKey": encode_public_key(&mock.signer.public())
    }]}))
}

async fn resolve(State(mock): State<MockState>, Json(body): Json<Value>) -> Response {
    let receiver = body["receiverEndpointId"].as_str().unwrap_or_default();
    let ticket = sign_ticket(
        &mock.signer,
        &TicketPayload {
            v: 1,
            kid: "k1".into(),
            sid: "share-src".into(),
            rep: receiver.to_owned(),
            exp: now_unix_secs() + 3600,
            scope: "recv".into(),
            root: None,
            pri: 0,
        },
    )
    .unwrap();
    let mut share = json!({
        "id": "share-src", "title": "payload.bin",
        "files": [serde_json::to_value(&mock.file).unwrap()],
        "senderDeviceName": "Sender PC", "senderEndpointId": mock.sender,
        "expiresAt": rfc3339_ms((now_unix_secs() + 3600) * 1000),
        "cloudState": *mock.cloud_state.lock().unwrap(),
    });
    if let Some(online) = *mock.sender_online.lock().unwrap() {
        share["senderOnline"] = json!(online);
    }
    Json(json!({ "share": share, "ticket": ticket, "relays": [], "guestToken": "guest-tok" }))
        .into_response()
}

async fn download_ticket(State(mock): State<MockState>, Json(body): Json<Value>) -> Response {
    if *mock.cloud_state.lock().unwrap() != "ready" {
        return error(StatusCode::CONFLICT, "share_cloud_not_ready");
    }
    mock.download_tickets.lock().unwrap().push(body.clone());
    let base = mock.base.get().unwrap();
    Json(json!({
        "url": format!("{base}/cdn/{}?sig=abc", body["fileIndex"]),
        "expiresAt": rfc3339_ms((now_unix_secs() + 600) * 1000),
        "leaseId": "lease-1"
    }))
    .into_response()
}

async fn bridge_ticket(State(mock): State<MockState>) -> Response {
    if mock.bridge_exhausted.load(Ordering::SeqCst) {
        return error(StatusCode::FORBIDDEN, "traffic_exhausted");
    }
    let base = mock.base.get().unwrap();
    Json(json!({
        "url": format!("{base}/bridge/share-src"), "token": "bridge-token",
        "expiresAt": rfc3339_ms((now_unix_secs() + 600) * 1000)
    }))
    .into_response()
}

fn ranged(mock: &Mock, headers: &HeaderMap) -> Response {
    let Some(range) = headers.get(header::RANGE).and_then(|v| v.to_str().ok()) else {
        return error(StatusCode::BAD_REQUEST, "range_required");
    };
    let (a, b) = range.trim_start_matches("bytes=").split_once('-').unwrap();
    let (a, b): (usize, usize) = (a.parse().unwrap(), b.parse().unwrap());
    let body = mock.data[a..=b].to_vec();
    (
        StatusCode::PARTIAL_CONTENT,
        [(
            header::CONTENT_RANGE,
            format!("bytes {a}-{b}/{}", mock.data.len()),
        )],
        body,
    )
        .into_response()
}

async fn cdn(
    State(mock): State<MockState>,
    AxPath(_file): AxPath<u32>,
    headers: HeaderMap,
) -> Response {
    // 签名 URL：不接受 Authorization（对象存储不该收到账号凭据）。
    if headers.contains_key(header::AUTHORIZATION) {
        return error(StatusCode::BAD_REQUEST, "unexpected_authorization");
    }
    mock.cdn_ranges.lock().unwrap().push(
        headers
            .get(header::RANGE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned(),
    );
    ranged(&mock, &headers)
}

async fn bridge(
    State(mock): State<MockState>,
    AxPath((_share, _file)): AxPath<(String, u32)>,
    headers: HeaderMap,
) -> Response {
    let auth = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    mock.bridge_requests
        .lock()
        .unwrap()
        .push((auth.clone(), range));
    if auth != "Bearer bridge-token" {
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    ranged(&mock, &headers)
}

async fn register_endpoint(Json(body): Json<Value>) -> Json<Value> {
    Json(json!({ "endpointId": body["endpointId"], "dnsOrigin": "" }))
}

async fn telemetry_net() -> Json<Value> {
    Json(json!({}))
}

async fn telemetry_sessions(State(mock): State<MockState>, Json(body): Json<Value>) -> Json<Value> {
    mock.sessions.lock().unwrap().push(body);
    Json(json!({ "killed": false }))
}

async fn start_mock(cloud_state: &'static str, online: Option<bool>, len: usize) -> MockState {
    let dir = std::env::temp_dir().join(format!(
        "fluxdown_share_src_files_{}_{}",
        std::process::id(),
        fluxdown_share::timefmt::now_unix_ms()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("payload.bin");
    let data: Vec<u8> = (0..len).map(|i| (i.wrapping_mul(31) >> 3) as u8).collect();
    std::fs::write(&path, &data).unwrap();
    let file = fluxdown_share::hash_file(path, None, |_| {}, CancellationToken::new())
        .await
        .unwrap();
    let mock = Arc::new(Mock {
        base: OnceLock::new(),
        signer: SecretKey::generate(),
        // 没有任何端点绑定在这个 ID 上：分享方等同离线。
        sender: SecretKey::generate().public().to_z32(),
        file,
        data,
        cloud_state: Mutex::new(cloud_state),
        sender_online: Mutex::new(online),
        bridge_exhausted: AtomicBool::new(false),
        download_tickets: Mutex::new(Vec::new()),
        cdn_ranges: Mutex::new(Vec::new()),
        bridge_requests: Mutex::new(Vec::new()),
        sessions: Mutex::new(Vec::new()),
    });
    let app = Router::new()
        .route("/api/v1/share/config", get(config))
        .route("/api/v1/share/keys", get(keys))
        .route("/api/v1/share/endpoint", put(register_endpoint))
        .route("/api/v1/share/resolve", post(resolve))
        .route("/api/v1/share/download-ticket", post(download_ticket))
        .route("/api/v1/share/bridge-ticket", post(bridge_ticket))
        .route("/api/v1/share/telemetry/net", post(telemetry_net))
        .route("/api/v1/share/telemetry/sessions", post(telemetry_sessions))
        .route("/cdn/{file}", get(cdn))
        .route("/bridge/{share}/{file}", get(bridge))
        .with_state(mock.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    mock.base
        .set(format!("http://{}", listener.local_addr().unwrap()))
        .unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    mock
}

// ───────────── 测试宿主 ─────────────

#[derive(Default)]
struct FakeTasks {
    requests: Mutex<Vec<CreateTaskRequest>>,
}

#[async_trait]
impl LinkTaskCreator for FakeTasks {
    async fn create_task(&self, request: CreateTaskRequest) -> Result<String, ApiError> {
        let mut requests = self.requests.lock().unwrap();
        requests.push(request);
        Ok(format!("recv-task-{}", requests.len()))
    }
}

fn task(id: &str, status: i32, dir: &Path, name: &str) -> TaskDto {
    serde_json::from_value(json!({
        "taskId": id, "url": "https://example.com/f", "fileName": name,
        "saveDir": dir.to_string_lossy(), "status": status, "downloadedBytes": 0,
        "totalBytes": 0, "errorMessage": "", "createdAt": "1", "proxyUrl": "",
        "queueId": "main", "checksum": ""
    }))
    .unwrap()
}

struct Node {
    service: Arc<ShareService>,
    events: AgentEventHub,
    tasks: Arc<FakeTasks>,
    cancel: CancellationToken,
    _feed: mpsc::UnboundedSender<Value>,
}

/// 游客接收方（不登录）。
async fn guest_node(base: &str, label: &str) -> Node {
    let dir = std::env::temp_dir().join(format!(
        "fluxdown_share_src_{label}_{}_{}",
        std::process::id(),
        now_unix_secs()
    ));
    let store = Arc::new(StateStore::open(dir).await.unwrap());
    let state = Arc::new(tokio::sync::Mutex::new(AgentState {
        device_name: format!("{label}-device"),
        ..AgentState::default()
    }));
    let events = AgentEventHub::new(AgentSnapshot::default());
    let client = CloudClient::new(base.to_owned(), state.clone(), store.clone()).unwrap();
    let tasks = Arc::new(FakeTasks::default());
    let (feed, cloud_events) = mpsc::unbounded_channel();
    let service = ShareService::new(ShareServiceParts {
        events: events.clone(),
        state,
        store,
        cloud: CloudApi::new(client),
        tasks: tasks.clone(),
        cloud_events,
        egress: Egress::system_default(),
    })
    .await
    .unwrap();
    service.with(|i| i.local.enabled = true);
    let cancel = CancellationToken::new();
    tokio::spawn(service.clone().run(cancel.clone()));
    Node {
        service,
        events,
        tasks,
        cancel,
        _feed: feed,
    }
}

async fn call<T: serde::de::DeserializeOwned>(
    node: &Node,
    method: &str,
    params: Value,
) -> Result<T, fluxdown_protocol::RpcErrorData> {
    let value = node.service.rpc(method, Some(params)).await?;
    Ok(serde_json::from_value(value).unwrap())
}

async fn wait_until(what: &str, mut probe: impl FnMut() -> bool) {
    for _ in 0..300 {
        if probe() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for {what}");
}

fn http() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

async fn receive_first(node: &Node) -> (String, ShareResolveDto) {
    let preview: ShareResolveDto = call(
        node,
        method::AGENT_SHARE_RESOLVE,
        json!({ "codeOrLink": "123456" }),
    )
    .await
    .unwrap();
    let received: ShareReceiveResultDto = call(
        node,
        method::AGENT_SHARE_RECEIVE,
        json!({ "resolveId": preview.resolve_id }),
    )
    .await
    .unwrap();
    assert_eq!(received.task_ids, ["recv-task-1"]);
    let url = node.tasks.requests.lock().unwrap()[0].url.clone();
    (url, preview)
}

fn facts(node: &Node) -> ShareTaskFactsDto {
    node.service
        .with(|i| i.state_dto(false).task_facts.remove("recv-task-1"))
        .unwrap()
}

// ───────────── 测试 ─────────────

/// 分享方不在线、云端副本就绪：每个块恰好一次 CDN 区间请求（票据区间 = 块），
/// 字节逐一校验，来源徽标是云端，计量与实际字节一致并上报。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn offline_sender_is_served_from_the_cloud_copy() {
    let mock = start_mock("ready", Some(false), 9 * MIB).await;
    let node = guest_node(mock.base.get().unwrap(), "cdn").await;
    let (url, preview) = receive_first(&node).await;
    assert!(preview.sources.cloud_copy);
    assert!(
        !preview.sources.bridge,
        "a cloud copy makes the bridge moot"
    );

    let response = http().get(&url).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.bytes().await.unwrap().as_ref(),
        mock.data.as_slice()
    );

    // 下载票据：偏移 / 长度恰好是三个块（4 + 4 + 1 MiB），CDN 收到同样的区间。
    let tickets = mock.download_tickets.lock().unwrap().clone();
    let mut asked: Vec<(u64, u64)> = tickets
        .iter()
        .map(|t| (t["offset"].as_u64().unwrap(), t["len"].as_u64().unwrap()))
        .collect();
    asked.sort_unstable();
    assert_eq!(
        asked,
        vec![
            (0, 4 * MIB as u64),
            (4 * MIB as u64, 4 * MIB as u64),
            (8 * MIB as u64, MIB as u64)
        ]
    );
    assert!(
        tickets
            .iter()
            .all(|t| t["shareId"] == "share-src" && t["ticket"].is_string())
    );
    assert_eq!(mock.cdn_ranges.lock().unwrap().len(), 3);

    assert_eq!(facts(&node).path_kind, SharePathKind::Cdn);

    // 任务完成 → 会话收尾；来源字节恰好等于文件大小并出现在上报里。
    node.events
        .publish(AgentEvent::Daemon(DaemonEvent::TaskChanged(task(
            "recv-task-1",
            3,
            &std::env::temp_dir(),
            "payload.bin",
        ))));
    wait_until("cdn bytes reported", || {
        let total: u64 = mock
            .sessions
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s["final"] == true && s["role"] == "receiver")
            .map(|s| s["summary"]["bytes"]["cdn"].as_u64().unwrap_or(0))
            .sum();
        total == 9 * MIB as u64
    })
    .await;
    node.cancel.cancel();
}

/// 副本未就绪、分享方不可达：下载 503；用户「切换桥接」后同一任务改走网关，
/// 请求带桥接令牌与块区间；额度耗尽时给出带类型的错误。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn switch_to_bridge_serves_the_same_task_through_the_gateway() {
    let mock = start_mock("none", None, 9 * MIB).await;
    let node = guest_node(mock.base.get().unwrap(), "bridge").await;
    let (url, preview) = receive_first(&node).await;
    assert!(!preview.sources.cloud_copy);
    assert!(preview.sources.bridge);

    let offline = http().get(&url).send().await.unwrap();
    assert_eq!(offline.status(), 503, "no source can serve yet");
    assert!(mock.bridge_requests.lock().unwrap().is_empty());

    // 流量额度耗尽：换不到桥接。
    mock.bridge_exhausted.store(true, Ordering::SeqCst);
    let denied: Result<ShareTaskFactsDto, _> = call(
        &node,
        method::AGENT_SHARE_SWITCH_TO_BRIDGE,
        json!({ "taskId": "recv-task-1" }),
    )
    .await;
    assert_eq!(
        denied.unwrap_err().reason,
        Some(ErrorReason::ShareTrafficExhausted)
    );
    mock.bridge_exhausted.store(false, Ordering::SeqCst);

    // 未知任务。
    let unknown: Result<ShareTaskFactsDto, _> = call(
        &node,
        method::AGENT_SHARE_SWITCH_TO_BRIDGE,
        json!({ "taskId": "nope" }),
    )
    .await;
    assert_eq!(
        unknown.unwrap_err().reason,
        Some(ErrorReason::ShareNotFound)
    );

    let switched: ShareTaskFactsDto = call(
        &node,
        method::AGENT_SHARE_SWITCH_TO_BRIDGE,
        json!({ "taskId": "recv-task-1" }),
    )
    .await
    .unwrap();
    assert!(switched.is_share_receive);

    let response = http()
        .get(&url)
        .header("Range", "bytes=0-")
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success(), "{}", response.status());
    assert_eq!(
        response.bytes().await.unwrap().as_ref(),
        mock.data.as_slice()
    );
    let requests = mock.bridge_requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 3, "one gateway request per chunk");
    assert!(
        requests
            .iter()
            .all(|(auth, _)| auth == "Bearer bridge-token")
    );
    let mut ranges: Vec<String> = requests.iter().map(|(_, range)| range.clone()).collect();
    ranges.sort();
    let mut expected = vec![
        format!("bytes=0-{}", 4 * MIB - 1),
        format!("bytes={}-{}", 4 * MIB, 8 * MIB - 1),
        format!("bytes={}-{}", 8 * MIB, 9 * MIB - 1),
    ];
    expected.sort();
    assert_eq!(ranges, expected);
    assert_eq!(facts(&node).path_kind, SharePathKind::Bridge);
    assert!(!facts(&node).relay_stuck);
    node.cancel.cancel();
}
