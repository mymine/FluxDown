//! 只收 POST 的下载端点（OneDrive `*.svc.ms/transform/zip` 打包下载，#970）：
//!
//! - 浏览器 POST 接管的任务按原 method + body 下载完成；
//! - 恢复 / 重试这类任务时请求体已不在（不落库），必须明确失败，且不得改发 HEAD / GET；
//! - 以 GET 建的任务撞上 `405 + Allow: OPTIONS, POST`，给出可读错误、不做无意义的重试。
//!
//! 运行：`cargo test -p fluxdown_engine --test engine post_only -- --ignored`

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fluxdown_engine::bt_config::BtConfig;
use fluxdown_engine::download_manager::NewTaskSpec;
use fluxdown_engine::downloader::{
    CapturedRequestBody, POST_ONLY_ENDPOINT_MESSAGE, POST_ONLY_RESUME_MESSAGE,
};
use fluxdown_engine::proxy_config::ProxyConfig;
use fluxdown_engine::{Engine, EngineConfig, NoopSelection, NoopSink};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const ZIP_BODY: &[u8] = b"PK\x03\x04 fake zip payload for the POST-only endpoint";
const FORM: &str = "zipFileName=OneDrive.zip&files=%7B%22items%22%3A%5B%5D%7D";

/// 读完整请求（头 + Content-Length 指定的 body），返回 (method, body)。
async fn read_request(stream: &mut TcpStream) -> Option<(String, String)> {
    let mut buf = Vec::with_capacity(1024);
    let mut tmp = [0u8; 1024];
    let header_end = loop {
        let n = stream.read(&mut tmp).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let method = head.split_whitespace().next()?.to_string();
    let content_length = head
        .split("\r\n")
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    while buf.len() < header_end + content_length {
        let n = stream.read(&mut tmp).await.ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    let body = String::from_utf8_lossy(&buf[header_end..]).to_string();
    Some((method, body))
}

/// 模拟 OneDrive 打包端点：POST 且带表单 → 200 zip；其余 → 405 `Allow: OPTIONS, POST`。
async fn serve(mut stream: TcpStream, log: Arc<Mutex<Vec<String>>>) {
    let Some((method, body)) = read_request(&mut stream).await else {
        return;
    };
    log.lock().unwrap().push(method.clone());
    let response = if method == "POST" && body == FORM {
        let mut head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/zip\r\n\
             Content-Disposition: attachment; filename=\"OneDrive.zip\"\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n",
            ZIP_BODY.len()
        )
        .into_bytes();
        head.extend_from_slice(ZIP_BODY);
        head
    } else {
        b"HTTP/1.1 405 Method Not Allowed\r\nAllow: OPTIONS, POST\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            .to_vec()
    };
    let _ignored = stream.write_all(&response).await;
}

struct Harness {
    engine: Engine,
    done_rx: tokio::sync::mpsc::Receiver<fluxdown_engine::download_manager::TaskDone>,
    addr: SocketAddr,
    log: Arc<Mutex<Vec<String>>>,
    work_dir: std::path::PathBuf,
}

impl Harness {
    async fn new(name: &str) -> Self {
        let work_dir =
            std::env::temp_dir().join(format!("fluxdown_post_only_{name}_{}", std::process::id()));
        if let Err(error) = tokio::fs::remove_dir_all(&work_dir).await {
            assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
        }
        tokio::fs::create_dir_all(&work_dir).await.unwrap();

        let log = Arc::new(Mutex::new(Vec::new()));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        {
            let log = log.clone();
            tokio::spawn(async move {
                while let Ok((stream, _)) = listener.accept().await {
                    tokio::spawn(serve(stream, log.clone()));
                }
            });
        }

        let dir = work_dir.to_string_lossy().to_string();
        let config = EngineConfig {
            max_concurrent: 5,
            speed_limit_bps: 0,
            upload_limit_bps: 0,
            default_save_dir: dir.clone(),
            app_data_dir: dir,
            bt_config: BtConfig::default(),
            proxy_config: ProxyConfig::default(),
            user_agent: String::new(),
            data_dir_override: Some(work_dir.clone()),
            database_url: None,
        };
        let mut engine = Engine::new(config, Arc::new(NoopSink), Arc::new(NoopSelection))
            .await
            .expect("engine");
        if let Some(mut rx) = engine.manager.take_progress_rx() {
            tokio::spawn(async move { while rx.recv().await.is_some() {} });
        }
        let done_rx = engine.manager.take_done_rx().expect("done receiver");
        Self {
            engine,
            done_rx,
            addr,
            log,
            work_dir,
        }
    }

    fn spec(&self, method: Option<&str>, start_paused: bool) -> NewTaskSpec {
        NewTaskSpec {
            url: format!("http://{}/transform/zip?cs=abc", self.addr),
            save_dir: self.work_dir.to_string_lossy().to_string(),
            file_name: "OneDrive.zip".to_string(),
            segments: 4,
            method: method.map(str::to_string),
            body: method.map(|_| CapturedRequestBody::Urlencoded {
                raw: FORM.to_string(),
            }),
            start_paused,
            ..Default::default()
        }
    }

    async fn finish(&mut self, task_id: &str) -> fluxdown_engine::model::TaskInfo {
        let done = tokio::time::timeout(Duration::from_secs(30), self.done_rx.recv())
            .await
            .expect("download should settle")
            .expect("done channel open");
        self.engine.manager.on_task_done(&done).await;
        self.engine
            .db
            .load_task_by_id(task_id)
            .await
            .unwrap()
            .unwrap()
    }

    fn methods(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }

    async fn cleanup(self) {
        let work_dir = self.work_dir.clone();
        drop(self);
        if let Err(error) = tokio::fs::remove_dir_all(&work_dir).await {
            eprintln!("cleanup {}: {error}", work_dir.display());
        }
    }
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "binds a local port; run with --ignored"]
async fn captured_post_downloads_with_original_method_and_body() {
    let mut h = Harness::new("captured").await;
    let spec = h.spec(Some("POST"), false);
    let task_id = h.engine.manager.create_task(spec).await.expect("create");
    let task = h.finish(&task_id).await;
    assert_eq!(task.status, 3, "error={}", task.error_message);
    let output = tokio::fs::read(h.work_dir.join(&task.file_name))
        .await
        .unwrap();
    assert_eq!(output, ZIP_BODY);
    assert!(
        h.methods().iter().all(|m| m == "POST"),
        "POST task must never probe with HEAD/GET: {:?}",
        h.methods()
    );
    h.cleanup().await;
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "binds a local port; run with --ignored"]
async fn resuming_a_post_task_fails_clearly_without_sending_get() {
    let mut h = Harness::new("resume").await;
    let spec = h.spec(Some("post"), true);
    let task_id = h.engine.manager.create_task(spec).await.expect("create");
    // 稍后下载会在后台按原请求探测一次元数据；等它结束再清空请求日志。
    tokio::time::sleep(Duration::from_millis(500)).await;
    h.log.lock().unwrap().clear();

    h.engine.manager.resume_task(&task_id).await;
    let task = h
        .engine
        .db
        .load_task_by_id(&task_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(task.status, 4);
    assert_eq!(task.error_message, POST_ONLY_RESUME_MESSAGE);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        h.methods().is_empty(),
        "resume must not replay the request as GET/HEAD: {:?}",
        h.methods()
    );
    h.cleanup().await;
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "binds a local port; run with --ignored"]
async fn get_task_on_post_only_endpoint_reports_readable_error() {
    let mut h = Harness::new("get").await;
    let spec = h.spec(None, false);
    let task_id = h.engine.manager.create_task(spec).await.expect("create");
    let task = h.finish(&task_id).await;
    assert_eq!(task.status, 4);
    assert_eq!(task.error_message, POST_ONLY_ENDPOINT_MESSAGE);
    // 一轮探测（HEAD ∥ ranged GET，必要时 plain GET）即停，不按 405 反复重试。
    let methods = h.methods();
    assert!(
        methods.len() <= 3 && methods.iter().all(|m| m != "POST"),
        "expected a single probe round: {methods:?}"
    );
    h.cleanup().await;
}
