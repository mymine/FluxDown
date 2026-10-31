//! 接收端的共享对端管理：连接、授权、位图刷新与按块取数。
//!
//! 宿主通过 [`SwarmState::update`] 提供 FluxCloud 返回的对端列表与 swarm 票据；后台维护任务
//! 保持最多 [`MAX_ACTIVE_PEERS`] 条连接，对每个文件发 `Grant`，对部分内容的对端定期刷新位图。
//! 调度器（[`crate::ReceiveSession`]）只通过 [`SwarmState::candidates`] / [`SwarmState::fetch`] 使用它。

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use bytes::Bytes;
use iroh::endpoint::{Connection, RecvStream};
use iroh::{EndpointAddr, EndpointId};
use tokio::sync::{Notify, watch};
use tokio::task::JoinSet;

use super::chunkset::ChunkSet;
use super::wire::{SWARM_ALPN, SwarmReply, SwarmRequest, SwarmStatus};
use crate::endpoint::ShareEndpoint;
use crate::hashing::{parse_hash, to_hex, verify_chunk};
use crate::receiver::FetchError;
use crate::types::{ShareFile, ShareManifest};
use crate::wire::{read_frame, write_frame};

/// 同时保持连接的对端数上限。
const MAX_ACTIVE_PEERS: usize = 8;
/// 已知对端数上限。
const MAX_KNOWN_PEERS: usize = 64;
/// 一条路线上的并发请求达到该值即视为饱和，调度时排到后面。
pub(crate) const ROUTE_SATURATION: usize = 4;
/// 单次连接的时间预算。
const CONNECT_BUDGET: Duration = Duration::from_secs(8);
/// 控制请求（Grant / Have）的时间上限。
const CONTROL_TIMEOUT: Duration = Duration::from_secs(15);
/// 单次 `Get` 的时间上限。
const GET_TIMEOUT: Duration = Duration::from_secs(30);
/// 部分内容对端的位图刷新间隔。
const HAVE_REFRESH: Duration = Duration::from_secs(10);
/// 维护任务的兜底周期。
const TICK: Duration = Duration::from_secs(5);
/// 传输失败 / 连接断开后的冷却。
const FAIL_COOLDOWN: Duration = Duration::from_secs(30);
/// 对端回 `Busy` 后的冷却。
const BUSY_COOLDOWN: Duration = Duration::from_secs(60);
/// 提供坏数据达到该次数即封禁。
const CORRUPT_BAN: u32 = 2;
/// 取块时没有路线可用、但有对端正在连接时的等待上限。
pub(crate) const READY_WAIT: Duration = Duration::from_secs(5);

/// 应用层关闭码：正常结束。
const CLOSE_DONE: u32 = 0;

/// 云端返回的某个文件的共享信息。
#[derive(Debug, Clone)]
pub struct SwarmFile {
    /// 清单里的文件下标。
    pub file_index: u32,
    /// 云端为本接收方签发的 swarm 票据（绑定该文件的根哈希）。
    pub ticket: String,
    /// 可能持有该文件的对端。
    pub peers: Vec<SwarmPeer>,
}

/// 一个共享对端。
#[derive(Debug, Clone)]
pub struct SwarmPeer {
    /// 对端 endpoint ID。
    pub endpoint_id: EndpointId,
    /// 云端登记的是否持有完整内容（连上后以对端自报为准）。
    pub complete: bool,
    /// 可选的地址提示（局域网发现、测试等）。
    pub addr_hint: Option<EndpointAddr>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

struct FileInfo {
    root_hex: String,
    size: u64,
    chunks: u64,
}

#[derive(Default)]
struct PeerFile {
    complete: bool,
    granted: bool,
    gone: bool,
    /// 上次被拒绝的票据：同一票据不再重发，等云端换新票。
    rejected: Option<String>,
    have: Option<(Arc<ChunkSet>, Instant)>,
}

/// 一个共享对端的连接与按文件状态。
pub(crate) struct PeerSlot {
    id: EndpointId,
    addr_hint: Mutex<Option<EndpointAddr>>,
    files: Mutex<HashMap<u32, PeerFile>>,
    conn: Mutex<Option<Connection>>,
    in_flight: AtomicUsize,
    cooldown_until: Mutex<Option<Instant>>,
    corrupt: AtomicU32,
    banned: AtomicBool,
}

impl PeerSlot {
    /// 对端 endpoint ID。
    pub(crate) fn id(&self) -> EndpointId {
        self.id
    }

    fn new(id: EndpointId, addr_hint: Option<EndpointAddr>) -> Self {
        Self {
            id,
            addr_hint: Mutex::new(addr_hint),
            files: Mutex::new(HashMap::new()),
            conn: Mutex::new(None),
            in_flight: AtomicUsize::new(0),
            cooldown_until: Mutex::new(None),
            corrupt: AtomicU32::new(0),
            banned: AtomicBool::new(false),
        }
    }

    /// 当前进行中的请求数（调度负载）。
    pub(crate) fn load(&self) -> usize {
        self.in_flight.load(Ordering::Relaxed)
    }

    fn conn(&self) -> Option<Connection> {
        lock(&self.conn)
            .as_ref()
            .filter(|c| c.close_reason().is_none())
            .cloned()
    }

    fn alive(&self) -> bool {
        self.conn().is_some()
    }

    fn cooling(&self, now: Instant) -> bool {
        lock(&self.cooldown_until).is_some_and(|until| now < until)
    }

    fn banned(&self) -> bool {
        self.banned.load(Ordering::Relaxed)
    }

    fn has_complete(&self) -> bool {
        lock(&self.files).values().any(|f| f.complete && !f.gone)
    }

    fn has_usable_file(&self) -> bool {
        lock(&self.files).values().any(|f| f.granted && !f.gone)
    }

    /// 断开连接：清授权与位图，进入冷却。
    fn disconnect(&self, cooldown: Duration, reason: &'static [u8]) {
        if let Some(conn) = lock(&self.conn).take() {
            conn.close(CLOSE_DONE.into(), reason);
        }
        for file in lock(&self.files).values_mut() {
            file.granted = false;
            file.have = None;
        }
        *lock(&self.cooldown_until) = Some(Instant::now() + cooldown);
    }

    fn with_file(&self, file_index: u32, f: impl FnOnce(&mut PeerFile)) {
        if let Some(file) = lock(&self.files).get_mut(&file_index) {
            f(file);
        }
    }
}

/// 在途请求计数守卫。
pub(crate) struct InFlight<'a>(&'a AtomicUsize);

impl<'a> InFlight<'a> {
    pub(crate) fn new(counter: &'a AtomicUsize) -> Self {
        counter.fetch_add(1, Ordering::Relaxed);
        Self(counter)
    }
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

/// 一次接收会话的共享状态。
pub(crate) struct SwarmState {
    endpoint: ShareEndpoint,
    /// 按清单下标；根哈希无法解析的文件为 `None`（不可共享）。
    files: Vec<Option<FileInfo>>,
    local_have: Vec<Arc<ChunkSet>>,
    tickets: Mutex<HashMap<u32, String>>,
    peers: Mutex<Vec<Arc<PeerSlot>>>,
    connected: watch::Sender<u32>,
    closed: AtomicBool,
    wake: Arc<Notify>,
    maintaining: AtomicBool,
}

impl SwarmState {
    pub(crate) fn new(endpoint: ShareEndpoint, manifest: &ShareManifest) -> Self {
        let files = manifest
            .files
            .iter()
            .map(|f| {
                let root = parse_hash(&f.root_hash)?;
                Some(FileInfo {
                    root_hex: to_hex(&root),
                    size: f.size,
                    chunks: f.chunk_count(),
                })
            })
            .collect();
        let local_have = manifest
            .files
            .iter()
            .map(|f| Arc::new(ChunkSet::new(f.chunk_count())))
            .collect();
        Self {
            endpoint,
            files,
            local_have,
            tickets: Mutex::new(HashMap::new()),
            peers: Mutex::new(Vec::new()),
            connected: watch::channel(0).0,
            closed: AtomicBool::new(false),
            wake: Arc::new(Notify::new()),
            maintaining: AtomicBool::new(false),
        }
    }

    fn info(&self, file_index: u32) -> Option<&FileInfo> {
        self.files
            .get(usize::try_from(file_index).ok()?)
            .and_then(Option::as_ref)
    }

    /// 合并云端返回的对端与票据。
    pub(crate) fn update(&self, files: Vec<SwarmFile>) {
        if self.closed.load(Ordering::SeqCst) {
            return;
        }
        let me = self.endpoint.id();
        let files: Vec<SwarmFile> = files
            .into_iter()
            .filter(|f| self.info(f.file_index).is_some())
            .collect();
        {
            let mut tickets = lock(&self.tickets);
            let mut renewed: Vec<u32> = Vec::new();
            for file in &files {
                let previous = tickets.insert(file.file_index, file.ticket.clone());
                if previous.as_ref().is_some_and(|old| *old != file.ticket) {
                    renewed.push(file.file_index);
                }
            }
            drop(tickets);
            // 票据换新：让所有持有该文件授权的对端在旧票过期前用新票重新授权。
            if !renewed.is_empty() {
                for slot in lock(&self.peers).iter() {
                    for index in &renewed {
                        slot.with_file(*index, |f| f.granted = false);
                    }
                }
            }
        }
        let seen: HashSet<EndpointId> = files
            .iter()
            .flat_map(|f| f.peers.iter().map(|p| p.endpoint_id))
            .collect();
        {
            let mut peers = lock(&self.peers);
            // 云端列表里已不在线的对端（没有连接、没有在途请求、没被封禁）直接忘掉，给新对端腾位置。
            peers.retain(|s| seen.contains(&s.id) || s.banned() || s.alive() || s.load() > 0);
            for file in &files {
                for peer in &file.peers {
                    if peer.endpoint_id == me {
                        continue;
                    }
                    let slot = match peers.iter().find(|s| s.id == peer.endpoint_id) {
                        Some(slot) => slot.clone(),
                        None if peers.len() < MAX_KNOWN_PEERS => {
                            let slot =
                                Arc::new(PeerSlot::new(peer.endpoint_id, peer.addr_hint.clone()));
                            peers.push(slot.clone());
                            slot
                        }
                        None => continue,
                    };
                    if let Some(hint) = &peer.addr_hint {
                        *lock(&slot.addr_hint) = Some(hint.clone());
                    }
                    let mut known = lock(&slot.files);
                    let entry = known.entry(file.file_index).or_default();
                    if !entry.granted {
                        entry.complete = peer.complete;
                    }
                }
            }
        }
        self.wake.notify_one();
    }

    /// 启动后台维护任务（只启动一次）。
    pub(crate) fn start(self: &Arc<Self>) {
        if !self.maintaining.swap(true, Ordering::SeqCst) {
            tokio::spawn(maintain(Arc::downgrade(self)));
        }
    }

    fn recount(&self) {
        let n = lock(&self.peers)
            .iter()
            .filter(|s| s.alive() && s.has_usable_file())
            .count();
        let n = u32::try_from(n).unwrap_or(u32::MAX);
        self.connected.send_if_modified(|current| {
            if *current == n {
                false
            } else {
                *current = n;
                true
            }
        });
    }

    async fn tick(self: &Arc<Self>) {
        let slots: Vec<Arc<PeerSlot>> = lock(&self.peers).clone();
        // 连接已死的先收尾。
        for slot in &slots {
            let dead = lock(&slot.conn)
                .as_ref()
                .is_some_and(|c| c.close_reason().is_some());
            if dead {
                slot.disconnect(FAIL_COOLDOWN, b"lost");
            }
        }
        let now = Instant::now();
        let ready = slots.iter().filter(|s| s.alive()).count();
        let mut room = MAX_ACTIVE_PEERS.saturating_sub(ready);
        let mut dialable: Vec<&Arc<PeerSlot>> = slots
            .iter()
            .filter(|s| !s.alive() && !s.banned() && !s.cooling(now) && self.wants(s))
            .collect();
        // 有完整内容的优先；同组内保持列表顺序。
        dialable.sort_by_key(|s| !s.has_complete());
        let dialing: Vec<Arc<PeerSlot>> = dialable
            .into_iter()
            .take_while(|_| {
                let ok = room > 0;
                room = room.saturating_sub(1);
                ok
            })
            .cloned()
            .collect();

        let mut set = JoinSet::new();
        for slot in slots {
            let dial = dialing.iter().any(|d| Arc::ptr_eq(d, &slot));
            if !dial && !slot.alive() {
                continue;
            }
            let this = self.clone();
            set.spawn(async move { this.service(slot, dial).await });
        }
        while let Some(done) = set.join_next().await {
            if let Err(e) = done {
                tracing::warn!(error = %e, "swarm peer task failed");
            }
        }
        self.recount();
    }

    /// 该对端是否还有值得连接的文件（有票据、未 gone）。
    fn wants(&self, slot: &PeerSlot) -> bool {
        let tickets = lock(&self.tickets);
        lock(&slot.files)
            .iter()
            .any(|(idx, f)| !f.gone && tickets.contains_key(idx))
    }

    async fn service(self: Arc<Self>, slot: Arc<PeerSlot>, dial: bool) {
        if dial && !self.dial(&slot).await {
            return;
        }
        self.grant_all(&slot).await;
        self.refresh_have(&slot).await;
    }

    async fn dial(&self, slot: &Arc<PeerSlot>) -> bool {
        let attempt = async {
            let endpoint = self.endpoint.ensure_bound().await?;
            let target = lock(&slot.addr_hint)
                .clone()
                .unwrap_or_else(|| EndpointAddr::new(slot.id));
            endpoint
                .connect(target, SWARM_ALPN)
                .await
                .map_err(anyhow::Error::from)
        };
        let conn = match tokio::time::timeout(CONNECT_BUDGET, attempt).await {
            Ok(Ok(conn)) => conn,
            Ok(Err(e)) => {
                tracing::debug!(peer = %slot.id.fmt_short(), error = %format!("{e:#}"), "swarm connect failed");
                slot.disconnect(FAIL_COOLDOWN, b"failed");
                return false;
            }
            Err(_) => {
                tracing::debug!(peer = %slot.id.fmt_short(), "swarm connect timed out");
                slot.disconnect(FAIL_COOLDOWN, b"timeout");
                return false;
            }
        };
        if self.closed.load(Ordering::SeqCst) {
            conn.close(CLOSE_DONE.into(), b"done");
            return false;
        }
        *lock(&slot.conn) = Some(conn);
        true
    }

    /// 对连接上所有尚未授权的文件发 `Grant`。
    async fn grant_all(&self, slot: &Arc<PeerSlot>) {
        let Some(conn) = slot.conn() else {
            return;
        };
        let pending: Vec<(u32, String)> = {
            let tickets = lock(&self.tickets);
            lock(&slot.files)
                .iter()
                .filter(|(_, f)| !f.granted && !f.gone)
                .filter_map(|(idx, f)| {
                    let ticket = tickets.get(idx)?;
                    (f.rejected.as_deref() != Some(ticket.as_str())).then(|| (*idx, ticket.clone()))
                })
                .collect()
        };
        for (idx, ticket) in pending {
            let Some(info) = self.info(idx) else {
                continue;
            };
            let sent = control(
                &conn,
                &SwarmRequest::Grant {
                    ticket: ticket.clone(),
                },
            )
            .await;
            let reply = match sent {
                Ok((reply, _)) => reply,
                Err(e) => {
                    tracing::debug!(peer = %slot.id.fmt_short(), error = %format!("{e:#}"), "swarm grant failed");
                    slot.disconnect(FAIL_COOLDOWN, b"failed");
                    return;
                }
            };
            match reply.status {
                SwarmStatus::Ok => slot.with_file(idx, |f| {
                    // K1：对端自报的大小必须等于清单大小，否则视为不持有。
                    if reply.size == info.size {
                        f.granted = true;
                        f.complete = reply.complete;
                        f.rejected = None;
                        f.have = None;
                    } else {
                        f.gone = true;
                    }
                }),
                SwarmStatus::NotFound | SwarmStatus::Changed => {
                    slot.with_file(idx, |f| f.gone = true);
                }
                SwarmStatus::Busy => {
                    slot.disconnect(BUSY_COOLDOWN, b"busy");
                    return;
                }
                other => {
                    tracing::debug!(peer = %slot.id.fmt_short(), status = ?other, message = %reply.message, "swarm grant refused");
                    slot.with_file(idx, |f| f.rejected = Some(ticket));
                }
            }
        }
    }

    /// 刷新部分内容对端的块位图。
    async fn refresh_have(&self, slot: &Arc<PeerSlot>) {
        let Some(conn) = slot.conn() else {
            return;
        };
        let now = Instant::now();
        let due: Vec<u32> = lock(&slot.files)
            .iter()
            .filter(|(_, f)| f.granted && !f.gone && !f.complete)
            .filter(|(_, f)| {
                f.have
                    .as_ref()
                    .is_none_or(|(_, at)| now.saturating_duration_since(*at) >= HAVE_REFRESH)
            })
            .map(|(idx, _)| *idx)
            .collect();
        for idx in due {
            let (Some(info), Some(local)) = (
                self.info(idx),
                self.local_have
                    .get(usize::try_from(idx).unwrap_or(usize::MAX)),
            ) else {
                continue;
            };
            if local.count() >= info.chunks {
                continue;
            }
            let sent = control(
                &conn,
                &SwarmRequest::Have {
                    root: info.root_hex.clone(),
                },
            )
            .await;
            let (reply, recv) = match sent {
                Ok(ok) => ok,
                Err(e) => {
                    tracing::debug!(peer = %slot.id.fmt_short(), error = %format!("{e:#}"), "swarm have failed");
                    slot.disconnect(FAIL_COOLDOWN, b"failed");
                    return;
                }
            };
            match reply.status {
                SwarmStatus::Ok if reply.complete => slot.with_file(idx, |f| {
                    f.complete = true;
                    f.have = None;
                }),
                SwarmStatus::Ok => {
                    let bitmap = read_bitmap(recv, reply.len, info.chunks).await;
                    slot.with_file(idx, |f| match bitmap {
                        Some(set) => f.have = Some((Arc::new(set), Instant::now())),
                        None => f.gone = true,
                    });
                }
                SwarmStatus::NotFound | SwarmStatus::Changed => {
                    slot.with_file(idx, |f| f.gone = true);
                }
                SwarmStatus::NotGranted | SwarmStatus::Expired => {
                    slot.with_file(idx, |f| f.granted = false);
                }
                SwarmStatus::Busy => {
                    slot.disconnect(BUSY_COOLDOWN, b"busy");
                    return;
                }
                _ => {}
            }
        }
    }

    /// 能提供 `file_index` 的第 `chunk` 块的对端：连接存活、未封禁、未冷却、已授权，
    /// 且持有完整内容或位图含该块；负载低的在前（稳定排序）。
    pub(crate) fn candidates(&self, file_index: u32, chunk: u64) -> Vec<Arc<PeerSlot>> {
        let now = Instant::now();
        let mut out: Vec<Arc<PeerSlot>> = lock(&self.peers)
            .iter()
            .filter(|s| !s.banned() && !s.cooling(now) && s.alive())
            .filter(|s| {
                lock(&s.files).get(&file_index).is_some_and(|f| {
                    f.granted
                        && !f.gone
                        && (f.complete || f.have.as_ref().is_some_and(|(h, _)| h.contains(chunk)))
                })
            })
            .cloned()
            .collect();
        out.sort_by_key(|s| s.load());
        out
    }

    /// 是否还有可能服务任何文件的对端（未封禁、至少一个文件未 gone）。
    pub(crate) fn has_peers(&self) -> bool {
        lock(&self.peers)
            .iter()
            .any(|s| !s.banned() && lock(&s.files).values().any(|f| !f.gone))
    }

    /// 是否有未封禁、该文件未 gone 的对端。
    pub(crate) fn has_peers_for(&self, file_index: u32) -> bool {
        lock(&self.peers)
            .iter()
            .any(|s| !s.banned() && lock(&s.files).get(&file_index).is_some_and(|f| !f.gone))
    }

    /// 是否有可连接但尚未就绪（未连接 / 未授权）、且不在冷却的对端。
    pub(crate) fn pending_for(&self, file_index: u32) -> bool {
        let now = Instant::now();
        let tickets = lock(&self.tickets);
        let Some(ticket) = tickets.get(&file_index) else {
            return false;
        };
        lock(&self.peers).iter().any(|s| {
            !s.banned()
                && !s.cooling(now)
                && lock(&s.files).get(&file_index).is_some_and(|f| {
                    !f.gone
                        && f.rejected.as_deref() != Some(ticket.as_str())
                        && (!f.granted || !s.alive())
                })
        })
    }

    /// 等到就绪对端数发生变化，最多 `d`。
    pub(crate) async fn wait_ready(&self, d: Duration) {
        let mut rx = self.connected.subscribe();
        self.wake.notify_one();
        if tokio::time::timeout(d, rx.changed()).await.is_err() {
            tracing::debug!("no swarm peer became ready in time");
        }
    }

    /// 就绪对端数：连接存活且至少一个已授权、未 gone 的文件。
    pub(crate) fn connected_count(&self) -> u32 {
        *self.connected.borrow()
    }

    pub(crate) fn subscribe_connected(&self) -> watch::Receiver<u32> {
        self.connected.subscribe()
    }

    /// 本机已持有（已校验）的块。
    pub(crate) fn local_have(&self, file_index: u32) -> Option<Arc<ChunkSet>> {
        self.local_have
            .get(usize::try_from(file_index).ok()?)
            .cloned()
    }

    /// 向 `peer` 取并校验一块。只返回非终态错误（[`FetchError::Transfer`] / [`FetchError::Corrupt`]）。
    pub(crate) async fn fetch(
        &self,
        peer: &Arc<PeerSlot>,
        file: &ShareFile,
        file_index: u32,
        chunk: u64,
        start: u64,
        end: u64,
    ) -> Result<Bytes, FetchError> {
        let _flight = InFlight::new(&peer.in_flight);
        let (Some(info), Some(conn)) = (self.info(file_index), peer.conn()) else {
            return Err(FetchError::Transfer(
                "swarm peer is not connected".to_owned(),
            ));
        };
        let want = end.saturating_sub(start);
        let request = SwarmRequest::Get {
            root: info.root_hex.clone(),
            chunk,
        };
        let exchange = async {
            let (reply, mut recv) = call(&conn, &request).await?;
            let data = if reply.status == SwarmStatus::Ok
                && reply.len == want
                && reply.size == file.size
            {
                let mut buf = vec![0u8; usize::try_from(want).context("chunk length")?];
                recv.read_exact(&mut buf).await.context("read chunk data")?;
                Some(buf)
            } else {
                None
            };
            Ok::<_, anyhow::Error>((reply, data))
        };
        let outcome = tokio::time::timeout(GET_TIMEOUT, exchange).await;
        let (reply, data) = match outcome {
            Ok(Ok(ok)) => ok,
            Ok(Err(e)) => {
                return Err(self.transfer_failed(peer, format!("{e:#}")));
            }
            Err(_) => return Err(self.transfer_failed(peer, "swarm get timed out".to_owned())),
        };
        match (reply.status, data) {
            (SwarmStatus::Ok, Some(data)) => {
                let data = Bytes::from(data);
                match verify_chunk(file, chunk, &data) {
                    Ok(()) => Ok(data),
                    Err(e) => {
                        peer.with_file(file_index, |f| {
                            if let Some((have, _)) = &f.have {
                                have.remove(chunk);
                            }
                        });
                        let strikes = peer.corrupt.fetch_add(1, Ordering::SeqCst) + 1;
                        tracing::warn!(peer = %peer.id.fmt_short(), strikes, error = %e, "swarm peer sent a corrupt chunk");
                        if strikes >= CORRUPT_BAN {
                            peer.banned.store(true, Ordering::SeqCst);
                            peer.disconnect(FAIL_COOLDOWN, b"corrupt");
                            self.recount();
                        }
                        Err(FetchError::Corrupt(e))
                    }
                }
            }
            (SwarmStatus::Ok, None) => Err(FetchError::Transfer(format!(
                "swarm peer announced {} bytes (size {}) for a {want}-byte chunk",
                reply.len, reply.size
            ))),
            (SwarmStatus::Missing, _) => {
                peer.with_file(file_index, |f| {
                    if let Some((have, _)) = &f.have {
                        have.remove(chunk);
                    }
                });
                Err(FetchError::Transfer(
                    "swarm peer does not have the chunk".to_owned(),
                ))
            }
            (SwarmStatus::NotFound | SwarmStatus::Changed, _) => {
                peer.with_file(file_index, |f| f.gone = true);
                self.recount();
                Err(FetchError::Transfer(
                    "swarm peer no longer has the content".to_owned(),
                ))
            }
            (SwarmStatus::NotGranted, _) => {
                peer.with_file(file_index, |f| f.granted = false);
                self.wake.notify_one();
                self.recount();
                Err(FetchError::Transfer("swarm peer lost the grant".to_owned()))
            }
            (SwarmStatus::Expired, _) => {
                let ticket = lock(&self.tickets).get(&file_index).cloned();
                peer.with_file(file_index, |f| {
                    f.granted = false;
                    f.rejected = ticket;
                });
                self.wake.notify_one();
                self.recount();
                Err(FetchError::Transfer("swarm ticket expired".to_owned()))
            }
            (SwarmStatus::Busy, _) => {
                peer.disconnect(BUSY_COOLDOWN, b"busy");
                self.recount();
                Err(FetchError::Transfer("swarm peer is busy".to_owned()))
            }
            (other, _) => Err(FetchError::Transfer(format!(
                "swarm peer replied {other:?}: {}",
                reply.message
            ))),
        }
    }

    fn transfer_failed(&self, peer: &Arc<PeerSlot>, message: String) -> FetchError {
        peer.disconnect(FAIL_COOLDOWN, b"failed");
        self.wake.notify_one();
        self.recount();
        FetchError::Transfer(message)
    }

    /// 关闭全部连接并停止维护任务。
    pub(crate) fn shutdown(&self) {
        self.closed.store(true, Ordering::SeqCst);
        for slot in lock(&self.peers).iter() {
            if let Some(conn) = lock(&slot.conn).take() {
                conn.close(CLOSE_DONE.into(), b"done");
            }
        }
        self.wake.notify_waiters();
        self.connected.send_replace(0);
    }
}

/// 后台维护循环：只持有弱引用，会话释放后自行退出。
async fn maintain(weak: Weak<SwarmState>) {
    loop {
        let Some(state) = weak.upgrade() else {
            return;
        };
        if state.closed.load(Ordering::SeqCst) {
            return;
        }
        state.tick().await;
        let wake = state.wake.clone();
        drop(state);
        if tokio::time::timeout(TICK, wake.notified()).await.is_err() {
            tracing::trace!("swarm maintenance tick");
        }
    }
}

/// 在新的双向流上发一个请求，读回复帧；数据部分留在返回的 `RecvStream` 里。
async fn call(conn: &Connection, request: &SwarmRequest) -> Result<(SwarmReply, RecvStream)> {
    let (mut send, mut recv) = conn.open_bi().await.context("open swarm stream")?;
    write_frame(&mut send, request).await?;
    send.finish().context("finish swarm request")?;
    let reply = read_frame(&mut recv).await?;
    Ok((reply, recv))
}

/// [`call`] 加控制请求的超时。
async fn control(conn: &Connection, request: &SwarmRequest) -> Result<(SwarmReply, RecvStream)> {
    tokio::time::timeout(CONTROL_TIMEOUT, call(conn, request))
        .await
        .context("swarm control request timed out")?
}

/// 读 `Have` 的位图数据；长度或内容不合法返回 `None`。
async fn read_bitmap(mut recv: RecvStream, len: u64, chunks: u64) -> Option<ChunkSet> {
    if len != chunks.div_ceil(8) {
        return None;
    }
    let mut buf = vec![0u8; usize::try_from(len).ok()?];
    match tokio::time::timeout(CONTROL_TIMEOUT, recv.read_exact(&mut buf)).await {
        Ok(Ok(())) => ChunkSet::from_bytes(chunks, &buf),
        Ok(Err(e)) => {
            tracing::debug!(error = %e, "cannot read the swarm bitmap");
            None
        }
        Err(_) => {
            tracing::debug!("timed out reading the swarm bitmap");
            None
        }
    }
}
