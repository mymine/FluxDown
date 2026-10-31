//! 共享提供端：`fluxdown/swarm/1` 协议处理器。
//!
//! 一条连接可以对多个内容授权（每个 `Grant` 一张 swarm 票据）；授权之后同一连接上的 `Have` / `Get`
//! 才会被服务。完整内容读盘直发（大小 / 修改时间不符即报 `changed`），部分内容读盘后逐块 BLAKE3 校验再发。

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use anyhow::{Context, Result};
use iroh::EndpointId;
use iroh::endpoint::{Connection, ConnectionError, RecvStream, SendStream};
use iroh::protocol::{AcceptError, ProtocolHandler};
use tokio::sync::{Semaphore, watch};
use tokio::task::JoinSet;

use super::chunkset::ChunkSet;
use super::content::{ContentEntry, ContentRegistry, ContentSource};
use super::wire::{SwarmReply, SwarmRequest, SwarmStatus};
use crate::hashing::{CHUNK_SIZE, file_mtime_ms, parse_hash, to_hex, verify_chunk};
use crate::limiter::RateLimiter;
use crate::observe::{Observers, SessionEvent};
use crate::sender::{CLOSE_KILLED, pump_file};
use crate::session::{MonitorConfig, SessionMonitor, close_error};
use crate::ticket::{
    KeyRing, SCOPE_SWARM, TicketError, TicketExpectation, parse_ticket, verify_ticket,
};
use crate::timefmt::{now_unix_ms, now_unix_secs};
use crate::types::{PathKind, SessionRole};
use crate::wire::{read_frame, write_frame};

/// 同一端点同时服务的共享连接上限。
const MAX_CONNECTIONS: usize = 32;
/// 单连接并发处理的流数。
const STREAMS_PER_CONNECTION: usize = 8;
/// 全局同时读盘 / 发送的 `Get` 数。
const MAX_CONCURRENT_GETS: usize = 16;
/// 单连接最多授权的内容数。
const MAX_GRANTS: usize = 4096;
/// 第一条流的等待上限。
const FIRST_STREAM_TIMEOUT: Duration = Duration::from_secs(15);
/// 读取请求帧的上限。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// 处理一条流（含向慢速对端发送）的总上限。
const STREAM_TIMEOUT: Duration = Duration::from_secs(180);
/// 部分内容分片发送的片大小。
const SLICE: usize = 256 * 1024;

/// QUIC 应用层关闭码：繁忙。
const CLOSE_BUSY: u32 = 0x12;
/// QUIC 应用层关闭码：没有请求。
const CLOSE_IDLE: u32 = 0x13;
/// 数据流复位码：读盘失败。
const RESET_INTERNAL: u32 = 0x20;

/// 保留槽位数上限（与云端 `swarmPriorityReservedSlots` 校验一致）。
const MAX_PRIORITY_RESERVED: usize = 16;

/// 每日提供预算：按 UI 提交的 UTC 偏移划分自然日。
#[derive(Debug, Default)]
struct DailyBudget {
    day: i64,
    used: u64,
    /// 0 = 不限。
    cap: u64,
    offset_secs: i64,
}

impl DailyBudget {
    fn day_of(&self, now_unix: u64) -> i64 {
        let now = i64::try_from(now_unix).unwrap_or(i64::MAX);
        now.saturating_add(self.offset_secs).div_euclid(86_400)
    }

    /// 跨日则清零。
    fn roll(&mut self, now_unix: u64) {
        let day = self.day_of(now_unix);
        if day != self.day {
            self.day = day;
            self.used = 0;
        }
    }
}

impl ServerInner {
    /// 预占 `len` 字节的当日额度：已达上限返回 `false`；否则累加（允许超出一块）。
    fn try_take_daily(&self, len: u64) -> bool {
        let mut d = self.daily.lock().unwrap_or_else(PoisonError::into_inner);
        d.roll(now_unix_secs());
        if d.cap > 0 && d.used >= d.cap {
            return false;
        }
        d.used = d.used.saturating_add(len);
        true
    }
}

struct ServerInner {
    content: ContentRegistry,
    keys: KeyRing,
    global_limiter: RateLimiter,
    observers: Observers,
    report_interval: Duration,
    local_id: EndpointId,
    counter: AtomicU64,
    connections: AtomicUsize,
    gets: Semaphore,
    /// 进行中的提供方会话（会话 ID → 连接上下文），供强制中断查找。
    live: Mutex<HashMap<String, Arc<ConnCtx>>>,
    /// 为优先请求保留的连接槽位数（连接序号落在最后 N 个的连接只接受 `pri = 1`）。
    priority_reserved: AtomicUsize,
    /// 每日提供预算（反 PCDN）。
    daily: Mutex<DailyBudget>,
}

/// 共享提供端（可克隆，内部共享）。
#[derive(Clone)]
pub(crate) struct SwarmServer {
    inner: Arc<ServerInner>,
}

impl SwarmServer {
    pub(crate) fn new(
        content: ContentRegistry,
        keys: KeyRing,
        global_limiter: RateLimiter,
        observers: Observers,
        report_interval: Duration,
        local_id: EndpointId,
    ) -> Self {
        Self {
            inner: Arc::new(ServerInner {
                content,
                keys,
                global_limiter,
                observers,
                report_interval,
                local_id,
                counter: AtomicU64::new(0),
                connections: AtomicUsize::new(0),
                gets: Semaphore::new(MAX_CONCURRENT_GETS),
                live: Mutex::new(HashMap::new()),
                priority_reserved: AtomicUsize::new(0),
                daily: Mutex::new(DailyBudget::default()),
            }),
        }
    }

    pub(crate) fn handler(&self) -> SwarmHandler {
        SwarmHandler {
            server: self.clone(),
        }
    }

    /// 更新保留给优先请求的连接槽位数（截断到 [`MAX_PRIORITY_RESERVED`]）。
    pub(crate) fn set_priority_reserved(&self, n: usize) {
        self.inner
            .priority_reserved
            .store(n.min(MAX_PRIORITY_RESERVED), Ordering::Relaxed);
    }

    /// 设置每日提供上限（0 = 不限）与自然日的 UTC 偏移（秒）。
    pub(crate) fn set_daily_cap(&self, cap: u64, utc_offset_secs: i64) {
        let mut d = self
            .inner
            .daily
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        d.cap = cap;
        d.offset_secs = utc_offset_secs;
        d.roll(now_unix_secs());
    }

    /// 当前（日序号，已提供字节）。
    pub(crate) fn daily_usage(&self) -> (i64, u64) {
        let mut d = self
            .inner
            .daily
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        d.roll(now_unix_secs());
        (d.day, d.used)
    }

    /// 启动时恢复落盘的当日用量（日序号不是今天则忽略）。
    pub(crate) fn restore_daily(&self, day: i64, used: u64) {
        let mut d = self
            .inner
            .daily
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        d.roll(now_unix_secs());
        if d.day == day {
            d.used = d.used.max(used);
        }
    }

    /// 强制中断一个提供方会话（关闭对应连接）。会话不存在返回 `false`。
    pub(crate) fn kill_session(&self, session_id: &str) -> bool {
        let ctx = self
            .inner
            .live
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(session_id)
            .cloned();
        match ctx {
            Some(ctx) => {
                ctx.killed.store(true, Ordering::SeqCst);
                ctx.conn.close(CLOSE_KILLED.into(), b"killed");
                true
            }
            None => false,
        }
    }
}

/// iroh 协议处理器（由 [`crate::ShareEndpoint`] 注册到 Router）。
#[derive(Clone)]
pub(crate) struct SwarmHandler {
    server: SwarmServer,
}

impl std::fmt::Debug for SwarmHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SwarmHandler")
    }
}

impl ProtocolHandler for SwarmHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        self.server.serve(connection).await;
        Ok(())
    }
}

/// 连接计数守卫。
struct ConnGuard(Arc<ServerInner>);

impl Drop for ConnGuard {
    fn drop(&mut self) {
        self.0.connections.fetch_sub(1, Ordering::SeqCst);
    }
}

/// 一条连接内共享的上下文。
struct ConnCtx {
    peer: EndpointId,
    conn: Connection,
    /// 已授权的内容 → 票据过期时间（Unix 秒）。
    grants: Mutex<HashMap<[u8; 32], u64>>,
    /// 提供方会话：第一次成功的 `Grant` 时建立。
    session: OnceLock<SwarmSession>,
    /// 会话被宿主 / 后台强制中断。
    killed: AtomicBool,
    /// 本连接落在为优先请求保留的槽位里：只接受 `pri = 1` 的票据。
    reserved_slot: bool,
}

/// 一条连接的提供方会话。
struct SwarmSession {
    id: String,
    monitor: SessionMonitor,
}

/// 会话收尾守卫：析构时注销会话并发出终态报告（只生效一次；连接从未授权成功则什么也不做）。
struct EndGuard {
    inner: Arc<ServerInner>,
    ctx: Arc<ConnCtx>,
    /// 连接结束原因；任务被中止（端点关闭）时为 `None`。
    ended: Option<ConnectionError>,
}

impl Drop for EndGuard {
    fn drop(&mut self) {
        let Some(session) = self.ctx.session.get() else {
            return;
        };
        self.inner
            .live
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&session.id);
        let error = if self.ctx.killed.load(Ordering::SeqCst) {
            Some("session killed".to_owned())
        } else {
            match &self.ended {
                Some(reason) => close_error(reason),
                None => Some("share endpoint shut down".to_owned()),
            }
        };
        session.monitor.finish(error);
    }
}

impl SwarmServer {
    async fn serve(&self, conn: Connection) {
        let before = self.inner.connections.fetch_add(1, Ordering::SeqCst);
        if before >= MAX_CONNECTIONS {
            self.inner.connections.fetch_sub(1, Ordering::SeqCst);
            conn.close(CLOSE_BUSY.into(), b"busy");
            return;
        }
        let reserved = self.inner.priority_reserved.load(Ordering::Relaxed);
        let reserved_slot = before >= MAX_CONNECTIONS.saturating_sub(reserved);
        let _guard = ConnGuard(self.inner.clone());
        let ctx = Arc::new(ConnCtx {
            peer: conn.remote_id(),
            conn: conn.clone(),
            grants: Mutex::new(HashMap::new()),
            session: OnceLock::new(),
            killed: AtomicBool::new(false),
            reserved_slot,
        });
        // 守卫保证：无论循环正常结束还是 Router 关闭时任务被中止，会话都会收尾并发出终态报告。
        let mut end = EndGuard {
            inner: self.inner.clone(),
            ctx: ctx.clone(),
            ended: None,
        };
        let permits = Arc::new(Semaphore::new(STREAMS_PER_CONNECTION));
        let mut streams: JoinSet<()> = JoinSet::new();
        let mut first = true;
        loop {
            let accepted = if first {
                match tokio::time::timeout(FIRST_STREAM_TIMEOUT, conn.accept_bi()).await {
                    Ok(accepted) => accepted,
                    Err(_) => {
                        conn.close(CLOSE_IDLE.into(), b"idle");
                        break;
                    }
                }
            } else {
                tokio::select! {
                    accepted = conn.accept_bi() => accepted,
                    done = streams.join_next(), if !streams.is_empty() => {
                        if let Some(Err(e)) = done {
                            tracing::warn!(error = %e, "swarm request task panicked");
                        }
                        continue;
                    }
                }
            };
            let (send, recv) = match accepted {
                Ok(pair) => pair,
                Err(e) => {
                    end.ended = Some(e);
                    break;
                }
            };
            first = false;
            let Ok(permit) = permits.clone().acquire_owned().await else {
                break;
            };
            let this = self.clone();
            let ctx = ctx.clone();
            streams.spawn(async move {
                let _permit = permit;
                let handled = tokio::time::timeout(STREAM_TIMEOUT, this.handle(&ctx, send, recv));
                match handled.await {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => {
                        tracing::debug!(error = %format!("{e:#}"), "swarm request failed");
                    }
                    Err(_) => tracing::debug!("swarm request timed out"),
                }
            });
        }
        streams.abort_all();
    }

    async fn handle(
        &self,
        ctx: &Arc<ConnCtx>,
        mut send: SendStream,
        mut recv: RecvStream,
    ) -> Result<()> {
        let request: SwarmRequest = tokio::time::timeout(REQUEST_TIMEOUT, read_frame(&mut recv))
            .await
            .context("timed out reading the swarm request")??;
        match request {
            SwarmRequest::Grant { ticket } => {
                let reply = self.grant(ctx, &ticket);
                let busy = reply.status == SwarmStatus::Busy && ctx.reserved_slot;
                let sent = reply_only(&mut send, &reply).await;
                if busy {
                    // 等对端收齐回复（或短暂超时）再关连接，否则 Busy 回复会被关闭码吞掉。
                    if tokio::time::timeout(Duration::from_secs(1), send.stopped())
                        .await
                        .is_err()
                    {
                        tracing::debug!("busy reply was not acknowledged in time");
                    }
                    ctx.conn.close(CLOSE_BUSY.into(), b"busy");
                }
                sent
            }
            SwarmRequest::Have { root } => match self.authorized(ctx, &root) {
                Err(status) => reply_only(&mut send, &SwarmReply::status(status)).await,
                Ok((_, entry)) => {
                    let (reply, bitmap) = have_reply(&entry);
                    write_frame(&mut send, &reply).await?;
                    if !bitmap.is_empty() {
                        send.write_all(&bitmap).await.context("write bitmap")?;
                    }
                    send.finish().context("finish have reply")?;
                    Ok(())
                }
            },
            SwarmRequest::Get { root, chunk } => match self.authorized(ctx, &root) {
                Err(status) => reply_only(&mut send, &SwarmReply::status(status)).await,
                Ok((root, entry)) => match ctx.session.get() {
                    Some(session) => {
                        self.get(&mut send, &session.monitor, &root, &entry, chunk)
                            .await
                    }
                    None => reply_only(&mut send, &SwarmReply::status(SwarmStatus::Internal)).await,
                },
            },
        }
    }

    fn grant(&self, ctx: &Arc<ConnCtx>, ticket: &str) -> SwarmReply {
        let bad = |status: SwarmStatus, message: String| SwarmReply {
            message,
            ..SwarmReply::status(status)
        };
        let ticket = match parse_ticket(ticket) {
            Ok(t) => t,
            Err(e) => return bad(SwarmStatus::BadTicket, e.to_string()),
        };
        let verified = verify_ticket(
            &self.inner.keys,
            &ticket,
            &TicketExpectation {
                now_unix: now_unix_secs(),
                share_id: None,
                peer: ctx.peer,
                scopes: &[SCOPE_SWARM],
            },
        );
        let payload = match verified {
            Ok(p) => p,
            Err(e) => {
                let status = match e {
                    TicketError::WrongPeer => SwarmStatus::PeerMismatch,
                    TicketError::Expired => SwarmStatus::Expired,
                    _ => SwarmStatus::BadTicket,
                };
                return bad(status, e.to_string());
            }
        };
        let Some(root) = payload.root.as_deref().and_then(parse_hash) else {
            return bad(
                SwarmStatus::BadTicket,
                "ticket carries no valid root".to_owned(),
            );
        };
        let Some(entry) = self.inner.content.get(&root) else {
            return bad(
                SwarmStatus::NotFound,
                "content is not available on this device".to_owned(),
            );
        };
        if ctx.reserved_slot && payload.pri == 0 {
            return bad(
                SwarmStatus::Busy,
                "reserved slots only serve priority tickets".to_owned(),
            );
        }
        self.open_session(ctx, &payload.sid);
        {
            let mut grants = ctx.grants.lock().unwrap_or_else(PoisonError::into_inner);
            if grants.len() >= MAX_GRANTS && !grants.contains_key(&root) {
                return bad(SwarmStatus::BadRequest, "too many grants".to_owned());
            }
            grants.insert(root, payload.exp);
        }
        SwarmReply {
            size: entry.size,
            complete: matches!(entry.source, ContentSource::Complete { .. }),
            ..SwarmReply::status(SwarmStatus::Ok)
        }
    }

    /// 打开本连接的提供方会话（只有第一次成功的 `Grant` 生效，之后返回已有会话）。
    ///
    /// 会话的分享 ID 取第一张被接受的票据里的 `sid`。同一连接上后续票据的 `sid` 不同时仍沿用它：
    /// 实际中同一连接的各个内容来自同一个接收会话（同一个分享），FluxCloud 按这个分享校验内容根哈希。
    fn open_session(&self, ctx: &Arc<ConnCtx>, share_id: &str) {
        ctx.session.get_or_init(|| {
            let session_id = self.new_session_id();
            let peer = ctx.peer.to_z32();
            let monitor = SessionMonitor::spawn(
                ctx.conn.clone(),
                MonitorConfig {
                    session_id: session_id.clone(),
                    share_id: share_id.to_owned(),
                    role: SessionRole::Swarm,
                    peer_endpoint_id: peer.clone(),
                    connect_ms: 0,
                    report_interval: self.inner.report_interval,
                    observers: self.inner.observers.clone(),
                    kind_tx: Arc::new(watch::channel(PathKind::Unknown).0),
                    sources: None,
                },
            );
            self.inner
                .live
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(session_id.clone(), ctx.clone());
            self.inner.observers.emit(SessionEvent::Opened {
                session_id: session_id.clone(),
                share_id: share_id.to_owned(),
                role: SessionRole::Swarm,
                peer_endpoint_id: peer,
            });
            SwarmSession {
                id: session_id,
                monitor,
            }
        });
    }

    fn new_session_id(&self) -> String {
        format!(
            "{}-w{:x}-{:x}",
            self.inner.local_id.fmt_short(),
            now_unix_ms(),
            self.inner.counter.fetch_add(1, Ordering::Relaxed)
        )
    }

    /// 本连接是否已就 `root` 授权且未过期，且本机仍持有该内容。
    fn authorized(
        &self,
        ctx: &ConnCtx,
        root_hex: &str,
    ) -> Result<([u8; 32], Arc<ContentEntry>), SwarmStatus> {
        let root = parse_hash(root_hex).ok_or(SwarmStatus::BadRequest)?;
        let exp = ctx
            .grants
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&root)
            .copied()
            .ok_or(SwarmStatus::NotGranted)?;
        if now_unix_secs() >= exp {
            return Err(SwarmStatus::Expired);
        }
        let entry = self.inner.content.get(&root).ok_or(SwarmStatus::NotFound)?;
        Ok((root, entry))
    }

    async fn get(
        &self,
        send: &mut SendStream,
        monitor: &SessionMonitor,
        root: &[u8; 32],
        entry: &ContentEntry,
        chunk: u64,
    ) -> Result<()> {
        let _active = monitor.activity();
        let size = entry.size;
        let Some(start) = chunk.checked_mul(CHUNK_SIZE).filter(|start| *start < size) else {
            return reply_only(send, &SwarmReply::status(SwarmStatus::BadRequest)).await;
        };
        let end = start.saturating_add(CHUNK_SIZE).min(size);
        let len = end - start;
        if !self.inner.try_take_daily(len) {
            return reply_only(send, &SwarmReply::status(SwarmStatus::Busy)).await;
        }
        let Ok(_permit) = self.inner.gets.acquire().await else {
            return reply_only(send, &SwarmReply::status(SwarmStatus::Busy)).await;
        };
        match &entry.source {
            ContentSource::Complete { path, mtime_ms } => {
                let (path, mtime_ms) = (path.clone(), *mtime_ms);
                let opened =
                    tokio::task::spawn_blocking(move || -> std::io::Result<Option<File>> {
                        let file = File::open(&path)?;
                        let meta = file.metadata()?;
                        if !meta.is_file()
                            || meta.len() != size
                            || file_mtime_ms(&meta)? != mtime_ms
                        {
                            return Ok(None);
                        }
                        Ok(Some(file))
                    })
                    .await;
                let file = match opened {
                    Ok(Ok(Some(file))) => file,
                    Ok(Ok(None)) => return self.content_changed(send, root).await,
                    Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                        return self.content_changed(send, root).await;
                    }
                    Ok(Err(e)) => {
                        tracing::warn!(error = %e, "cannot open content for swarm");
                        return reply_only(send, &SwarmReply::status(SwarmStatus::Internal)).await;
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "swarm open task failed");
                        return reply_only(send, &SwarmReply::status(SwarmStatus::Internal)).await;
                    }
                };
                write_frame(
                    send,
                    &SwarmReply {
                        size,
                        complete: true,
                        len,
                        ..SwarmReply::status(SwarmStatus::Ok)
                    },
                )
                .await?;
                match pump_file(send, file, start, len, &[&self.inner.global_limiter], |n| {
                    monitor.add_bytes(n)
                })
                .await
                {
                    Ok(()) => {
                        send.finish().context("finish swarm data")?;
                        monitor.add_served(&to_hex(root), len);
                        Ok(())
                    }
                    Err(e) => {
                        if let Err(reset) = send.reset(RESET_INTERNAL.into()) {
                            tracing::debug!(error = %reset, "swarm data stream was already closed");
                        }
                        Err(e)
                    }
                }
            }
            ContentSource::Partial { path, file, have } => {
                if !have.contains(chunk) {
                    return reply_only(send, &SwarmReply::status(SwarmStatus::Missing)).await;
                }
                let path = path.clone();
                let read = tokio::task::spawn_blocking(move || -> std::io::Result<Vec<u8>> {
                    let mut f = File::open(&path)?;
                    f.seek(SeekFrom::Start(start))?;
                    let mut buf = vec![0u8; usize::try_from(len).unwrap_or(0)];
                    f.read_exact(&mut buf)?;
                    Ok(buf)
                })
                .await;
                let data = match read {
                    Ok(Ok(data)) => data,
                    Ok(Err(e)) => {
                        tracing::debug!(error = %e, "partial content is not readable");
                        return reply_only(send, &SwarmReply::status(SwarmStatus::Missing)).await;
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "swarm read task failed");
                        return reply_only(send, &SwarmReply::status(SwarmStatus::Internal)).await;
                    }
                };
                if verify_chunk(file, chunk, &data).is_err() {
                    return reply_only(send, &SwarmReply::status(SwarmStatus::Missing)).await;
                }
                write_frame(
                    send,
                    &SwarmReply {
                        size,
                        complete: false,
                        len,
                        ..SwarmReply::status(SwarmStatus::Ok)
                    },
                )
                .await?;
                for slice in data.chunks(SLICE) {
                    self.inner.global_limiter.acquire(slice.len()).await;
                    send.write_all(slice).await.context("write swarm data")?;
                    monitor.add_bytes(slice.len() as u64);
                }
                send.finish().context("finish swarm data")?;
                monitor.add_served(&to_hex(root), len);
                Ok(())
            }
        }
    }

    async fn content_changed(&self, send: &mut SendStream, root: &[u8; 32]) -> Result<()> {
        self.inner.observers.emit(SessionEvent::ContentChanged {
            root_hash: to_hex(root),
        });
        reply_only(send, &SwarmReply::status(SwarmStatus::Changed)).await
    }
}

/// `Have` 的回复与随后的位图字节。
fn have_reply(entry: &ContentEntry) -> (SwarmReply, Vec<u8>) {
    match &entry.source {
        ContentSource::Complete { .. } => (
            SwarmReply {
                size: entry.size,
                complete: true,
                ..SwarmReply::status(SwarmStatus::Ok)
            },
            Vec::new(),
        ),
        ContentSource::Partial { have, .. } => {
            let bitmap = ChunkSet::to_bytes(have);
            (
                SwarmReply {
                    size: entry.size,
                    complete: false,
                    len: bitmap.len() as u64,
                    ..SwarmReply::status(SwarmStatus::Ok)
                },
                bitmap,
            )
        }
    }
}

async fn reply_only(send: &mut SendStream, reply: &SwarmReply) -> Result<()> {
    write_frame(send, reply).await?;
    send.finish().context("finish swarm reply")?;
    Ok(())
}
