//! 本机区间适配器：只监听回环地址的 HTTP 服务，让 daemon 把分享当普通 HTTP 下载处理。
//!
//! 地址形如 `http://127.0.0.1:<port>/<capability-token>/<fileIndex>[/<name>]`：
//! - 令牌是每个接收会话一个的不可猜随机串，未知令牌一律 403；
//! - 只接受回环对端，并校验 `Host` 头，抵御 DNS 重绑定；
//! - 完整支持 `GET` / `HEAD` 与单区间 `Range`（206、`Content-Range`、`Accept-Ranges`、`If-Range`）；
//! - 响应字节按 4 MiB 块从分享方取回并经 BLAKE3 校验后才写给客户端，块级流水线预取。
//!
//! 错误映射：分享方离线 → 503 + `Retry-After`；源文件被改 → 409；分享已停止 / 过期 → 410；
//! 令牌错误 → 403；块反复校验失败或传输失败 → 502。所有错误响应带 `X-FluxShare-Status` 头便于诊断。

use std::collections::{HashMap, VecDeque};
use std::convert::Infallible;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, PoisonError, RwLock};

use anyhow::{Context, Result};
use bytes::Bytes;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt as _, Empty, Full, StreamBody};
use hyper::body::{Frame, Incoming};
use hyper::header::{
    ACCEPT_RANGES, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, ETAG, HOST,
    IF_RANGE, RANGE, RETRY_AFTER,
};
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_stream::StreamExt as _;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;

use crate::hashing::CHUNK_SIZE;
use crate::receiver::{FetchError, ReceiveSession};

/// 预取窗口：同时在途的块数。
const WINDOW: usize = 3;
/// 503 的 `Retry-After` 秒数。
const RETRY_AFTER_SECS: &str = "5";

type Body = BoxBody<Bytes, io::Error>;

struct Ctx {
    sessions: RwLock<HashMap<String, ReceiveSession>>,
}

/// 回环 HTTP 适配器。丢弃或 [`Self::shutdown`] 后停止监听。
pub struct LocalAdapter {
    ctx: Arc<Ctx>,
    addr: SocketAddr,
    cancel: CancellationToken,
    task: JoinHandle<()>,
    /// 丢弃适配器即取消监听。
    _guard: tokio_util::sync::DropGuard,
}

impl LocalAdapter {
    /// 在 `127.0.0.1:0` 上启动。无连接时只有一个阻塞在 `accept` 的任务，没有定时器。
    pub async fn start() -> Result<Self> {
        let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
            .await
            .context("bind loopback adapter")?;
        let addr = listener.local_addr().context("adapter local address")?;
        let ctx = Arc::new(Ctx {
            sessions: RwLock::new(HashMap::new()),
        });
        let cancel = CancellationToken::new();
        let task = tokio::spawn(accept_loop(listener, ctx.clone(), cancel.clone()));
        Ok(Self {
            ctx,
            addr,
            _guard: cancel.clone().drop_guard(),
            cancel,
            task,
        })
    }

    /// 监听地址。
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// 注册接收会话，返回其能力令牌。
    pub fn register(&self, session: ReceiveSession) -> String {
        let token = new_token();
        self.ctx
            .sessions
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(token.clone(), session);
        token
    }

    /// 注销令牌，之后该令牌的请求得到 403。
    pub fn unregister(&self, token: &str) -> bool {
        self.ctx
            .sessions
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(token)
            .is_some()
    }

    /// 文件的本机下载地址（交给 daemon 的任务 URL）。
    pub fn url(&self, token: &str, file_index: u32) -> String {
        format!("http://{}/{token}/{file_index}", self.addr)
    }

    /// 停止监听（已建立的连接随任务一起结束）。
    pub async fn shutdown(self) {
        self.cancel.cancel();
        if let Err(e) = self.task.await {
            tracing::debug!(error = %e, "adapter accept task ended abnormally");
        }
    }
}

/// 128 位随机能力令牌（hex）。随机源是 iroh 的密钥生成（系统 CSPRNG）。
fn new_token() -> String {
    let a = iroh::SecretKey::generate().to_bytes();
    crate::hashing::to_hex(a.get(..16).unwrap_or(&a))
}

async fn accept_loop(listener: TcpListener, ctx: Arc<Ctx>, cancel: CancellationToken) {
    let mut conns = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, peer)) => {
                    if !peer.ip().is_loopback() {
                        tracing::warn!(%peer, "rejecting non-loopback adapter peer");
                        continue;
                    }
                    let ctx = ctx.clone();
                    conns.spawn(async move {
                        let service = service_fn(move |req| {
                            let ctx = ctx.clone();
                            async move { Ok::<_, Infallible>(handle(req, ctx).await) }
                        });
                        let served = hyper::server::conn::http1::Builder::new()
                            .serve_connection(TokioIo::new(stream), service)
                            .await;
                        if let Err(e) = served {
                            tracing::debug!(error = %e, "adapter connection ended");
                        }
                    });
                }
                Err(e) => {
                    tracing::warn!(error = %e, "adapter accept failed");
                    break;
                }
            },
            // 回收已结束的连接任务。
            Some(_) = conns.join_next(), if !conns.is_empty() => {}
            () = cancel.cancelled() => break,
        }
    }
    conns.abort_all();
}

fn empty() -> Body {
    Empty::<Bytes>::new()
        .map_err(|never| match never {})
        .boxed()
}

fn full(text: &'static str) -> Body {
    Full::new(Bytes::from_static(text.as_bytes()))
        .map_err(|never| match never {})
        .boxed()
}

fn error_response(status: StatusCode, tag: &str, message: &'static str) -> Response<Body> {
    let mut builder = Response::builder()
        .status(status)
        .header("X-FluxShare-Status", tag)
        .header(CONTENT_TYPE, "text/plain; charset=utf-8");
    if status == StatusCode::SERVICE_UNAVAILABLE {
        builder = builder.header(RETRY_AFTER, RETRY_AFTER_SECS);
    }
    builder
        .body(full(message))
        .unwrap_or_else(|_| Response::new(empty()))
}

fn fetch_error_response(err: &FetchError) -> Response<Body> {
    match err {
        FetchError::Offline(_) => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "offline",
            "sender is offline",
        ),
        FetchError::Changed => {
            error_response(StatusCode::CONFLICT, "changed", "shared file was changed")
        }
        FetchError::Stopped => error_response(StatusCode::GONE, "stopped", "share was stopped"),
        FetchError::Expired => error_response(StatusCode::GONE, "expired", "share has expired"),
        FetchError::NotFound | FetchError::OutOfRange => {
            error_response(StatusCode::NOT_FOUND, "notFound", "no such file")
        }
        FetchError::Rejected(_) => {
            error_response(StatusCode::FORBIDDEN, "rejected", "ticket rejected")
        }
        FetchError::Corrupt(_) => error_response(
            StatusCode::BAD_GATEWAY,
            "corrupt",
            "chunk verification failed",
        ),
        FetchError::TrafficExhausted => error_response(
            StatusCode::TOO_MANY_REQUESTS,
            "trafficExhausted",
            "cloud traffic quota exhausted",
        ),
        FetchError::Transfer(_) => {
            error_response(StatusCode::BAD_GATEWAY, "transfer", "transfer failed")
        }
        FetchError::Closed => error_response(StatusCode::GONE, "closed", "receive session closed"),
    }
}

/// 解析后的请求区间（闭区间）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RangeSpec {
    /// 无 Range（或按 RFC 9110 忽略）：整个文件。
    Full,
    /// 满足的闭区间 `[start, end]`。
    Partial(u64, u64),
    /// 不可满足。
    Unsatisfiable,
}

/// 解析单区间 `Range: bytes=...`。多区间 / 非 `bytes` 单位 / 语法错误按规范忽略（返回整个文件）。
fn parse_range(header: Option<&str>, size: u64) -> RangeSpec {
    let Some(value) = header else {
        return RangeSpec::Full;
    };
    let Some(spec) = value.trim().strip_prefix("bytes=") else {
        return RangeSpec::Full;
    };
    if spec.contains(',') {
        return RangeSpec::Full;
    }
    let Some((first, last)) = spec.trim().split_once('-') else {
        return RangeSpec::Full;
    };
    let (first, last) = (first.trim(), last.trim());
    if first.is_empty() {
        // 后缀区间 `-N`：最后 N 字节。
        let Ok(n) = last.parse::<u64>() else {
            return RangeSpec::Full;
        };
        if n == 0 || size == 0 {
            return RangeSpec::Unsatisfiable;
        }
        return RangeSpec::Partial(size.saturating_sub(n), size - 1);
    }
    let Ok(start) = first.parse::<u64>() else {
        return RangeSpec::Full;
    };
    let end = if last.is_empty() {
        u64::MAX
    } else {
        match last.parse::<u64>() {
            Ok(e) => e,
            Err(_) => return RangeSpec::Full,
        }
    };
    if end < start {
        return RangeSpec::Full;
    }
    if start >= size {
        return RangeSpec::Unsatisfiable;
    }
    RangeSpec::Partial(start, end.min(size - 1))
}

fn host_allowed(host: &str) -> bool {
    let name = if let Some(rest) = host.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else {
        host.rsplit_once(':').map_or(host, |(h, _)| h)
    };
    if name.eq_ignore_ascii_case("localhost") {
        return true;
    }
    name.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// 百分号编码（RFC 5987 `filename*`）。
fn encode_filename(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

async fn handle(req: Request<Incoming>, ctx: Arc<Ctx>) -> Response<Body> {
    let head = req.method() == Method::HEAD;
    if !head && req.method() != Method::GET {
        return error_response(
            StatusCode::METHOD_NOT_ALLOWED,
            "method",
            "only GET and HEAD are supported",
        );
    }
    if let Some(host) = req.headers().get(HOST).and_then(|v| v.to_str().ok())
        && !host_allowed(host)
    {
        return error_response(StatusCode::FORBIDDEN, "host", "forbidden host");
    }
    let path = req.uri().path().to_owned();
    let mut parts = path.trim_start_matches('/').split('/');
    let token = parts.next().unwrap_or("");
    let session = ctx
        .sessions
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .get(token)
        .cloned();
    let Some(session) = session else {
        return error_response(
            StatusCode::FORBIDDEN,
            "badToken",
            "invalid capability token",
        );
    };
    let Some(file_index) = parts.next().and_then(|s| s.parse::<u32>().ok()) else {
        return error_response(StatusCode::NOT_FOUND, "notFound", "no such file");
    };
    let Some(file) = usize::try_from(file_index)
        .ok()
        .and_then(|i| session.manifest().files.get(i))
        .cloned()
    else {
        return error_response(StatusCode::NOT_FOUND, "notFound", "no such file");
    };

    let etag = format!("\"{}\"", file.root_hash);
    let range_header = req.headers().get(RANGE).and_then(|v| v.to_str().ok());
    // If-Range：校验器不是我们的 ETag 就忽略 Range，返回整个文件。
    let honor_range = req
        .headers()
        .get(IF_RANGE)
        .and_then(|v| v.to_str().ok())
        .is_none_or(|v| v.trim() == etag);
    let spec = if honor_range {
        parse_range(range_header, file.size)
    } else {
        RangeSpec::Full
    };

    let (status, start, end) = match spec {
        RangeSpec::Unsatisfiable => {
            return Response::builder()
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(CONTENT_RANGE, format!("bytes */{}", file.size))
                .header("X-FluxShare-Status", "range")
                .body(empty())
                .unwrap_or_else(|_| Response::new(empty()));
        }
        RangeSpec::Partial(s, e) => (StatusCode::PARTIAL_CONTENT, s, e),
        RangeSpec::Full => (StatusCode::OK, 0, file.size.saturating_sub(1)),
    };
    let length = if file.size == 0 { 0 } else { end - start + 1 };

    let mut builder = Response::builder()
        .status(status)
        .header(ACCEPT_RANGES, "bytes")
        .header(CONTENT_LENGTH, length)
        .header(CONTENT_TYPE, "application/octet-stream")
        .header(ETAG, &etag)
        .header(
            CONTENT_DISPOSITION,
            format!(
                "attachment; filename*=UTF-8''{}",
                encode_filename(&file.name)
            ),
        );
    if status == StatusCode::PARTIAL_CONTENT {
        builder = builder.header(CONTENT_RANGE, format!("bytes {start}-{end}/{}", file.size));
    }

    // 零字节文件与 HEAD：不取块，但仍探测分享方状态，让离线 / 已改 / 已停止在这里就暴露。
    if head || length == 0 {
        if let Err(e) = session.probe(file_index).await {
            return fetch_error_response(&e);
        }
        return builder
            .body(empty())
            .unwrap_or_else(|_| Response::new(empty()));
    }

    // 先取到第一块再决定状态码：分享方离线 / 文件已改必须在响应头发出前反映成 503 / 409。
    let mut chunks = stream_range(session, file_index, start, end);
    let first = match chunks.recv().await {
        Some(Ok(bytes)) => bytes,
        Some(Err(e)) => return fetch_error_response(&e),
        None => return error_response(StatusCode::BAD_GATEWAY, "transfer", "no data"),
    };
    let rest = ReceiverStream::new(chunks).map(|item| match item {
        Ok(bytes) => Ok(Frame::data(bytes)),
        Err(e) => Err(io::Error::other(e.to_string())),
    });
    let body = StreamBody::new(tokio_stream::once(Ok(Frame::data(first))).chain(rest)).boxed();
    builder
        .body(body)
        .unwrap_or_else(|_| Response::new(empty()))
}

/// 窗口内在途的块任务；析构时中止全部，客户端断开后不再白取数据。
struct Window(VecDeque<JoinHandle<Result<Bytes, FetchError>>>);

impl Drop for Window {
    fn drop(&mut self) {
        for handle in &self.0 {
            handle.abort();
        }
    }
}

/// 把 `[start, end]`（闭区间）切成块、流水线取回，按序产出已校验且裁剪好的字节。
fn stream_range(
    session: ReceiveSession,
    file_index: u32,
    start: u64,
    end: u64,
) -> mpsc::Receiver<Result<Bytes, FetchError>> {
    let (tx, rx) = mpsc::channel(2);
    tokio::spawn(async move {
        let first = start / CHUNK_SIZE;
        let last = end / CHUNK_SIZE;
        let mut next = first;
        let mut window = Window(VecDeque::new());
        let mut index = first;
        while index <= last {
            while next <= last && window.0.len() < WINDOW {
                let session = session.clone();
                let chunk = next;
                window.0.push_back(tokio::spawn(async move {
                    if chunk == first {
                        session.fetch_chunk_revalidated(file_index, chunk).await
                    } else {
                        session.fetch_chunk(file_index, chunk).await
                    }
                }));
                next += 1;
            }
            let Some(handle) = window.0.pop_front() else {
                break;
            };
            let fetched = match handle.await {
                Ok(result) => result,
                Err(e) => Err(FetchError::Transfer(format!("fetch task failed: {e}"))),
            };
            let item = fetched.and_then(|bytes| {
                let chunk_start = index * CHUNK_SIZE;
                let from = start.saturating_sub(chunk_start);
                let to = (end + 1 - chunk_start).min(bytes.len() as u64);
                let (from, to) = (usize::try_from(from).ok(), usize::try_from(to).ok());
                match (from, to) {
                    (Some(from), Some(to)) if from <= to && to <= bytes.len() => {
                        Ok(bytes.slice(from..to))
                    }
                    _ => Err(FetchError::OutOfRange),
                }
            });
            let failed = item.is_err();
            if tx.send(item).await.is_err() || failed {
                break;
            }
            index += 1;
        }
    });
    rx
}
