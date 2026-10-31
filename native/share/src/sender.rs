//! 分享方：分享登记表与 `fluxdown/share/1` 协议处理器。
//!
//! 宿主维护 [`ShareRegistry`]（有哪些分享、文件在本机哪里、是否仍可用），本模块只在请求到达时
//! 读取它：验票 → 查分享 → 逐请求在阻塞线程读文件区间 → 限速写回，并为每条连接维护一个会话。

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use bytes::Bytes;
use iroh::EndpointId;
use iroh::endpoint::{Connection, ConnectionError, RecvStream, SendStream};
use iroh::protocol::{AcceptError, ProtocolHandler};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;

use crate::hashing::file_mtime_ms;
use crate::limiter::RateLimiter;
use crate::observe::{Observers, SessionEvent};
use crate::session::{MonitorConfig, SessionMonitor, close_error};
use crate::swarm::content::ContentRegistry;
use crate::swarm::serve::{SwarmHandler, SwarmServer};
use crate::ticket::{
    KeyRing, SCOPE_BRIDGE, SCOPE_RECV, TicketError, TicketExpectation, parse_ticket, verify_ticket,
};
use crate::timefmt::{now_unix_ms, now_unix_secs};
use crate::types::{PathKind, SessionRole, ShareManifest};
use crate::wire::{
    Header, Hello, HelloReply, MAX_REQUEST_LEN, PROTOCOL_VERSION, Request, Status, read_frame,
    write_frame,
};

/// 握手（Hello）阶段的超时。
const HELLO_TIMEOUT: Duration = Duration::from_secs(15);
/// 发送回复后等待对端确认收到的上限。
const REPLY_FLUSH_TIMEOUT: Duration = Duration::from_secs(5);
/// 每次读文件 / 写流的块大小。
const PUMP_CHUNK: usize = 256 * 1024;
/// 读线程与写协程之间的预读深度（块数）。
const PUMP_DEPTH: usize = 4;

/// QUIC 应用层关闭码：握手被拒绝。
const CLOSE_REJECTED: u32 = 0x10;
/// QUIC 应用层关闭码：会话被宿主 / 后台强制中断。
pub(crate) const CLOSE_KILLED: u32 = 0x11;
/// 数据流复位码：发送方读文件失败。
const RESET_INTERNAL: u32 = 0x20;

/// 分享的可用状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareState {
    /// 可接收。
    Active,
    /// 已被分享方停止或后台下架。
    Stopped,
    /// 已过期。
    Expired,
}

/// 宿主登记的一个分享。
#[derive(Debug, Clone)]
pub struct ShareEntry {
    /// 清单（`files[i]` 对应 `paths[i]`）。
    pub manifest: ShareManifest,
    /// 每个文件在本机的路径，与清单文件一一对应。
    pub paths: Vec<PathBuf>,
    /// 初始状态。
    pub state: ShareState,
    /// 到期时间（Unix 秒），`None` 表示由宿主通过 [`ShareRegistry::set_state`] 控制。
    pub expires_at: Option<u64>,
    /// 本分享的发送限速（字节 / 秒），`None` / `0` 不限。
    pub rate_limit_bytes_per_sec: Option<u64>,
}

struct Registered {
    manifest: ShareManifest,
    paths: Vec<PathBuf>,
    status: Mutex<(ShareState, Option<u64>)>,
    limiter: RateLimiter,
}

impl Registered {
    /// 以 `Status` 表示当前可用性：`Ok` / `Stopped` / `Expired`。
    fn availability(&self, now_secs: u64) -> Status {
        let (state, expires_at) = *self.status.lock().unwrap_or_else(PoisonError::into_inner);
        match state {
            ShareState::Stopped => Status::Stopped,
            ShareState::Expired => Status::Expired,
            ShareState::Active => {
                if expires_at.is_some_and(|t| now_secs >= t) {
                    Status::Expired
                } else {
                    Status::Ok
                }
            }
        }
    }
}

/// 分享登记表。可克隆，克隆体共享内容；宿主随时增删改，发送方在每个请求上读取最新状态。
#[derive(Clone, Default)]
pub struct ShareRegistry {
    inner: Arc<RwLock<HashMap<String, Arc<Registered>>>>,
}

impl std::fmt::Debug for ShareRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShareRegistry")
            .field("shares", &self.read().len())
            .finish()
    }
}

impl ShareRegistry {
    /// 空登记表。
    pub fn new() -> Self {
        Self::default()
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, HashMap<String, Arc<Registered>>> {
        self.inner.read().unwrap_or_else(PoisonError::into_inner)
    }

    /// 登记（或整体替换）一个分享。
    pub fn insert(&self, entry: ShareEntry) -> Result<()> {
        ensure!(
            entry.paths.len() == entry.manifest.files.len(),
            "share {} has {} files but {} paths",
            entry.manifest.share_id,
            entry.manifest.files.len(),
            entry.paths.len()
        );
        let id = entry.manifest.share_id.clone();
        let registered = Registered {
            manifest: entry.manifest,
            paths: entry.paths,
            status: Mutex::new((entry.state, entry.expires_at)),
            limiter: RateLimiter::new(entry.rate_limit_bytes_per_sec.unwrap_or(0)),
        };
        self.inner
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, Arc::new(registered));
        Ok(())
    }

    /// 移除分享。之后对它的请求得到 `notFound`（宿主想表达“已停止”应改用 [`Self::set_state`]）。
    pub fn remove(&self, share_id: &str) -> bool {
        self.inner
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(share_id)
            .is_some()
    }

    /// 改变分享状态；分享不存在返回 `false`。
    pub fn set_state(&self, share_id: &str, state: ShareState) -> bool {
        self.update(share_id, |r| {
            r.status.lock().unwrap_or_else(PoisonError::into_inner).0 = state;
        })
    }

    /// 改变到期时间（Unix 秒）；分享不存在返回 `false`。
    pub fn set_expiry(&self, share_id: &str, expires_at: Option<u64>) -> bool {
        self.update(share_id, |r| {
            r.status.lock().unwrap_or_else(PoisonError::into_inner).1 = expires_at;
        })
    }

    /// 改变分享限速；分享不存在返回 `false`。
    pub fn set_rate_limit(&self, share_id: &str, bytes_per_sec: Option<u64>) -> bool {
        self.update(share_id, |r| r.limiter.set_rate(bytes_per_sec.unwrap_or(0)))
    }

    fn update(&self, share_id: &str, f: impl FnOnce(&Registered)) -> bool {
        match self.read().get(share_id) {
            Some(r) => {
                f(r);
                true
            }
            None => false,
        }
    }

    /// 是否登记了该分享。
    pub fn contains(&self, share_id: &str) -> bool {
        self.read().contains_key(share_id)
    }

    /// 当前仍可接收的分享数（状态 `Active` 且未到期）。宿主据此判断端点能否空闲关闭。
    pub fn active_count(&self) -> usize {
        let now = now_unix_secs();
        self.read()
            .values()
            .filter(|r| r.availability(now) == Status::Ok)
            .count()
    }

    fn get(&self, share_id: &str) -> Option<Arc<Registered>> {
        self.read().get(share_id).cloned()
    }
}

/// 一条正在进行的发送会话（对应一个接收方连接）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SenderSessionInfo {
    /// 会话 ID（与 [`SessionEvent`] 里的一致）。
    pub session_id: String,
    /// 分享 ID。
    pub share_id: String,
    /// 接收方 endpoint ID（z32）。
    pub peer_endpoint_id: String,
    /// 当前路径类别。
    pub path_kind: PathKind,
}

struct LiveSession {
    conn: Connection,
    share_id: String,
    peer: String,
    killed: Arc<AtomicBool>,
    kind: Arc<watch::Sender<PathKind>>,
}

struct SenderInner {
    registry: ShareRegistry,
    keys: KeyRing,
    global_limiter: RateLimiter,
    observers: Observers,
    report_interval: Duration,
    local_id: EndpointId,
    counter: AtomicU64,
    sessions: Mutex<HashMap<String, LiveSession>>,
    swarm: SwarmServer,
}

/// 发送方：协议处理器的宿主侧句柄。可克隆。
#[derive(Clone)]
pub struct ShareSender {
    inner: Arc<SenderInner>,
}

impl ShareSender {
    pub(crate) fn new(
        registry: ShareRegistry,
        keys: KeyRing,
        observers: Observers,
        report_interval: Duration,
        global_rate_bytes_per_sec: u64,
        local_id: EndpointId,
        content: ContentRegistry,
    ) -> Self {
        let global_limiter = RateLimiter::new(global_rate_bytes_per_sec);
        Self {
            inner: Arc::new(SenderInner {
                registry,
                swarm: SwarmServer::new(
                    content,
                    keys.clone(),
                    global_limiter.clone(),
                    observers.clone(),
                    report_interval,
                    local_id,
                ),
                keys,
                global_limiter,
                observers,
                report_interval,
                local_id,
                counter: AtomicU64::new(0),
                sessions: Mutex::new(HashMap::new()),
            }),
        }
    }

    pub(crate) fn swarm_handler(&self) -> SwarmHandler {
        self.inner.swarm.handler()
    }

    pub(crate) fn set_swarm_policy(&self, priority_reserved: usize, daily_cap: u64, offset: i64) {
        self.inner.swarm.set_priority_reserved(priority_reserved);
        self.inner.swarm.set_daily_cap(daily_cap, offset);
    }

    pub(crate) fn swarm_daily_usage(&self) -> (i64, u64) {
        self.inner.swarm.daily_usage()
    }

    pub(crate) fn restore_swarm_daily(&self, day: i64, used: u64) {
        self.inner.swarm.restore_daily(day, used);
    }

    pub(crate) fn handler(&self) -> ShareHandler {
        ShareHandler {
            sender: self.clone(),
        }
    }

    /// 分享登记表。
    pub fn registry(&self) -> &ShareRegistry {
        &self.inner.registry
    }

    /// 调整全局发送限速（字节 / 秒，0 = 不限），立即生效。
    pub fn set_global_rate_limit(&self, bytes_per_sec: u64) {
        self.inner.global_limiter.set_rate(bytes_per_sec);
    }

    /// 当前进行中的发送会话。
    pub fn sessions(&self) -> Vec<SenderSessionInfo> {
        let mut out: Vec<_> = self
            .inner
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|(id, s)| SenderSessionInfo {
                session_id: id.clone(),
                share_id: s.share_id.clone(),
                peer_endpoint_id: s.peer.clone(),
                path_kind: *s.kind.borrow(),
            })
            .collect();
        out.sort_by(|a, b| a.session_id.cmp(&b.session_id));
        out
    }

    /// 强制中断一个会话（后台 `share.sessionKill`、分享方手动断开），含共享提供方会话。
    /// 会话不存在返回 `false`。
    pub fn kill_session(&self, session_id: &str) -> bool {
        {
            let sessions = self
                .inner
                .sessions
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if let Some(s) = sessions.get(session_id) {
                s.killed.store(true, Ordering::SeqCst);
                s.conn.close(CLOSE_KILLED.into(), b"killed");
                return true;
            }
        }
        self.inner.swarm.kill_session(session_id)
    }

    /// 中断某个分享的全部会话（分享被停止 / 下架时）。返回被中断的会话数。
    pub fn kill_share_sessions(&self, share_id: &str) -> usize {
        let ids: Vec<String> = self
            .inner
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|(_, s)| s.share_id == share_id)
            .map(|(id, _)| id.clone())
            .collect();
        ids.iter().filter(|id| self.kill_session(id)).count()
    }

    fn new_session_id(&self) -> String {
        format!(
            "{}-{:x}-{:x}",
            self.inner.local_id.fmt_short(),
            now_unix_ms(),
            self.inner.counter.fetch_add(1, Ordering::Relaxed)
        )
    }
}

/// iroh 协议处理器（由 [`crate::ShareEndpoint`] 注册到 Router）。
#[derive(Clone)]
pub(crate) struct ShareHandler {
    sender: ShareSender,
}

impl std::fmt::Debug for ShareHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ShareHandler")
    }
}

impl ProtocolHandler for ShareHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        self.sender.serve(connection).await;
        Ok(())
    }
}

/// 通过 Hello 校验后的连接上下文。
#[derive(Clone)]
struct Authorized {
    share: Arc<Registered>,
    share_id: String,
    /// 票据过期时间（Unix 秒）。
    ticket_exp: u64,
}

/// 会话收尾守卫：析构时注销会话并发出终态报告（只生效一次）。
struct EndGuard {
    sender: ShareSender,
    session_id: String,
    monitor: SessionMonitor,
    killed: Arc<AtomicBool>,
    /// 连接结束原因；任务被中止（端点关闭）时为 `None`。
    ended: Option<ConnectionError>,
}

impl Drop for EndGuard {
    fn drop(&mut self) {
        self.sender
            .inner
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.session_id);
        let error = if self.killed.load(Ordering::SeqCst) {
            Some("session killed".to_owned())
        } else {
            match &self.ended {
                Some(reason) => close_error(reason),
                None => Some("share endpoint shut down".to_owned()),
            }
        };
        self.monitor.finish(error);
    }
}

enum Prepared {
    Reject(Header),
    Serve {
        header: Header,
        file: File,
        offset: u64,
        len: u64,
    },
}

fn reject(status: Status) -> Header {
    Header {
        status,
        file_size: 0,
        root_hash: String::new(),
        len: 0,
    }
}

impl ShareSender {
    async fn serve(&self, conn: Connection) {
        let peer = conn.remote_id();
        let auth = match self.handshake(&conn, peer).await {
            Ok(auth) => auth,
            Err(e) => {
                tracing::debug!(peer = %peer.fmt_short(), error = %format!("{e:#}"), "share handshake rejected");
                conn.close(CLOSE_REJECTED.into(), b"rejected");
                return;
            }
        };

        let session_id = self.new_session_id();
        let kind = Arc::new(watch::channel(PathKind::Unknown).0);
        let monitor = SessionMonitor::spawn(
            conn.clone(),
            MonitorConfig {
                session_id: session_id.clone(),
                share_id: auth.share_id.clone(),
                role: SessionRole::Sender,
                peer_endpoint_id: peer.to_z32(),
                connect_ms: 0,
                report_interval: self.inner.report_interval,
                observers: self.inner.observers.clone(),
                kind_tx: kind.clone(),
                sources: None,
            },
        );
        let killed = Arc::new(AtomicBool::new(false));
        self.inner
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                session_id.clone(),
                LiveSession {
                    conn: conn.clone(),
                    share_id: auth.share_id.clone(),
                    peer: peer.to_z32(),
                    killed: killed.clone(),
                    kind,
                },
            );
        self.inner.observers.emit(SessionEvent::Opened {
            session_id: session_id.clone(),
            share_id: auth.share_id.clone(),
            role: SessionRole::Sender,
            peer_endpoint_id: peer.to_z32(),
        });

        // 守卫保证：无论循环正常结束还是 Router 关闭时任务被中止，会话都会收尾并发出终态报告。
        let mut end = EndGuard {
            sender: self.clone(),
            session_id: session_id.clone(),
            monitor: monitor.clone(),
            killed,
            ended: None,
        };
        let mut streams: JoinSet<()> = JoinSet::new();
        end.ended = Some(loop {
            tokio::select! {
                accepted = conn.accept_bi() => match accepted {
                    Ok((send, recv)) => {
                        let this = self.clone();
                        let auth = auth.clone();
                        let monitor = monitor.clone();
                        streams.spawn(async move {
                            if let Err(e) = this.serve_request(&auth, &monitor, send, recv).await {
                                tracing::debug!(error = %format!("{e:#}"), "share request failed");
                            }
                        });
                    }
                    Err(e) => break e,
                },
                done = streams.join_next(), if !streams.is_empty() => {
                    if let Some(Err(e)) = done {
                        tracing::warn!(error = %e, "share request task panicked");
                    }
                }
            }
        });
        streams.abort_all();
    }

    /// Hello 握手：读 Hello、验票、查分享、回复；失败时先把拒绝原因送达再关闭连接。
    async fn handshake(&self, conn: &Connection, peer: EndpointId) -> Result<Authorized> {
        let (mut send, mut recv) = tokio::time::timeout(HELLO_TIMEOUT, conn.accept_bi())
            .await
            .context("timed out waiting for the hello stream")?
            .context("connection closed before hello")?;
        let hello: Hello = tokio::time::timeout(HELLO_TIMEOUT, read_frame(&mut recv))
            .await
            .context("timed out reading hello")??;

        let verdict = self.authorize(&hello, peer);
        let reply = match &verdict {
            Ok(_) => HelloReply {
                status: Status::Ok,
                message: String::new(),
            },
            Err((status, message)) => HelloReply {
                status: *status,
                message: message.clone(),
            },
        };
        write_frame(&mut send, &reply).await?;
        send.finish().context("finish hello reply")?;
        match verdict {
            Ok(auth) => Ok(auth),
            Err((status, message)) => {
                // 让拒绝原因先送达对端，再由调用方关闭连接。
                match tokio::time::timeout(REPLY_FLUSH_TIMEOUT, send.stopped()).await {
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => tracing::debug!(error = %e, "peer reset the hello reply stream"),
                    Err(_) => {
                        tracing::debug!("timed out waiting for the hello reply to be acknowledged")
                    }
                }
                anyhow::bail!("hello rejected ({status:?}): {message}")
            }
        }
    }

    fn authorize(&self, hello: &Hello, peer: EndpointId) -> Result<Authorized, (Status, String)> {
        if hello.v != PROTOCOL_VERSION {
            return Err((
                Status::BadRequest,
                format!("unsupported protocol version {}", hello.v),
            ));
        }
        let ticket = parse_ticket(&hello.ticket).map_err(|e| (Status::BadTicket, e.to_string()))?;
        let now = now_unix_secs();
        let payload = verify_ticket(
            &self.inner.keys,
            &ticket,
            &TicketExpectation {
                now_unix: now,
                share_id: None,
                peer,
                scopes: &[SCOPE_RECV, SCOPE_BRIDGE],
            },
        )
        .map_err(|e| {
            let status = match e {
                TicketError::WrongPeer => Status::PeerMismatch,
                TicketError::Expired => Status::Expired,
                _ => Status::BadTicket,
            };
            (status, e.to_string())
        })?;
        let share = self.inner.registry.get(&payload.sid).ok_or((
            Status::NotFound,
            "share is not available on this device".to_owned(),
        ))?;
        match share.availability(now) {
            Status::Ok => Ok(Authorized {
                share,
                share_id: payload.sid.clone(),
                ticket_exp: payload.exp,
            }),
            status => Err((status, "share is no longer available".to_owned())),
        }
    }

    /// 处理一条请求流：读请求 → 校验 → 写头 → 限速写数据。
    async fn serve_request(
        &self,
        auth: &Authorized,
        monitor: &SessionMonitor,
        mut send: SendStream,
        mut recv: RecvStream,
    ) -> Result<()> {
        let _active = monitor.activity();
        let request: Request = read_frame(&mut recv).await?;
        match self.prepare(auth, request).await {
            Prepared::Reject(header) => {
                write_frame(&mut send, &header).await?;
                send.finish().context("finish rejected response")?;
                Ok(())
            }
            Prepared::Serve {
                header,
                file,
                offset,
                len,
            } => {
                write_frame(&mut send, &header).await?;
                match self.pump(auth, monitor, &mut send, file, offset, len).await {
                    Ok(()) => {
                        send.finish().context("finish data stream")?;
                        Ok(())
                    }
                    Err(e) => {
                        if let Err(reset) = send.reset(RESET_INTERNAL.into()) {
                            tracing::debug!(error = %reset, "data stream was already closed");
                        }
                        Err(e)
                    }
                }
            }
        }
    }

    /// 源文件已变化：通知宿主（事件通道无界，只入队），并以 `changed` 拒绝本次请求。
    fn source_changed(&self, auth: &Authorized) -> Prepared {
        self.inner.observers.emit(SessionEvent::SourceChanged {
            share_id: auth.share_id.clone(),
        });
        Prepared::Reject(reject(Status::Changed))
    }

    async fn prepare(&self, auth: &Authorized, request: Request) -> Prepared {
        if request.len > MAX_REQUEST_LEN {
            return Prepared::Reject(reject(Status::BadRequest));
        }
        let now = now_unix_secs();
        match auth.share.availability(now) {
            Status::Ok => {}
            status => return Prepared::Reject(reject(status)),
        }
        if now >= auth.ticket_exp {
            return Prepared::Reject(reject(Status::Expired));
        }
        let Ok(index) = usize::try_from(request.file_index) else {
            return Prepared::Reject(reject(Status::NotFound));
        };
        let (Some(entry), Some(path)) = (
            auth.share.manifest.files.get(index),
            auth.share.paths.get(index),
        ) else {
            return Prepared::Reject(reject(Status::NotFound));
        };
        let size = entry.size;
        let expected_mtime = entry.mtime_ms;
        let root_hash = entry.root_hash.clone();
        let path = path.clone();
        let opened = tokio::task::spawn_blocking(move || -> std::io::Result<Option<File>> {
            let file = File::open(&path)?;
            let meta = file.metadata()?;
            if !meta.is_file() || meta.len() != size || file_mtime_ms(&meta)? != expected_mtime {
                return Ok(None);
            }
            Ok(Some(file))
        })
        .await;
        let file = match opened {
            Ok(Ok(Some(file))) => file,
            // 大小或修改时间与登记不符，或文件已被删除：统一报 changed，并通知宿主（只入队，不阻塞）。
            Ok(Ok(None)) => return self.source_changed(auth),
            Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                return self.source_changed(auth);
            }
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "cannot open a shared file");
                return Prepared::Reject(reject(Status::Internal));
            }
            Err(e) => {
                tracing::warn!(error = %e, "open task failed");
                return Prepared::Reject(reject(Status::Internal));
            }
        };
        if request.offset > size {
            return Prepared::Reject(reject(Status::BadRequest));
        }
        let len = request.len.min(size - request.offset);
        Prepared::Serve {
            header: Header {
                status: Status::Ok,
                file_size: size,
                root_hash,
                len,
            },
            file,
            offset: request.offset,
            len,
        }
    }

    /// 专用阻塞线程顺序读文件并预读，协程侧限速后写入 QUIC 流。
    async fn pump(
        &self,
        auth: &Authorized,
        monitor: &SessionMonitor,
        send: &mut SendStream,
        file: File,
        offset: u64,
        len: u64,
    ) -> Result<()> {
        pump_file(
            send,
            file,
            offset,
            len,
            &[&auth.share.limiter, &self.inner.global_limiter],
            |n| monitor.add_bytes(n),
        )
        .await
    }
}

/// 专用阻塞线程顺序读 `file` 的 `[offset, offset + len)` 并预读，协程侧依次过 `limiters` 限速后写入 QUIC 流。
/// 每写出一片调用一次 `on_bytes`（字节数）。
pub(crate) async fn pump_file(
    send: &mut SendStream,
    mut file: File,
    offset: u64,
    len: u64,
    limiters: &[&RateLimiter],
    mut on_bytes: impl FnMut(u64),
) -> Result<()> {
    let (tx, mut rx) = mpsc::channel::<std::io::Result<Bytes>>(PUMP_DEPTH);
    let reader = tokio::task::spawn_blocking(move || {
        if let Err(e) = file.seek(SeekFrom::Start(offset)) {
            // 接收端已走则忽略发送失败。
            if tx.blocking_send(Err(e)).is_err() {
                tracing::debug!("pump receiver is gone");
            }
            return;
        }
        let mut remaining = len;
        while remaining > 0 {
            let want = usize::try_from(remaining)
                .unwrap_or(PUMP_CHUNK)
                .min(PUMP_CHUNK);
            let mut buf = vec![0u8; want];
            let item = file.read_exact(&mut buf).map(|()| Bytes::from(buf));
            let failed = item.is_err();
            if tx.blocking_send(item).is_err() || failed {
                return;
            }
            remaining -= want as u64;
        }
    });

    let mut sent: u64 = 0;
    while let Some(item) = rx.recv().await {
        let chunk = item.context("read shared file")?;
        for limiter in limiters {
            limiter.acquire(chunk.len()).await;
        }
        send.write_all(&chunk)
            .await
            .context("write data to stream")?;
        sent += chunk.len() as u64;
        on_bytes(chunk.len() as u64);
    }
    reader.await.context("file reader task failed")?;
    ensure!(sent == len, "file ended after {sent} of {len} bytes");
    Ok(())
}
