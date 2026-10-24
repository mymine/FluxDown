//! 换源续传回归：浏览器扩展 hint 任务下到一半后 `change_task_url`。
//!
//! - 换成**另一个**文件（只小 300 KiB，落在 CDN 漂移容差内）：必须真实探测新链接、
//!   丢弃旧进度从头下载，成品逐字节等于新文件（修复前：沿用 hint 跳过探测，旧大小
//!   被当作新大小，分段越过新文件 EOF 持续 416 卡死，或拼成新旧混合文件）。
//! - 换成**同一文件**的新链接（CDN 签名过期场景）：沿用进度续传，成品等于原文件。
//!
//! 运行：`cargo test -p fluxdown_engine --test engine change_url_resume -- --ignored`

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use fluxdown_engine::bt_config::BtConfig;
use fluxdown_engine::proxy_config::ProxyConfig;
use fluxdown_engine::{Engine, EngineConfig, NoopSelection, NoopSink};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const SIZE_A: usize = 8 * 1024 * 1024;
/// 新文件只小 300 KiB：< 1 MiB 漂移容差，修复前会被当成「同一文件」。
const SIZE_B: usize = SIZE_A - 300 * 1024;

fn gen_body(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed.max(1);
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        out.extend_from_slice(&x.wrapping_mul(0x2545_F491_4F6C_DD1D).to_le_bytes());
    }
    out.truncate(len);
    out
}

struct Request {
    method: String,
    path: String,
    range: Option<(usize, Option<usize>)>,
}

async fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut buf = Vec::with_capacity(1024);
    let mut tmp = [0u8; 1024];
    loop {
        let n = stream.read(&mut tmp).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    let text = String::from_utf8_lossy(&buf);
    let mut lines = text.split("\r\n");
    let mut first = lines.next()?.split_whitespace();
    let method = first.next()?.to_string();
    let path = first.next()?.to_string();
    let mut range = None;
    for line in lines {
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("range")
        {
            let (s, e) = value.trim().strip_prefix("bytes=")?.split_once('-')?;
            let end = if e.trim().is_empty() {
                None
            } else {
                Some(e.trim().parse().ok()?)
            };
            range = Some((s.trim().parse().ok()?, end));
        }
    }
    Some(Request {
        method,
        path,
        range,
    })
}

/// 按路径分发文件；慢速发送（16 KiB / 20 ms）留出暂停窗口。`zero_starts` 统计
/// 「从字节 0 开始」的数据请求数（用于判定是否从头重下）。
async fn serve(
    mut stream: TcpStream,
    files: Arc<HashMap<String, Arc<Vec<u8>>>>,
    zero_starts: Arc<AtomicUsize>,
) {
    let Some(req) = read_request(&mut stream).await else {
        return;
    };
    let Some(body) = files.get(&req.path) else {
        let _ignored = stream
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await;
        return;
    };
    let total = body.len();
    let common =
        "Accept-Ranges: bytes\r\nContent-Type: application/octet-stream\r\nConnection: close";
    if req.method.eq_ignore_ascii_case("HEAD") {
        let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {total}\r\n{common}\r\n\r\n");
        let _ignored = stream.write_all(head.as_bytes()).await;
        return;
    }
    let (start, end, status) = match req.range {
        Some((start, end)) => {
            let end = end.unwrap_or(total - 1).min(total - 1);
            if start > end {
                let head = format!(
                    "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{total}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
                let _ignored = stream.write_all(head.as_bytes()).await;
                return;
            }
            (start, end, "206 Partial Content")
        }
        None => (0, total - 1, "200 OK"),
    };
    let chunk = &body[start..=end];
    if start == 0 && chunk.len() > 1 {
        zero_starts.fetch_add(1, Ordering::SeqCst);
    }
    let range_header = if req.range.is_some() {
        format!("Content-Range: bytes {start}-{end}/{total}\r\n")
    } else {
        String::new()
    };
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\n{range_header}{common}\r\n\r\n",
        chunk.len()
    );
    if stream.write_all(head.as_bytes()).await.is_err() {
        return;
    }
    for piece in chunk.chunks(16 * 1024) {
        if stream.write_all(piece).await.is_err() {
            return;
        }
        if chunk.len() > 4096 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

/// 建 hint 任务下载 `/a.bin` → 暂停 → 换到 `new_path` → 恢复到完成，返回
/// （成品字节，恢复后从 0 起的数据请求数，换源前已下字节）。
async fn run(name: &str, new_path: &str) -> (Vec<u8>, usize, i64) {
    let work_dir = std::env::temp_dir().join(format!("fluxdown_cu_{name}_{}", std::process::id()));
    if let Err(error) = tokio::fs::remove_dir_all(&work_dir).await {
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    }
    tokio::fs::create_dir_all(&work_dir).await.unwrap();

    let body_a = Arc::new(gen_body(SIZE_A, 1));
    let mut files = HashMap::new();
    files.insert("/a.bin".to_string(), body_a.clone());
    files.insert("/a-resigned.bin".to_string(), body_a.clone());
    files.insert("/b.bin".to_string(), Arc::new(gen_body(SIZE_B, 2)));
    let files = Arc::new(files);
    let zero_starts = Arc::new(AtomicUsize::new(0));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    {
        let files = files.clone();
        let zero_starts = zero_starts.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(serve(stream, files.clone(), zero_starts.clone()));
            }
        });
    }

    let dir = work_dir.to_string_lossy().to_string();
    let config = EngineConfig {
        max_concurrent: 5,
        speed_limit_bps: 0,
        upload_limit_bps: 0,
        default_save_dir: dir.clone(),
        app_data_dir: dir.clone(),
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
    let mut done_rx = engine.manager.take_done_rx().expect("done receiver");

    // 浏览器扩展捕获：带大小 hint（续传时 manager 以库存大小作 hint 跳过探测）。
    let task_id = engine
        .manager
        .create_task(fluxdown_engine::download_manager::NewTaskSpec {
            url: format!("http://{addr}/a.bin"),
            save_dir: dir.clone(),
            file_name: "file.bin".to_string(),
            segments: 4,
            hint_file_size: SIZE_A as i64,
            ..Default::default()
        })
        .await
        .expect("create_task");

    let mut waited = 0;
    let downloaded_before = loop {
        let downloaded: i64 = engine
            .db
            .load_segments(&task_id)
            .await
            .unwrap_or_default()
            .iter()
            .map(|s| s.downloaded_bytes)
            .sum();
        if downloaded >= 512 * 1024 || waited >= 15_000 {
            break downloaded;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        waited += 100;
    };
    assert!(downloaded_before > 0, "[{name}] 换源前应已有进度");

    engine.manager.pause_task(&task_id).await;
    let done = tokio::time::timeout(Duration::from_secs(10), done_rx.recv())
        .await
        .expect("pause should stop the downloader")
        .expect("done channel open");
    engine.manager.on_task_done(&done).await;

    engine
        .manager
        .change_task_url(&task_id, &format!("http://{addr}{new_path}"))
        .await
        .expect("change url");
    zero_starts.store(0, Ordering::SeqCst);
    engine.manager.resume_task(&task_id).await;

    let done = tokio::time::timeout(Duration::from_secs(60), done_rx.recv())
        .await
        .expect("resumed download must finish instead of hanging")
        .expect("done channel open");
    engine.manager.on_task_done(&done).await;
    let task = engine.db.load_task_by_id(&task_id).await.unwrap().unwrap();
    assert_eq!(
        task.status, 3,
        "[{name}] 应完成，实际 error={}",
        task.error_message
    );

    let output = tokio::fs::read(work_dir.join("file.bin")).await.unwrap();
    let restarted = zero_starts.load(Ordering::SeqCst);
    drop(engine);
    if let Err(error) = tokio::fs::remove_dir_all(&work_dir).await {
        eprintln!("cleanup {name}: {error}");
    }
    (output, restarted, downloaded_before)
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "binds a local port; run with --ignored"]
async fn switching_to_a_different_file_restarts_from_zero() {
    let (output, restarted, _) = run("different", "/b.bin").await;
    assert_eq!(output.len(), SIZE_B, "成品大小应为新文件大小");
    assert!(
        output == gen_body(SIZE_B, 2),
        "成品必须逐字节等于新文件，不得拼接"
    );
    assert!(restarted > 0, "换成另一个文件后必须从字节 0 重新下载");
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "binds a local port; run with --ignored"]
async fn switching_to_a_resigned_link_of_the_same_file_keeps_progress() {
    let (output, restarted, before) = run("resigned", "/a-resigned.bin").await;
    assert!(output == gen_body(SIZE_A, 1), "成品必须等于原文件");
    assert!(before > 0);
    assert_eq!(restarted, 0, "同一文件换链应沿用进度，不从字节 0 重下");
}
