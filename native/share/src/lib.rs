//! FluxDown FluxMesh（临时文件分享）核心：宿主无关的 Rust 库，设计见 `docs/temp-share-design.md`。
//!
//! 不依赖 agent / engine / daemon / GPUI / 数据库；宿主（agent）负责：持久化私钥、从 FluxCloud 拉取
//! 网络策略与票据公钥、维护 [`ShareRegistry`]、把 [`ShareTelemetrySink`] 接到 FluxCloud、
//! 按空闲规则调用 [`ShareEndpoint::ensure_bound`] / [`ShareEndpoint::shutdown`]。
//!
//! # 零 n0 基础设施
//!
//! 端点用 `presets::Minimal` 构建，中继 / 地址发布 / 地址查找只来自 [`NetworkPolicy`]。
//! 本 crate 不使用 iroh 官方的任何中继、`iroh.link` DNS / pkarr 服务或 mainline DHT，
//! 也不会在策略缺项时回落到它们。
//!
//! # 模块
//!
//! - [`types`]：契约类型（策略、清单、票据、快照、会话摘要…）。
//! - [`hashing`]：4 MiB 分块 BLAKE3、块校验。
//! - [`ticket`]：票据解析与离线验签（[`KeyRing`]）。
//! - [`endpoint`]：[`ShareEndpoint`]，懒绑定、状态监听、快照。
//! - [`sender`]：[`ShareRegistry`] 与 `fluxdown/share/1` 协议处理器。
//! - [`receiver`]：[`ReceiveSession`]，块取数与校验；直连失败时回落到宿主提供的 [`ChunkSource`]。
//! - [`adapter`]：[`LocalAdapter`]，回环 HTTP 区间适配器。
//! - [`observe`]：[`ShareTelemetrySink`] 与 [`SessionEvent`]。
//! - [`swarm`]：共享（多源共享）：内容表、`fluxdown/swarm/1` 提供端与接收端多对端调度。

#![deny(missing_docs)]

pub mod adapter;
pub mod endpoint;
pub mod hashing;
mod limiter;
pub mod observe;
pub mod receiver;
pub mod sender;
mod session;
pub mod source;
pub mod swarm;
pub mod ticket;
pub mod timefmt;
pub mod types;
pub mod wire;

pub use adapter::LocalAdapter;
pub use bytes::Bytes;
pub use endpoint::{ShareEndpoint, ShareEndpointParams, nat_type_of};
pub use hashing::{
    CHUNK_SIZE, ChunkError, HashProgress, hash_file, hash_file_blocking, verify_chunk,
};
pub use iroh::{EndpointAddr, EndpointId, SecretKey, Signature};
pub use limiter::RateLimiter;
pub use observe::{Observers, SessionEvent, SessionEventSender, ShareTelemetrySink};
pub use receiver::{FetchError, ReceiveParams, ReceiveSession};
pub use sender::{SenderSessionInfo, ShareEntry, ShareRegistry, ShareSender, ShareState};
pub use session::classify_addr;
pub use source::{
    ChunkSource, SourceBytes, SourceError, SourceFuture, SourceKind, SourcePreference,
    SourceRequest,
};
pub use swarm::{
    ChunkSet, ContentEntry, ContentRegistry, ContentSource, SWARM_ALPN, SwarmFile, SwarmPeer,
};
pub use ticket::{
    KeyEntry, KeyRing, KeysResponse, SCOPE_BRIDGE, SCOPE_RECV, SCOPE_SWARM, TicketError,
    TicketExpectation, parse_ticket, verify_ticket,
};
pub use types::*;
pub use wire::ALPN;
