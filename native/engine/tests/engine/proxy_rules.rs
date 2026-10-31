//! 站点代理规则的逐跳路由 e2e：源站与假代理都是本地 raw-tokio HTTP/1.1 服务，
//! 假代理记录收到的请求行（reqwest 经 HTTP 代理发 absolute-form 请求）。
//!
//! 覆盖：规则命中的链接经代理、重定向后命中规则的那一跳起改走代理、
//! 命中任务出口规则后重定向到未命中规则的地址仍沿用任务出口、不挂路由器时规则不起作用。

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fluxdown_engine::db::Db;
use fluxdown_engine::downloader::{
    DownloadParams, ProgressUpdate, RequestSpec, build_client, run_download,
};
use fluxdown_engine::events::{EngineEvent, EventSink};
use fluxdown_engine::proxy_config::{ProxyConfig, ProxyMode, ProxyType};
use fluxdown_engine::proxy_rules::ProxyRuleSet;
use fluxdown_engine::rule_route::EgressRouter;
use fluxdown_engine::speed_limiter::SpeedLimiter;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const BODY_LEN: usize = 1024 * 1024;
const TEST_UA: &str = "FluxDownProxyRulesTest/1.0";
const SEGMENTS: i32 = 4;

struct NoopTestSink;
impl EventSink for NoopTestSink {
    fn emit(&self, _event: EngineEvent) {}
}

fn gen_body(len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    let mut x: u64 = 0x5eed;
    while out.len() < len {
        x = x
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        out.extend_from_slice(&x.to_le_bytes());
    }
    out.truncate(len);
    out
}

/// 源站 / 假代理两用：对任意请求目标按路径应答，`/start` 在配置了 `location` 时 302。
struct TestServer {
    /// 收到的请求行（`GET /path HTTP/1.1` 或 absolute-form）。
    log: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}

impl TestServer {
    fn lines(&self) -> Vec<String> {
        self.log.lock().expect("log lock").clone()
    }

    /// 含给定子串的请求行数。
    fn count(&self, needle: &str) -> usize {
        self.lines().iter().filter(|l| l.contains(needle)).count()
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn bind() -> (TcpListener, u16) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    (listener, port)
}

fn serve(listener: TcpListener, body: Arc<Vec<u8>>, location: Option<String>) -> TestServer {
    let log = Arc::new(Mutex::new(Vec::new()));
    let shared = log.clone();
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let (body, location, log) = (body.clone(), location.clone(), shared.clone());
            tokio::spawn(async move {
                if let Err(error) = conn(stream, body, location, log).await {
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
        }
    });
    TestServer { log, task }
}

async fn conn(
    mut stream: TcpStream,
    body: Arc<Vec<u8>>,
    location: Option<String>,
    log: Arc<Mutex<Vec<String>>>,
) -> std::io::Result<()> {
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
    let text = String::from_utf8_lossy(&buf).into_owned();
    let mut lines = text.split("\r\n");
    let request_line = lines.next().unwrap_or_default().to_string();
    log.lock().expect("log lock").push(request_line.clone());
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();
    let path = match target.split_once("://") {
        Some((_, rest)) => rest.find('/').map_or("/", |i| &rest[i..]),
        None => target,
    };
    let mut range: Option<(usize, Option<usize>)> = None;
    for line in lines {
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("range")
            && let Some(v) = value.trim().strip_prefix("bytes=")
            && let Some((s, e)) = v.split_once('-')
            && let Ok(start) = s.trim().parse::<usize>()
        {
            range = Some((start, e.trim().parse::<usize>().ok()));
        }
    }

    if path.starts_with("/start")
        && let Some(location) = &location
    {
        let head = format!(
            "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        stream.write_all(head.as_bytes()).await?;
        return stream.shutdown().await;
    }

    let total = body.len();
    let (status, start, end) = match range {
        Some((s, _)) if s >= total => {
            let head = format!(
                "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{total}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            stream.write_all(head.as_bytes()).await?;
            return stream.shutdown().await;
        }
        Some((s, e)) => (
            "206 Partial Content",
            s,
            e.unwrap_or(total - 1).min(total - 1),
        ),
        None => ("200 OK", 0, total - 1),
    };
    let len = end - start + 1;
    let mut head = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {len}\r\nAccept-Ranges: bytes\r\nETag: \"rules-v1\"\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n"
    );
    if range.is_some() {
        head.push_str(&format!("Content-Range: bytes {start}-{end}/{total}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await?;
    if method != "HEAD" {
        stream.write_all(&body[start..=end]).await?;
    }
    stream.shutdown().await
}

fn manual_proxy(port: u16) -> ProxyConfig {
    ProxyConfig {
        mode: ProxyMode::Manual,
        proxy_type: ProxyType::Http,
        host: "127.0.0.1".into(),
        port,
        username: String::new(),
        password: String::new(),
        no_proxy_list: String::new(),
        rules: None,
    }
}

fn rules_for(pattern: &str, proxy_port: u16) -> Arc<ProxyRuleSet> {
    let raw = format!(
        r#"[{{"id":"r1","enabled":true,"name":"","match":"regex","pattern":"{pattern}","action":"custom","proxy":"http://127.0.0.1:{proxy_port}"}}]"#
    );
    let rules = ProxyRuleSet::compile(&raw, &ProxyConfig::default());
    assert_eq!(rules.len(), 1, "the rule compiles");
    Arc::new(rules)
}

struct Run {
    status: i32,
    dest: PathBuf,
}

/// 用给定任务 client / 出口配置 / 路由器跑完整 `run_download`。
async fn run(
    tag: &str,
    url: String,
    task_proxy: ProxyConfig,
    egress: Option<Arc<EgressRouter>>,
) -> Run {
    // 假源站与假代理都在环回上，须关闭引擎的环回直连才能验证路由。
    fluxdown_engine::proxy_config::disable_loopback_bypass_for_tests();
    let dir = std::env::temp_dir().join(format!(
        "fluxdown_proxy_rules_{}_{}",
        tag,
        std::process::id()
    ));
    if let Err(error) = std::fs::remove_dir_all(&dir) {
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound, "clean: {error}");
    }
    std::fs::create_dir_all(&dir).expect("create work dir");
    let db = Db::open(&dir).await.expect("Db::open");
    let task_id = format!("proxy-rules-{tag}");
    let file_name = "f.bin";
    db.insert_task(
        &task_id,
        &url,
        file_name,
        &dir.to_string_lossy(),
        SEGMENTS,
        BODY_LEN as i64,
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

    let mut spec = RequestSpec::empty_get();
    spec.egress = egress;
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
        task_id,
        url,
        save_dir: dir.to_string_lossy().to_string(),
        file_name: file_name.to_string(),
        segment_count: SEGMENTS,
        is_resume: false,
        range_verified: true,
        db: db.clone(),
        client: build_client(&task_proxy, TEST_UA).expect("build_client"),
        progress_tx: tx,
        cancel_token: CancellationToken::new(),
        speed_limiter: SpeedLimiter::new(0),
        cookies: String::new(),
        referrer: String::new(),
        hint_file_size: 0,
        proxy_config: task_proxy,
        sink: Arc::new(NoopTestSink),
        selector: Arc::new(fluxdown_engine::NoopSelection),
        checksum: String::new(),
        extra_headers: std::collections::HashMap::new(),
        spec,
        audio_url: None,
        use_server_time: false,
        overwrite: fluxdown_engine::file_exists::OverwritePolicy::Never,
        ffmpeg_path: None,
        cdn: fluxdown_engine::cdn::CdnTaskInput::default(),
    };
    tokio::time::timeout(Duration::from_secs(60), run_download(params))
        .await
        .expect("run_download must finish within 60s");
    collector.await.expect("collector must not panic");
    Run {
        status: last_status.load(Ordering::SeqCst),
        dest: dir.join(file_name),
    }
}

async fn assert_complete(run: &Run, body: &[u8]) {
    assert_eq!(run.status, 3, "task must complete");
    let got = tokio::fs::read(&run.dest).await.expect("read dest");
    assert!(got == body, "dest bytes differ from origin body");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn matching_rule_sends_every_request_through_the_rule_proxy() {
    let body = Arc::new(gen_body(BODY_LEN));
    let (origin_listener, origin_port) = bind().await;
    let (proxy_listener, proxy_port) = bind().await;
    let origin = serve(origin_listener, body.clone(), None);
    let proxy = serve(proxy_listener, body.clone(), None);

    let router = Arc::new(EgressRouter::new(
        rules_for("/via-proxy/", proxy_port),
        None,
        TEST_UA,
        false,
    ));
    let url = format!("http://127.0.0.1:{origin_port}/via-proxy/f.bin");
    let out = run("hit", url, ProxyConfig::default(), Some(router)).await;

    assert_complete(&out, &body).await;
    assert_eq!(origin.lines().len(), 0, "origin must see no direct request");
    assert!(
        proxy.count(&format!("http://127.0.0.1:{origin_port}/via-proxy/f.bin")) >= 1,
        "proxy sees absolute-form requests: {:?}",
        proxy.lines()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn redirect_target_matching_a_rule_switches_to_the_proxy_from_that_hop() {
    let body = Arc::new(gen_body(BODY_LEN));
    let (origin_listener, origin_port) = bind().await;
    let (proxy_listener, proxy_port) = bind().await;
    let location = format!("http://127.0.0.1:{origin_port}/via-proxy/f.bin");
    let origin = serve(origin_listener, body.clone(), Some(location));
    let proxy = serve(proxy_listener, body.clone(), None);

    let router = Arc::new(EgressRouter::new(
        rules_for("/via-proxy/", proxy_port),
        None,
        TEST_UA,
        false,
    ));
    let url = format!("http://127.0.0.1:{origin_port}/start");
    let out = run("redirect", url, ProxyConfig::default(), Some(router)).await;

    assert_complete(&out, &body).await;
    assert!(
        origin.count("/start") >= 1,
        "the first hop reaches the origin directly: {:?}",
        origin.lines()
    );
    assert_eq!(
        origin.count("/via-proxy/"),
        0,
        "the origin never sees the rule-matched hop: {:?}",
        origin.lines()
    );
    assert!(
        proxy.count("/via-proxy/f.bin") >= 1,
        "the second hop and the segments go through the proxy: {:?}",
        proxy.lines()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unmatched_redirect_target_keeps_the_task_egress() {
    let body = Arc::new(gen_body(BODY_LEN));
    let (origin_listener, origin_port) = bind().await;
    let (proxy_listener, proxy_port) = bind().await;
    let location = format!("http://127.0.0.1:{origin_port}/plain/f.bin");
    let origin = serve(origin_listener, body.clone(), Some(location.clone()));
    // 任务出口是代理，所以 `/start` 的 302 由假代理下发。
    let proxy = serve(proxy_listener, body.clone(), Some(location));

    // 任务出口由规则 0（匹配 /start）决定，即任务 client 本身就是代理 client。
    let router = Arc::new(EgressRouter::new(
        rules_for("/start", proxy_port),
        Some(0),
        TEST_UA,
        false,
    ));
    let url = format!("http://127.0.0.1:{origin_port}/start");
    let out = run("base", url, manual_proxy(proxy_port), Some(router)).await;

    assert_complete(&out, &body).await;
    assert_eq!(
        origin.lines().len(),
        0,
        "the unmatched hop still follows the task egress: {:?}",
        origin.lines()
    );
    assert!(proxy.count("/plain/f.bin") >= 1, "{:?}", proxy.lines());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_a_router_rules_have_no_effect() {
    let body = Arc::new(gen_body(BODY_LEN));
    let (origin_listener, origin_port) = bind().await;
    let (proxy_listener, _proxy_port) = bind().await;
    let origin = serve(origin_listener, body.clone(), None);
    let proxy = serve(proxy_listener, body.clone(), None);

    let url = format!("http://127.0.0.1:{origin_port}/via-proxy/f.bin");
    let out = run("off", url, ProxyConfig::default(), None).await;

    assert_complete(&out, &body).await;
    assert!(origin.count("/via-proxy/f.bin") >= 1);
    assert_eq!(proxy.lines().len(), 0, "{:?}", proxy.lines());
}
