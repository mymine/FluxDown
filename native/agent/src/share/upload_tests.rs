//! 云端暂存上传端到端测试：mock FluxCloud（分享 / 上传方案 / 秒传证明 / complete / 删除副本）
//! + mock S3（预签名 PUT，记录收到的分片，可指定分片失败 / 变慢）+ 真实的 `ShareService`。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path as AxPath, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use fluxdown_api::service::ApiError;
use fluxdown_protocol::{
    AgentEvent, AgentSnapshot, ApplicationErrorCode, CreateTaskRequest, ErrorReason, RpcErrorData,
    ServiceEvent, ShareCloudState, ShareDto, ShareFileDto, ShareMode, ShareStatus, ShareUploadDto,
    ShareUploadStatus, method,
};
use fluxdown_share::ticket::encode_public_key;
use fluxdown_share::timefmt::{now_unix_secs, rfc3339_ms};
use fluxdown_share::{SecretKey, ShareManifest};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::store::StoredShare;
use super::upload_job::Timing;
use super::{ShareService, ShareServiceParts};
use crate::cloud::{CloudApi, CloudClient};
use crate::egress::Egress;
use crate::event_hub::AgentEventHub;
use crate::link::LinkTaskCreator;
use crate::state::{AgentState, CloudCredentials, StateStore};

/// mock 云端下发的分片大小（真实为 16 MiB）。
const PART: u64 = 1024 * 1024;

// ───────────── mock FluxCloud + mock S3 ─────────────

struct MockFile {
    root_hash: String,
    size: u64,
}

struct MockShare {
    files: Vec<MockFile>,
    cloud_state: &'static str,
    mode: &'static str,
    tier: Option<String>,
    /// `(文件, 分片)` → (字节, etag)：云端 ListParts 的事实。
    uploaded: HashMap<(u32, u32), (Vec<u8>, String)>,
    done: HashSet<u32>,
    assembled: HashMap<u32, Vec<u8>>,
    plan_calls: Vec<Value>,
    proofs: Vec<(u32, Value)>,
    completes: Vec<(u32, Value)>,
    failed_proofs: HashSet<u32>,
}

#[derive(Default)]
struct Behavior {
    /// 该 `(文件, 分片)` 的 PUT 一律回 500。
    fail_parts: HashSet<(u32, u32)>,
    /// 每个 PUT 先睡这么久。
    put_delay_ms: u64,
    quota_exceeded: bool,
    cloud_disabled: bool,
    /// 同用户已有同根哈希的对象（触发秒传挑战）：根哈希 → 对象字节。
    existing: HashMap<String, Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PutRecord {
    share: String,
    file: u32,
    part: u32,
    ok: bool,
}

struct Mock {
    base: OnceLock<String>,
    signer: SecretKey,
    shares: Mutex<HashMap<String, MockShare>>,
    behavior: Mutex<Behavior>,
    puts: Mutex<Vec<PutRecord>>,
    put_started: AtomicUsize,
    cloud_deletes: AtomicUsize,
}

type MockState = Arc<Mock>;

fn error(status: StatusCode, code: &str) -> Response {
    (status, Json(json!({ "code": code, "message": code }))).into_response()
}

async fn config(State(mock): State<MockState>) -> Response {
    let cloud_enabled = !mock.behavior.lock().unwrap().cloud_disabled;
    let body = json!({
        "enabled": true, "guestReceive": true, "guestBridge": false, "guestShare": false,
        "pickupCodeLength": 6, "chunkBytes": 4_194_304,
        "liveTiers": [{ "id": "t1", "hours": 24, "enabled": true, "default": true }],
        "retentionTiers": [
            { "id": "cloud-24h", "hours": 24, "enabled": true, "default": false },
            { "id": "cloud-72h", "hours": 72, "enabled": true, "default": true },
            { "id": "cloud-7d", "hours": 168, "enabled": true, "default": false }
        ],
        "cloudEnabled": cloud_enabled,
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

async fn register_endpoint(Json(body): Json<Value>) -> Json<Value> {
    Json(json!({ "endpointId": body["endpointId"], "dnsOrigin": "" }))
}

fn share_json(id: &str, share: &MockShare) -> Value {
    json!({
        "id": id, "slug": format!("slug-{id}"), "pickupCode": "123456",
        "url": format!("https://fluxdown.test/s/slug-{id}"),
        "title": id, "mode": share.mode, "tierId": share.tier.clone().unwrap_or_else(|| "t1".into()),
        "expiresAt": rfc3339_ms((now_unix_secs() + 3600) * 1000),
        "maxClaims": null, "claimCount": 0, "notifyOnClaim": false,
        "status": "active", "cloudState": share.cloud_state,
        "cloudBytes": if share.cloud_state == "ready" {
            share.files.iter().map(|f| f.size).sum::<u64>()
        } else { 0 }
    })
}

async fn list_shares(State(mock): State<MockState>) -> Json<Value> {
    let shares = mock.shares.lock().unwrap();
    Json(json!({ "shares": shares.iter().map(|(id, s)| share_json(id, s)).collect::<Vec<_>>() }))
}

fn finish_if_complete(share: &mut MockShare) {
    if (0..share.files.len() as u32).all(|i| share.done.contains(&i)) {
        share.cloud_state = "ready";
        share.mode = "cloud";
    }
}

async fn upload_plan(
    State(mock): State<MockState>,
    AxPath(id): AxPath<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if !headers.contains_key(header::AUTHORIZATION) {
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    let (quota, existing) = {
        let behavior = mock.behavior.lock().unwrap();
        (behavior.quota_exceeded, behavior.existing.clone())
    };
    if quota {
        return error(StatusCode::FORBIDDEN, "share_quota_exceeded");
    }
    let base = mock.base.get().unwrap().clone();
    let mut shares = mock.shares.lock().unwrap();
    let Some(share) = shares.get_mut(&id) else {
        return error(StatusCode::NOT_FOUND, "not_found");
    };
    share.plan_calls.push(body.clone());
    share.cloud_state = "uploading";
    if let Some(tier) = body["tierId"].as_str() {
        share.tier = Some(tier.to_owned());
    }
    let mut files = Vec::new();
    for file in body["files"].as_array().unwrap() {
        let index = file["index"].as_u64().unwrap() as u32;
        let root = file["rootHash"].as_str().unwrap();
        let size = file["size"].as_u64().unwrap();
        if share.done.contains(&index) {
            files.push(json!({ "index": index, "state": "done", "partBytes": PART, "parts": [] }));
            continue;
        }
        if existing.contains_key(root) && !share.failed_proofs.contains(&index) {
            files.push(json!({
                "index": index, "state": "challenge",
                "challenge": { "offset": 1000, "len": 5000 },
                "partBytes": PART, "parts": []
            }));
            continue;
        }
        let total = size.div_ceil(PART) as u32;
        let parts: Vec<Value> = (1..=total)
            .filter(|n| !share.uploaded.contains_key(&(index, *n)))
            .map(|n| json!({ "number": n, "url": format!("{base}/s3/{id}/{index}/{n}?sig=abc") }))
            .collect();
        files.push(json!({
            "index": index, "state": "needUpload", "uploadId": format!("up-{id}-{index}"),
            "partBytes": PART, "parts": parts
        }));
    }
    Json(json!({ "files": files })).into_response()
}

async fn upload_proof(
    State(mock): State<MockState>,
    AxPath((id, index)): AxPath<(String, u32)>,
    Json(body): Json<Value>,
) -> Response {
    let existing = mock.behavior.lock().unwrap().existing.clone();
    let mut shares = mock.shares.lock().unwrap();
    let Some(share) = shares.get_mut(&id) else {
        return error(StatusCode::NOT_FOUND, "not_found");
    };
    share.proofs.push((index, body.clone()));
    let root = share.files[index as usize].root_hash.clone();
    let offset = body["offset"].as_u64().unwrap() as usize;
    let len = body["len"].as_u64().unwrap() as usize;
    let expected = existing.get(&root).map(|bytes| {
        blake3::hash(&bytes[offset..offset + len])
            .to_hex()
            .to_string()
    });
    if expected.as_deref() == body["blake3"].as_str() {
        share.done.insert(index);
        finish_if_complete(share);
        StatusCode::NO_CONTENT.into_response()
    } else {
        share.failed_proofs.insert(index);
        error(StatusCode::FORBIDDEN, "share_proof_failed")
    }
}

async fn upload_complete(
    State(mock): State<MockState>,
    AxPath((id, index)): AxPath<(String, u32)>,
    Json(body): Json<Value>,
) -> Response {
    let mut shares = mock.shares.lock().unwrap();
    let Some(share) = shares.get_mut(&id) else {
        return error(StatusCode::NOT_FOUND, "not_found");
    };
    share.completes.push((index, body.clone()));
    let total = share.files[index as usize].size.div_ceil(PART) as u32;
    let mut assembled = Vec::new();
    for number in 1..=total {
        let Some((bytes, etag)) = share.uploaded.get(&(index, number)) else {
            return error(StatusCode::CONFLICT, "validation_error");
        };
        // 提交的 etag 必须与云端记录一致（原样含引号）。
        let sent = body["parts"]
            .as_array()
            .and_then(|parts| parts.iter().find(|p| p["number"] == number));
        if sent.is_some_and(|p| p["etag"].as_str() != Some(etag.as_str())) {
            return error(StatusCode::CONFLICT, "validation_error");
        }
        assembled.extend_from_slice(bytes);
    }
    share.assembled.insert(index, assembled);
    share.done.insert(index);
    finish_if_complete(share);
    StatusCode::NO_CONTENT.into_response()
}

async fn delete_cloud(State(mock): State<MockState>, AxPath(id): AxPath<String>) -> Response {
    let mut shares = mock.shares.lock().unwrap();
    let Some(share) = shares.get_mut(&id) else {
        return error(StatusCode::NOT_FOUND, "not_found");
    };
    mock.cloud_deletes.fetch_add(1, Ordering::SeqCst);
    share.cloud_state = "deleted";
    share.mode = "live";
    share.uploaded.clear();
    share.done.clear();
    share.assembled.clear();
    Json(share_json(&id, share)).into_response()
}

async fn put_part(
    State(mock): State<MockState>,
    AxPath((id, index, number)): AxPath<(String, u32, u32)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    mock.put_started.fetch_add(1, Ordering::SeqCst);
    let (delay, fail) = {
        let behavior = mock.behavior.lock().unwrap();
        (
            behavior.put_delay_ms,
            behavior.fail_parts.contains(&(index, number)),
        )
    };
    if delay > 0 {
        tokio::time::sleep(Duration::from_millis(delay)).await;
    }
    let declared = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok());
    let record = |ok| PutRecord {
        share: id.clone(),
        file: index,
        part: number,
        ok,
    };
    if declared != Some(body.len()) {
        mock.puts.lock().unwrap().push(record(false));
        return error(StatusCode::BAD_REQUEST, "length_mismatch");
    }
    if fail {
        mock.puts.lock().unwrap().push(record(false));
        return error(StatusCode::INTERNAL_SERVER_ERROR, "boom");
    }
    let etag = format!("\"etag-{index}-{number}\"");
    {
        let mut shares = mock.shares.lock().unwrap();
        if let Some(share) = shares.get_mut(&id) {
            share
                .uploaded
                .insert((index, number), (body.to_vec(), etag.clone()));
        }
    }
    mock.puts.lock().unwrap().push(record(true));
    (StatusCode::OK, [(header::ETAG, etag)]).into_response()
}

async fn start_mock() -> (String, MockState) {
    let mock = Arc::new(Mock {
        base: OnceLock::new(),
        signer: SecretKey::generate(),
        shares: Mutex::new(HashMap::new()),
        behavior: Mutex::new(Behavior::default()),
        puts: Mutex::new(Vec::new()),
        put_started: AtomicUsize::new(0),
        cloud_deletes: AtomicUsize::new(0),
    });
    let app = Router::new()
        .route("/api/v1/share/config", get(config))
        .route("/api/v1/share/keys", get(keys))
        .route("/api/v1/share/endpoint", put(register_endpoint))
        .route("/api/v1/share/shares", get(list_shares))
        .route("/api/v1/share/shares/{id}/upload", post(upload_plan))
        .route(
            "/api/v1/share/shares/{id}/upload/{index}/proof",
            post(upload_proof),
        )
        .route(
            "/api/v1/share/shares/{id}/upload/{index}/complete",
            post(upload_complete),
        )
        .route("/api/v1/share/shares/{id}/cloud", delete(delete_cloud))
        .route("/s3/{id}/{index}/{number}", put(put_part))
        .layer(DefaultBodyLimit::max(64 << 20))
        .with_state(mock.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    mock.base.set(base.clone()).unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (base, mock)
}

// ───────────── 测试宿主 ─────────────

struct NoTasks;

#[async_trait]
impl LinkTaskCreator for NoTasks {
    async fn create_task(&self, _request: CreateTaskRequest) -> Result<String, ApiError> {
        Ok("unused".to_owned())
    }
}

struct Node {
    service: Arc<ShareService>,
    events: AgentEventHub,
    cancel: CancellationToken,
    dir: PathBuf,
    _feed: mpsc::UnboundedSender<Value>,
}

fn temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "fluxdown_upload_{label}_{}_{}",
        std::process::id(),
        now_unix_secs()
    ));
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => panic!("clean {}: {error}", dir.display()),
    }
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 测试里把各类等待压到毫秒级。
fn fast() -> Timing {
    Timing {
        part_backoff: Duration::from_millis(20),
        retry_first: Duration::from_millis(100),
        retry_max: Duration::from_millis(200),
        replan: Duration::from_millis(10),
        stall: Duration::from_secs(5),
        progress_every: Duration::from_millis(20),
    }
}

/// 暂停后不自动重试（测试里手动“重启”）。
fn parked() -> Timing {
    Timing {
        retry_first: Duration::from_secs(600),
        retry_max: Duration::from_secs(600),
        ..fast()
    }
}

async fn open_store(dir: &Path) -> Arc<StateStore> {
    // 旧服务的收尾任务可能还没放开状态目录锁。
    let mut last = None;
    for _ in 0..100 {
        match StateStore::open(dir.to_path_buf()).await {
            Ok(store) => return Arc::new(store),
            Err(error) => {
                last = Some(error);
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
    panic!("state dir stayed locked: {last:?}");
}

async fn start_node(base: &str, dir: &Path, logged_in: bool, timing: Timing) -> Node {
    let store = open_store(dir).await;
    let mut state = AgentState {
        device_name: "uploader".to_owned(),
        ..AgentState::default()
    };
    if logged_in {
        state.credentials = Some(CloudCredentials {
            access_token: "access".into(),
            refresh_token: "refresh".into(),
            expires_at_unix: i64::MAX / 2,
            session: None,
        });
    }
    let state = Arc::new(tokio::sync::Mutex::new(state));
    let events = AgentEventHub::new(AgentSnapshot::default());
    let client = CloudClient::new(base.to_owned(), state.clone(), store.clone()).unwrap();
    let (feed, cloud_events) = mpsc::unbounded_channel();
    let service = ShareService::new(ShareServiceParts {
        events: events.clone(),
        state,
        store,
        cloud: CloudApi::new(client),
        tasks: Arc::new(NoTasks),
        cloud_events,
        egress: Egress::system_default(),
    })
    .await
    .unwrap();
    service.with(|i| {
        i.upload_timing = timing;
        i.local.enabled = true;
    });
    let cancel = CancellationToken::new();
    tokio::spawn(service.clone().run(cancel.clone()));
    Node {
        service,
        events,
        cancel,
        dir: dir.to_path_buf(),
        _feed: feed,
    }
}

/// 本机写出文件、算清单、登记进 mock 云端与本机分享表。返回根哈希。
async fn seed(mock: &MockState, node: &Node, id: &str, name: &str, data: &[u8]) -> String {
    let files_dir = node.dir.join("files");
    std::fs::create_dir_all(&files_dir).unwrap();
    let path = files_dir.join(name);
    std::fs::write(&path, data).unwrap();
    let file = fluxdown_share::hash_file(
        path.clone(),
        Some(name.to_owned()),
        |_| {},
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let root = file.root_hash.clone();
    mock.shares.lock().unwrap().insert(
        id.to_owned(),
        MockShare {
            files: vec![MockFile {
                root_hash: root.clone(),
                size: file.size,
            }],
            cloud_state: "none",
            mode: "live",
            tier: None,
            uploaded: HashMap::new(),
            done: HashSet::new(),
            assembled: HashMap::new(),
            plan_calls: Vec::new(),
            proofs: Vec::new(),
            completes: Vec::new(),
            failed_proofs: HashSet::new(),
        },
    );
    let dto = ShareDto {
        id: id.to_owned(),
        slug: format!("slug-{id}"),
        title: name.to_owned(),
        files: vec![ShareFileDto {
            name: name.to_owned(),
            size: file.size,
        }],
        total_bytes: file.size,
        tier_id: "t1".to_owned(),
        expires_at: rfc3339_ms((now_unix_secs() + 3600) * 1000),
        status: ShareStatus::Active,
        ..ShareDto::default()
    };
    let stored = StoredShare {
        dto,
        manifest: ShareManifest {
            share_id: id.to_owned(),
            files: vec![file],
        },
        paths: vec![path],
        guest_owned: false,
    };
    node.service.with(|i| i.shares.push(stored));
    node.service.persist_shares().await;
    root
}

async fn call<T: serde::de::DeserializeOwned>(
    node: &Node,
    method: &str,
    params: Value,
) -> Result<T, RpcErrorData> {
    let value = node.service.rpc(method, Some(params)).await?;
    Ok(serde_json::from_value(value).unwrap())
}

async fn wait_until(what: &str, mut probe: impl FnMut() -> bool) {
    for _ in 0..400 {
        if probe() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out waiting for {what}");
}

fn pseudo_random(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed | 1;
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        out.extend_from_slice(&state.wrapping_mul(0x2545_F491_4F6C_DD1D).to_le_bytes());
    }
    out.truncate(len);
    out
}

fn cloud_state(node: &Node, id: &str) -> ShareCloudState {
    node.service.with(|i| {
        i.shares
            .iter()
            .find(|s| s.dto.id == id)
            .map(|s| s.dto.cloud_state)
            .unwrap()
    })
}

fn upload_status(node: &Node, id: &str) -> Option<ShareUploadDto> {
    node.service
        .with(|i| i.uploads.get(id).map(|e| e.progress.clone()))
}

/// 收集 `ShareChanged` 里该分享的 `upload` 进度。
fn watch_uploads(events: &AgentEventHub, id: &str) -> Arc<Mutex<Vec<ShareUploadDto>>> {
    let mut receiver = events.subscribe();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let id = id.to_owned();
    tokio::spawn(async move {
        loop {
            match receiver.recv().await {
                Ok(frame) => {
                    if let ServiceEvent::Agent(AgentEvent::ShareChanged(state)) = frame.event
                        && let Some(upload) = state
                            .shares
                            .iter()
                            .find(|s| s.id == id)
                            .and_then(|s| s.upload.clone())
                    {
                        sink.lock().unwrap().push(upload);
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
    seen
}

fn successful_puts(mock: &MockState) -> Vec<(u32, u32)> {
    mock.puts
        .lock()
        .unwrap()
        .iter()
        .filter(|p| p.ok)
        .map(|p| (p.file, p.part))
        .collect()
}

// ───────────── 测试 ─────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn multi_part_upload_completes_with_progress() {
    let (base, mock) = start_mock().await;
    mock.behavior.lock().unwrap().put_delay_ms = 40;
    let dir = temp_dir("multi");
    let node = start_node(&base, &dir, true, fast()).await;
    let data = pseudo_random(3 * PART as usize + 12_345, 7);
    seed(&mock, &node, "s1", "multi.bin", &data).await;
    let seen = watch_uploads(&node.events, "s1");

    // 不在保留期档位里的 tierId 被拒。
    let bad: Result<ShareDto, _> = call(
        &node,
        method::AGENT_SHARE_CONVERT_TO_CLOUD,
        json!({ "id": "s1", "tierId": "cloud-1000d" }),
    )
    .await;
    assert_eq!(bad.unwrap_err().field.as_deref(), Some("tierId"));

    let started: ShareDto = call(
        &node,
        method::AGENT_SHARE_CONVERT_TO_CLOUD,
        json!({ "id": "s1" }),
    )
    .await
    .unwrap();
    assert_eq!(started.cloud_state, ShareCloudState::Uploading);
    let upload = started.upload.expect("upload progress");
    assert_eq!(upload.status, ShareUploadStatus::Uploading);
    assert_eq!(upload.total_bytes, data.len() as u64);

    wait_until("cloud copy ready", || {
        cloud_state(&node, "s1") == ShareCloudState::Ready
    })
    .await;

    let shares: Vec<ShareDto> = call(&node, method::AGENT_SHARE_LIST, json!({}))
        .await
        .unwrap();
    assert_eq!(shares[0].mode, ShareMode::Cloud);
    assert_eq!(shares[0].cloud_state, ShareCloudState::Ready);
    assert_eq!(shares[0].cloud_bytes, data.len() as u64);
    assert!(shares[0].upload.is_none());
    assert!(node.service.with(|i| i.uploads.is_empty()));
    assert!(!dir.join("share/uploads/s1.json").exists());

    {
        let shares = mock.shares.lock().unwrap();
        let share = &shares["s1"];
        assert_eq!(share.assembled[&0], data);
        // 默认档位随首次方案请求带上；文件清单带根哈希与大小。
        assert_eq!(share.plan_calls[0]["tierId"], "cloud-72h");
        assert_eq!(share.plan_calls[0]["files"][0]["size"], data.len());
        assert_eq!(share.completes.len(), 1);
    }
    let mut puts = successful_puts(&mock);
    puts.sort_unstable();
    assert_eq!(puts, [(0, 1), (0, 2), (0, 3), (0, 4)]);

    let seen = seen.lock().unwrap();
    assert!(
        seen.len() >= 2,
        "expected several progress events, got {seen:?}"
    );
    assert!(
        seen.iter()
            .all(|u| u.total_bytes == data.len() as u64 && u.done_bytes <= u.total_bytes)
    );
    assert!(
        seen.iter().any(|u| u.done_bytes > 0 && u.rate_bps > 0),
        "{seen:?}"
    );
    node.cancel.cancel();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn upload_resumes_after_restart_and_sends_only_missing_parts() {
    let (base, mock) = start_mock().await;
    let dir = temp_dir("resume");
    let data = pseudo_random(3 * PART as usize + 777, 11);
    // 第 3 片一直失败：其余三片完成后上传暂停。
    mock.behavior.lock().unwrap().fail_parts.insert((0, 3));
    let first = start_node(&base, &dir, true, parked()).await;
    seed(&mock, &first, "s1", "resume.bin", &data).await;
    let started: ShareDto = call(
        &first,
        method::AGENT_SHARE_CONVERT_TO_CLOUD,
        json!({ "id": "s1" }),
    )
    .await
    .unwrap();
    assert_eq!(started.cloud_state, ShareCloudState::Uploading);
    wait_until("upload paused", || {
        upload_status(&first, "s1").is_some_and(|u| u.status == ShareUploadStatus::Paused)
    })
    .await;
    let paused = upload_status(&first, "s1").unwrap();
    assert_eq!(paused.reason, Some(ErrorReason::CloudUnreachable));
    assert_eq!(paused.rate_bps, 0);
    {
        let shares = mock.shares.lock().unwrap();
        let mut uploaded: Vec<_> = shares["s1"].uploaded.keys().copied().collect();
        uploaded.sort_unstable();
        assert_eq!(uploaded, [(0, 1), (0, 2), (0, 4)]);
    }
    // 断点文件只含已完成分片的 etag。
    let state: Value =
        serde_json::from_slice(&std::fs::read(dir.join("share/uploads/s1.json")).unwrap()).unwrap();
    assert_eq!(state["files"][0]["uploadId"], "up-s1-0");
    assert_eq!(state["files"][0]["doneParts"].as_array().unwrap().len(), 3);

    // “重启”：停掉旧服务，同一数据目录起新服务。
    first.cancel.cancel();
    drop(first);
    mock.behavior.lock().unwrap().fail_parts.clear();
    let puts_before = mock.puts.lock().unwrap().len();
    let second = start_node(&base, &dir, true, fast()).await;
    wait_until("resumed upload ready", || {
        mock.shares.lock().unwrap()["s1"].cloud_state == "ready"
    })
    .await;
    wait_until("local state ready", || {
        cloud_state(&second, "s1") == ShareCloudState::Ready
    })
    .await;

    // 只补了缺的那一片。
    let after: Vec<PutRecord> = mock.puts.lock().unwrap()[puts_before..].to_vec();
    assert_eq!(
        after,
        [PutRecord {
            share: "s1".into(),
            file: 0,
            part: 3,
            ok: true
        }]
    );
    let shares = mock.shares.lock().unwrap();
    let share = &shares["s1"];
    assert_eq!(share.assembled[&0], data);
    // complete 带上了重启前上传的分片 etag（来自断点文件）与这次的第 3 片。
    let (_, body) = share.completes.last().unwrap();
    let etags: Vec<(u64, String)> = body["parts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["number"].as_u64().unwrap(),
                p["etag"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        etags,
        [
            (1, "\"etag-0-1\"".to_owned()),
            (2, "\"etag-0-2\"".to_owned()),
            (3, "\"etag-0-3\"".to_owned()),
            (4, "\"etag-0-4\"".to_owned()),
        ]
    );
    drop(shares);
    assert!(!dir.join("share/uploads/s1.json").exists());
    second.cancel.cancel();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn instant_upload_by_proof_and_fallback_when_proof_fails() {
    let (base, mock) = start_mock().await;
    let dir = temp_dir("instant");
    let node = start_node(&base, &dir, true, fast()).await;

    // s1：云端已有同内容对象，证明通过 → 秒传，没有任何分片 PUT。
    let good = pseudo_random(200_000, 21);
    let root = seed(&mock, &node, "s1", "good.bin", &good).await;
    mock.behavior
        .lock()
        .unwrap()
        .existing
        .insert(root, good.clone());
    let _: ShareDto = call(
        &node,
        method::AGENT_SHARE_CONVERT_TO_CLOUD,
        json!({ "id": "s1" }),
    )
    .await
    .unwrap();
    wait_until("instant upload ready", || {
        cloud_state(&node, "s1") == ShareCloudState::Ready
    })
    .await;
    assert!(mock.puts.lock().unwrap().is_empty());
    {
        let shares = mock.shares.lock().unwrap();
        let (index, proof) = &shares["s1"].proofs[0];
        assert_eq!(*index, 0);
        assert_eq!(proof["offset"], 1000);
        assert_eq!(proof["len"], 5000);
        assert_eq!(
            proof["blake3"].as_str().unwrap(),
            blake3::hash(&good[1000..6000]).to_hex().as_str()
        );
        assert!(shares["s1"].completes.is_empty());
    }

    // s2：云端同根哈希对象的内容与本机不符（持有性证明失败）→ 回落为普通上传。
    let other = pseudo_random(PART as usize + 5_000, 33);
    let root = seed(&mock, &node, "s2", "other.bin", &other).await;
    let mut forged = other.clone();
    forged[1500] ^= 0xff;
    mock.behavior.lock().unwrap().existing.insert(root, forged);
    let _: ShareDto = call(
        &node,
        method::AGENT_SHARE_CONVERT_TO_CLOUD,
        json!({ "id": "s2" }),
    )
    .await
    .unwrap();
    wait_until("fallback upload ready", || {
        cloud_state(&node, "s2") == ShareCloudState::Ready
    })
    .await;
    let shares = mock.shares.lock().unwrap();
    assert_eq!(shares["s2"].proofs.len(), 1);
    assert_eq!(shares["s2"].assembled[&0], other);
    drop(shares);
    let mut s2_puts: Vec<_> = mock
        .puts
        .lock()
        .unwrap()
        .iter()
        .filter(|p| p.share == "s2" && p.ok)
        .map(|p| p.part)
        .collect();
    s2_puts.sort_unstable();
    assert_eq!(s2_puts, [1, 2]);
    node.cancel.cancel();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn delete_cloud_copy_cancels_a_running_upload() {
    let (base, mock) = start_mock().await;
    // PUT 久久不返回：上传停在传输中。
    mock.behavior.lock().unwrap().put_delay_ms = 30_000;
    let dir = temp_dir("delete");
    let node = start_node(&base, &dir, true, fast()).await;
    let data = pseudo_random(2 * PART as usize, 5);
    seed(&mock, &node, "s1", "slow.bin", &data).await;
    let _: ShareDto = call(
        &node,
        method::AGENT_SHARE_CONVERT_TO_CLOUD,
        json!({ "id": "s1" }),
    )
    .await
    .unwrap();
    wait_until("a part is in flight", || {
        mock.put_started.load(Ordering::SeqCst) >= 1
    })
    .await;
    assert!(dir.join("share/uploads/s1.json").exists());

    let deleted: ShareDto = call(
        &node,
        method::AGENT_SHARE_DELETE_CLOUD_COPY,
        json!({ "id": "s1" }),
    )
    .await
    .unwrap();
    assert_eq!(deleted.cloud_state, ShareCloudState::Deleted);
    assert_eq!(deleted.mode, ShareMode::Live);
    assert!(deleted.upload.is_none());
    assert!(node.service.with(|i| i.uploads.is_empty()));
    assert!(!dir.join("share/uploads/s1.json").exists());
    assert_eq!(mock.cloud_deletes.load(Ordering::SeqCst), 1);
    // 取消后没有任何分片完成、没有 complete。
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(successful_puts(&mock).is_empty());
    assert!(mock.shares.lock().unwrap()["s1"].completes.is_empty());
    node.cancel.cancel();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn convert_preflight_errors_are_typed() {
    let (base, mock) = start_mock().await;
    let data = pseudo_random(50_000, 9);

    // 游客：Unauthorized，没有 reason。
    let guest_dir = temp_dir("guest");
    let guest = start_node(&base, &guest_dir, false, fast()).await;
    seed(&mock, &guest, "s1", "g.bin", &data).await;
    let denied: Result<ShareDto, _> = call(
        &guest,
        method::AGENT_SHARE_CONVERT_TO_CLOUD,
        json!({ "id": "s1" }),
    )
    .await;
    let denied = denied.unwrap_err();
    assert_eq!(denied.code, ApplicationErrorCode::Unauthorized);
    assert_eq!(denied.reason, None);
    let denied: Result<ShareDto, _> = call(
        &guest,
        method::AGENT_SHARE_DELETE_CLOUD_COPY,
        json!({ "id": "s1" }),
    )
    .await;
    assert_eq!(denied.unwrap_err().code, ApplicationErrorCode::Unauthorized);
    guest.cancel.cancel();

    // 额度 / 权益：云端 403 → ShareQuotaExceeded，本机不留上传状态。
    let dir = temp_dir("quota");
    let node = start_node(&base, &dir, true, fast()).await;
    seed(&mock, &node, "s2", "q.bin", &data).await;
    mock.behavior.lock().unwrap().quota_exceeded = true;
    let quota: Result<ShareDto, _> = call(
        &node,
        method::AGENT_SHARE_CONVERT_TO_CLOUD,
        json!({ "id": "s2" }),
    )
    .await;
    assert_eq!(
        quota.unwrap_err().reason,
        Some(ErrorReason::ShareQuotaExceeded)
    );
    assert!(node.service.with(|i| i.uploads.is_empty()));
    assert_eq!(cloud_state(&node, "s2"), ShareCloudState::Idle);
    mock.behavior.lock().unwrap().quota_exceeded = false;

    // 不存在的分享。
    let missing: Result<ShareDto, _> = call(
        &node,
        method::AGENT_SHARE_CONVERT_TO_CLOUD,
        json!({ "id": "nope" }),
    )
    .await;
    assert_eq!(
        missing.unwrap_err().reason,
        Some(ErrorReason::ShareNotFound)
    );

    // 源文件在转暂存前被改：ShareFileChanged，且没有请求云端。
    std::fs::write(dir.join("files/q.bin"), b"changed").unwrap();
    let plans_before = mock.shares.lock().unwrap()["s2"].plan_calls.len();
    let changed: Result<ShareDto, _> = call(
        &node,
        method::AGENT_SHARE_CONVERT_TO_CLOUD,
        json!({ "id": "s2" }),
    )
    .await;
    assert_eq!(
        changed.unwrap_err().reason,
        Some(ErrorReason::ShareFileChanged)
    );
    assert_eq!(
        mock.shares.lock().unwrap()["s2"].plan_calls.len(),
        plans_before
    );
    node.cancel.cancel();

    // 云端存储未配置：转暂存不可用。
    mock.behavior.lock().unwrap().cloud_disabled = true;
    let dir = temp_dir("disabled");
    let node = start_node(&base, &dir, true, fast()).await;
    seed(&mock, &node, "s3", "d.bin", &data).await;
    let disabled: Result<ShareDto, _> = call(
        &node,
        method::AGENT_SHARE_CONVERT_TO_CLOUD,
        json!({ "id": "s3" }),
    )
    .await;
    assert_eq!(
        disabled.unwrap_err().reason,
        Some(ErrorReason::ShareCloudUnavailable)
    );
    node.cancel.cancel();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn source_changed_during_upload_fails_with_a_reason_until_retried_or_deleted() {
    let (base, mock) = start_mock().await;
    mock.behavior.lock().unwrap().put_delay_ms = 150;
    let dir = temp_dir("changed");
    let node = start_node(&base, &dir, true, fast()).await;
    let data = pseudo_random(3 * PART as usize, 3);
    seed(&mock, &node, "s1", "c.bin", &data).await;
    let _: ShareDto = call(
        &node,
        method::AGENT_SHARE_CONVERT_TO_CLOUD,
        json!({ "id": "s1" }),
    )
    .await
    .unwrap();
    // 上传期间源文件被改。
    std::fs::write(dir.join("files/c.bin"), b"short").unwrap();
    wait_until("upload failed", || {
        upload_status(&node, "s1").is_some_and(|u| u.status == ShareUploadStatus::Failed)
    })
    .await;
    let failed = upload_status(&node, "s1").unwrap();
    assert_eq!(failed.reason, Some(ErrorReason::ShareFileChanged));
    assert_eq!(failed.rate_bps, 0);
    // 失败的上传仍作为「上传中 + failed」出现在状态里，UI 据此给出重试。
    let listed: Vec<ShareDto> = call(&node, method::AGENT_SHARE_LIST, json!({}))
        .await
        .unwrap();
    assert_eq!(listed[0].cloud_state, ShareCloudState::Uploading);
    assert_eq!(
        listed[0].upload.as_ref().map(|u| u.status),
        Some(ShareUploadStatus::Failed)
    );
    // 删除副本清掉失败状态。
    let deleted: ShareDto = call(
        &node,
        method::AGENT_SHARE_DELETE_CLOUD_COPY,
        json!({ "id": "s1" }),
    )
    .await
    .unwrap();
    assert_eq!(deleted.cloud_state, ShareCloudState::Deleted);
    assert!(deleted.upload.is_none());
    assert!(node.service.with(|i| i.uploads.is_empty()));
    node.cancel.cancel();
}
