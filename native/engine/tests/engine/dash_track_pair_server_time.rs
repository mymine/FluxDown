//! 音视频轨对「使用服务器文件时间」端到端测试。
//!
//! ffmpeg 指向不存在的路径 → mux 必然失败 → 视频 / 音频保留为两个独立文件，
//! 两者的修改时间必须各自等于本轨探测响应的 `Last-Modified`；关闭开关时保持本地时间。
//!
//! 用法（绑定本地端口，默认 ignore）：
//!   cargo test -p fluxdown_engine --test engine dash_track_pair_server_time -- --ignored
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fluxdown_engine::db::Db;
use fluxdown_engine::downloader::{DownloadParams, ProgressUpdate, RequestSpec, build_client};
use fluxdown_engine::events::{EngineEvent, EventSink};
use fluxdown_engine::proxy_config::ProxyConfig;
use fluxdown_engine::speed_limiter::SpeedLimiter;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// 视频轨 `Last-Modified`：2024-06-07 08:09:10 UTC。
const VIDEO_LM: &str = "Fri, 07 Jun 2024 08:09:10 GMT";
const VIDEO_SECS: u64 = 1_717_747_750;
/// 音频轨 `Last-Modified`：2025-01-02 03:04:05 UTC。
const AUDIO_LM: &str = "Thu, 02 Jan 2025 03:04:05 GMT";
const AUDIO_SECS: u64 = 1_735_787_045;

struct NoopTestSink;
impl EventSink for NoopTestSink {
    fn emit(&self, _event: EngineEvent) {}
}

/// `bytes=0-0` → 206 + Content-Range + Last-Modified；其余请求 → 200 全量（同样带
/// Last-Modified）。`/audio` 与 `/video` 各自返回不同的 body 与时间。
async fn start_server(video: Arc<Vec<u8>>, audio: Arc<Vec<u8>>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let (video, audio) = (video.clone(), audio.clone());
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let Ok(n) = stream.read(&mut buf).await else {
                    return;
                };
                let head = String::from_utf8_lossy(&buf[..n]).to_ascii_lowercase();
                let (body, last_modified) = if head.contains("/audio") {
                    (&audio, AUDIO_LM)
                } else {
                    (&video, VIDEO_LM)
                };
                let total = body.len();
                let (header, payload) = if head.contains("range: bytes=0-0") {
                    (
                        format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Length: 1\r\nContent-Range: bytes 0-0/{total}\r\nAccept-Ranges: bytes\r\nLast-Modified: {last_modified}\r\nConnection: close\r\n\r\n"
                        ),
                        &body[..1],
                    )
                } else {
                    (
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {total}\r\nLast-Modified: {last_modified}\r\nConnection: close\r\n\r\n"
                        ),
                        &body[..],
                    )
                };
                // 客户端提前断开属正常（探测只读响应头），写失败只记录并结束本连接。
                let written = match stream.write_all(header.as_bytes()).await {
                    Ok(()) => stream.write_all(payload).await,
                    Err(error) => Err(error),
                };
                if let Err(error) = written.and(stream.flush().await) {
                    eprintln!("test server connection ended early: {error}");
                }
            });
        }
    });
    format!("http://{addr}")
}

async fn run_pair(use_server_time: bool, tag: &str) -> (PathBuf, PathBuf, PathBuf) {
    let video: Arc<Vec<u8>> = Arc::new((0..200_000u32).map(|i| (i % 251) as u8).collect());
    let audio: Arc<Vec<u8>> = Arc::new((0..50_000u32).map(|i| (i % 241) as u8).collect());
    let base = start_server(video.clone(), audio.clone()).await;

    let work_dir = std::env::temp_dir().join(format!(
        "fluxdown-rt-trackpair-mtime-{tag}-{}",
        std::process::id()
    ));
    if let Err(error) = tokio::fs::remove_dir_all(&work_dir).await {
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound, "{error}");
    }
    tokio::fs::create_dir_all(&work_dir).await.unwrap();
    let db = Db::open(&work_dir).await.expect("db");
    db.insert_task(
        "tp",
        &format!("{base}/video"),
        "pair.mp4",
        &work_dir.to_string_lossy(),
        1,
        0,
        "",
        "",
        "",
        0,
    )
    .await
    .expect("insert_task");

    let (tx, mut rx) = mpsc::channel::<ProgressUpdate>(1024);
    let collector = tokio::spawn(async move {
        let mut last_status = 0;
        while let Some(update) = rx.recv().await {
            last_status = update.status;
        }
        last_status
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
        task_id: "tp".to_string(),
        url: format!("{base}/video"),
        save_dir: work_dir.to_string_lossy().to_string(),
        file_name: "pair.mp4".to_string(),
        segment_count: 1,
        is_resume: false,
        range_verified: true,
        db: db.clone(),
        client: build_client(&ProxyConfig::default(), "FluxDownRealTest/1.0").expect("client"),
        progress_tx: tx,
        cancel_token: CancellationToken::new(),
        sink: Arc::new(NoopTestSink),
        cookies: String::new(),
        referrer: String::new(),
        speed_limiter: SpeedLimiter::new(0),
        hint_file_size: 0,
        proxy_config: ProxyConfig::default(),
        selector: Arc::new(fluxdown_engine::NoopSelection),
        checksum: String::new(),
        extra_headers: std::collections::HashMap::new(),
        spec: RequestSpec::empty_get(),
        audio_url: Some(format!("{base}/audio")),
        auto_max_connections: 0,
        use_server_time,
        overwrite: fluxdown_engine::file_exists::OverwritePolicy::Never,
        // 不存在的 ffmpeg：mux 确定性失败，保留两个独立文件。
        ffmpeg_path: Some(work_dir.join("missing-ffmpeg")),
        cdn: fluxdown_engine::cdn::CdnTaskInput::default(),
    };

    fluxdown_engine::dash_downloader::run_dash_download(params).await;
    let last_status = collector.await.expect("collector");
    assert_eq!(last_status, 3, "track pair must complete");

    let video_out = work_dir.join("pair.mp4");
    let audio_out = work_dir.join("pair.audio.m4a");
    assert_eq!(tokio::fs::read(&video_out).await.unwrap(), *video);
    assert_eq!(tokio::fs::read(&audio_out).await.unwrap(), *audio);
    (work_dir, video_out, audio_out)
}

fn mtime(path: &Path) -> SystemTime {
    std::fs::metadata(path).unwrap().modified().unwrap()
}

async fn cleanup(dir: &Path) {
    if let Err(error) = tokio::fs::remove_dir_all(dir).await
        && error.kind() != std::io::ErrorKind::NotFound
    {
        eprintln!("best-effort test directory cleanup: {error}");
    }
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "binds a local port; run with --ignored"]
async fn separate_tracks_take_their_own_server_time() {
    let (dir, video_out, audio_out) = run_pair(true, "on").await;
    assert_eq!(
        mtime(&video_out),
        UNIX_EPOCH + Duration::from_secs(VIDEO_SECS)
    );
    assert_eq!(
        mtime(&audio_out),
        UNIX_EPOCH + Duration::from_secs(AUDIO_SECS)
    );
    cleanup(&dir).await;
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "binds a local port; run with --ignored"]
async fn disabled_server_time_keeps_local_completion_time() {
    let before = SystemTime::now() - Duration::from_secs(5);
    let (dir, video_out, audio_out) = run_pair(false, "off").await;
    assert!(mtime(&video_out) >= before);
    assert!(mtime(&audio_out) >= before);
    cleanup(&dir).await;
}
