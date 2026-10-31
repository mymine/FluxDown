//! 插件解析出口 e2e：解析（`flux.fetch`）与随后的下载必须同出口，并遵循代理设定。
//!
//! 源站主机都在 `.invalid` 下（直连 DNS 必然失败），只有假代理能「访问」它们：假代理是本地
//! raw-tokio HTTP/1.1 服务，按 absolute-form 请求的路径直接应答并记录请求行。
//!
//! 覆盖：
//! - Auto：直连解析断在网络层 → 换手动代理重跑成功 → 下载钉在同一代理（单路径、无换路）；
//! - 站点规则：页面链接命中规则、解析出的媒体链接未命中 → 下载沿用页面规则的出口；
//! - 任务自带代理：解析的 `flux.fetch` 与下载都走任务代理。

#![cfg(feature = "plugins")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use fluxdown_engine::bt_config::BtConfig;
use fluxdown_engine::download_manager::NewTaskSpec;
use fluxdown_engine::proxy_config::{ProxyConfig, ProxyMode, ProxyType};
use fluxdown_engine::proxy_rules::ProxyRuleSet;
use fluxdown_engine::{Engine, EngineConfig, NoopSelection, NoopSink};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const FILE_BODY: &[u8] =
    b"fluxdown plugin resolve egress payload: same exit for resolve and download\n";

/// 假代理：`/watch` 应答 `media` 链接文本，`/file.bin` 应答 [`FILE_BODY`]，其余 404。
struct FakeProxy {
    port: u16,
    log: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}

impl FakeProxy {
    async fn start(media_url: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind proxy");
        let port = listener.local_addr().expect("proxy addr").port();
        let log = Arc::new(Mutex::new(Vec::new()));
        let shared = log.clone();
        let media_url = media_url.to_string();
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let (log, media_url) = (shared.clone(), media_url.clone());
                tokio::spawn(async move {
                    if let Err(error) = serve(stream, &media_url, log).await {
                        assert!(
                            matches!(
                                error.kind(),
                                std::io::ErrorKind::BrokenPipe
                                    | std::io::ErrorKind::ConnectionReset
                                    | std::io::ErrorKind::ConnectionAborted
                                    | std::io::ErrorKind::UnexpectedEof
                            ),
                            "fake proxy connection failed: {error}"
                        );
                    }
                });
            }
        });
        Self { port, log, task }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn count(&self, needle: &str) -> usize {
        self.log
            .lock()
            .expect("log lock")
            .iter()
            .filter(|line| line.contains(needle))
            .count()
    }
}

impl Drop for FakeProxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve(
    mut stream: TcpStream,
    media_url: &str,
    log: Arc<Mutex<Vec<String>>>,
) -> std::io::Result<()> {
    let mut head = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Ok(());
        }
        head.extend_from_slice(&chunk[..n]);
    }
    let head = String::from_utf8_lossy(&head).into_owned();
    let line = head.lines().next().unwrap_or_default().to_string();
    log.lock().expect("log lock").push(line.clone());
    let is_head = line.starts_with("HEAD ");
    let body: &[u8] = if line.contains("/watch") {
        media_url.as_bytes()
    } else if line.contains("/file.bin") {
        FILE_BODY
    } else {
        stream
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await?;
        return stream.flush().await;
    };
    let status = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(status.as_bytes()).await?;
    if !is_head {
        stream.write_all(body).await?;
    }
    stream.flush().await
}

/// resolver：`flux.fetch` 源链接，把应答体当作媒体直链。
async fn write_plugin(dir: &std::path::Path, page_host: &str) {
    tokio::fs::create_dir_all(dir).await.expect("mkdir plugin");
    let manifest = format!(
        r#"{{
      "identity": "test@egress",
      "name": "Egress Test",
      "version": "1.0.0",
      "resolvers": [{{ "match": {{ "urls": ["*://{page_host}/*"] }}, "entry": "resolve.js", "timeoutMs": 8000 }}]
    }}"#
    );
    tokio::fs::write(dir.join("manifest.json"), manifest)
        .await
        .expect("write manifest");
    let resolve_js = r#"
      globalThis.resolve = async (ctx) => {
        const r = await flux.fetch({ url: ctx.url });
        if (r.status !== 200) throw new Error('page status ' + r.status);
        return { url: r.body.trim() };
      };
    "#;
    tokio::fs::write(dir.join("resolve.js"), resolve_js)
        .await
        .expect("write resolve.js");
}

fn engine_config(work: &std::path::Path, proxy_config: ProxyConfig) -> EngineConfig {
    EngineConfig {
        max_concurrent: 4,
        speed_limit_bps: 0,
        upload_limit_bps: 0,
        default_save_dir: work.to_string_lossy().into_owned(),
        app_data_dir: work.to_string_lossy().into_owned(),
        bt_config: BtConfig::default(),
        proxy_config,
        user_agent: String::new(),
        data_dir_override: Some(work.to_path_buf()),
        database_url: None,
    }
}

fn manual_fields(mode: ProxyMode, port: u16) -> ProxyConfig {
    ProxyConfig {
        mode,
        proxy_type: ProxyType::Http,
        host: "127.0.0.1".to_string(),
        port,
        ..ProxyConfig::default()
    }
}

struct Run {
    work: std::path::PathBuf,
    engine: Engine,
    task_id: String,
}

/// 装插件、建任务，驱动 resolve 回流直到下载结束；返回结束时的任务行。
async fn run_task(
    tag: &str,
    page_host: &str,
    proxy_config: ProxyConfig,
    task_proxy_url: &str,
) -> (Run, fluxdown_engine::model::TaskInfo) {
    let work = std::env::temp_dir().join(format!(
        "fluxdown-resolve-egress-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    tokio::fs::create_dir_all(&work).await.expect("mkdir work");
    let plugin_src = work.join("plugin_src");
    write_plugin(&plugin_src, page_host).await;
    let mut engine = Engine::new(
        engine_config(&work, proxy_config),
        Arc::new(NoopSink),
        Arc::new(NoopSelection),
    )
    .await
    .expect("engine");
    let pm = engine.manager.plugin_manager().expect("pm installed");
    pm.install_from_dir(&plugin_src).await.expect("install");
    let mut resolve_rx = engine.manager.take_resolve_rx().expect("resolve_rx");
    let mut done_rx = engine.manager.take_done_rx().expect("done_rx");

    let task_id = engine
        .manager
        .create_task(NewTaskSpec {
            url: format!("http://{page_host}/watch?v=1"),
            save_dir: work.to_string_lossy().into_owned(),
            file_name: "out.bin".to_string(),
            segments: 1,
            proxy_url: task_proxy_url.to_string(),
            ..Default::default()
        })
        .await
        .expect("create task");
    let done = loop {
        tokio::select! {
            Some(out) = resolve_rx.recv() => engine.manager.on_resolve_ready(out).await,
            Some(done) = done_rx.recv() => break done,
            () = tokio::time::sleep(Duration::from_secs(30)) => {
                let row = engine.db.load_task_by_id(&task_id).await.expect("load task");
                panic!("timeout waiting for download; task = {row:?}");
            }
        }
    };
    engine.manager.on_task_done(&done).await;
    let row = engine
        .db
        .load_task_by_id(&task_id)
        .await
        .expect("load task")
        .expect("task row");
    (
        Run {
            work,
            engine,
            task_id,
        },
        row,
    )
}

async fn assert_downloaded(run: &Run, row: &fluxdown_engine::model::TaskInfo) {
    assert_eq!(row.status, 3, "task must complete: {}", row.error_message);
    let bytes = tokio::fs::read(run.work.join("out.bin"))
        .await
        .expect("read downloaded file");
    assert_eq!(bytes, FILE_BODY);
}

async fn cleanup(run: Run) {
    drop(run.engine);
    if let Err(error) = tokio::fs::remove_dir_all(&run.work).await
        && error.kind() != std::io::ErrorKind::NotFound
    {
        eprintln!("best-effort test directory cleanup: {error}");
    }
}

/// Auto：直连解析断在网络层 → 改经手动代理重跑；下载钉在同一代理（单路径起飞即成功，
/// 不经直连、不再换路），路由标签如实标为「直连失败转移到手动代理」。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auto_resolve_fails_over_to_proxy_and_download_stays_on_it() {
    let page_host = "page-auto.fluxdown-test.invalid";
    let proxy = FakeProxy::start("http://media-auto.fluxdown-test.invalid/file.bin").await;
    let (run, row) = run_task(
        "auto",
        page_host,
        manual_fields(ProxyMode::Auto, proxy.port),
        "",
    )
    .await;
    assert_downloaded(&run, &row).await;
    assert_eq!(
        proxy.count("http://page-auto.fluxdown-test.invalid/watch"),
        1
    );
    assert!(proxy.count("http://media-auto.fluxdown-test.invalid/file.bin") >= 1);
    assert_eq!(
        row.auto_route, "proxy:failover:manual",
        "task {}: download must take off on the resolve's proxy",
        run.task_id
    );
    cleanup(run).await;
}

/// 站点规则：页面链接命中 custom 规则、媒体链接未命中 → 解析与下载都经规则代理
/// （解析产物视同重定向，沿用源链接出口）。全局为直连模式，若下载按媒体链接走全局
/// 出口，`.invalid` 直连必然失败。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unmatched_media_url_inherits_the_page_rule_egress() {
    let page_host = "page-rule.fluxdown-test.invalid";
    let proxy = FakeProxy::start("http://media-rule.fluxdown-test.invalid/file.bin").await;
    let rules = format!(
        r#"[{{"id":"r","match":"domain","pattern":"{page_host}","action":"custom","proxy":"{}"}}]"#,
        proxy.url()
    );
    let global = ProxyConfig {
        rules: Some(Arc::new(ProxyRuleSet::compile(
            &rules,
            &ProxyConfig::default(),
        ))),
        ..ProxyConfig::default()
    };
    let (run, row) = run_task("rule", page_host, global, "").await;
    assert_downloaded(&run, &row).await;
    assert_eq!(
        proxy.count("http://page-rule.fluxdown-test.invalid/watch"),
        1
    );
    assert!(proxy.count("http://media-rule.fluxdown-test.invalid/file.bin") >= 1);
    assert_eq!(row.auto_route, "rule:custom");
    cleanup(run).await;
}

/// 任务自带代理：插件解析的 `flux.fetch` 与下载都走任务代理（不看全局与规则）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn task_proxy_routes_the_resolve_and_the_download() {
    let page_host = "page-task.fluxdown-test.invalid";
    let proxy = FakeProxy::start("http://media-task.fluxdown-test.invalid/file.bin").await;
    let (run, row) = run_task("task", page_host, ProxyConfig::default(), &proxy.url()).await;
    assert_downloaded(&run, &row).await;
    assert_eq!(
        proxy.count("http://page-task.fluxdown-test.invalid/watch"),
        1
    );
    assert!(proxy.count("http://media-task.fluxdown-test.invalid/file.bin") >= 1);
    cleanup(run).await;
}
