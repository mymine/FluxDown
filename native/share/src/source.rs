//! 直连之外的取块来源：宿主提供的 HTTP 区间来源（云端暂存副本 / 网关桥接）。
//!
//! 接收会话优先走 iroh 直连 / 中继；直连失败（分享方离线、传输反复出错）或用户选择「切换桥接」时，
//! 按块改走 [`ChunkSource`]。来源只负责「把 `[offset, offset + len)` 的字节取回来」——
//! **每个块仍由会话按清单里的 BLAKE3 校验**，来源不可信（CDN 被劫持、网关被替换都不会写出坏数据）；
//! 校验失败的来源会被拉黑，换来源重取。切换发生在块与块之间，已校验的块不会重新下载。
//!
//! 计量：来源取回并通过校验的字节数按种类累计到 [`SessionBytes`](crate::SessionBytes) 的 `cdn` / `bridge`，
//! 与云端按签发区间预占的计量口径一致（每个块恰好一次区间请求）。

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use crate::types::{MAX_PEER_BYTES, PeerBytes};

use bytes::Bytes;

/// 来源种类（任务行来源徽标 / 计量分项）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SourceKind {
    /// 云端暂存副本（CDN / 对象存储的签名 URL）。
    Cdn,
    /// 实时桥接（share-gateway 经 iroh 向在线分享方取块再转发）。
    Bridge,
}

impl SourceKind {
    /// 稳定的小写名（日志与诊断用）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cdn => "cdn",
            Self::Bridge => "bridge",
        }
    }
}

/// 一次区间请求（恰好一个块）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRequest {
    /// 文件下标（分享清单里的位置）。
    pub file_index: u32,
    /// 区间起点（字节）。
    pub offset: u64,
    /// 区间长度（字节，非零；不超过文件剩余长度）。
    pub len: u64,
}

/// 来源取块失败原因。会话据此决定是否换来源、是否拉黑该来源。
#[derive(Debug, Clone, thiserror::Error)]
pub enum SourceError {
    /// 暂时取不到（网络错误、5xx、分享方不在线…）：换其他来源 / 稍后重试，来源保持可用。
    #[error("source unavailable: {0}")]
    Unavailable(String),
    /// 该来源现在不能提供这份数据（副本未就绪、被拒绝…）：本会话内不再使用它。
    #[error("source refused: {0}")]
    Refused(String),
    /// 云端流量额度耗尽：本块换来源；全部来源失败时向上报告额度不足。
    #[error("traffic quota exhausted")]
    TrafficExhausted,
    /// 源文件已被分享方修改 / 删除。
    #[error("the shared file was changed by the sender")]
    Changed,
    /// 分享已结束（过期、被停止、被下架）。
    #[error("the share has ended")]
    Ended,
}

/// 来源返回的 future。
pub type SourceFuture<'a> = Pin<Box<dyn Future<Output = Result<Bytes, SourceError>> + Send + 'a>>;

/// 宿主实现的 HTTP 区间来源。实现必须**恰好**取回请求的字节（`len` 个）；长度不符按坏块处理。
pub trait ChunkSource: Send + Sync + 'static {
    /// 来源种类。
    fn kind(&self) -> SourceKind;

    /// 取回 `request` 指定的区间。
    fn fetch(&self, request: SourceRequest) -> SourceFuture<'_>;
}

/// 会话优先使用的来源。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SourcePreference {
    /// 直连优先，失败时回落到来源（缺省）。
    #[default]
    Direct,
    /// 来源优先（用户点了「切换桥接」），来源全部失败才回落直连。
    Sources,
}

/// 各来源已取回并通过校验的字节数。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SourceBytes {
    /// 云端暂存副本。
    pub cdn: u64,
    /// 网关桥接。
    pub bridge: u64,
    /// 共享对端（持有同一文件的其他设备）。
    pub peer: u64,
}

/// 会话级累计（跨重连保持）。
#[derive(Debug, Default)]
pub(crate) struct SourceCounters {
    cdn: AtomicU64,
    bridge: AtomicU64,
    peer: AtomicU64,
    per_peer: Mutex<HashMap<String, u64>>,
}

impl SourceCounters {
    pub(crate) fn add(&self, kind: SourceKind, bytes: u64) {
        match kind {
            SourceKind::Cdn => self.cdn.fetch_add(bytes, Ordering::Relaxed),
            SourceKind::Bridge => self.bridge.fetch_add(bytes, Ordering::Relaxed),
        };
    }

    pub(crate) fn add_peer(&self, from_z32: &str, bytes: usize) {
        let bytes = bytes as u64;
        self.peer.fetch_add(bytes, Ordering::Relaxed);
        let mut map = self.per_peer.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(v) = map.get_mut(from_z32) {
            *v = v.saturating_add(bytes);
        } else {
            map.insert(from_z32.to_owned(), bytes);
        }
    }

    /// 全部对端的累计字节（不排序、不截断）。
    pub(crate) fn per_peer_all(&self) -> HashMap<String, u64> {
        self.per_peer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// 相对 `base` 新增的逐对端字节，按 bytes 降序截取 [`MAX_PEER_BYTES`] 条。
    pub(crate) fn per_peer_delta(&self, base: &HashMap<String, u64>) -> Vec<PeerBytes> {
        let map = self.per_peer.lock().unwrap_or_else(PoisonError::into_inner);
        let mut out: Vec<PeerBytes> = map
            .iter()
            .filter_map(|(k, v)| {
                let bytes = v.saturating_sub(base.get(k).copied().unwrap_or(0));
                (bytes > 0).then(|| PeerBytes {
                    endpoint_id: k.clone(),
                    bytes,
                })
            })
            .collect();
        out.sort_by(|a, b| {
            b.bytes
                .cmp(&a.bytes)
                .then_with(|| a.endpoint_id.cmp(&b.endpoint_id))
        });
        out.truncate(MAX_PEER_BYTES);
        out
    }

    pub(crate) fn snapshot(&self) -> SourceBytes {
        SourceBytes {
            cdn: self.cdn.load(Ordering::Relaxed),
            bridge: self.bridge.load(Ordering::Relaxed),
            peer: self.peer.load(Ordering::Relaxed),
        }
    }
}
