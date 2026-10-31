//! FluxMesh服务端到端测试：mock FluxCloud（axum）+ 两个 `ShareService`（分享方 / 游客接收方）。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use fluxdown_api::service::ApiError;
use fluxdown_protocol::{
    AgentEvent, AgentSnapshot, CreateTaskRequest, DaemonEvent, DaemonSnapshot, ShareDto,
    ShareReceiveResultDto, ShareResolveDto, ShareStatus, TaskDto, method,
};
use fluxdown_share::ticket::{encode_public_key, sign_ticket};
use fluxdown_share::timefmt::{now_unix_secs, rfc3339_ms};
use fluxdown_share::{EndpointAddr, EndpointId, SecretKey, TicketPayload};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{ShareService, ShareServiceParts};
use crate::cloud::{CloudApi, CloudClient};
use crate::event_hub::AgentEventHub;
use crate::link::LinkTaskCreator;
use crate::state::{AgentState, CloudCredentials, StateStore};

// ───────────── mock FluxCloud ─────────────

pub(super) struct MockShare {
    id: String,
    slug: String,
    code: String,
    pub(super) create_body: Value,
    pub(super) status: String,
    /// 创建时的 Bearer：账号令牌或游客令牌；列出 / 停止只对同一身份可见。
    owner: String,
    /// 已领取次数（每次 resolve +1，与真实服务端「新接收方才计数」不同，这里只需要有变化）。
    claims: u64,
    /// 已恢复分享的次数（用于生成新的取件码）。
    resumes: u32,
}

pub(super) struct Mock {
    signer: SecretKey,
    pub(super) shares: Mutex<Vec<MockShare>>,
    nets: Mutex<Vec<Value>>,
    pub(super) sessions: Mutex<Vec<Value>>,
    endpoints: Mutex<Vec<String>>,
    guest_share: std::sync::atomic::AtomicBool,
    /// 已签发的游客发送方令牌（每次换取一条）。
    guest_sessions: Mutex<Vec<String>>,
    /// 下发的 `network.republishSecs`：测试改它模拟后台修改网络策略。
    republish_secs: std::sync::atomic::AtomicU64,
    /// 列表先取快照、再等这么久才返回：模拟请求在途期间本机又改了分享表。
    list_delay_ms: std::sync::atomic::AtomicU64,
    /// 共享登记行：（endpoint，根哈希，分享 ID，是否完整）。
    pub(super) swarm_rows: Mutex<Vec<(String, String, String, bool)>>,
    /// 收到的共享登记请求原文。
    pub(super) announces: Mutex<Vec<Value>>,
    /// `GET /share/usage` 的响应体；`None` = 返回 500。
    pub(super) usage: Mutex<Option<Value>>,
    /// 注入 `POST /share/swarm/peers` 的错误：（状态码，错误码）。
    pub(super) peers_error: Mutex<Option<(StatusCode, &'static str)>>,
    /// 收到的 `POST /share/report` 请求体。
    pub(super) reports: Mutex<Vec<Value>>,
    /// 收到的请求总数（任何路由）：开关关闭期间必须保持 0。
    pub(super) requests: std::sync::atomic::AtomicUsize,
}

pub(super) type MockState = Arc<Mock>;

async fn count_request(
    State(mock): State<MockState>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    mock.requests
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    next.run(request).await
}

fn error(status: StatusCode, code: &str) -> Response {
    (status, Json(json!({ "code": code, "message": code }))).into_response()
}

fn bearer(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::to_owned)
}

/// `POST /share/report`：校验 endpoint 签名串（`fluxdown-share-report/v1`），记录请求体，204。
async fn report_share(State(mock): State<MockState>, Json(body): Json<Value>) -> Response {
    let endpoint = body["endpointId"].as_str().unwrap_or_default();
    let timestamp = body["timestamp"].as_u64().unwrap_or_default();
    let message = format!("fluxdown-share-report/v1\n{endpoint}\n{timestamp}");
    let verified = (|| {
        use base64::Engine;
        let key = EndpointId::from_z32(endpoint).ok()?;
        let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(body["signature"].as_str()?)
            .ok()?;
        let signature = fluxdown_share::Signature::from_bytes(&raw.try_into().ok()?);
        key.verify(message.as_bytes(), &signature).ok()
    })();
    if verified.is_none() {
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    mock.reports.lock().unwrap().push(body);
    StatusCode::NO_CONTENT.into_response()
}

/// `POST /share/shares/{id}/appeal`：仅 takedown 可申诉；已申诉 409；成功 201 并写入分享。
async fn appeal_share(
    State(mock): State<MockState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    let caller = bearer(&headers).unwrap_or_default();
    let mut shares = mock.shares.lock().unwrap();
    let Some(share) = shares
        .iter_mut()
        .find(|s| s.id == id && same_owner(&s.owner, &caller))
    else {
        return error(StatusCode::NOT_FOUND, "not_found");
    };
    if share.status != "takedown" {
        return error(StatusCode::BAD_REQUEST, "validation_error");
    }
    if !share.create_body["appeal"].is_null() {
        return error(StatusCode::CONFLICT, "share_appeal_exists");
    }
    let appeal = json!({
        "status": "pending", "createdAt": "2026-10-29T00:00:00Z",
        "handledAt": null, "adminNote": null, "statement": body["statement"]
    });
    share.create_body["appeal"] = appeal.clone();
    (StatusCode::CREATED, Json(appeal)).into_response()
}

async fn usage(State(mock): State<MockState>) -> Response {
    match mock.usage.lock().unwrap().clone() {
        Some(body) => Json(body).into_response(),
        None => error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
    }
}

async fn config(State(mock): State<MockState>) -> Response {
    let guest_share = mock.guest_share.load(std::sync::atomic::Ordering::SeqCst);
    let republish = mock
        .republish_secs
        .load(std::sync::atomic::Ordering::SeqCst);
    let body = json!({
        "enabled": true, "guestReceive": true, "guestBridge": false, "guestShare": guest_share,
        "pickupCodeLength": 6, "chunkBytes": 4_194_304,
        "liveTiers": [{ "id": "t1", "hours": 24, "enabled": true, "default": true }],
        "network": {
            "relays": [], "dnsOrigin": "", "pkarrUrl": "", "nameservers": [], "dohUrl": null,
            "pkarrTtlSecs": 30, "republishSecs": republish, "publishDirectAddrs": false,
            "portmapper": false, "holePunchHintSecs": 8, "reportIntervalSecs": 1
        },
        "revision": 1
    });
    // ETag 随游客开关 / 网络策略变化，保证切换后客户端拿到新配置。
    let etag = format!("\"v1-{guest_share}-{republish}\"");
    ([(header::ETAG, etag)], Json(body)).into_response()
}

/// 校验 endpoint 私钥签名（与 FluxCloud `share::guest` 同一消息格式），签发 `guest-<endpoint>-<n>`。
async fn guest_session(State(mock): State<MockState>, Json(body): Json<Value>) -> Response {
    if !mock.guest_share.load(std::sync::atomic::Ordering::SeqCst) {
        return error(StatusCode::FORBIDDEN, "share_guest_disabled");
    }
    let endpoint = body["endpointId"].as_str().unwrap_or_default();
    let timestamp = body["timestamp"].as_u64().unwrap_or_default();
    let message = format!("fluxdown-share-guest-session/v1\n{endpoint}\n{timestamp}");
    let verified = (|| {
        use base64::Engine;
        let key = EndpointId::from_z32(endpoint).ok()?;
        let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(body["signature"].as_str()?)
            .ok()?;
        let signature = fluxdown_share::Signature::from_bytes(&raw.try_into().ok()?);
        key.verify(message.as_bytes(), &signature).ok()
    })();
    if verified.is_none() {
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    let mut sessions = mock.guest_sessions.lock().unwrap();
    let token = format!("guest-{endpoint}-{}", sessions.len());
    sessions.push(token.clone());
    Json(json!({
        "guestToken": token,
        "expiresAt": rfc3339_ms((now_unix_secs() + 86_400) * 1000),
        "endpointId": endpoint,
    }))
    .into_response()
}

async fn keys(State(mock): State<MockState>) -> Json<Value> {
    Json(json!({ "keys": [{
        "kid": "k1", "publicKey": encode_public_key(&mock.signer.public())
    }]}))
}

async fn register_endpoint(State(mock): State<MockState>, Json(body): Json<Value>) -> Json<Value> {
    if let Some(id) = body["endpointId"].as_str() {
        mock.endpoints.lock().unwrap().push(id.to_owned());
    }
    Json(json!({ "endpointId": body["endpointId"], "dnsOrigin": "" }))
}

fn share_json(share: &MockShare) -> Value {
    let size: u64 = share.create_body["files"]
        .as_array()
        .map(|files| files.iter().filter_map(|f| f["size"].as_u64()).sum())
        .unwrap_or(0);
    let active = share.status == "active";
    json!({
        "id": share.id, "slug": share.slug,
        "pickupCode": if active { json!(share.code) } else { Value::Null },
        "url": format!("https://fluxdown.test/s/{}", share.slug),
        "title": share.create_body["title"], "mode": "live", "tierId": "t1",
        "expiresAt": rfc3339_ms((now_unix_secs() + 3600) * 1000),
        "maxClaims": null, "claimCount": share.claims, "notifyOnClaim": false,
        "status": share.status, "totalBytes": size,
        "swarm": share.create_body["swarm"].as_bool().unwrap_or(false),
        "swarmLocked": share.create_body["swarmLocked"].as_bool().unwrap_or(false),
        "takedownReason": share.create_body["takedownReason"],
        "appeal": share.create_body["appeal"]
    })
}

async fn create_share(
    State(mock): State<MockState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let Some(owner) = bearer(&headers) else {
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    };
    let mut shares = mock.shares.lock().unwrap();
    let n = shares.len() + 1;
    let share = MockShare {
        id: format!("s{n}"),
        slug: format!("slug{n}"),
        code: format!("12345{n}"),
        create_body: body,
        status: "active".to_owned(),
        owner,
        claims: 0,
        resumes: 0,
    };
    let out = share_json(&share);
    shares.push(share);
    Json(out).into_response()
}

/// 游客令牌只认同一 endpoint 签发的任一令牌（令牌会续期）；账号令牌只认账号。
fn same_owner(owner: &str, caller: &str) -> bool {
    match (owner.strip_prefix("guest-"), caller.strip_prefix("guest-")) {
        (Some(a), Some(b)) => a.rsplit_once('-').map(|x| x.0) == b.rsplit_once('-').map(|x| x.0),
        (None, None) => true,
        _ => false,
    }
}

async fn list_shares(State(mock): State<MockState>, headers: HeaderMap) -> Response {
    let Some(caller) = bearer(&headers) else {
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    };
    let mine: Vec<Value> = mock
        .shares
        .lock()
        .unwrap()
        .iter()
        .filter(|s| same_owner(&s.owner, &caller))
        .map(share_json)
        .collect();
    let delay = mock.list_delay_ms.load(std::sync::atomic::Ordering::SeqCst);
    if delay > 0 {
        tokio::time::sleep(Duration::from_millis(delay)).await;
    }
    Json(json!({ "shares": mine })).into_response()
}

async fn stop_share(
    State(mock): State<MockState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let caller = bearer(&headers).unwrap_or_default();
    let mut shares = mock.shares.lock().unwrap();
    match shares
        .iter_mut()
        .find(|s| s.id == id && same_owner(&s.owner, &caller))
    {
        Some(share) => {
            share.status = "stopped".to_owned();
            StatusCode::NO_CONTENT.into_response()
        }
        None => error(StatusCode::NOT_FOUND, "not_found"),
    }
}

/// `POST /share/shares/{id}/resume`：stopped / expired → active，同 id 同 slug，重新分配取件码。
async fn resume_share(
    State(mock): State<MockState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let caller = bearer(&headers).unwrap_or_default();
    let mut shares = mock.shares.lock().unwrap();
    match shares
        .iter_mut()
        .find(|s| s.id == id && same_owner(&s.owner, &caller))
    {
        Some(share) => {
            if share.status == "active" {
                return error(StatusCode::CONFLICT, "share_ended");
            }
            share.resumes += 1;
            share.status = "active".to_owned();
            share.code = format!("98765{}", share.resumes);
            Json(share_json(share)).into_response()
        }
        None => error(StatusCode::NOT_FOUND, "not_found"),
    }
}

/// `DELETE /share/shares/{id}/record`：记录整条删除（分享中的先按停止处理），204。
async fn delete_share_record(
    State(mock): State<MockState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let caller = bearer(&headers).unwrap_or_default();
    let mut shares = mock.shares.lock().unwrap();
    let before = shares.len();
    shares.retain(|s| !(s.id == id && same_owner(&s.owner, &caller)));
    if shares.len() == before {
        error(StatusCode::NOT_FOUND, "not_found")
    } else {
        StatusCode::NO_CONTENT.into_response()
    }
}

/// `PATCH /share/shares/{id}`：把请求体并进创建体并回传。
async fn patch_share(
    State(mock): State<MockState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    let caller = bearer(&headers).unwrap_or_default();
    let mut shares = mock.shares.lock().unwrap();
    match shares
        .iter_mut()
        .find(|s| s.id == id && same_owner(&s.owner, &caller))
    {
        Some(share) => {
            // 管理员锁定共享后，owner 不能再把它打开（关闭仍允许）。
            if share.create_body["swarmLocked"].as_bool() == Some(true)
                && body["swarm"].as_bool() == Some(true)
            {
                return error(StatusCode::FORBIDDEN, "share_swarm_locked");
            }
            if let Some(fields) = body.as_object() {
                for (key, value) in fields {
                    share.create_body[key.as_str()] = value.clone();
                }
            }
            Json(share_json(share)).into_response()
        }
        None => error(StatusCode::NOT_FOUND, "not_found"),
    }
}

/// 能向 `rep` 提供 `root` 的设备（排除 `rep` 与被领取分享的分享方）：加入共享的有效分享的发送方（完整）
/// 加上登记行；同一 endpoint 去重，任一来源完整即完整。
fn swarm_holders(
    mock: &Mock,
    shares: &[MockShare],
    idx: usize,
    rep: &str,
    root: &str,
) -> Vec<(String, bool)> {
    let own_sender = shares[idx].create_body["endpointId"]
        .as_str()
        .unwrap_or_default();
    let rows = mock.swarm_rows.lock().unwrap();
    let mut out: Vec<(String, bool)> = Vec::new();
    let mut push = |endpoint: &str, complete: bool| {
        if endpoint == rep || endpoint == own_sender {
            return;
        }
        match out.iter_mut().find(|(known, _)| known == endpoint) {
            Some(entry) => entry.1 |= complete,
            None => out.push((endpoint.to_owned(), complete)),
        }
    };
    for share in shares
        .iter()
        .filter(|s| s.status == "active" && s.create_body["swarm"].as_bool() == Some(true))
    {
        let has_root = share.create_body["files"]
            .as_array()
            .is_some_and(|files| files.iter().any(|f| f["rootHash"].as_str() == Some(root)));
        if has_root && let Some(endpoint) = share.create_body["endpointId"].as_str() {
            push(endpoint, true);
        }
    }
    for (endpoint, row_root, share_id, complete) in rows.iter() {
        if row_root == root
            && shares
                .iter()
                .any(|s| &s.id == share_id && s.status == "active")
        {
            push(endpoint, *complete);
        }
    }
    out
}

/// `POST /share/swarm/peers`。
async fn swarm_peers(State(mock): State<MockState>, Json(body): Json<Value>) -> Response {
    if let Some((status, code)) = *mock.peers_error.lock().unwrap() {
        let details = if code == "swarm_quota_exhausted" {
            json!({ "resetAt": "2099-01-01T00:00:00Z", "retryAfterSecs": 600 })
        } else {
            Value::Null
        };
        let mut body = json!({ "code": code, "message": code });
        if !details.is_null() {
            body["details"] = details;
        }
        return (status, Json(body)).into_response();
    }
    let Ok(ticket) = fluxdown_share::parse_ticket(body["ticket"].as_str().unwrap_or_default())
    else {
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    };
    let payload = ticket.payload();
    if body["shareId"].as_str() != Some(payload.sid.as_str()) {
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    let shares = mock.shares.lock().unwrap();
    let Some(idx) = shares.iter().position(|s| s.id == payload.sid) else {
        return error(StatusCode::GONE, "share_ended");
    };
    if shares[idx].status != "active" {
        return error(StatusCode::GONE, "share_ended");
    }
    let files = shares[idx].create_body["files"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let wanted: Vec<usize> = match body["files"].as_array().filter(|a| !a.is_empty()) {
        Some(list) => list
            .iter()
            .filter_map(|v| usize::try_from(v.as_u64()?).ok())
            .collect(),
        None => (0..files.len()).collect(),
    };
    let mut out = Vec::new();
    for index in wanted {
        let Some(root) = files.get(index).and_then(|f| f["rootHash"].as_str()) else {
            return error(StatusCode::BAD_REQUEST, "validation_error");
        };
        let peers = swarm_holders(&mock, &shares, idx, &payload.rep, root);
        let signed = sign_ticket(
            &mock.signer,
            &TicketPayload {
                v: 1,
                kid: "k1".into(),
                sid: payload.sid.clone(),
                rep: payload.rep.clone(),
                exp: payload.exp,
                scope: "swarm".into(),
                root: Some(root.to_owned()),
                pri: 0,
            },
        )
        .unwrap();
        out.push(json!({
            "fileIndex": index, "rootHash": root, "ticket": signed,
            "peers": peers.iter()
                .map(|(endpoint, complete)| json!({ "endpointId": endpoint, "complete": complete }))
                .collect::<Vec<_>>(),
        }));
    }
    Json(json!({ "files": out, "refreshSecs": 30 })).into_response()
}

/// `POST /share/swarm/announce`：校验 endpoint 签名；分享已停止 → `ended`，否则 upsert；按 `withdraw` 删除。
async fn swarm_announce(State(mock): State<MockState>, Json(body): Json<Value>) -> Response {
    mock.announces.lock().unwrap().push(body.clone());
    let endpoint = body["endpointId"].as_str().unwrap_or_default().to_owned();
    let timestamp = body["timestamp"].as_u64().unwrap_or_default();
    let message = format!("fluxdown-share-swarm-announce/v1\n{endpoint}\n{timestamp}");
    let verified = (|| {
        use base64::Engine;
        let key = EndpointId::from_z32(&endpoint).ok()?;
        let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(body["signature"].as_str()?)
            .ok()?;
        let signature = fluxdown_share::Signature::from_bytes(&raw.try_into().ok()?);
        key.verify(message.as_bytes(), &signature).ok()
    })();
    if verified.is_none() {
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    let shares = mock.shares.lock().unwrap();
    let mut rows = mock.swarm_rows.lock().unwrap();
    for root in body["withdraw"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        rows.retain(|r| !(r.0 == endpoint && r.1 == root));
    }
    let mut results = Vec::new();
    for entry in body["entries"].as_array().into_iter().flatten() {
        let root = entry["rootHash"].as_str().unwrap_or_default();
        let share_id = entry["shareId"].as_str().unwrap_or_default();
        let active = shares
            .iter()
            .any(|s| s.id == share_id && s.status == "active");
        let state = if active {
            let complete = entry["complete"].as_bool().unwrap_or(false);
            match rows.iter_mut().find(|r| r.0 == endpoint && r.1 == root) {
                Some(row) => {
                    row.2 = share_id.to_owned();
                    row.3 = complete;
                }
                None => rows.push((
                    endpoint.clone(),
                    root.to_owned(),
                    share_id.to_owned(),
                    complete,
                )),
            }
            "ok"
        } else {
            rows.retain(|r| !(r.0 == endpoint && r.1 == root && r.2 == share_id));
            "ended"
        };
        results.push(json!({ "rootHash": root, "state": state }));
    }
    Json(json!({ "entries": results })).into_response()
}

async fn resolve(
    State(mock): State<MockState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let mut shares = mock.shares.lock().unwrap();
    let found = shares.iter().position(|s| {
        body["code"].as_str() == Some(s.code.as_str())
            || body["slug"].as_str() == Some(s.slug.as_str())
    });
    let Some(idx) = found else {
        return error(StatusCode::NOT_FOUND, "not_found");
    };
    if shares[idx].status != "active" {
        return error(StatusCode::GONE, "share_ended");
    }
    let receiver = body["receiverEndpointId"].as_str().unwrap_or_default();
    shares[idx].claims += 1;
    // 与真实 FluxCloud 一致：登录设备 resolve 前，接收方 endpoint 必须已登记到本设备。
    if headers.contains_key(header::AUTHORIZATION)
        && !mock.endpoints.lock().unwrap().iter().any(|e| e == receiver)
    {
        return error(StatusCode::BAD_REQUEST, "validation");
    }
    let mut holders: Vec<String> = Vec::new();
    for file in shares[idx].create_body["files"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let root = file["rootHash"].as_str().unwrap_or_default();
        for (endpoint, _) in swarm_holders(&mock, &shares, idx, receiver, root) {
            if !holders.contains(&endpoint) {
                holders.push(endpoint);
            }
        }
    }
    let share = &shares[idx];
    let ticket = sign_ticket(
        &mock.signer,
        &TicketPayload {
            v: 1,
            kid: "k1".into(),
            sid: share.id.clone(),
            rep: receiver.to_owned(),
            exp: now_unix_secs() + 3600,
            scope: "recv".into(),
            root: None,
            pri: 0,
        },
    )
    .unwrap();
    let mut out = json!({
        "share": {
            "id": share.id, "title": share.create_body["title"],
            "files": share.create_body["files"],
            "senderDeviceName": "Sender PC",
            "senderEndpointId": share.create_body["endpointId"],
            "expiresAt": rfc3339_ms((now_unix_secs() + 3600) * 1000),
            "swarmPeers": holders.len()
        },
        "ticket": ticket, "relays": []
    });
    if !headers.contains_key(header::AUTHORIZATION) {
        out["guestToken"] = json!("guest-tok");
    }
    Json(out).into_response()
}

async fn telemetry_net(State(mock): State<MockState>, Json(body): Json<Value>) -> Json<Value> {
    mock.nets.lock().unwrap().push(body);
    Json(json!({ "globalV4Geo": { "province": "Zhejiang", "isp": "TestNet" } }))
}

async fn telemetry_sessions(State(mock): State<MockState>, Json(body): Json<Value>) -> Json<Value> {
    mock.sessions.lock().unwrap().push(body);
    Json(json!({ "killed": false }))
}

pub(super) async fn start_mock() -> (String, MockState) {
    let mock = Arc::new(Mock {
        signer: SecretKey::generate(),
        shares: Mutex::new(Vec::new()),
        nets: Mutex::new(Vec::new()),
        sessions: Mutex::new(Vec::new()),
        endpoints: Mutex::new(Vec::new()),
        guest_share: std::sync::atomic::AtomicBool::new(false),
        guest_sessions: Mutex::new(Vec::new()),
        republish_secs: std::sync::atomic::AtomicU64::new(300),
        list_delay_ms: std::sync::atomic::AtomicU64::new(0),
        swarm_rows: Mutex::new(Vec::new()),
        announces: Mutex::new(Vec::new()),
        usage: Mutex::new(None),
        peers_error: Mutex::new(None),
        reports: Mutex::new(Vec::new()),
        requests: std::sync::atomic::AtomicUsize::new(0),
    });
    let app = Router::new()
        .route("/api/v1/share/config", get(config))
        .route("/api/v1/share/keys", get(keys))
        .route("/api/v1/share/guest-session", post(guest_session))
        .route("/api/v1/share/endpoint", put(register_endpoint))
        .route("/api/v1/share/shares", post(create_share).get(list_shares))
        .route(
            "/api/v1/share/shares/{id}",
            delete(stop_share).patch(patch_share),
        )
        .route("/api/v1/share/shares/{id}/resume", post(resume_share))
        .route(
            "/api/v1/share/shares/{id}/record",
            delete(delete_share_record),
        )
        .route("/api/v1/share/resolve", post(resolve))
        .route("/api/v1/share/swarm/peers", post(swarm_peers))
        .route("/api/v1/share/usage", get(usage))
        .route("/api/v1/share/report", post(report_share))
        .route("/api/v1/share/shares/{id}/appeal", post(appeal_share))
        .route("/api/v1/share/swarm/announce", post(swarm_announce))
        .route("/api/v1/share/telemetry/net", post(telemetry_net))
        .route("/api/v1/share/telemetry/sessions", post(telemetry_sessions))
        .layer(axum::middleware::from_fn_with_state(
            mock.clone(),
            count_request,
        ))
        .with_state(mock.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (base, mock)
}

// ───────────── 测试宿主 ─────────────

/// 记录建任务请求的假 daemon。
#[derive(Default)]
pub(super) struct FakeTasks {
    pub(super) requests: Mutex<Vec<CreateTaskRequest>>,
}

#[async_trait]
impl LinkTaskCreator for FakeTasks {
    async fn create_task(&self, request: CreateTaskRequest) -> Result<String, ApiError> {
        let mut requests = self.requests.lock().unwrap();
        requests.push(request);
        Ok(format!("recv-task-{}", requests.len()))
    }
}

pub(super) fn task(id: &str, status: i32, dir: &std::path::Path, name: &str) -> TaskDto {
    serde_json::from_value(json!({
        "taskId": id, "url": "https://example.com/f", "fileName": name,
        "saveDir": dir.to_string_lossy(), "status": status, "downloadedBytes": 0,
        "totalBytes": 0, "errorMessage": "", "createdAt": "1", "proxyUrl": "",
        "queueId": "main", "checksum": ""
    }))
    .unwrap()
}

pub(super) struct Node {
    pub(super) service: Arc<ShareService>,
    pub(super) events: AgentEventHub,
    pub(super) tasks: Arc<FakeTasks>,
    pub(super) cancel: CancellationToken,
    _feed: mpsc::UnboundedSender<Value>,
}

pub(super) async fn node(base: &str, label: &str, logged_in: bool, tasks: Vec<TaskDto>) -> Node {
    let dir = std::env::temp_dir().join(format!(
        "fluxdown_share_{label}_{}_{}",
        std::process::id(),
        now_unix_secs()
    ));
    node_at(base, dir, label, logged_in, tasks).await
}

/// 指定数据目录的节点：同一目录再起一个节点即「重启」。
pub(super) async fn node_at(
    base: &str,
    dir: std::path::PathBuf,
    label: &str,
    logged_in: bool,
    tasks: Vec<TaskDto>,
) -> Node {
    node_with(base, dir, label, logged_in, tasks, true).await
}

/// `enabled`：本机FluxMesh开关（产品默认关；其余用例都在开启状态下测）。
pub(super) async fn node_with(
    base: &str,
    dir: std::path::PathBuf,
    label: &str,
    logged_in: bool,
    tasks: Vec<TaskDto>,
    enabled: bool,
) -> Node {
    // 旧服务的收尾任务可能还没放开状态目录锁。
    let store = {
        let mut opened = None;
        for _ in 0..100 {
            match StateStore::open(dir.clone()).await {
                Ok(store) => {
                    opened = Some(Arc::new(store));
                    break;
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }
        opened.expect("state dir stayed locked")
    };
    let mut state = AgentState {
        device_name: format!("{label}-device"),
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
    let events = AgentEventHub::new(AgentSnapshot {
        daemon: DaemonSnapshot {
            tasks,
            ..DaemonSnapshot::default()
        },
        ..AgentSnapshot::default()
    });
    let client = CloudClient::new(base.to_owned(), state.clone(), store.clone()).unwrap();
    let fake = Arc::new(FakeTasks::default());
    let (feed, cloud_events) = mpsc::unbounded_channel();
    let service = ShareService::new(ShareServiceParts {
        events: events.clone(),
        state,
        store,
        cloud: CloudApi::new(client),
        tasks: fake.clone(),
        cloud_events,
        egress: crate::egress::Egress::system_default(),
    })
    .await
    .unwrap();
    service.with(|i| i.local.enabled = enabled);
    let cancel = CancellationToken::new();
    tokio::spawn(service.clone().run(cancel.clone()));
    Node {
        service,
        events,
        tasks: fake,
        cancel,
        _feed: feed,
    }
}

pub(super) async fn call<T: serde::de::DeserializeOwned>(
    node: &Node,
    method: &str,
    params: Value,
) -> Result<T, fluxdown_protocol::RpcErrorData> {
    let value = node.service.rpc(method, Some(params)).await?;
    Ok(serde_json::from_value(value).unwrap())
}

pub(super) async fn wait_until(what: &str, mut probe: impl FnMut() -> bool) {
    for _ in 0..300 {
        if probe() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for {what}");
}

pub(super) fn pseudo_random(len: usize) -> Vec<u8> {
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn share_end_to_end_with_mock_cloud() {
    let (base, mock) = start_mock().await;

    // 分享方：本机有一个已完成任务，文件在磁盘上。
    let files_dir =
        std::env::temp_dir().join(format!("fluxdown_share_files_{}", std::process::id()));
    std::fs::create_dir_all(&files_dir).unwrap();
    let payload = pseudo_random(5 * 1024 * 1024);
    std::fs::write(files_dir.join("payload.bin"), &payload).unwrap();
    let sender = node(
        &base,
        "sender",
        true,
        vec![task("t1", 3, &files_dir, "payload.bin")],
    )
    .await;
    let receiver = node(&base, "receiver", false, Vec::new()).await;

    // 未完成 / 不存在的任务不可分享。
    let missing: Result<ShareDto, _> = call(
        &sender,
        method::AGENT_SHARE_CREATE,
        json!({ "taskIds": ["nope"] }),
    )
    .await;
    assert_eq!(
        missing.unwrap_err().reason,
        Some(fluxdown_protocol::ErrorReason::ShareTaskNotShareable)
    );

    // 创建分享：哈希 → 云端登记 → 发送方登记表。
    let share: ShareDto = call(
        &sender,
        method::AGENT_SHARE_CREATE,
        json!({ "taskIds": ["t1"] }),
    )
    .await
    .unwrap();
    assert_eq!(share.pickup_code, "123451");
    assert_eq!(share.total_bytes, payload.len() as u64);
    assert!(sender.service.registry.contains(&share.id));
    assert!(
        sender
            .service
            .with(|i| i.endpoint.as_ref().is_some_and(|e| e.is_bound()))
    );

    // 接收方：测试钩子给出分享方的回环地址（云端契约没有地址提示）。
    let sender_service = sender.service.clone();
    *receiver.service.addr_hint.lock().unwrap() = Some(Arc::new(move |z32: &str| {
        let endpoint = sender_service.with(|i| i.endpoint.clone())?;
        let port = endpoint
            .bound_endpoint()?
            .bound_sockets()
            .iter()
            .find(|a| a.is_ipv4())?
            .port();
        let id = EndpointId::from_z32(z32).ok()?;
        Some(EndpointAddr::new(id).with_ip_addr(([127, 0, 0, 1], port).into()))
    }));

    // 游客凭取件码 resolve。
    let preview: ShareResolveDto = call(
        &receiver,
        method::AGENT_SHARE_RESOLVE,
        json!({ "codeOrLink": "123451" }),
    )
    .await
    .unwrap();
    assert_eq!(preview.files.len(), 1);
    assert_eq!(preview.sender_device_name, "Sender PC");
    assert_eq!(preview.total_bytes, payload.len() as u64);

    // 接收：建出指向回环适配器、强制直连的普通下载任务。
    let save_dir = std::env::temp_dir().join("fluxdown_share_recv");
    let received: ShareReceiveResultDto = call(
        &receiver,
        method::AGENT_SHARE_RECEIVE,
        json!({ "resolveId": preview.resolve_id, "saveDir": save_dir.to_string_lossy() }),
    )
    .await
    .unwrap();
    assert_eq!(received.task_ids, ["recv-task-1"]);
    let request = receiver.tasks.requests.lock().unwrap()[0].clone();
    assert!(
        request.url.starts_with("http://127.0.0.1:"),
        "{}",
        request.url
    );
    assert_eq!(request.proxy_url, "direct://");
    assert_eq!(request.file_name, "payload.bin");

    // 适配器经 iroh 取到并校验过的字节。
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let response = http.get(&request.url).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.bytes().await.unwrap().as_ref(), payload.as_slice());

    // 有接收方连上来 → 分享方对账，领取次数随云端更新（游客分享收不到云端推送，只能靠这一步）。
    wait_until("sender claim count", || {
        sender.service.with(|i| {
            i.shares
                .iter()
                .any(|s| s.dto.id == share.id && s.dto.claim_count == 1)
        })
    })
    .await;

    // 接收方状态：任务事实 + 接收记录。
    let state = receiver.service.with(|i| i.state_dto(false));
    assert!(state.task_facts.contains_key("recv-task-1"));
    assert_eq!(state.receive_records.len(), 1);

    // 分享方停止 → 适配器对已建立的会话返回 410。
    let stopped: ShareDto = call(&sender, method::AGENT_SHARE_STOP, json!({ "id": share.id }))
        .await
        .unwrap();
    assert_eq!(stopped.status, ShareStatus::Stopped);
    let gone = http.get(&request.url).send().await.unwrap();
    assert_eq!(gone.status(), 410);

    // 接收任务完成 → 会话与适配器收尾，接收方端点关闭（空闲静默）。
    receiver
        .events
        .publish(AgentEvent::Daemon(DaemonEvent::TaskChanged(task(
            "recv-task-1",
            3,
            &save_dir,
            "payload.bin",
        ))));
    wait_until("receiver idle", || {
        receiver.service.with(|i| {
            i.receive_tasks.is_empty()
                && i.adapter.is_none()
                && i.endpoint.as_ref().is_none_or(|e| !e.is_bound())
        })
    })
    .await;

    // 上报：双方的会话报告与网络快照都送达云端。
    wait_until("telemetry", || {
        let sessions = mock.sessions.lock().unwrap();
        let has = |role: &str| {
            sessions
                .iter()
                .any(|s| s["role"] == role && s["shareId"] == share.id)
        };
        has("sender") && has("receiver") && !mock.nets.lock().unwrap().is_empty()
    })
    .await;
    let receiver_state = receiver.service.with(|i| i.state_dto(false));
    assert!(receiver_state.task_facts.is_empty());

    // 云端回传的归属折进分享方网络状态。
    wait_until("geo", || {
        sender
            .service
            .with(|i| i.net.isp.as_deref() == Some("TestNet"))
    })
    .await;

    // 诊断：会话详情来自上报簿；日志过滤的非法正则被拒绝。
    let diag: fluxdown_protocol::ShareDiagnosticsDto =
        call(&receiver, method::AGENT_SHARE_DIAGNOSTICS, json!({}))
            .await
            .unwrap();
    assert!(!diag.sessions.is_empty());
    let bad_regex: Result<fluxdown_protocol::ShareLogsDto, _> =
        call(&receiver, method::AGENT_SHARE_LOGS, json!({ "regex": "(" })).await;
    assert_eq!(bad_regex.unwrap_err().field.as_deref(), Some("regex"));

    // 阶段 E 之前切换桥接恒不可用。
    let bridge: Result<Value, _> = call(
        &receiver,
        method::AGENT_SHARE_SWITCH_TO_BRIDGE,
        json!({ "taskId": "recv-task-1" }),
    )
    .await;
    assert_eq!(
        bridge.unwrap_err().reason,
        Some(fluxdown_protocol::ErrorReason::ShareBridgeUnavailable)
    );

    sender.cancel.cancel();
    receiver.cancel.cancel();
}

/// 回归（真实栈 smoke 发现）：登录设备的 `resolve` 要求接收方 endpoint 已登记到本设备（否则服务端 400，
/// 被映射成 `shareCodeInvalid`）。此前只有 `create` / `receive` 会登记，冷启动后直接 resolve 必败。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn logged_in_receiver_registers_its_endpoint_before_resolve() {
    let (base, _mock) = start_mock().await;
    let files_dir =
        std::env::temp_dir().join(format!("fluxdown_share_files_login_{}", std::process::id()));
    std::fs::create_dir_all(&files_dir).unwrap();
    std::fs::write(files_dir.join("payload.bin"), pseudo_random(1024 * 1024)).unwrap();
    let sender = node(
        &base,
        "sender_login",
        true,
        vec![task("t1", 3, &files_dir, "payload.bin")],
    )
    .await;
    let share: ShareDto = call(
        &sender,
        method::AGENT_SHARE_CREATE,
        json!({ "taskIds": ["t1"] }),
    )
    .await
    .unwrap();

    let receiver = node(&base, "receiver_login", true, Vec::new()).await;
    let preview: ShareResolveDto = call(
        &receiver,
        method::AGENT_SHARE_RESOLVE,
        json!({ "codeOrLink": share.pickup_code }),
    )
    .await
    .unwrap();
    assert_eq!(preview.share_id, share.id);

    sender.cancel.cancel();
    receiver.cancel.cancel();
}

/// 回归：本机输入自己分享的取件码。iroh 端点不能连接自身，此前会建出指向回环适配器的任务，
/// 首次探测就得到 503「分享方离线」。现在 resolve 就以 `shareOwnShare` 拒绝，也不留待接收项。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resolving_own_share_is_rejected() {
    let (base, _mock) = start_mock().await;
    let files_dir =
        std::env::temp_dir().join(format!("fluxdown_share_files_own_{}", std::process::id()));
    std::fs::create_dir_all(&files_dir).unwrap();
    std::fs::write(files_dir.join("payload.bin"), pseudo_random(64 * 1024)).unwrap();
    let node = node(
        &base,
        "own_share",
        true,
        vec![task("t1", 3, &files_dir, "payload.bin")],
    )
    .await;
    let share: ShareDto = call(
        &node,
        method::AGENT_SHARE_CREATE,
        json!({ "taskIds": ["t1"] }),
    )
    .await
    .unwrap();

    let resolved: Result<ShareResolveDto, _> = call(
        &node,
        method::AGENT_SHARE_RESOLVE,
        json!({ "codeOrLink": share.pickup_code }),
    )
    .await;
    assert_eq!(
        resolved.unwrap_err().reason,
        Some(fluxdown_protocol::ErrorReason::ShareOwnShare)
    );
    assert!(node.service.with(|i| i.resolves.is_empty()));

    node.cancel.cancel();
}

/// 游客发送方：未登录时凭 endpoint 签名换游客令牌创建分享；令牌缓存复用；登录后游客分享仍用游客令牌
/// 停止，且账号对账不会把账号列表里没有的游客分享误判为已停止。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn guest_sender_shares_without_login_and_keeps_ownership_after_login() {
    let (base, mock) = start_mock().await;
    mock.guest_share
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let files_dir =
        std::env::temp_dir().join(format!("fluxdown_share_files_guest_{}", std::process::id()));
    std::fs::create_dir_all(&files_dir).unwrap();
    std::fs::write(files_dir.join("a.bin"), pseudo_random(256 * 1024)).unwrap();
    std::fs::write(files_dir.join("b.bin"), pseudo_random(128 * 1024)).unwrap();
    let sender = node(
        &base,
        "sender_guest",
        false,
        vec![
            task("t1", 3, &files_dir, "a.bin"),
            task("t2", 3, &files_dir, "b.bin"),
        ],
    )
    .await;

    let first: ShareDto = call(
        &sender,
        method::AGENT_SHARE_CREATE,
        json!({ "taskIds": ["t1"] }),
    )
    .await
    .unwrap();
    let second: ShareDto = call(
        &sender,
        method::AGENT_SHARE_CREATE,
        json!({ "taskIds": ["t2"] }),
    )
    .await
    .unwrap();
    assert!(!first.pickup_code.is_empty());
    assert_eq!(
        mock.guest_sessions.lock().unwrap().len(),
        1,
        "fresh guest token is reused"
    );
    assert!(
        mock.shares
            .lock()
            .unwrap()
            .iter()
            .all(|s| s.owner.starts_with("guest-"))
    );
    // 游客分享从未登记设备 endpoint（PUT /share/endpoint 只给登录设备）。
    assert!(mock.endpoints.lock().unwrap().is_empty());

    // 登录后：账号对账看不到游客分享，但它们仍有效。
    sender.service.state.lock().await.credentials = Some(CloudCredentials {
        access_token: "access".into(),
        refresh_token: "refresh".into(),
        expires_at_unix: i64::MAX / 2,
        session: None,
    });
    sender.service.sync_shares().await;
    let status_of = |id: &str| {
        sender.service.with(|i| {
            i.shares
                .iter()
                .find(|s| s.dto.id == id)
                .map(|s| s.dto.status)
        })
    };
    assert_eq!(status_of(&first.id), Some(ShareStatus::Active));
    assert_eq!(status_of(&second.id), Some(ShareStatus::Active));

    // 停止游客分享走游客令牌（账号令牌会被 mock 当作他人分享返回 404）。
    let stopped: ShareDto = call(&sender, method::AGENT_SHARE_STOP, json!({ "id": first.id }))
        .await
        .unwrap();
    assert_eq!(stopped.status, ShareStatus::Stopped);
    let cloud_status = mock
        .shares
        .lock()
        .unwrap()
        .iter()
        .find(|s| s.id == first.id)
        .map(|s| s.status.clone());
    assert_eq!(cloud_status.as_deref(), Some("stopped"));

    sender.cancel.cancel();
}

/// 回归（本机联调发现）：分享期间后台改了网络策略（pkarr / DNS / 中继），已绑定的端点仍按旧策略
/// 发布地址，对端永远查不到。再次分享时必须取最新策略并按新策略重建端点。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn new_share_rebuilds_endpoint_after_network_policy_change() {
    let (base, mock) = start_mock().await;
    let files_dir = std::env::temp_dir().join(format!(
        "fluxdown_share_files_policy_{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&files_dir).unwrap();
    std::fs::write(files_dir.join("a.bin"), pseudo_random(64 * 1024)).unwrap();
    std::fs::write(files_dir.join("b.bin"), pseudo_random(64 * 1024)).unwrap();
    let sender = node(
        &base,
        "sender_policy",
        true,
        vec![
            task("t1", 3, &files_dir, "a.bin"),
            task("t2", 3, &files_dir, "b.bin"),
        ],
    )
    .await;
    let republish = |node: &Node| {
        node.service
            .with(|i| i.endpoint.as_ref().map(|e| e.policy().republish_secs))
    };
    let _: ShareDto = call(
        &sender,
        method::AGENT_SHARE_CREATE,
        json!({ "taskIds": ["t1"] }),
    )
    .await
    .unwrap();
    assert_eq!(republish(&sender), Some(300));

    mock.republish_secs
        .store(120, std::sync::atomic::Ordering::SeqCst);
    let _: ShareDto = call(
        &sender,
        method::AGENT_SHARE_CREATE,
        json!({ "taskIds": ["t2"] }),
    )
    .await
    .unwrap();
    assert_eq!(republish(&sender), Some(120));
    assert!(
        sender
            .service
            .with(|i| i.endpoint.as_ref().is_some_and(|e| e.is_bound()))
    );

    sender.cancel.cancel();
}

/// 回归（`upload_resumes_after_restart…` 偶发失败的根因）：多个任务同时持久化分享表时共用同一个
/// 临时文件，交错写会撕裂 `shares.json`（重启后整张表被当作损坏丢弃），或把旧快照写在新快照之后。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_share_persists_never_tear_or_reorder() {
    let (base, _mock) = start_mock().await;
    let node = node(&base, "persist", false, Vec::new()).await;
    let mut writers = Vec::new();
    for n in 0..64_usize {
        let service = node.service.clone();
        writers.push(tokio::spawn(async move {
            let id = format!("share-{}", "x".repeat(n));
            service.with(|i| {
                i.shares.push(super::store::StoredShare {
                    dto: ShareDto {
                        id: id.clone(),
                        status: ShareStatus::Active,
                        ..ShareDto::default()
                    },
                    manifest: fluxdown_share::ShareManifest {
                        share_id: id,
                        files: Vec::new(),
                    },
                    paths: Vec::new(),
                    guest_owned: false,
                });
            });
            service.persist_shares().await;
        }));
    }
    for writer in writers {
        writer.await.unwrap();
    }
    let on_disk = node.service.store.load_shares().await;
    assert_eq!(
        on_disk.len(),
        64,
        "last write must hold the full, untorn table"
    );
    node.cancel.cancel();
}

/// 回归：对账请求在途时本机停止了分享，旧列表（仍是 active）不得把它复活；同理新建的分享不得被
/// 旧列表判为已停止。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stale_share_list_never_overrides_local_changes() {
    let (base, mock) = start_mock().await;
    let files_dir =
        std::env::temp_dir().join(format!("fluxdown_share_files_stale_{}", std::process::id()));
    std::fs::create_dir_all(&files_dir).unwrap();
    std::fs::write(files_dir.join("a.bin"), pseudo_random(64 * 1024)).unwrap();
    std::fs::write(files_dir.join("b.bin"), pseudo_random(64 * 1024)).unwrap();
    let sender = node(
        &base,
        "sender_stale",
        true,
        vec![
            task("t1", 3, &files_dir, "a.bin"),
            task("t2", 3, &files_dir, "b.bin"),
        ],
    )
    .await;
    let first: ShareDto = call(
        &sender,
        method::AGENT_SHARE_CREATE,
        json!({ "taskIds": ["t1"] }),
    )
    .await
    .unwrap();

    mock.list_delay_ms
        .store(300, std::sync::atomic::Ordering::SeqCst);
    let syncing = {
        let service = sender.service.clone();
        tokio::spawn(async move { service.sync_shares().await })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    let _: ShareDto = call(&sender, method::AGENT_SHARE_STOP, json!({ "id": first.id }))
        .await
        .unwrap();
    let second: ShareDto = call(
        &sender,
        method::AGENT_SHARE_CREATE,
        json!({ "taskIds": ["t2"] }),
    )
    .await
    .unwrap();
    syncing.await.unwrap();

    let status_of = |id: &str| {
        sender.service.with(|i| {
            i.shares
                .iter()
                .find(|s| s.dto.id == id)
                .map(|s| s.dto.status)
        })
    };
    assert_eq!(status_of(&first.id), Some(ShareStatus::Stopped));
    assert_eq!(status_of(&second.id), Some(ShareStatus::Active));
    assert!(sender.service.registry.contains(&second.id));

    sender.cancel.cancel();
}

// ───────────── 我的分享：停止 / 开始 / 删除 ─────────────

/// 登录的分享方，磁盘上有一个已完成任务 `t1`（`payload.bin`）；返回节点、文件目录与内容。
pub(super) async fn sender_with_file(
    base: &str,
    label: &str,
    len: usize,
) -> (Node, std::path::PathBuf, Vec<u8>) {
    let files_dir = std::env::temp_dir().join(format!(
        "fluxdown_share_files_{label}_{}_{}",
        std::process::id(),
        now_unix_secs()
    ));
    std::fs::create_dir_all(&files_dir).unwrap();
    let payload = pseudo_random(len);
    std::fs::write(files_dir.join("payload.bin"), &payload).unwrap();
    let sender = node(
        base,
        label,
        true,
        vec![task("t1", 3, &files_dir, "payload.bin")],
    )
    .await;
    (sender, files_dir, payload)
}

/// 测试钩子：接收方通过分享方当前绑定的回环端口连接它（云端契约没有地址提示）。
pub(super) fn point_receiver_at(receiver: &Node, sender: &Node) {
    let sender_service = sender.service.clone();
    *receiver.service.addr_hint.lock().unwrap() = Some(Arc::new(move |z32: &str| {
        let endpoint = sender_service.with(|i| i.endpoint.clone())?;
        let port = endpoint
            .bound_endpoint()?
            .bound_sockets()
            .iter()
            .find(|a| a.is_ipv4())?
            .port();
        let id = EndpointId::from_z32(z32).ok()?;
        Some(EndpointAddr::new(id).with_ip_addr(([127, 0, 0, 1], port).into()))
    }));
}

/// resolve → receive，返回接收方建出的回环下载地址。
pub(super) async fn receive_url(receiver: &Node, code: &str, save_dir: &std::path::Path) -> String {
    let preview: ShareResolveDto = call(
        receiver,
        method::AGENT_SHARE_RESOLVE,
        json!({ "codeOrLink": code }),
    )
    .await
    .unwrap();
    let _: ShareReceiveResultDto = call(
        receiver,
        method::AGENT_SHARE_RECEIVE,
        json!({ "resolveId": preview.resolve_id, "saveDir": save_dir.to_string_lossy() }),
    )
    .await
    .unwrap();
    let requests = receiver.tasks.requests.lock().unwrap();
    requests.last().unwrap().url.clone()
}

fn local_share(node: &Node, id: &str) -> Option<ShareDto> {
    node.service.with(|i| {
        i.shares
            .iter()
            .find(|s| s.dto.id == id)
            .map(|s| s.dto.clone())
    })
}

/// 停止后再开始是同一个分享（同 id、同链接），只换取件码；新取件码接收方能成功下载，旧码失效。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stopped_share_resumes_with_same_link_and_a_new_pickup_code() {
    let (base, _mock) = start_mock().await;
    let (sender, _dir, payload) = sender_with_file(&base, "resume_sender", 512 * 1024).await;
    let receiver = node(&base, "resume_receiver", false, Vec::new()).await;
    point_receiver_at(&receiver, &sender);

    let share: ShareDto = call(
        &sender,
        method::AGENT_SHARE_CREATE,
        json!({ "taskIds": ["t1"] }),
    )
    .await
    .unwrap();
    let old_code = share.pickup_code.clone();
    assert_eq!(old_code, "123451");

    let stopped: ShareDto = call(&sender, method::AGENT_SHARE_STOP, json!({ "id": share.id }))
        .await
        .unwrap();
    assert_eq!(stopped.status, ShareStatus::Stopped);
    assert!(stopped.pickup_code.is_empty());
    assert_eq!(sender.service.registry.active_count(), 0);

    let resumed: ShareDto = call(
        &sender,
        method::AGENT_SHARE_RESUME,
        json!({ "id": share.id }),
    )
    .await
    .unwrap();
    assert_eq!(resumed.id, share.id);
    assert_eq!(resumed.slug, share.slug);
    assert_eq!(resumed.url, share.url);
    assert_eq!(resumed.status, ShareStatus::Active);
    assert_eq!(resumed.pickup_code, "987651");
    assert!(!resumed.source_missing);
    assert_eq!(sender.service.registry.active_count(), 1);
    assert_eq!(local_share(&sender, &share.id), Some(resumed.clone()));

    // 已在分享中再开始：幂等，原样返回。
    let again: ShareDto = call(
        &sender,
        method::AGENT_SHARE_RESUME,
        json!({ "id": share.id }),
    )
    .await
    .unwrap();
    assert_eq!(again, resumed);

    // 旧取件码已释放；新取件码可以接收并取到完整内容。
    let stale: Result<ShareResolveDto, _> = call(
        &receiver,
        method::AGENT_SHARE_RESOLVE,
        json!({ "codeOrLink": old_code }),
    )
    .await;
    assert!(stale.is_err());
    let save_dir = std::env::temp_dir().join("fluxdown_share_resume_recv");
    let url = receive_url(&receiver, &resumed.pickup_code, &save_dir).await;
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let response = http.get(&url).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.bytes().await.unwrap().as_ref(), payload.as_slice());

    // 不存在的分享：ShareNotFound。
    let missing: Result<ShareDto, _> =
        call(&sender, method::AGENT_SHARE_RESUME, json!({ "id": "nope" })).await;
    assert_eq!(
        missing.unwrap_err().reason,
        Some(fluxdown_protocol::ErrorReason::ShareNotFound)
    );

    sender.cancel.cancel();
    receiver.cancel.cancel();
}

/// 源文件被删除 / 被改后不能再开始：返回 `shareSourceMissing`，DTO 带 `sourceMissing`，云端分享仍是停止状态。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_is_refused_when_the_source_file_is_gone_or_modified() {
    let (base, mock) = start_mock().await;
    let (sender, dir, _payload) = sender_with_file(&base, "resume_missing", 64 * 1024).await;
    let share: ShareDto = call(
        &sender,
        method::AGENT_SHARE_CREATE,
        json!({ "taskIds": ["t1"] }),
    )
    .await
    .unwrap();
    let _: ShareDto = call(&sender, method::AGENT_SHARE_STOP, json!({ "id": share.id }))
        .await
        .unwrap();

    // 被改：大小与修改时间都变了。
    let path = dir.join("payload.bin");
    std::fs::write(&path, pseudo_random(64 * 1024 + 1)).unwrap();
    let modified: Result<ShareDto, _> = call(
        &sender,
        method::AGENT_SHARE_RESUME,
        json!({ "id": share.id }),
    )
    .await;
    assert_eq!(
        modified.unwrap_err().reason,
        Some(fluxdown_protocol::ErrorReason::ShareSourceMissing)
    );
    assert!(local_share(&sender, &share.id).unwrap().source_missing);

    // 被删除。
    std::fs::remove_file(&path).unwrap();
    let deleted: Result<ShareDto, _> = call(
        &sender,
        method::AGENT_SHARE_RESUME,
        json!({ "id": share.id }),
    )
    .await;
    let err = deleted.unwrap_err();
    assert_eq!(
        err.reason,
        Some(fluxdown_protocol::ErrorReason::ShareSourceMissing)
    );
    assert!(!err.retryable);

    // 列表（含云端对账）里该分享仍是已停止 + 文件缺失，云端没有被恢复。
    let listed: Vec<ShareDto> = call(&sender, method::AGENT_SHARE_LIST, json!({}))
        .await
        .unwrap();
    let entry = listed.iter().find(|s| s.id == share.id).unwrap();
    assert_eq!(entry.status, ShareStatus::Stopped);
    assert!(entry.source_missing);
    assert_eq!(mock.shares.lock().unwrap()[0].status, "stopped");
    assert_eq!(sender.service.registry.active_count(), 0);

    sender.cancel.cancel();
}

/// 删除：云端记录与本机记录都没了，之后再对账也不会加回来；云端已不认识（404）视为成功。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deleted_share_is_gone_and_never_comes_back_on_sync() {
    let (base, mock) = start_mock().await;
    let (sender, _dir, _payload) = sender_with_file(&base, "delete_sender", 64 * 1024).await;
    let first: ShareDto = call(
        &sender,
        method::AGENT_SHARE_CREATE,
        json!({ "taskIds": ["t1"] }),
    )
    .await
    .unwrap();
    let second: ShareDto = call(
        &sender,
        method::AGENT_SHARE_CREATE,
        json!({ "taskIds": ["t1"] }),
    )
    .await
    .unwrap();

    // 分享中的直接删除（云端先按停止处理）。
    let result: Value = call(
        &sender,
        method::AGENT_SHARE_DELETE,
        json!({ "id": first.id }),
    )
    .await
    .unwrap();
    assert_eq!(result, json!({ "ok": true }));
    assert!(local_share(&sender, &first.id).is_none());
    assert!(!sender.service.registry.contains(&first.id));
    assert!(mock.shares.lock().unwrap().iter().all(|s| s.id != first.id));

    let listed: Vec<ShareDto> = call(&sender, method::AGENT_SHARE_LIST, json!({}))
        .await
        .unwrap();
    assert_eq!(
        listed.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
        [second.id.as_str()]
    );
    sender.service.sync_shares().await;
    assert!(local_share(&sender, &first.id).is_none());

    // 删除已停止的分享；云端先一步丢了记录（404）也算成功。
    let _: ShareDto = call(
        &sender,
        method::AGENT_SHARE_STOP,
        json!({ "id": second.id }),
    )
    .await
    .unwrap();
    mock.shares.lock().unwrap().clear();
    let _: Value = call(
        &sender,
        method::AGENT_SHARE_DELETE,
        json!({ "id": second.id }),
    )
    .await
    .unwrap();
    let listed: Vec<ShareDto> = call(&sender, method::AGENT_SHARE_LIST, json!({}))
        .await
        .unwrap();
    assert!(listed.is_empty());

    // 本机没有的分享：ShareNotFound。
    let unknown: Result<Value, _> = call(
        &sender,
        method::AGENT_SHARE_DELETE,
        json!({ "id": first.id }),
    )
    .await;
    assert_eq!(
        unknown.unwrap_err().reason,
        Some(fluxdown_protocol::ErrorReason::ShareNotFound)
    );

    sender.cancel.cancel();
}

/// 分享进行中源文件被删除：接收方取数得到 changed → 分享方把分享停掉并标记 `sourceMissing`。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn source_deleted_while_sharing_stops_the_share() {
    let (base, mock) = start_mock().await;
    let (sender, dir, _payload) = sender_with_file(&base, "gone_sender", 256 * 1024).await;
    let receiver = node(&base, "gone_receiver", false, Vec::new()).await;
    point_receiver_at(&receiver, &sender);
    let share: ShareDto = call(
        &sender,
        method::AGENT_SHARE_CREATE,
        json!({ "taskIds": ["t1"] }),
    )
    .await
    .unwrap();

    let save_dir = std::env::temp_dir().join("fluxdown_share_gone_recv");
    let url = receive_url(&receiver, &share.pickup_code, &save_dir).await;
    std::fs::remove_file(dir.join("payload.bin")).unwrap();
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let response = http.get(&url).send().await.unwrap();
    assert_eq!(response.status(), 409);

    wait_until("share stopped for missing source", || {
        local_share(&sender, &share.id)
            .is_some_and(|s| s.status == ShareStatus::Stopped && s.source_missing)
    })
    .await;
    let dto = local_share(&sender, &share.id).unwrap();
    assert!(dto.pickup_code.is_empty());
    assert_eq!(mock.shares.lock().unwrap()[0].status, "stopped");
    assert_eq!(sender.service.registry.active_count(), 0);

    // 已停止 + 文件缺失：不能再开始，只能删除。
    let resumed: Result<ShareDto, _> = call(
        &sender,
        method::AGENT_SHARE_RESUME,
        json!({ "id": share.id }),
    )
    .await;
    assert_eq!(
        resumed.unwrap_err().reason,
        Some(fluxdown_protocol::ErrorReason::ShareSourceMissing)
    );
    let _: Value = call(
        &sender,
        method::AGENT_SHARE_DELETE,
        json!({ "id": share.id }),
    )
    .await
    .unwrap();
    assert!(local_share(&sender, &share.id).is_none());

    sender.cancel.cancel();
    receiver.cancel.cancel();
}

/// 重启时对仍在分享中的分享做一次源文件检查：文件在停机期间被删，启动后即停止并标记 `sourceMissing`。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restart_stops_active_shares_whose_source_vanished() {
    let (base, mock) = start_mock().await;
    let files_dir = std::env::temp_dir().join(format!(
        "fluxdown_share_files_restart_{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&files_dir).unwrap();
    std::fs::write(files_dir.join("payload.bin"), pseudo_random(64 * 1024)).unwrap();
    let data_dir = std::env::temp_dir().join(format!(
        "fluxdown_share_restart_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let tasks = || vec![task("t1", 3, &files_dir, "payload.bin")];
    let first = node_at(&base, data_dir.clone(), "restart", true, tasks()).await;
    let share: ShareDto = call(
        &first,
        method::AGENT_SHARE_CREATE,
        json!({ "taskIds": ["t1"] }),
    )
    .await
    .unwrap();
    first.cancel.cancel();
    drop(first);

    std::fs::remove_file(files_dir.join("payload.bin")).unwrap();
    let second = node_at(&base, data_dir, "restart", true, tasks()).await;
    wait_until("restart check stops the share", || {
        local_share(&second, &share.id)
            .is_some_and(|s| s.status == ShareStatus::Stopped && s.source_missing)
    })
    .await;
    assert_eq!(mock.shares.lock().unwrap()[0].status, "stopped");
    assert_eq!(second.service.registry.active_count(), 0);

    second.cancel.cancel();
}

/// 下架：状态映射为 `Takedown` 并带出原因；resume 被拒；申诉成功写回、再次申诉映射 409。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn takedown_maps_reason_and_appeal_round_trips() {
    use fluxdown_protocol::{ErrorReason, ShareAppealStatus};
    let (base, mock) = start_mock().await;
    let (sender, _dir, _payload) = sender_with_file(&base, "takedown_sender", 64 * 1024).await;
    let share: ShareDto = call(
        &sender,
        method::AGENT_SHARE_CREATE,
        json!({ "taskIds": ["t1"] }),
    )
    .await
    .unwrap();

    // 未下架时申诉被本机拒绝。
    let early = call::<ShareDto>(
        &sender,
        method::AGENT_SHARE_APPEAL,
        json!({ "shareId": share.id, "statement": "x" }),
    )
    .await
    .unwrap_err();
    assert_eq!(early.reason, Some(ErrorReason::ShareStopped));

    {
        let mut shares = mock.shares.lock().unwrap();
        shares[0].status = "takedown".to_owned();
        shares[0].create_body["takedownReason"] = json!("侵权");
    }
    sender.service.sync_shares().await;
    let taken = local_share(&sender, &share.id).unwrap();
    assert_eq!(taken.status, ShareStatus::Takedown);
    assert_eq!(taken.takedown_reason.as_deref(), Some("侵权"));
    assert!(taken.appeal.is_none());
    assert_eq!(sender.service.registry.active_count(), 0);

    let resume = call::<ShareDto>(
        &sender,
        method::AGENT_SHARE_RESUME,
        json!({ "id": share.id }),
    )
    .await
    .unwrap_err();
    assert_eq!(resume.reason, Some(ErrorReason::ShareStopped));

    let appealed: ShareDto = call(
        &sender,
        method::AGENT_SHARE_APPEAL,
        json!({ "shareId": share.id, "statement": "这是我的原创内容" }),
    )
    .await
    .unwrap();
    assert_eq!(
        appealed.appeal.as_ref().map(|a| a.status),
        Some(ShareAppealStatus::Pending)
    );
    assert_eq!(
        local_share(&sender, &share.id).unwrap().appeal,
        appealed.appeal
    );

    let again = call::<ShareDto>(
        &sender,
        method::AGENT_SHARE_APPEAL,
        json!({ "shareId": share.id, "statement": "再来一次" }),
    )
    .await
    .unwrap_err();
    assert_eq!(again.reason, Some(ErrorReason::ShareAppealExists));
    sender.cancel.cancel();
}

/// 举报：签名串可被本机 endpoint 公钥验证，reason 为规范中文串，空白可选项不上送。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn report_is_signed_with_endpoint_key_and_uses_canonical_reason() {
    let (base, mock) = start_mock().await;
    let reporter = node(&base, "report_node", false, Vec::new()).await;
    let ok: Value = call(
        &reporter,
        method::AGENT_SHARE_REPORT,
        json!({ "shareId": "s9", "reason": "porn", "detail": "  ", "contact": "a@b.c" }),
    )
    .await
    .unwrap();
    assert_eq!(ok["ok"], json!(true));
    let sent = mock.reports.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["reason"], json!("色情低俗"));
    assert_eq!(sent[0]["shareId"], json!("s9"));
    assert_eq!(sent[0]["contact"], json!("a@b.c"));
    assert!(sent[0].get("detail").is_none());
    assert_eq!(
        sent[0]["endpointId"].as_str(),
        Some(reporter.service.secret.public().to_z32().as_str())
    );
    reporter.cancel.cancel();
}

/// `share_strike_limited` 的 `field` 携带解除时刻；缺失 details 时安全降级。
#[test]
fn strike_limited_carries_until_in_field() {
    use crate::cloud::CloudError;
    use fluxdown_protocol::ErrorReason;
    let mut err = CloudError {
        status: Some(403),
        code: Some("share_strike_limited".to_owned()),
        message: String::new(),
        retryable: false,
        unreachable: false,
        details: Some(Box::new(json!({ "until": "2027-01-01T00:00:00Z" }))),
    };
    let mapped = super::cloud::share_error(&err);
    assert_eq!(mapped.reason, Some(ErrorReason::ShareStrikeLimited));
    assert_eq!(mapped.field.as_deref(), Some("2027-01-01T00:00:00Z"));
    err.details = None;
    assert_eq!(super::cloud::share_error(&err).field, None);
}

// ───────────── 实验性开关 ─────────────

fn requests(mock: &Mock) -> usize {
    mock.requests.load(std::sync::atomic::Ordering::SeqCst)
}

fn endpoint_bound(node: &Node) -> bool {
    node.service
        .with(|i| i.endpoint.as_ref().is_some_and(|e| e.is_bound()))
}

/// 本机开关关闭（产品默认）：服务在跑但不访问云端、不绑定端点；`agent.share.*` 除 `config` /
/// `setLocalConfig` 外报不可用，分享链接被拒，快照只带本机设置。开启后才拉策略、变为可用。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disabled_share_stays_offline_until_enabled() {
    use crate::share_link::ShareLinkSink;
    use fluxdown_protocol::{ErrorReason, ShareConfigDto};
    let (base, mock) = start_mock().await;
    let dir = std::env::temp_dir().join(format!(
        "fluxdown_share_switch_off_{}_{}",
        std::process::id(),
        now_unix_secs()
    ));
    let node = node_with(&base, dir, "switch_off", true, Vec::new(), false).await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    let created: Result<ShareDto, _> = call(
        &node,
        method::AGENT_SHARE_CREATE,
        json!({ "taskIds": ["t1"] }),
    )
    .await;
    assert_eq!(
        created.unwrap_err().reason,
        Some(ErrorReason::ShareUnavailable)
    );
    let usage: Result<Value, _> = call(&node, method::AGENT_SHARE_USAGE_REFRESH, json!({})).await;
    assert_eq!(
        usage.unwrap_err().reason,
        Some(ErrorReason::ShareUnavailable)
    );
    let opened = node.service.open_link("fluxdown://share/abcdef").await;
    assert_eq!(
        opened.unwrap_err().reason,
        Some(ErrorReason::ShareUnavailable)
    );
    let config: ShareConfigDto = call(&node, method::AGENT_SHARE_CONFIG, json!({}))
        .await
        .unwrap();
    assert!(!config.local_config.enabled);
    assert!(config.policy.live_tiers.is_empty());
    let state = node.events.inspect(|s| s.share.clone());
    assert!(!state.available);
    assert!(!state.local_config.enabled);
    assert_eq!(requests(&mock), 0);
    assert!(node.service.with(|i| i.endpoint.is_none()));

    let mut local = serde_json::to_value(config.local_config).unwrap();
    local["enabled"] = json!(true);
    let _: Value = call(&node, method::AGENT_SHARE_SET_LOCAL_CONFIG, local)
        .await
        .unwrap();
    wait_until("policy fetched after enabling", || {
        node.events
            .inspect(|s| s.share.available && s.share.local_config.enabled)
    })
    .await;
    assert!(requests(&mock) > 0);
    node.cancel.cancel();
}

/// 关闭开关即收掉端点与运行期状态、之后不再访问云端；再次开启按启动流程恢复原有分享。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disabling_share_tears_down_and_reenabling_restores() {
    use fluxdown_protocol::{ErrorReason, ShareConfigDto};
    let (base, mock) = start_mock().await;
    let (sender, _dir, _payload) = sender_with_file(&base, "switch_toggle", 64 * 1024).await;
    let share: ShareDto = call(
        &sender,
        method::AGENT_SHARE_CREATE,
        json!({ "taskIds": ["t1"] }),
    )
    .await
    .unwrap();
    wait_until("endpoint bound", || endpoint_bound(&sender)).await;

    let config: ShareConfigDto = call(&sender, method::AGENT_SHARE_CONFIG, json!({}))
        .await
        .unwrap();
    let mut local = serde_json::to_value(config.local_config).unwrap();
    local["enabled"] = json!(false);
    let _: Value = call(&sender, method::AGENT_SHARE_SET_LOCAL_CONFIG, local.clone())
        .await
        .unwrap();
    wait_until("runtime torn down", || {
        sender
            .service
            .with(|i| i.endpoint.is_none() && i.config.is_none())
    })
    .await;
    let state = sender.events.inspect(|s| s.share.clone());
    assert!(!state.available);
    assert!(state.shares.is_empty());
    let stopped: Result<ShareDto, _> =
        call(&sender, method::AGENT_SHARE_STOP, json!({ "id": share.id })).await;
    assert_eq!(
        stopped.unwrap_err().reason,
        Some(ErrorReason::ShareUnavailable)
    );
    let quiet = requests(&mock);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(requests(&mock), quiet);

    local["enabled"] = json!(true);
    let _: Value = call(&sender, method::AGENT_SHARE_SET_LOCAL_CONFIG, local)
        .await
        .unwrap();
    wait_until("share restored after re-enabling", || {
        sender.events.inspect(|s| {
            s.share.available
                && s.share
                    .shares
                    .iter()
                    .any(|s| s.id == share.id && s.status == ShareStatus::Active)
        })
    })
    .await;
    wait_until("endpoint bound again", || endpoint_bound(&sender)).await;
    sender.cancel.cancel();
}
