//! 共享数据面协议 `fluxdown/swarm/1` 的帧格式。
//!
//! 每个请求占一条双向流：写 [`SwarmRequest`]，读 [`SwarmReply`]，随后读恰好 `reply.len` 字节原始数据
//! （`Have` 为位图，`Get` 为整块）。帧编码沿用 [`crate::wire::write_frame`] / [`crate::wire::read_frame`]。

use serde::{Deserialize, Serialize};

/// 共享 ALPN。
pub const SWARM_ALPN: &[u8] = b"fluxdown/swarm/1";

/// 请求。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum SwarmRequest {
    /// 出示 swarm 票据，授权本连接读取票据绑定的内容。
    #[serde(rename_all = "camelCase")]
    Grant {
        /// 票据原文。
        ticket: String,
    },
    /// 查询对端持有的块位图。
    #[serde(rename_all = "camelCase")]
    Have {
        /// 内容根哈希（hex）。
        root: String,
    },
    /// 取一个整块。
    #[serde(rename_all = "camelCase")]
    Get {
        /// 内容根哈希（hex）。
        root: String,
        /// 块下标。
        chunk: u64,
    },
}

/// 回复状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SwarmStatus {
    /// 成功。
    Ok,
    /// 票据无效（签名、密钥、作用域、缺少 root）。
    BadTicket,
    /// QUIC 对端与票据 `rep` 不一致。
    PeerMismatch,
    /// 票据已过期。
    Expired,
    /// 本连接尚未就该内容授权。
    NotGranted,
    /// 本机不持有该内容。
    NotFound,
    /// 部分内容里没有这一块（或磁盘数据校验失败）。
    Missing,
    /// 完整内容的文件已被删除 / 修改。
    Changed,
    /// 对端繁忙。
    Busy,
    /// 请求不合法。
    BadRequest,
    /// 内部错误。
    Internal,
    /// 未知状态（来自更新版本的对端）。
    #[serde(other)]
    Unknown,
}

/// 回复。之后紧跟恰好 `len` 字节原始数据。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwarmReply {
    /// 状态。
    pub status: SwarmStatus,
    /// 对端持有的该内容的文件大小。
    #[serde(default)]
    pub size: u64,
    /// 对端是否持有完整内容。
    #[serde(default)]
    pub complete: bool,
    /// 随后的原始数据字节数。
    #[serde(default)]
    pub len: u64,
    /// 面向日志的说明。
    #[serde(default)]
    pub message: String,
}

impl SwarmReply {
    /// 只带状态、无数据的回复。
    pub fn status(status: SwarmStatus) -> Self {
        Self {
            status,
            size: 0,
            complete: false,
            len: 0,
            message: String::new(),
        }
    }
}
