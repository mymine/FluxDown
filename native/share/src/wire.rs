//! 数据面协议 `fluxdown/share/1`（设计 §9.1）的帧格式。
//!
//! - 接收方先开一条双向流，写 [`Hello`]，读 [`HelloReply`]。
//! - 此后每个请求一条双向流：写 [`Request`]，读 [`Header`] 再读 `header.len` 字节原始数据。
//! - 所有帧都是 `u32 BE 长度 + JSON`，长度上限 [`MAX_FRAME`]。

use anyhow::{Context, Result, ensure};
use iroh::endpoint::{RecvStream, SendStream};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

/// ALPN。
pub const ALPN: &[u8] = b"fluxdown/share/1";

/// 当前协议版本。
pub const PROTOCOL_VERSION: u32 = 1;

/// 单个控制帧的最大字节数。
pub const MAX_FRAME: u32 = 64 * 1024;

/// 单个请求最大字节数（16 MiB）。
pub const MAX_REQUEST_LEN: u64 = 16 * 1024 * 1024;

/// 首帧：接收方出示票据。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hello {
    /// 协议版本。
    pub v: u32,
    /// 票据原文。
    pub ticket: String,
}

/// 响应状态。`Header` 只会用到前五个；其余出现在 [`HelloReply`] 与协议错误里。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    /// 成功。
    Ok,
    /// 分享或文件下标不存在。
    NotFound,
    /// 源文件大小或修改时间与登记不符（含文件已被删除）。
    Changed,
    /// 分享已被停止。
    Stopped,
    /// 票据或分享已过期。
    Expired,
    /// 票据无效（签名、密钥、作用域、分享不符）。
    BadTicket,
    /// QUIC 对端与票据 `rep` 不一致。
    PeerMismatch,
    /// 请求不合法（范围越界、`len` 过大、协议版本不支持等）。
    BadRequest,
    /// 发送方内部错误（读文件失败等）。
    Internal,
}

/// 对 [`Hello`] 的回复。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HelloReply {
    /// 结果；非 [`Status::Ok`] 时发送方随后关闭连接。
    pub status: Status,
    /// 面向日志的说明，不含敏感信息。
    #[serde(default)]
    pub message: String,
}

/// 数据请求。
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    /// 清单里的文件下标。
    pub file_index: u32,
    /// 起始字节偏移。
    pub offset: u64,
    /// 请求字节数（≤ 16 MiB）；超出文件末尾时被截断。
    pub len: u64,
}

/// 数据响应头。`status == Ok` 时紧随其后是 `len` 字节原始数据。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Header {
    /// 状态。
    pub status: Status,
    /// 文件当前大小。
    #[serde(default)]
    pub file_size: u64,
    /// 清单登记的文件根哈希（hex）。
    #[serde(default)]
    pub root_hash: String,
    /// 随后实际发送的字节数（`Ok` 时为 `min(request.len, fileSize - offset)`）。
    #[serde(default)]
    pub len: u64,
}

/// 写一帧。
pub async fn write_frame<T: Serialize>(send: &mut SendStream, value: &T) -> Result<()> {
    let body = serde_json::to_vec(value).context("encode frame")?;
    let len = u32::try_from(body.len()).context("frame too large")?;
    ensure!(len <= MAX_FRAME, "frame of {len} bytes exceeds the limit");
    send.write_all(&len.to_be_bytes())
        .await
        .context("write frame length")?;
    send.write_all(&body).await.context("write frame body")?;
    Ok(())
}

/// 读一帧。
pub async fn read_frame<T: DeserializeOwned>(recv: &mut RecvStream) -> Result<T> {
    let mut len = [0u8; 4];
    recv.read_exact(&mut len)
        .await
        .context("read frame length")?;
    let len = u32::from_be_bytes(len);
    ensure!(len <= MAX_FRAME, "frame of {len} bytes exceeds the limit");
    let mut body = vec![0u8; usize::try_from(len).context("frame length")?];
    recv.read_exact(&mut body)
        .await
        .context("read frame body")?;
    serde_json::from_slice(&body).context("decode frame")
}
