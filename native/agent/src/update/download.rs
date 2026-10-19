//! 更新包的可续传下载与完整性校验：`<asset>.part` + Range 续传，服务端忽略 Range 时整体重下，
//! 完成后校验大小与 SHA-256 再原子改名为 `<asset>`。

use std::io::Read;
use std::path::{Path, PathBuf};

use fluxdown_protocol::UpdateFailure;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

#[derive(Debug, thiserror::Error)]
pub(super) enum DownloadError {
    #[error("download HTTP client unavailable: {0}")]
    Client(#[from] crate::http_client::HttpClientError),
    #[error("invalid download URL: {0}")]
    InvalidUrl(String),
    #[error("download request failed: {0}")]
    Network(#[from] reqwest::Error),
    #[error("download server returned status {0}")]
    Status(u16),
    #[error("download storage error at {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("downloaded size {actual} does not match expected {expected}")]
    SizeMismatch { expected: u64, actual: u64 },
    #[error("SHA-256 mismatch: expected {expected}, got {actual}")]
    ChecksumMismatch { expected: String, actual: String },
    #[error("download cancelled")]
    Cancelled,
}

impl DownloadError {
    pub(super) fn failure(&self) -> UpdateFailure {
        match self {
            Self::Network(_) | Self::Client(_) | Self::Status(_) => UpdateFailure::Network,
            Self::Io { .. } => UpdateFailure::Storage,
            Self::InvalidUrl(_) => UpdateFailure::Unknown,
            Self::SizeMismatch { .. } | Self::ChecksumMismatch { .. } => UpdateFailure::Verify,
            Self::Cancelled => UpdateFailure::Unknown,
        }
    }

    fn io(path: &Path) -> impl FnOnce(std::io::Error) -> Self + '_ {
        move |source| Self::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

pub(super) struct DownloadSpec<'a> {
    pub url: &'a str,
    /// 出口方案给出的附加请求头（插件改写地址才会有）。
    pub headers: &'a [(String, String)],
    /// 版本私有目录（`updates/<version>`）。
    pub dir: &'a Path,
    pub name: &'a str,
    /// 发布声明的大小；0 = 未知，跳过大小校验。
    pub size: u64,
    /// 小写十六进制 SHA-256。
    pub sha256: &'a str,
}

/// 下载并校验；成功返回最终文件路径。`progress` 以已落盘字节数回调。
/// 取消时保留 `.part` 供续传；校验失败会删除 `.part`。
pub(super) async fn download_verified(
    client: &reqwest::Client,
    spec: &DownloadSpec<'_>,
    cancel: &CancellationToken,
    mut progress: impl FnMut(u64) + Send,
) -> Result<PathBuf, DownloadError> {
    tokio::fs::create_dir_all(spec.dir)
        .await
        .map_err(DownloadError::io(spec.dir))?;
    let final_path = spec.dir.join(spec.name);
    let part_path = spec.dir.join(format!("{}.part", spec.name));

    // 已有完整且校验通过的包（重启后 / 重新检查）直接复用。
    if tokio::fs::try_exists(&final_path)
        .await
        .map_err(DownloadError::io(&final_path))?
    {
        match verify(&final_path, spec).await {
            Ok(()) => {
                progress(file_len(&final_path).await?);
                return Ok(final_path);
            }
            Err(error) => {
                tracing::debug!(error = %error, "cached update package invalid; downloading again");
                tokio::fs::remove_file(&final_path)
                    .await
                    .map_err(DownloadError::io(&final_path))?;
            }
        }
    }

    let mut restarted = false;
    loop {
        let mut existing = match tokio::fs::metadata(&part_path).await {
            Ok(meta) => meta.len(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(error) => return Err(DownloadError::io(&part_path)(error)),
        };
        if spec.size > 0 && existing > spec.size {
            remove_part(&part_path).await?;
            existing = 0;
        }
        if spec.size == 0 || existing < spec.size {
            match fetch_into_part(client, spec, &part_path, existing, cancel, &mut progress).await?
            {
                FetchOutcome::Done => {}
                FetchOutcome::RangeRejected if !restarted => {
                    restarted = true;
                    remove_part(&part_path).await?;
                    continue;
                }
                FetchOutcome::RangeRejected => return Err(DownloadError::Status(416)),
            }
        } else {
            progress(existing);
        }
        break;
    }

    if let Err(error) = verify(&part_path, spec).await {
        if matches!(
            error,
            DownloadError::SizeMismatch { .. } | DownloadError::ChecksumMismatch { .. }
        ) {
            remove_part(&part_path).await?;
        }
        return Err(error);
    }
    tokio::fs::rename(&part_path, &final_path)
        .await
        .map_err(DownloadError::io(&final_path))?;
    Ok(final_path)
}

enum FetchOutcome {
    Done,
    /// 本地 `.part` 与服务端不一致（416）：需丢弃后整体重下。
    RangeRejected,
}

async fn fetch_into_part(
    client: &reqwest::Client,
    spec: &DownloadSpec<'_>,
    part_path: &Path,
    existing: u64,
    cancel: &CancellationToken,
    progress: &mut (impl FnMut(u64) + Send),
) -> Result<FetchOutcome, DownloadError> {
    let mut request = client.get(spec.url);
    for (name, value) in spec.headers {
        request = request.header(name.as_str(), value.as_str());
    }
    if existing > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={existing}-"));
    }
    let mut response = tokio::select! {
        () = cancel.cancelled() => return Err(DownloadError::Cancelled),
        sent = request.send() => sent?,
    };
    let status = response.status();
    if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
        return Ok(FetchOutcome::RangeRejected);
    }
    let resumed = match status {
        reqwest::StatusCode::PARTIAL_CONTENT => {
            let starts_at = response
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.strip_prefix("bytes "))
                .and_then(|value| value.split('-').next())
                .and_then(|value| value.trim().parse::<u64>().ok());
            if starts_at != Some(existing) {
                return Ok(FetchOutcome::RangeRejected);
            }
            true
        }
        reqwest::StatusCode::OK => false,
        other => return Err(DownloadError::Status(other.as_u16())),
    };

    let mut options = tokio::fs::OpenOptions::new();
    options.create(true).write(true);
    if resumed {
        options.append(true);
    } else {
        // 服务端忽略 Range：从头覆盖。
        options.truncate(true);
    }
    let mut file = options
        .open(part_path)
        .await
        .map_err(DownloadError::io(part_path))?;
    let mut written = if resumed { existing } else { 0 };
    progress(written);
    loop {
        let chunk = tokio::select! {
            () = cancel.cancelled() => {
                file.flush().await.map_err(DownloadError::io(part_path))?;
                return Err(DownloadError::Cancelled);
            }
            chunk = response.chunk() => chunk?,
        };
        let Some(chunk) = chunk else { break };
        written += chunk.len() as u64;
        if spec.size > 0 && written > spec.size {
            return Err(DownloadError::SizeMismatch {
                expected: spec.size,
                actual: written,
            });
        }
        file.write_all(&chunk)
            .await
            .map_err(DownloadError::io(part_path))?;
        progress(written);
    }
    file.flush().await.map_err(DownloadError::io(part_path))?;
    file.sync_all()
        .await
        .map_err(DownloadError::io(part_path))?;
    Ok(FetchOutcome::Done)
}

async fn file_len(path: &Path) -> Result<u64, DownloadError> {
    tokio::fs::metadata(path)
        .await
        .map(|meta| meta.len())
        .map_err(DownloadError::io(path))
}

async fn remove_part(path: &Path) -> Result<(), DownloadError> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(DownloadError::io(path)(error)),
    }
}

/// 丢弃某个包的未完成 `.part`：换下载来源前调用，避免把一个来源的残片接到另一个来源上。
pub(super) async fn discard_partial(dir: &Path, name: &str) -> Result<(), DownloadError> {
    remove_part(&dir.join(format!("{name}.part"))).await
}

/// 大小 + SHA-256；哈希在阻塞线程里流式计算。
async fn verify(path: &Path, spec: &DownloadSpec<'_>) -> Result<(), DownloadError> {
    let actual_size = file_len(path).await?;
    if spec.size > 0 && actual_size != spec.size {
        return Err(DownloadError::SizeMismatch {
            expected: spec.size,
            actual: actual_size,
        });
    }
    let owned = path.to_path_buf();
    let digest = tokio::task::spawn_blocking(move || sha256_file(&owned))
        .await
        .map_err(|error| DownloadError::Io {
            path: path.to_path_buf(),
            source: std::io::Error::other(error),
        })?
        .map_err(DownloadError::io(path))?;
    if digest.eq_ignore_ascii_case(spec.sha256) {
        Ok(())
    } else {
        Err(DownloadError::ChecksumMismatch {
            expected: spec.sha256.to_ascii_lowercase(),
            actual: digest,
        })
    }
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 256 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::Router;
    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode, header};
    use axum::response::{IntoResponse, Response};
    use axum::routing::get;

    use super::*;

    struct Server {
        body: Vec<u8>,
        honor_range: bool,
        hits: AtomicUsize,
    }

    async fn serve(State(server): State<Arc<Server>>, headers: HeaderMap) -> Response {
        server.hits.fetch_add(1, Ordering::SeqCst);
        let range = headers
            .get(header::RANGE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("bytes="))
            .and_then(|value| value.strip_suffix('-'))
            .and_then(|value| value.parse::<usize>().ok());
        match range {
            Some(start) if server.honor_range => {
                let total = server.body.len();
                (
                    StatusCode::PARTIAL_CONTENT,
                    [(
                        header::CONTENT_RANGE,
                        format!("bytes {start}-{}/{total}", total - 1),
                    )],
                    server.body[start..].to_vec(),
                )
                    .into_response()
            }
            _ => server.body.clone().into_response(),
        }
    }

    async fn start(body: Vec<u8>, honor_range: bool) -> (String, Arc<Server>) {
        let server = Arc::new(Server {
            body,
            honor_range,
            hits: AtomicUsize::new(0),
        });
        let app = Router::new()
            .route("/pkg", get(serve))
            .with_state(server.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/pkg", listener.local_addr().unwrap());
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (url, server)
    }

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fluxdown-dl-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn body() -> Vec<u8> {
        (0..100_000_u32).map(|n| (n % 251) as u8).collect()
    }

    fn sha(bytes: &[u8]) -> String {
        hex::encode(Sha256::digest(bytes))
    }

    #[tokio::test]
    async fn downloads_and_verifies() {
        let data = body();
        let (url, _) = start(data.clone(), true).await;
        let dir = temp_dir();
        let digest = sha(&data);
        let spec = DownloadSpec {
            url: &url,
            headers: &[],
            dir: &dir,
            name: "pkg.bin",
            size: data.len() as u64,
            sha256: &digest,
        };
        let mut last = 0;
        let path = download_verified(
            &reqwest::Client::new(),
            &spec,
            &CancellationToken::new(),
            |n| {
                last = n;
            },
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), data);
        assert_eq!(last, data.len() as u64);
        assert!(!dir.join("pkg.bin.part").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn resumes_partial_download_with_range() {
        let data = body();
        let (url, _) = start(data.clone(), true).await;
        let dir = temp_dir();
        std::fs::write(dir.join("pkg.bin.part"), &data[..40_000]).unwrap();
        let digest = sha(&data);
        let spec = DownloadSpec {
            url: &url,
            headers: &[],
            dir: &dir,
            name: "pkg.bin",
            size: data.len() as u64,
            sha256: &digest,
        };
        let mut first = None;
        let path = download_verified(
            &reqwest::Client::new(),
            &spec,
            &CancellationToken::new(),
            |n| {
                first.get_or_insert(n);
            },
        )
        .await
        .unwrap();
        assert_eq!(first, Some(40_000));
        assert_eq!(std::fs::read(&path).unwrap(), data);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn restarts_when_server_ignores_range() {
        let data = body();
        let (url, _) = start(data.clone(), false).await;
        let dir = temp_dir();
        std::fs::write(dir.join("pkg.bin.part"), &data[..40_000]).unwrap();
        let digest = sha(&data);
        let spec = DownloadSpec {
            url: &url,
            headers: &[],
            dir: &dir,
            name: "pkg.bin",
            size: data.len() as u64,
            sha256: &digest,
        };
        let path = download_verified(
            &reqwest::Client::new(),
            &spec,
            &CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), data);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn checksum_mismatch_fails_verify_and_drops_part() {
        let data = body();
        let (url, _) = start(data.clone(), true).await;
        let dir = temp_dir();
        let wrong = "0".repeat(64);
        let spec = DownloadSpec {
            url: &url,
            headers: &[],
            dir: &dir,
            name: "pkg.bin",
            size: data.len() as u64,
            sha256: &wrong,
        };
        let error = download_verified(
            &reqwest::Client::new(),
            &spec,
            &CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap_err();
        assert_eq!(error.failure(), UpdateFailure::Verify);
        assert!(!dir.join("pkg.bin.part").exists());
        assert!(!dir.join("pkg.bin").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn verified_package_is_reused_without_network() {
        let data = body();
        let (url, server) = start(data.clone(), true).await;
        let dir = temp_dir();
        std::fs::write(dir.join("pkg.bin"), &data).unwrap();
        let digest = sha(&data);
        let spec = DownloadSpec {
            url: &url,
            headers: &[],
            dir: &dir,
            name: "pkg.bin",
            size: data.len() as u64,
            sha256: &digest,
        };
        download_verified(
            &reqwest::Client::new(),
            &spec,
            &CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap();
        assert_eq!(server.hits.load(Ordering::SeqCst), 0);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
