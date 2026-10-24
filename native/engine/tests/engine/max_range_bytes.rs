//! 插件 `maxRangeBytes`（`RequestSpec::max_range_bytes`）的确定性 e2e。
//!
//! 本地 raw-tokio HTTP/1.1 服务器模拟「网盘 CDN 直链」：HEAD、无 Range、开放式
//! `bytes=N-`、跨度超过上限的 Range 一律 403，只有跨度 ≤ 上限的有界 Range 才回 206。
//! 声明了上限的任务必须在多段 / 单流 / hint 跳过探测 / 回退单流各条路径上都完整下完，
//! 且服务器从未见到违规请求；同一服务器上不声明上限的单流任务则被拒绝（对照组）。

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fluxdown_engine::db::Db;
use fluxdown_engine::downloader::{
    DownloadParams, ProgressUpdate, RequestSpec, build_client, run_download,
};
use fluxdown_engine::events::{EngineEvent, EventSink};
use fluxdown_engine::proxy_config::ProxyConfig;
use fluxdown_engine::speed_limiter::SpeedLimiter;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const KIB: usize = 1024;
/// 服务器接受的最大 Range 跨度，也是任务声明的 `maxRangeBytes`。
const CAP: usize = 256 * KIB;
const TEST_UA: &str = "FluxDownMaxRangeTest/1.0";
const LAST_MODIFIED: &str = "Wed, 01 Jan 2025 00:00:00 GMT";

struct NoopTestSink;
impl EventSink for NoopTestSink {
    fn emit(&self, _event: EngineEvent) {}
}

/// 确定性伪随机 body（LCG）。
fn gen_body(len: usize, seed: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    let mut x = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
    while out.len() < len {
        x = x
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        out.extend_from_slice(&x.to_le_bytes());
    }
    out.truncate(len);
    out
}

// ===========================================================================
// 严格服务器
// ===========================================================================

#[derive(Clone, Copy, Debug)]
struct Served {
    start: usize,
    span: usize,
    /// ETag 版本（1 / 2）。
    version: u8,
}

struct ServerState {
    body: Arc<Vec<u8>>,
    /// 第 N 个数据请求（跨度 > 1）之后把 ETag 换成 v2（模拟文件被替换）。
    flip_after: Option<usize>,
    /// 违规请求数：无 Range、开放式 Range、跨度超过 [`CAP`]。
    violations: AtomicUsize,
    data_gets: AtomicUsize,
    served: Mutex<Vec<Served>>,
}

struct TestServer {
    port: u16,
    state: Arc<ServerState>,
    accept_task: tokio::task::JoinHandle<()>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.accept_task.abort();
    }
}

impl TestServer {
    fn violations(&self) -> usize {
        self.state.violations.load(Ordering::SeqCst)
    }

    fn served(&self) -> Vec<Served> {
        self.state.served.lock().expect("served").clone()
    }

    /// 已回 206 的最大跨度。
    fn max_span(&self) -> usize {
        self.served().iter().map(|s| s.span).max().unwrap_or(0)
    }
}

async fn start_server(body: &Arc<Vec<u8>>, flip_after: Option<usize>) -> TestServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("local_addr").port();
    let state = Arc::new(ServerState {
        body: body.clone(),
        flip_after,
        violations: AtomicUsize::new(0),
        data_gets: AtomicUsize::new(0),
        served: Mutex::new(Vec::new()),
    });
    let shared = state.clone();
    let accept_task = tokio::spawn(async move {
        let mut conns = tokio::task::JoinSet::new();
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let state = shared.clone();
            conns.spawn(async move {
                if let Err(error) = serve_conn(stream, state).await {
                    assert!(
                        matches!(
                            error.kind(),
                            std::io::ErrorKind::BrokenPipe
                                | std::io::ErrorKind::ConnectionReset
                                | std::io::ErrorKind::ConnectionAborted
                                | std::io::ErrorKind::UnexpectedEof
                        ),
                        "test server connection failed: {error}"
                    );
                }
            });
            while conns.try_join_next().is_some() {}
        }
    });
    TestServer {
        port,
        state,
        accept_task,
    }
}

async fn reject(stream: &mut TcpStream) -> std::io::Result<()> {
    stream
        .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        .await?;
    stream.shutdown().await
}

async fn serve_conn(mut stream: TcpStream, state: Arc<ServerState>) -> std::io::Result<()> {
    let mut buf = Vec::with_capacity(1024);
    let mut tmp = [0u8; 1024];
    loop {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if buf.len() > 64 * 1024 {
            return Ok(());
        }
    }
    let text = String::from_utf8_lossy(&buf);
    let mut is_head = false;
    // (起点, 终点)；终点缺省 = 开放式。
    let mut range: Option<(usize, Option<usize>)> = None;
    for (i, line) in text.split("\r\n").enumerate() {
        if i == 0 {
            is_head = line.starts_with("HEAD ");
            continue;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("range")
            && let Some(v) = value.trim().strip_prefix("bytes=")
            && let Some((s, e)) = v.split_once('-')
            && let Ok(start) = s.trim().parse::<usize>()
        {
            range = Some((start, e.trim().parse::<usize>().ok()));
        }
    }

    if is_head {
        return reject(&mut stream).await;
    }
    let total = state.body.len();
    let Some((start, Some(end))) = range else {
        // 无 Range 或开放式 Range。
        state.violations.fetch_add(1, Ordering::SeqCst);
        return reject(&mut stream).await;
    };
    if start >= total {
        let head = format!(
            "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{total}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        stream.write_all(head.as_bytes()).await?;
        return stream.shutdown().await;
    }
    let span = end.saturating_sub(start) + 1;
    if span > CAP {
        state.violations.fetch_add(1, Ordering::SeqCst);
        return reject(&mut stream).await;
    }
    let end = end.min(total - 1);
    let len = end - start + 1;
    let version = if span > 1 {
        let seen = state.data_gets.fetch_add(1, Ordering::SeqCst);
        if state.flip_after.is_some_and(|n| seen >= n) {
            2
        } else {
            1
        }
    } else {
        1
    };
    state.served.lock().expect("served").push(Served {
        start,
        span: len,
        version,
    });
    let head = format!(
        "HTTP/1.1 206 Partial Content\r\nContent-Length: {len}\r\nContent-Range: bytes {start}-{end}/{total}\r\nAccept-Ranges: bytes\r\nETag: \"max-range-v{version}\"\r\nLast-Modified: {LAST_MODIFIED}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(&state.body[start..=end]).await?;
    stream.flush().await?;
    stream.shutdown().await
}

// ===========================================================================
// run_download 驱动
// ===========================================================================

fn work_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("fluxdown_maxrange_{}_{}", tag, std::process::id()));
    if let Err(error) = std::fs::remove_dir_all(&d) {
        assert_eq!(
            error.kind(),
            std::io::ErrorKind::NotFound,
            "clean test path: {error}"
        );
    }
    std::fs::create_dir_all(&d).expect("create work dir");
    d
}

#[derive(Clone, Copy)]
struct Scenario {
    segments: i32,
    /// 跳过探测（插件 ephemeral + totalBytes）。
    hint: bool,
    cap: Option<i64>,
}

struct RunOutcome {
    status: i32,
    dest: PathBuf,
}

async fn run(tag: &str, server: &TestServer, sc: Scenario) -> RunOutcome {
    let dir = work_dir(tag);
    let db = Db::open(&dir).await.expect("Db::open");
    let task_id = format!("maxrange-{tag}");
    let url = format!("http://127.0.0.1:{}/file.bin", server.port);
    let file_name = "file.bin";
    let total = server.state.body.len() as i64;
    db.insert_task(
        &task_id,
        &url,
        file_name,
        &dir.to_string_lossy(),
        sc.segments,
        total,
        "",
        "",
        "",
        0,
    )
    .await
    .expect("insert_task");

    let (tx, mut rx) = mpsc::channel::<ProgressUpdate>(256);
    let last_status = Arc::new(AtomicI32::new(0));
    let ls = last_status.clone();
    let collector = tokio::spawn(async move {
        while let Some(u) = rx.recv().await {
            if u.status >= 3 {
                ls.store(u.status, Ordering::SeqCst);
            }
        }
    });

    let params = DownloadParams {
        spawn_gen: 1,
        unattended: false,
        preprobed: None,
        interceptor: fluxdown_engine::intercept::noop(),
        connection_cap: 0,
        run_meta: fluxdown_engine::intercept::RunMeta::default(),
        auto_proxy: None,
        multi_nic: None,
        auto_max_connections: 0,
        task_id: task_id.clone(),
        url,
        save_dir: dir.to_string_lossy().to_string(),
        file_name: file_name.to_string(),
        segment_count: sc.segments,
        is_resume: false,
        range_verified: !sc.hint,
        db: db.clone(),
        client: build_client(&ProxyConfig::default(), TEST_UA).expect("build_client"),
        progress_tx: tx,
        cancel_token: CancellationToken::new(),
        speed_limiter: SpeedLimiter::new(0),
        cookies: String::new(),
        referrer: String::new(),
        hint_file_size: if sc.hint { total } else { 0 },
        proxy_config: ProxyConfig::default(),
        sink: Arc::new(NoopTestSink),
        selector: Arc::new(fluxdown_engine::NoopSelection),
        checksum: String::new(),
        extra_headers: std::collections::HashMap::new(),
        spec: RequestSpec {
            max_range_bytes: sc.cap,
            ..RequestSpec::empty_get()
        },
        audio_url: None,
        use_server_time: false,
        overwrite: fluxdown_engine::file_exists::OverwritePolicy::Never,
        ffmpeg_path: None,
        cdn: fluxdown_engine::cdn::CdnTaskInput::default(),
    };

    tokio::time::timeout(Duration::from_secs(60), run_download(params))
        .await
        .expect("run_download must finish within 60s");
    collector
        .await
        .expect("test background task must not panic");

    RunOutcome {
        status: last_status.load(Ordering::SeqCst),
        dest: dir.join(file_name),
    }
}

async fn assert_complete_and_capped(out: &RunOutcome, server: &TestServer, body: &[u8]) {
    assert_eq!(out.status, 3, "task must complete");
    let got = tokio::fs::read(&out.dest).await.expect("read dest");
    assert_eq!(got.len(), body.len(), "dest length mismatch");
    assert!(got == body, "dest bytes differ from origin body");
    assert_eq!(
        server.violations(),
        0,
        "engine sent requests the capped endpoint rejects"
    );
    assert!(
        server.max_span() <= CAP,
        "served span {} exceeds the cap {CAP}",
        server.max_span()
    );
    // 整个文件只能由 ≤ CAP 的窗口拼成：数据请求数不少于 ceil(len / CAP)。
    let windows = server.served().iter().filter(|s| s.span > 1).count();
    assert!(
        windows >= body.len().div_ceil(CAP),
        "expected at least {} windowed requests, got {windows}",
        body.len().div_ceil(CAP)
    );
}

fn capped(segments: i32, hint: bool) -> Scenario {
    Scenario {
        segments,
        hint,
        cap: Some(CAP as i64),
    }
}

// ===========================================================================
// 场景
// ===========================================================================

/// 探测路径 + 多段：HEAD 403、`bytes=0-0` 206 的探测照常通过；每个分段 worker 在自己的
/// 区间内逐窗口请求直到段完成。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn capped_multi_segment_probe_downloads_complete_file() {
    let body = Arc::new(gen_body(3 * 1024 * KIB + 12_345, 0xB1));
    let server = start_server(&body, None).await;
    let out = run("multi_probe", &server, capped(4, false)).await;
    assert_complete_and_capped(&out, &server, &body).await;
}

/// 探测路径 + 单流：全新下载也必须是有界 Range 窗口，窗口间由外层循环衔接。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn capped_single_stream_probe_downloads_complete_file() {
    let body = Arc::new(gen_body(1024 * KIB + 777, 0xB2));
    let server = start_server(&body, None).await;
    let out = run("single_probe", &server, capped(1, false)).await;
    assert_complete_and_capped(&out, &server, &body).await;
}

/// hint（跳过探测）+ 多段：首连接不再是无 Range 的 plain GET。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn capped_hint_multi_segment_never_sends_plain_first_connection() {
    let body = Arc::new(gen_body(3 * 1024 * KIB + 4_321, 0xB3));
    let server = start_server(&body, None).await;
    let out = run("multi_hint", &server, capped(4, true)).await;
    assert_complete_and_capped(&out, &server, &body).await;
}

/// hint + 单流：Range 能力未经验证（`range_verified = false`），声明上限的任务仍必须带 Range。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn capped_hint_single_stream_sends_bounded_ranges() {
    let body = Arc::new(gen_body(1024 * KIB + 999, 0xB4));
    let server = start_server(&body, None).await;
    let out = run("single_hint", &server, capped(1, true)).await;
    assert_complete_and_capped(&out, &server, &body).await;
}

/// 文件在多段下载中途被替换（ETag 变化）→ 清盘并回退单流；回退单流同样只发有界 Range，
/// 并以新版本完整下完。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn capped_version_change_fallback_single_keeps_window_limit() {
    let body = Arc::new(gen_body(3 * 1024 * KIB + 2_468, 0xB5));
    let server = start_server(&body, Some(5)).await;
    let out = run("fallback", &server, capped(4, false)).await;
    assert_complete_and_capped(&out, &server, &body).await;
    let restarted = server
        .served()
        .iter()
        .any(|s| s.start == 0 && s.span == CAP && s.version == 2);
    assert!(
        restarted,
        "expected the single-stream fallback to restart from byte 0 under the new version: {:?}",
        server.served()
    );
}

/// 对照：同一服务器上不声明上限的单流任务发无 Range 的 GET，被拒绝，任务失败。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn uncapped_single_stream_is_rejected_by_the_strict_server() {
    let body = Arc::new(gen_body(1024 * KIB + 111, 0xB6));
    let server = start_server(&body, None).await;
    let out = run(
        "uncapped",
        &server,
        Scenario {
            segments: 1,
            hint: false,
            cap: None,
        },
    )
    .await;
    assert_eq!(
        out.status, 4,
        "uncapped task must fail against the cap-only endpoint"
    );
    assert!(server.violations() >= 1, "expected a rejected request");
}
