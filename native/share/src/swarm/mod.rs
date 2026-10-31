//! 共享（多源共享）：按文件根哈希识别同一内容，接收方可同时向原分享方与其他持有该内容的设备取块。
//!
//! - [`wire`]：数据面协议 `fluxdown/swarm/1` 的帧。
//! - [`content`]：本机可向别人提供的内容表（完整文件 / 下载中的部分块）。
//! - [`chunkset`]：块位图。
//! - `serve`：提供端协议处理器。
//! - `client`：接收端的对端状态与调度（由 [`crate::ReceiveSession`] 驱动）。
//!
//! 对端一律视为不可信：接收方对每个块按清单里的 BLAKE3 校验，坏对端两次即封禁。

pub mod chunkset;
mod client;
pub mod content;
pub(crate) mod serve;
pub mod wire;

pub use chunkset::ChunkSet;
pub(crate) use client::{InFlight, PeerSlot, READY_WAIT, ROUTE_SATURATION, SwarmState};
pub use client::{SwarmFile, SwarmPeer};
pub use content::{ContentEntry, ContentRegistry, ContentSource};
pub use wire::SWARM_ALPN;
