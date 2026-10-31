//! 分享票据解析与离线验签。
//!
//! 票据格式（设计 §9.3）：`base64url(payloadJson) "." base64url(ed25519Sig)`。
//! 签名对象是 **payload JSON 的原始字节**（即 base64 解码后、未经任何重新序列化的字节），
//! 公钥来自 `GET /share/keys`，宿主把它填进 [`KeyRing`]，之后验票完全离线。

use std::collections::HashMap;
use std::sync::{Arc, PoisonError, RwLock};

use base64::Engine as _;
use base64::alphabet;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use iroh::{EndpointId, PublicKey, Signature};
use serde::{Deserialize, Serialize};

use crate::types::{Ticket, TicketPayload};

/// 票据作用域：接收方凭它直连分享方取数。
pub const SCOPE_RECV: &str = "recv";

/// 票据作用域：网关桥接（`rep` 是 share-gateway 的 endpoint ID，网关代接收方经 iroh 取块并转发）。
pub const SCOPE_BRIDGE: &str = "bridge";

/// 票据作用域：共享（接收方向持有同一内容的对端取块，票据额外绑定内容根哈希 `root`）。
pub const SCOPE_SWARM: &str = "swarm";

/// 目前唯一支持的票据版本。
pub const TICKET_VERSION: u32 = 1;

/// base64url，编码不带填充，解码兼容有无填充。
const B64: GeneralPurpose = GeneralPurpose::new(
    &alphabet::URL_SAFE,
    GeneralPurposeConfig::new()
        .with_encode_padding(false)
        .with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

/// 票据校验失败原因。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TicketError {
    /// 结构 / 编码 / JSON 不合法。
    #[error("malformed ticket: {0}")]
    Malformed(&'static str),
    /// 票据版本不受支持。
    #[error("unsupported ticket version {0}")]
    UnsupportedVersion(u32),
    /// 本机密钥环里没有该 `kid`（可能需要重新拉取 `/share/keys`）。
    #[error("unknown ticket key id {0:?}")]
    UnknownKey(String),
    /// 密钥已过 `notAfter`。
    #[error("ticket key {0:?} is no longer valid")]
    KeyRetired(String),
    /// 签名不匹配。
    #[error("ticket signature is invalid")]
    BadSignature,
    /// 作用域不在调用方允许的范围内。
    #[error("ticket scope {0:?} is not accepted here")]
    WrongScope(String),
    /// 已过期。
    #[error("ticket expired")]
    Expired,
    /// 票据属于另一个分享。
    #[error("ticket is for a different share")]
    WrongShare,
    /// QUIC 对端不是票据里的 `rep`。
    #[error("ticket was issued to a different endpoint")]
    WrongPeer,
}

/// `GET /share/keys` 响应里的一项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyEntry {
    /// 密钥 ID。
    pub kid: String,
    /// Ed25519 公钥，32 字节的 base64url。
    pub public_key: String,
    /// 该密钥可用于验签的最后时刻（Unix 秒）；缺省不限。
    ///
    /// 超过该时刻后即使签名正确也拒绝：控制面保证到此时该 kid 签发的票据都已过期。
    #[serde(default)]
    pub not_after: Option<u64>,
}

/// `GET /share/keys` 响应体。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeysResponse {
    /// 当前有效的密钥。
    pub keys: Vec<KeyEntry>,
}

#[derive(Debug, Clone)]
struct RingKey {
    key: PublicKey,
    not_after: Option<u64>,
}

/// 票据验签密钥环。可克隆，克隆体共享同一份内容，宿主刷新后所有持有者立即可见。
#[derive(Debug, Clone, Default)]
pub struct KeyRing {
    inner: Arc<RwLock<HashMap<String, RingKey>>>,
}

impl KeyRing {
    /// 空密钥环。
    pub fn new() -> Self {
        Self::default()
    }

    /// 整体替换为 `entries`（任何一项非法则整体拒绝并保持原状）。返回收录的密钥数。
    pub fn replace(&self, entries: &[KeyEntry]) -> Result<usize, TicketError> {
        let mut next = HashMap::with_capacity(entries.len());
        for entry in entries {
            let raw = B64
                .decode(entry.public_key.as_bytes())
                .map_err(|_| TicketError::Malformed("public key is not base64url"))?;
            let bytes = <[u8; 32]>::try_from(raw.as_slice())
                .map_err(|_| TicketError::Malformed("public key is not 32 bytes"))?;
            let key = PublicKey::from_bytes(&bytes)
                .map_err(|_| TicketError::Malformed("public key is not a valid Ed25519 point"))?;
            next.insert(
                entry.kid.clone(),
                RingKey {
                    key,
                    not_after: entry.not_after,
                },
            );
        }
        let count = next.len();
        *self.inner.write().unwrap_or_else(PoisonError::into_inner) = next;
        Ok(count)
    }

    /// 密钥环是否为空（宿主据此决定是否需要先拉取 `/share/keys`）。
    pub fn is_empty(&self) -> bool {
        self.inner
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .is_empty()
    }

    fn get(&self, kid: &str) -> Option<RingKey> {
        self.inner
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(kid)
            .cloned()
    }
}

/// 解析票据（只做结构解析，**不验签**）。
pub fn parse_ticket(raw: &str) -> Result<Ticket, TicketError> {
    let (payload_b64, sig_b64) = raw
        .split_once('.')
        .ok_or(TicketError::Malformed("missing '.' separator"))?;
    if sig_b64.contains('.') {
        return Err(TicketError::Malformed("too many '.' separators"));
    }
    let payload_bytes = B64
        .decode(payload_b64.as_bytes())
        .map_err(|_| TicketError::Malformed("payload is not base64url"))?;
    let sig_raw = B64
        .decode(sig_b64.as_bytes())
        .map_err(|_| TicketError::Malformed("signature is not base64url"))?;
    let signature = <[u8; 64]>::try_from(sig_raw.as_slice())
        .map_err(|_| TicketError::Malformed("signature is not 64 bytes"))?;
    let payload: TicketPayload = serde_json::from_slice(&payload_bytes)
        .map_err(|_| TicketError::Malformed("payload is not the expected JSON"))?;
    Ok(Ticket {
        raw: raw.to_owned(),
        payload,
        payload_bytes,
        signature,
    })
}

/// 验票时要核对的上下文。
#[derive(Debug, Clone)]
pub struct TicketExpectation<'a> {
    /// 当前 Unix 秒（由调用方注入，便于测试）。
    pub now_unix: u64,
    /// 必须匹配的分享 ID；`None` 不核对（发送方在查到分享前先验票时使用）。
    pub share_id: Option<&'a str>,
    /// QUIC 握手确认的对端 endpoint ID，必须等于票据 `rep`。
    pub peer: EndpointId,
    /// 允许的作用域；其余作用域即使签名有效也以 [`TicketError::WrongScope`] 拒绝。
    pub scopes: &'a [&'a str],
}

/// 验票：版本 → 密钥 → 签名 → 作用域 → 有效期 → 分享 → 对端，任何一项不符即拒绝。
///
/// 先验签再看声明，保证没有通过签名的内容不会影响错误分类。
pub fn verify_ticket<'t>(
    ring: &KeyRing,
    ticket: &'t Ticket,
    expect: &TicketExpectation<'_>,
) -> Result<&'t TicketPayload, TicketError> {
    let payload = &ticket.payload;
    if payload.v != TICKET_VERSION {
        return Err(TicketError::UnsupportedVersion(payload.v));
    }
    let ring_key = ring
        .get(&payload.kid)
        .ok_or_else(|| TicketError::UnknownKey(payload.kid.clone()))?;
    if ring_key.not_after.is_some_and(|t| expect.now_unix > t) {
        return Err(TicketError::KeyRetired(payload.kid.clone()));
    }
    let signature = Signature::from_bytes(&ticket.signature);
    ring_key
        .key
        .verify(&ticket.payload_bytes, &signature)
        .map_err(|_| TicketError::BadSignature)?;
    if !expect.scopes.contains(&payload.scope.as_str()) {
        return Err(TicketError::WrongScope(payload.scope.clone()));
    }
    if expect.now_unix >= payload.exp {
        return Err(TicketError::Expired);
    }
    if expect.share_id.is_some_and(|sid| sid != payload.sid) {
        return Err(TicketError::WrongShare);
    }
    if !expect.peer.to_z32().eq_ignore_ascii_case(&payload.rep) {
        return Err(TicketError::WrongPeer);
    }
    Ok(payload)
}

/// 签发票据（仅供测试与离线工具使用；生产环境由 FluxCloud 签发）。
pub fn sign_ticket(
    secret: &iroh::SecretKey,
    payload: &TicketPayload,
) -> Result<String, TicketError> {
    let json = serde_json::to_vec(payload)
        .map_err(|_| TicketError::Malformed("payload is not serializable"))?;
    let sig = secret.sign(&json);
    Ok(format!(
        "{}.{}",
        B64.encode(&json),
        B64.encode(sig.to_bytes())
    ))
}

/// 把 32 字节公钥编码成 `/share/keys` 使用的 base64url 文本。
pub fn encode_public_key(key: &PublicKey) -> String {
    B64.encode(key.as_bytes())
}
