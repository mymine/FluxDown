//! 多来源：直连 ↔ HTTP 来源（CDN / 桥接）在传输中途切换，已校验的块不重取，来源字节计量准确。

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use fluxdown_share::{
    ChunkSource, ReceiveParams, ReceiveSession, SourceError, SourceFuture, SourceKind,
    SourcePreference, SourceRequest, ticket::sign_ticket,
};
use reqwest::StatusCode;

use crate::common::{far_future, http, loopback_hint, payload, rig_with};

const MIB: usize = 1024 * 1024;

/// 内存里的来源：记录每一次区间请求；可被设成“返回坏数据”或固定错误。
struct MemSource {
    kind: SourceKind,
    data: Arc<Vec<u8>>,
    requests: Mutex<Vec<SourceRequest>>,
    corrupt: AtomicBool,
    failure: Mutex<Option<SourceError>>,
    calls: AtomicUsize,
}

impl MemSource {
    fn new(kind: SourceKind, data: Vec<u8>) -> Arc<Self> {
        Arc::new(Self {
            kind,
            data: Arc::new(data),
            requests: Mutex::new(Vec::new()),
            corrupt: AtomicBool::new(false),
            failure: Mutex::new(None),
            calls: AtomicUsize::new(0),
        })
    }

    fn offsets(&self) -> Vec<u64> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.offset)
            .collect()
    }
}

impl ChunkSource for MemSource {
    fn kind(&self) -> SourceKind {
        self.kind
    }

    fn fetch(&self, request: SourceRequest) -> SourceFuture<'_> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some(error) = self.failure.lock().unwrap().clone() {
                return Err(error);
            }
            self.requests.lock().unwrap().push(request.clone());
            let start = usize::try_from(request.offset).unwrap();
            let end = start + usize::try_from(request.len).unwrap();
            let mut bytes = self.data[start..end].to_vec();
            if self.corrupt.load(Ordering::SeqCst) {
                bytes[0] ^= 0xFF;
            }
            Ok(Bytes::from(bytes))
        })
    }
}

async fn get_range(url: &str, range: &str) -> Vec<u8> {
    let resp = http().get(url).header("Range", range).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT, "{range}");
    resp.bytes().await.unwrap().to_vec()
}

/// 直连取走前两块后分享方离线：其余块改走 CDN 来源；拼接结果逐字节一致，
/// 来源只收到未取过的块的请求，字节计量与实际一致，直连恢复后自动切回。
#[tokio::test]
async fn direct_to_cdn_mid_transfer_does_not_refetch_verified_chunks() {
    let rig = rig_with("share-switch", 20 * MIB, |p| {
        p.connect_timeout = Duration::from_millis(1500);
    })
    .await;
    let url = rig.url();
    let data = rig.file.data.clone();
    let cdn = MemSource::new(SourceKind::Cdn, data.clone());
    rig.session.set_source(cdn.clone());
    assert_eq!(rig.session.source_kinds(), vec![SourceKind::Cdn]);

    // 前两块（8 MiB）走直连。
    let head = get_range(&url, &format!("bytes=0-{}", 8 * MIB - 1)).await;
    assert_eq!(&head[..], &data[..8 * MIB]);
    assert_eq!(cdn.calls.load(Ordering::SeqCst), 0, "direct comes first");
    assert_eq!(rig.session.active_source(), None);

    // 分享方离线，下载器从 8 MiB 处续传：剩余三块全部来自 CDN。
    rig.sender.ep.shutdown().await;
    let tail = get_range(&url, &format!("bytes={}-", 8 * MIB)).await;
    assert_eq!(&tail[..], &data[8 * MIB..]);
    assert_eq!(
        cdn.offsets(),
        vec![8 * MIB as u64, 12 * MIB as u64, 16 * MIB as u64],
        "only the missing chunks are requested from the source"
    );
    assert_eq!(rig.session.active_source(), Some(SourceKind::Cdn));

    // 拼接后与原文件 BLAKE3 一致。
    let mut assembled = head;
    assembled.extend_from_slice(&tail);
    assert_eq!(blake3::hash(&assembled), blake3::hash(&data));

    // 计量：来源字节恰好是 12 MiB；直连字节是前两块（含少量协议开销）。
    assert_eq!(rig.session.source_bytes().cdn, 12 * MIB as u64);
    assert_eq!(rig.session.source_bytes().bridge, 0);
    let summary = rig.session.summary().await.unwrap();
    assert_eq!(summary.bytes.cdn, 12 * MIB as u64);
    assert!(
        summary.bytes.direct >= 8 * MIB as u64 && summary.bytes.direct < 9 * MIB as u64,
        "direct bytes {}",
        summary.bytes.direct
    );

    // 终态上报：全部来源字节恰好出现一次（连接报告 + 无连接补发合计），不重复、不遗漏。
    rig.session.close().await;
    let reports = rig.receiver.sink.sessions.lock().unwrap().clone();
    let finals: Vec<_> = reports.iter().filter(|r| r.is_final).collect();
    let cdn_total: u64 = finals.iter().map(|r| r.summary.bytes.cdn).sum();
    assert_eq!(cdn_total, 12 * MIB as u64, "{finals:#?}");
}

/// 用户点「切换桥接」（来源优先）再切回直连：路线按偏好切换，块各只取一次。
#[tokio::test]
async fn preference_switch_round_trip_fetches_each_chunk_once() {
    let rig = rig_with("share-pref", 16 * MIB, |_| {}).await;
    let url = rig.url();
    let data = rig.file.data.clone();
    let bridge = MemSource::new(SourceKind::Bridge, data.clone());
    rig.session.set_source(bridge.clone());

    let a = get_range(&url, &format!("bytes=0-{}", 4 * MIB - 1)).await;
    assert_eq!(bridge.calls.load(Ordering::SeqCst), 0);

    rig.session.set_preference(SourcePreference::Sources);
    let b = get_range(&url, &format!("bytes={}-{}", 4 * MIB, 12 * MIB - 1)).await;
    assert_eq!(bridge.offsets(), vec![4 * MIB as u64, 8 * MIB as u64]);
    assert_eq!(rig.session.active_source(), Some(SourceKind::Bridge));

    rig.session.set_preference(SourcePreference::Direct);
    let c = get_range(&url, &format!("bytes={}-", 12 * MIB)).await;
    assert_eq!(bridge.calls.load(Ordering::SeqCst), 2, "back on direct");
    assert_eq!(rig.session.active_source(), None);

    let mut all = a;
    all.extend_from_slice(&b);
    all.extend_from_slice(&c);
    assert_eq!(blake3::hash(&all), blake3::hash(&data));
    assert_eq!(rig.session.source_bytes().bridge, 8 * MIB as u64);
    assert_eq!(rig.session.source_bytes().cdn, 0);
}

/// 来源返回坏数据：块校验失败，来源被拉黑，数据改从直连取回且结果正确。
#[tokio::test]
async fn corrupt_source_is_banned_and_direct_takes_over() {
    let rig = rig_with("share-corrupt", 8 * MIB, |_| {}).await;
    let url = rig.url();
    let data = rig.file.data.clone();
    let cdn = MemSource::new(SourceKind::Cdn, data.clone());
    cdn.corrupt.store(true, Ordering::SeqCst);
    rig.session.set_source(cdn.clone());
    rig.session.set_preference(SourcePreference::Sources);

    let body = get_range(&url, "bytes=0-").await;
    assert_eq!(blake3::hash(&body), blake3::hash(&data));
    assert!(
        rig.session.source_kinds().is_empty(),
        "corrupt source banned"
    );
    assert_eq!(
        rig.session.source_bytes().cdn,
        0,
        "nothing unverified is counted"
    );
    let summary = rig.session.summary().await.unwrap();
    assert!(summary.verify_failures >= 2, "{}", summary.verify_failures);
    // 拉黑后不再向它发请求。
    let calls = cdn.calls.load(Ordering::SeqCst);
    assert_eq!(calls, 2, "two attempts for the first chunk, then banned");
}

/// 分享方离线且来源报告流量耗尽 / 分享已结束：适配器给出可区分的状态。
#[tokio::test]
async fn source_errors_map_to_adapter_statuses() {
    let rig = rig_with("share-errors", 4 * MIB, |p| {
        p.connect_timeout = Duration::from_millis(800);
    })
    .await;
    let url = rig.url();
    let cdn = MemSource::new(SourceKind::Cdn, rig.file.data.clone());
    rig.session.set_source(cdn.clone());
    rig.sender.ep.shutdown().await;

    *cdn.failure.lock().unwrap() = Some(SourceError::TrafficExhausted);
    let resp = http().get(&url).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(resp.headers()["x-fluxshare-status"], "trafficExhausted");

    *cdn.failure.lock().unwrap() = Some(SourceError::Ended);
    let resp = http().get(&url).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::GONE);

    // 暂时不可用：来源保持可用，分享方离线 → 503 让下载器稍后重试。
    *cdn.failure.lock().unwrap() = Some(SourceError::Unavailable("http 503".into()));
    let resp = http().get(&url).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(rig.session.source_kinds(), vec![SourceKind::Cdn]);

    // 恢复后整块取回。
    *cdn.failure.lock().unwrap() = None;
    let resp = http().get(&url).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        blake3::hash(&resp.bytes().await.unwrap()),
        blake3::hash(&rig.file.data)
    );
}

/// 网关桥接：网关端点凭 `scope = bridge` 的票据（`rep` 是网关自己）直连在线的分享方取块，
/// 取回的块照常通过清单校验——这正是桥接来源背后的数据面。
#[tokio::test]
async fn gateway_endpoint_fetches_with_a_bridge_scoped_ticket() {
    let rig = rig_with("share-bridge", 5 * MIB, |_| {}).await;
    // 复用接收端点当“网关”：票据发给它，scope 为 bridge。
    let mut claims = payload("share-bridge", &rig.receiver.ep.id_z32(), far_future());
    claims.scope = "bridge".to_owned();
    let ticket = sign_ticket(&rig.signer.secret, &claims).unwrap();
    let mut params = ReceiveParams::new(rig.file.manifest.clone(), ticket, rig.sender.ep.id());
    params.addr_hint = Some(loopback_hint(&rig.sender.ep).await);
    let gateway = ReceiveSession::new(rig.receiver.ep.clone(), params);
    let chunk = gateway.fetch_chunk(0, 1).await.unwrap();
    assert_eq!(&chunk[..], &rig.file.data[4 * MIB..5 * MIB]);
    gateway.close().await;
}
