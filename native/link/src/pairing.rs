//! 配对协议（v2）：一次性配对码 + 临时公钥承诺-揭示 + X25519 ECDH + SAS 短认证串 +
//! Ed25519 身份绑定。
//!
//! # 握手（三次往返）
//! 1. `hello`（发起方 → 响应方）：配对码、发起方身份公钥、**临时公钥与随机数的承诺**
//!    `sha256(eph_I || nonce_I)`、自报的 `name`/`platform`/`app_version`/`initiator_addrs`，
//!    整体由发起方身份私钥签名。响应方回出本次会话全新的临时公钥 `eph_R` 与随机数
//!    `nonce_R`——此刻它还看不到 `eph_I`。
//! 2. `reveal`（发起方 → 响应方）：发起方揭示 `eph_I`/`nonce_I`。响应方核对承诺，算出
//!    `z`、完整转录与 SAS，并用身份私钥对**完整转录**签名回给发起方；发起方验签后
//!    算出同一个 SAS。响应方在这一步才向本机用户展示 SAS。
//! 3. `confirm`：双方用户核对 SAS 后各自批准；响应方放行后两端以同一把链路密钥入册。
//!
//! # 安全模型
//! - **配对码**（6 位数字，TTL 120s，单次使用）：一次性引导凭据，授权本次配对并
//!   限速暴力尝试；本身非长期密钥，泄漏一张过期码无害。错误猜码另受节流器限制
//!   （按请求来源分桶：单一来源 120 秒时窗内至多 `MAX_FAILED_HELLOS` 次；另有
//!   远大于此的全局兜底 `MAX_FAILED_HELLOS_GLOBAL`，专防分布式换源 IP 绕过单
//!   来源阈值，见 `PairingResponder::is_throttled`）。超限时 `handle_hello` 在
//!   查码、验签之前直接拒绝；命中节流时不查码、不消费任何码、不做任何签名
//!   验证运算。`Throttled` 与 `InvalidCode` 各自持有独立错误信息——这个区别
//!   是**故意**向对端暴露的：区分信息本身不构成额外的暴力破解助力（真正限速
//!   的是节流阈值本身），暴露它是为了让发起方 UI 能给出准确提示，而非笼统
//!   显示「配对失败」。配对码经明文 HTTP 传输，因此它只是准入凭据，**不**提供
//!   抗中间人的保证——抗中间人靠下面的承诺-揭示 + SAS 核对。
//! - **承诺-揭示**：发起方先交出临时公钥的承诺，响应方亮出自己的临时值之后发起方才
//!   揭示。中间人要让两端 SAS 相同，必须在看到对一端的揭示之前就为另一端钉死自己的
//!   临时值，两端 SAS 于是相互独立，单次猜中概率为 `10^-6`；配对码单次使用，每次
//!   尝试都要作废一张码并让发起方重新输入，无法在同一会话里反复试探。
//! - **完整转录**：SAS 与链路密钥的 HKDF 输入都是整段转录的摘要——配对码与发起方
//!   自报信息（经 hello 被签名字节的摘要）、双方身份公钥、双方临时公钥、双方随机数、
//!   响应方自报信息的摘要。响应方的身份签名覆盖同一份转录，转录里任何字段在传输中
//!   被改，两端 SAS 与链路密钥就不同，响应方签名也无法通过验证。
//! - **SAS**（6 位，双端肉眼核对）：核对是**双向**的：响应方用户必须在本机核对 SAS 并
//!   显式批准（见 `PairingResponder::set_local_decision`）后，`handle_confirm` 才会放行
//!   ——响应方是配对能否成立的最终把关人，而非仅凭一次性码验证通过就自动登记对端。
//! - **Ed25519 身份绑定**：发起方在 hello 里签名（覆盖码、承诺、自报字段；变长字段按
//!   长度前缀分帧，杜绝拼接歧义），杜绝明文 HTTP 中间人转发时篡改自报字段——尤其
//!   `initiator_addrs`：一旦被篡改会被响应方原样写入 `PeerRecord.candidates` 长期
//!   生效，等于让中间人植入回连地址。响应方在 reveal 回复里对完整转录签名。
//! - **发现指纹固定**：发起方若已从 mDNS 或 `/ping` 得知目标地址的设备指纹，握手里
//!   出示的响应方身份公钥指纹必须与之一致（见 `PairingInitiator::on_hello_response`）。
//! - **版本**：`PAIRING_PROTOCOL_VERSION` 两端必须逐字相等，不做降级——旧版握手没有
//!   承诺，任何兼容分支都会让中间人重新获得操纵 SAS 的机会。
//! - **每对设备独立链路密钥**：`derive_link_key(z, 转录摘要)`，用于后续数据面 HMAC
//!   鉴权，绝不上网络明文。
//!
//! 全部密码学步骤在引擎内完成（wire 层只做 base64 编解码），便于集中审计与单测。

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use rand::{Rng, RngCore};
use tokio::sync::Notify;
use x25519_dalek::{PublicKey, StaticSecret};

use super::address::PeerAddress;
use super::crypto::{
    derive_link_key, derive_sas, eph_commitment, fingerprint, hash_transcript,
    verify_eph_commitment,
};
use super::error::{LinkError, LinkResult};
use super::identity::LinkIdentity;
use super::text::{
    MAX_NAME_CHARS, MAX_PLATFORM_CHARS, MAX_VERSION_CHARS, sanitize_optional, sanitize_text,
};
use super::types::{MAX_DIRECT_CANDIDATES, PeerCandidate, PeerRecord, TransportKind};

/// 配对协议版本。`hello` 与其回复都携带它，两端必须逐字相等，否则一律
/// [`LinkError::UnsupportedVersion`]——不提供向下兼容的握手形态。
pub const PAIRING_PROTOCOL_VERSION: u32 = 2;
/// 配对码有效期（秒）。宿主向用户展示倒计时 / 计算过期时刻时引用它，避免手写字面量漂移。
pub const CODE_TTL_SECS: u64 = 120;
const CODE_TTL: Duration = Duration::from_secs(CODE_TTL_SECS);
/// confirm 会话有效期（自 hello 抵达起算）。
///
/// 反向约束：必须严格大于 `REVEAL_TIMEOUT + LOCAL_DECISION_TIMEOUT`——会话从 hello 起
/// 存活，依次要经历「等待发起方揭示」与「等待响应方用户表态」两个阶段。若
/// `SESSION_TTL` 不够长，`prune_sessions` 会抢在决策窗口关闭之前，把一条仍在等待响应方
/// 用户表态的会话当「过期」删掉：用户点了批准，`handle_confirm` 却已经找不到会话，只能
/// 得到文不对题的 `SessionExpired`。
const SESSION_TTL: Duration = Duration::from_secs(180);
/// hello 之后等待发起方揭示临时公钥的时限。`begin_pairing` 在收到 hello 回复后立即
/// 揭示；拖过这个时限的会话视为放弃并作废，避免决策窗口（锚在揭示时刻）的终点越过
/// `SESSION_TTL`。
const REVEAL_TIMEOUT: Duration = Duration::from_secs(30);
/// 无匹配 hello（猜码）在时窗内、**单一来源**的上限——达到即拒绝该来源的
/// 新 hello（见 `PairingResponder::is_throttled`），防止单一来源在线暴力
/// 猜码。与配对码生命周期**解耦**：错误猜测绝不作废有效码（避免猜码 DoS）。
const MAX_FAILED_HELLOS: usize = 10;
/// 无匹配 hello 在时窗内、**全部来源合计**的上限——独立于按来源分桶的
/// `MAX_FAILED_HELLOS`，专防攻击者用大量不同来源 IP 分布式绕过单来源
/// 阈值。取值远大于单来源阈值：6 位配对码空间为 1e6 种，时窗内命中正确
/// 码的概率仅 200/1e6 = 2e-4，安全上完全可接受；而分布式 DoS 的攻击成本
/// 因此被抬高两个数量级——不再是单机脚本打满一个来源就够，而是必须真的
/// 凑齐这么多不同来源 IP。
const MAX_FAILED_HELLOS_GLOBAL: usize = 200;
/// 失败 hello 的统计时窗（对单来源桶与全局合计都适用）。
const FAILED_HELLO_WINDOW: Duration = Duration::from_secs(120);
/// 单个来源桶内失败 hello 时间戳的硬上限，与 `MAX_FAILED_HELLOS` 无关：
/// `record_failed_hello` 必须始终 push（否则最后一条失败记录永远卡在时窗
/// 尾部不再前移，节流会从「限速」退化成「永久封禁」），这里只用来防止单个
/// 桶长期没有新 hello 时向量无界增长。
const MAX_FAILED_HELLO_RECORDS: usize = 64;
/// 失败 hello 分桶表的桶数硬上限：来源 IP 数量不可控（尤其是被攻击时），
/// 桶表本身必须有界，否则「按来源分桶」会退化成又一个无界内存增长的 DoS
/// 面。达到上限后不再为新来源单独开桶，一律并入共享的 `UNKNOWN_SOURCE_KEY`
/// 桶——仍然计入全局阈值 `MAX_FAILED_HELLOS_GLOBAL`，不会因此绕过节流。
const MAX_FAILURE_BUCKETS: usize = 256;
/// 拿不到请求来源地址时的固定分桶键——即便如此也必须正常计入节流，不能
/// 让「拿不到来源地址」变成绕过节流的手段；桶表满员后的溢出来源也并入
/// 这个桶（见 `MAX_FAILURE_BUCKETS`）。
const UNKNOWN_SOURCE_KEY: &str = "unknown";
/// hello 自报的单条回连地址的长度上限（字节）。
const MAX_ADDRESS_LEN: usize = 256;
/// 等待响应方本机用户核验 SAS 并做出批准/拒绝决策的超时时长。
///
/// 不变量：锚点是揭示被受理的时刻（即 `IncomingPairing` 广播给本机 UI 的时刻），与
/// 两端客户端的倒计时同源——响应方/发起方两个客户端在收到（或广播）`IncomingPairing`
/// 后各自的 60 秒 UI 倒计时都从这一刻起算，`PairingResponder::handle_confirm` 的决策
/// 截止时间必须锚在同一个时刻（`SessionStage::Revealed::announced`），而不是「本次
/// confirm 抵达时刻」——否则发起方核对 SAS 耗时越久，响应方 UI 弹窗与 backend 等待
/// 窗口错位越大：UI 早已按自己的倒计时自动关闭，backend 却还在傻等一个不会再来的决策。
/// `REVEAL_TIMEOUT + LOCAL_DECISION_TIMEOUT` 必须严格小于 `SESSION_TTL`（见该常量上方
/// 反向约束注释）。
/// 响应方用户核验窗口（秒）：从揭示被受理的时刻起算，超时后 confirm 一律 `PairingTimeout`。
pub const DECISION_WINDOW_SECS: u64 = 60;
const LOCAL_DECISION_TIMEOUT: Duration = Duration::from_secs(DECISION_WINDOW_SECS);

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 把一个变长字节串以「4 字节小端长度前缀 + 内容」写入 `t`——长度前缀分帧，
/// 避免多个变长字段直接拼接产生的边界歧义（如 `"ab"+"c"` 与 `"a"+"bc"` 拼接后
/// 字节完全相同）。
fn push_framed(t: &mut Vec<u8>, bytes: &[u8]) {
    t.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    t.extend_from_slice(bytes);
}

/// 握手转录各部分的域分隔标签。
const HELLO_DOMAIN: &[u8] = b"fluxdown-link-hello-v2";
const SESSION_DOMAIN: &[u8] = b"fluxdown-link-session-v2";
const RESPONDER_SIG_DOMAIN: &[u8] = b"fluxdown-link-resp-v2";
const RESPONDER_INFO_DOMAIN: &[u8] = b"fluxdown-link-resp-info-v2";
/// 「请求完全控制」标志的签名域标签：仅当 hello 带 `request_control` 时追加进被签名字节。
const CONTROL_DOMAIN: &[u8] = b"fluxdown-link-control-v1";

/// 发起方 hello 的被签名字节：配对码、发起方身份公钥、临时公钥承诺，以及发起方自报的
/// `name`/`platform`/`app_version`/`initiator_addrs`。后四者若不入签，明文
/// HTTP 中间人可在转发时任意篡改而不影响 SAS——尤其 `initiator_addrs` 会被
/// 响应方原样写入 `PeerRecord.candidates` 长期生效，等于让中间人植入回连
/// 地址。全部变长字段均按 `push_framed` 长度前缀分帧；`initiator_addrs` 额外
/// 先写元素个数再逐个分帧写入；`platform`/`app_version` 用空串归一 `None`
/// （发起方与响应方必须对同一组字段算出完全相同的字节串，否则验签
/// 必然失败——两处调用点见 `PairingInitiator::build_hello` 与
/// `PairingResponder::handle_hello`）。这段字节的摘要同时是完整转录里「配对码 +
/// 发起方自报信息」的来源。
///
/// `request_control == false` 时输出与未引入该标志之前**逐字节相同**（旧版对端照常验签）；
/// `true` 时在最后追加 `framed(CONTROL_DOMAIN)`——标志因此进入 `hello_digest` → 转录 →
/// SAS / 链路密钥，被中间人增删或篡改都会让配对失败。签名不覆盖 `protocol_version` 与
/// `initiator_sig`。
fn hello_message(req: &HelloRequest) -> Vec<u8> {
    let mut t = Vec::with_capacity(256);
    t.extend_from_slice(HELLO_DOMAIN);
    push_framed(&mut t, req.code.as_bytes());
    t.extend_from_slice(&req.initiator_id_pub);
    t.extend_from_slice(&req.initiator_commit);
    push_framed(&mut t, req.name.as_bytes());
    push_framed(&mut t, req.platform.as_deref().unwrap_or("").as_bytes());
    push_framed(&mut t, req.app_version.as_deref().unwrap_or("").as_bytes());
    t.extend_from_slice(&(req.initiator_addrs.len() as u32).to_le_bytes());
    for addr in &req.initiator_addrs {
        push_framed(&mut t, addr.as_bytes());
    }
    if req.request_control {
        push_framed(&mut t, CONTROL_DOMAIN);
    }
    t
}

/// 响应方自报信息（`name`/`platform`/`app_version`）的摘要，纳入完整转录——这些字段在
/// hello 回复里明文传输，发起方会把它们写进名册，必须被响应方的转录签名覆盖。
fn responder_info_digest(
    name: &str,
    platform: Option<&str>,
    app_version: Option<&str>,
) -> [u8; 32] {
    let mut t = Vec::with_capacity(96);
    t.extend_from_slice(RESPONDER_INFO_DOMAIN);
    push_framed(&mut t, name.as_bytes());
    push_framed(&mut t, platform.unwrap_or("").as_bytes());
    push_framed(&mut t, app_version.unwrap_or("").as_bytes());
    hash_transcript(&t)
}

/// 一次握手的完整转录。所有成员都是定长 32 字节，按固定顺序拼接，不存在分帧歧义。
/// 两端各自从自己看到的值构造它：SAS、链路密钥与响应方签名都只依赖它（加上 `z`），
/// 所以任意一个成员在传输中被改，两端结果就不同。
struct Transcript {
    /// `sha256(hello_message)`：配对码 + 发起方身份公钥 + 承诺 + 发起方自报信息。
    hello_digest: [u8; 32],
    initiator_id_pub: [u8; 32],
    responder_id_pub: [u8; 32],
    initiator_eph_pub: [u8; 32],
    responder_eph_pub: [u8; 32],
    initiator_nonce: [u8; 32],
    responder_nonce: [u8; 32],
    responder_info_digest: [u8; 32],
}

impl Transcript {
    fn bytes(&self) -> Vec<u8> {
        let mut t = Vec::with_capacity(SESSION_DOMAIN.len() + 8 * 32);
        t.extend_from_slice(SESSION_DOMAIN);
        for part in [
            &self.hello_digest,
            &self.initiator_id_pub,
            &self.responder_id_pub,
            &self.initiator_eph_pub,
            &self.responder_eph_pub,
            &self.initiator_nonce,
            &self.responder_nonce,
            &self.responder_info_digest,
        ] {
            t.extend_from_slice(part);
        }
        t
    }

    /// SAS 与链路密钥 HKDF 的输入。
    fn hash(&self) -> [u8; 32] {
        hash_transcript(&self.bytes())
    }

    /// 响应方身份私钥签名的字节：域分隔串 + 完整转录。
    fn responder_signing_message(&self) -> Vec<u8> {
        let mut m = Vec::with_capacity(RESPONDER_SIG_DOMAIN.len() + SESSION_DOMAIN.len() + 8 * 32);
        m.extend_from_slice(RESPONDER_SIG_DOMAIN);
        m.extend_from_slice(&self.bytes());
        m
    }
}

/// 本机在配对响应与数据面信息交换中呈现的自身信息。设备名与云设备 ID 在运行期可变（改名、
/// 云账号登录 / 登出），宿主经 `LinkManager::update_self` 更新。
#[derive(Debug, Clone, Default)]
pub struct SelfInfo {
    pub name: String,
    pub platform: Option<String>,
    pub app_version: Option<String>,
    /// 本机已登录云账号时的云设备 ID（`state.device_id`），未登录为 `None`。**只经已认证的
    /// 数据面信息交换告知已配对对端**：绝不进 mDNS、`/ping`、配对 hello。
    pub cloud_device_id: Option<String>,
}

/// 发起方 `hello` 请求（byte-oriented；wire 层负责 base64 编解码）。
#[derive(Debug, Clone)]
pub struct HelloRequest {
    /// 发起方的配对协议版本，必须等于 [`PAIRING_PROTOCOL_VERSION`]。
    pub protocol_version: u32,
    pub code: String,
    pub initiator_id_pub: [u8; 32],
    /// 发起方临时公钥与随机数的承诺（见 [`crate::crypto::eph_commitment`]）。
    pub initiator_commit: [u8; 32],
    pub initiator_sig: [u8; 64],
    pub name: String,
    pub platform: Option<String>,
    pub app_version: Option<String>,
    /// 发起方自报的可达候选地址（`ip:port`），供响应方存为回连候选。
    pub initiator_addrs: Vec<String>,
    /// 发起方请求「完全控制」本机（受签名保护，见 [`hello_message`]）。
    pub request_control: bool,
}

/// 响应方 `hello` 回复：本次会话的临时公钥与随机数。不含 SAS、不含签名——响应方此刻
/// 还不知道发起方的临时公钥，SAS 与转录签名在 `reveal` 之后才产生。
#[derive(Debug, Clone)]
pub struct HelloResponse {
    /// 响应方的配对协议版本，必须等于 [`PAIRING_PROTOCOL_VERSION`]。
    pub protocol_version: u32,
    pub session_id: String,
    pub responder_eph_pub: [u8; 32],
    pub responder_nonce: [u8; 32],
    pub responder_id_pub: [u8; 32],
    pub name: String,
    pub platform: Option<String>,
    pub app_version: Option<String>,
}

/// 发起方 `reveal` 请求：揭示 hello 里承诺过的临时公钥与随机数。
#[derive(Debug, Clone)]
pub struct RevealRequest {
    pub session_id: String,
    pub initiator_eph_pub: [u8; 32],
    pub initiator_nonce: [u8; 32],
}

/// 响应方 `reveal` 回复（上线的部分）：响应方对完整转录的身份签名。
#[derive(Debug, Clone)]
pub struct RevealResponse {
    pub responder_sig: [u8; 64],
}

/// 响应方受理一次 `reveal` 的结果：回给发起方的 [`RevealResponse`]，以及仅留在本机、
/// 供 UI 核验的 SAS 与发起方信息。
#[derive(Debug, Clone)]
pub struct RevealAccepted {
    pub response: RevealResponse,
    pub sas: String,
    pub peer_name: String,
    pub peer_platform: Option<String>,
    pub peer_id_pub: [u8; 32],
    /// 发起方在 hello 里请求了「完全控制」本机。
    pub request_control: bool,
}

/// 会话所处阶段。
enum SessionStage {
    /// hello 已受理，等待发起方揭示临时公钥与随机数。
    AwaitingReveal {
        commit: [u8; 32],
        eph_secret: StaticSecret,
        nonce: [u8; 32],
        hello_digest: [u8; 32],
        /// hello 回复里出示的响应方自报信息的摘要（hello 当刻定格）：reveal 时的转录必须
        /// 与发起方据 hello 回复算出的一致，即使本机在两步之间改了设备名。
        responder_info_digest: [u8; 32],
    },
    /// 承诺已核对、SAS 已展示给本机用户，等待双方 confirm 与本机用户表态。
    Revealed {
        z: [u8; 32],
        transcript_hash: [u8; 32],
        /// 揭示被受理（`IncomingPairing` 广播）的时刻，本机用户核验窗口的锚点。
        announced: Instant,
        /// 本机用户对该入站会话的核验结论：`None` = 尚未表态；`Some(true)` = 批准；
        /// `Some(false)` = 拒绝。由 `PairingResponder::set_local_decision` 写入，
        /// `handle_confirm` 据此放行或拒绝——响应方用户是配对成立与否的最终把关人。
        decision: Option<bool>,
    },
}

/// 一条入站会话（hello 已完成，等待揭示 → SAS 核对 + confirm）。
struct ConfirmEntry {
    initiator_id_pub: [u8; 32],
    /// 已清洗的发起方自报名 / 平台 / 版本（签名覆盖的是原始值，入册与展示用清洗后的值）。
    name: String,
    platform: Option<String>,
    app_version: Option<String>,
    /// 已校验并规范化的回连地址（至多 [`MAX_DIRECT_CANDIDATES`] 条）。
    initiator_addrs: Vec<String>,
    /// hello 抵达时刻，会话总寿命（`SESSION_TTL`）与揭示时限（`REVEAL_TIMEOUT`）的锚点。
    created: Instant,
    /// 发起方在（已验签的）hello 里请求了「完全控制」本机：本机用户批准即授权。
    request_control: bool,
    stage: SessionStage,
}

/// 一个已生成、尚未消费的配对码。
struct CodeEntry {
    code: String,
    created: Instant,
}

/// 剪枝配对码表：清除超过 `CODE_TTL` 的过期码。抽成独立函数是因为同一逻辑
/// 在生成新码、`handle_hello` 查码、后台 GC（见 `PairingResponder::
/// prune_expired`）三处都要用到，避免各处各写一份容易漂移的 retain。
fn prune_codes(codes: &mut Vec<CodeEntry>) {
    codes.retain(|c| c.created.elapsed() < CODE_TTL);
}

/// 剪枝待确认会话表：清除超过 `SESSION_TTL` 的过期会话。同上，`handle_hello`
/// 建会话、`set_local_decision`、`handle_confirm` 的两处以及后台 GC 都复用它。
fn prune_sessions(sessions: &mut HashMap<String, ConfirmEntry>) {
    sessions.retain(|_, e| e.created.elapsed() < SESSION_TTL);
}

/// 把请求来源标准化为失败 hello 的节流分桶键。`None`（宿主层拿不到来源
/// 地址，如某些部署形态取不到连接对端 IP）统一归入固定的
/// `UNKNOWN_SOURCE_KEY` 桶——不能因为拿不到地址就放弃对这类请求节流。
///
/// IPv4-mapped IPv6（`::ffff:a.b.c.d`）与对应的 IPv4 是同一来源；IPv6 按 /64 归一——
/// 一个 /64 前缀内的隐私地址可以任意轮换，按 /128 分桶等于没有单来源限额。
fn failure_bucket_key(source: Option<IpAddr>) -> String {
    let Some(ip) = source else {
        return UNKNOWN_SOURCE_KEY.to_string();
    };
    let ip = match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
        v4 @ IpAddr::V4(_) => v4,
    };
    match ip {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => {
            let s = v6.segments();
            format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
        }
    }
}

/// 校验 hello 自报的回连地址：至多 [`MAX_DIRECT_CANDIDATES`] 条，且每条都能被
/// [`PeerAddress::parse`] 解析；返回规范化（去重）后的候选形态。这些地址会被写进名册、此后
/// 周期性被本机探测，来源虽经签名覆盖，但发起方是任何持有配对码的人，必须当作不可信输入。
fn validate_initiator_addrs(addrs: &[String]) -> LinkResult<Vec<String>> {
    if addrs.len() > MAX_DIRECT_CANDIDATES {
        return Err(LinkError::BadPayload(
            "too many initiator addresses".to_owned(),
        ));
    }
    let mut canonical: Vec<String> = Vec::with_capacity(addrs.len());
    for addr in addrs {
        if addr.len() > MAX_ADDRESS_LEN {
            return Err(LinkError::BadPayload(
                "initiator address too long".to_owned(),
            ));
        }
        let parsed = PeerAddress::parse(addr)
            .map_err(|_| LinkError::BadPayload("invalid initiator address".to_owned()))?;
        let candidate = parsed.to_candidate();
        if !canonical.contains(&candidate) {
            canonical.push(candidate);
        }
    }
    Ok(canonical)
}

/// 剪枝失败 hello 分桶表：每个来源桶按 `FAILED_HELLO_WINDOW` 剪掉过期
/// 记录，剪完为空的桶整个删除——否则见过一次的来源 IP 会在桶表里永久占
/// 一个空位，配合 `MAX_FAILURE_BUCKETS` 就会把桶表挤满，后来的新来源被
/// 迫并入 `UNKNOWN_SOURCE_KEY` 桶，节流粒度退化。`record_failed_hello`、
/// `is_throttled` 与后台 GC 复用它。
fn prune_failures(failures: &mut HashMap<String, Vec<Instant>>) {
    failures.retain(|_, bucket| {
        bucket.retain(|t| t.elapsed() < FAILED_HELLO_WINDOW);
        !bucket.is_empty()
    });
}

/// 常量时间判断字节串是否全为零：按位或全部字节而不提前 short-circuit，
/// 避免比较耗时随「第一个非零字节出现的位置」变化而产生时序侧信道。仅用于
/// 检测退化 ECDH 输出（见 `handle_hello`/`PairingInitiator::on_hello_response`），
/// 不是通用的常量时间比较原语。
fn is_all_zero(bytes: &[u8]) -> bool {
    bytes.iter().fold(0u8, |acc, b| acc | b) == 0
}

/// 配对**响应方**（被添加的设备侧）。持有一次性配对码与待确认会话表。
pub struct PairingResponder {
    identity: LinkIdentity,
    /// 本机自报信息：设备名可在运行期更新（改名 / 云登录后同步），所以放在锁里。
    self_info: RwLock<SelfInfo>,
    codes: Mutex<Vec<CodeEntry>>,
    sessions: Mutex<HashMap<String, ConfirmEntry>>,
    /// 失败 hello（猜码）时间戳，按 [`failure_bucket_key`] 分桶——与配对码
    /// 解耦的节流器（防在线暴力猜码 DoS）。见 `MAX_FAILED_HELLOS`（单来源
    /// 阈值）与 `MAX_FAILED_HELLOS_GLOBAL`（全局兜底）。
    failures: Mutex<HashMap<String, Vec<Instant>>>,
    /// 本机用户对某会话作出核验决策时的唤醒信号：`handle_confirm` 等待期间
    /// 挂在这上面，`set_local_decision` 写入决策后 `notify_waiters` 唤醒。
    decision_notify: Arc<Notify>,
}

impl PairingResponder {
    #[must_use]
    pub fn new(identity: LinkIdentity, self_info: SelfInfo) -> Self {
        Self {
            identity,
            self_info: RwLock::new(self_info),
            codes: Mutex::new(Vec::new()),
            sessions: Mutex::new(HashMap::new()),
            failures: Mutex::new(HashMap::new()),
            decision_notify: Arc::new(Notify::new()),
        }
    }

    /// 本机当前自报信息（快照）。
    #[must_use]
    pub fn self_info(&self) -> SelfInfo {
        match self.self_info.read() {
            Ok(info) => info.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// 更新本机自报信息（设备名改变后调用）；只影响之后的新会话，进行中的会话沿用 hello 当刻
    /// 定格的自报信息。
    pub fn set_self_info(&self, info: SelfInfo) {
        match self.self_info.write() {
            Ok(mut current) => *current = info,
            Err(poisoned) => *poisoned.into_inner() = info,
        }
    }

    /// 记录一次失败 hello（无匹配码），按 [`failure_bucket_key`] 分桶。按
    /// 时窗剪枝后**始终**记录一条——若达到上限就不再记录，最后一条失败记录
    /// 会永远卡在时窗尾部、不再随时间前移，节流就从「限速」退化成「永久
    /// 封禁」。单桶大小用与 `MAX_FAILED_HELLOS` 无关的硬上限
    /// `MAX_FAILED_HELLO_RECORDS` 截断，仅防止向量无界增长；桶表本身也有
    /// 界（`MAX_FAILURE_BUCKETS`）——到达上限后不再为新来源单独开桶，一律
    /// 并入共享的 `UNKNOWN_SOURCE_KEY` 桶，否则攻击者只需不断更换来源
    /// 地址就能让分桶节流形同虚设。与配对码解耦：错误猜测绝不影响任何
    /// 有效码的生命周期。
    fn record_failed_hello(&self, source: Option<IpAddr>) {
        let key = failure_bucket_key(source);
        if let Ok(mut failures) = self.failures.lock() {
            prune_failures(&mut failures);
            let key = if failures.contains_key(&key) || failures.len() < MAX_FAILURE_BUCKETS {
                key
            } else {
                UNKNOWN_SOURCE_KEY.to_string()
            };
            let bucket = failures.entry(key).or_default();
            if bucket.len() >= MAX_FAILED_HELLO_RECORDS {
                bucket.remove(0);
            }
            bucket.push(Instant::now());
        }
    }

    /// 当前来源是否处于猜码节流状态：**该来源**时窗内失败次数达到
    /// `MAX_FAILED_HELLOS`，或**全部来源合计**达到 `MAX_FAILED_HELLOS_GLOBAL`
    /// （分布式猜码兜底）。`handle_hello` 在查码之前调用；命中时不查码、不
    /// 验签、不消费任何码。`Throttled` 与 `InvalidCode` 各自持有独立错误
    /// 信息——这个区别是**故意**向对端暴露的，用于发起方 UI 给出准确提示。
    fn is_throttled(&self, source: Option<IpAddr>) -> bool {
        let key = failure_bucket_key(source);
        let Ok(mut failures) = self.failures.lock() else {
            return false;
        };
        prune_failures(&mut failures);
        let global: usize = failures.values().map(Vec::len).sum();
        if global >= MAX_FAILED_HELLOS_GLOBAL {
            return true;
        }
        failures
            .get(&key)
            .is_some_and(|bucket| bucket.len() >= MAX_FAILED_HELLOS)
    }

    /// 生成一个新配对码（在被添加设备的 UI 上展示，2 分钟内有效、单次使用）。
    pub fn generate_code(&self) -> String {
        let code = format!("{:06}", rand::rng().random_range(0..1_000_000u32));
        if let Ok(mut codes) = self.codes.lock() {
            // 清空而非仅剪枝过期码：生成新码即令旧码立即失效。同一响应方
            // 同一时刻只应有一个有效码，否则用户点「刷新配对码」后，已被
            // 窥屏泄露的旧码仍会在自身 120s 窗口内保持可用，与用户直觉相悖。
            codes.clear();
            codes.push(CodeEntry {
                code: code.clone(),
                created: Instant::now(),
            });
        }
        code
    }

    /// 立即作废当前展示的配对码（用户关闭配对界面 / 主动停止配对时调用）。
    pub fn revoke_codes(&self) {
        if let Ok(mut codes) = self.codes.lock() {
            codes.clear();
        }
    }

    /// 处理 `hello`：版本 → 输入校验 → 节流 → 自配对 → 查码 → 验发起方签名 → **签名通过后才
    /// 消费配对码** → 建会话并回出本次会话全新的临时公钥与随机数。此刻只有发起方临时公钥的
    /// 承诺，没有 ECDH、没有 SAS——它们在 [`Self::handle_reveal`] 里产生。
    /// `source`：请求来源地址，用于按来源分桶节流（见 [`failure_bucket_key`]）；
    /// 宿主层拿不到时传 `None`，仍会计入固定的 `UNKNOWN_SOURCE_KEY` 桶。
    pub fn handle_hello(
        &self,
        req: &HelloRequest,
        source: Option<IpAddr>,
    ) -> LinkResult<HelloResponse> {
        // 版本门禁在最前面：版本不符的请求既不是一次猜码，也不该消耗任何状态——
        // 不计入节流、不触碰配对码，让旧版发起方得到明确的版本不兼容提示。
        if req.protocol_version != PAIRING_PROTOCOL_VERSION {
            return Err(LinkError::UnsupportedVersion);
        }
        // 结构性输入校验（廉价、不碰任何状态）：回连地址条数 / 可解析性。自报的 name /
        // platform / app_version 不拒绝，而是在通过验签后清洗（签名覆盖的是原始值）。
        let initiator_addrs = validate_initiator_addrs(&req.initiator_addrs)?;
        // 节流检查放在查码与验签之前：命中时立即返回，既不消费任何
        // 配对码也不做任何签名验证运算。`Throttled` 与 `InvalidCode` 各自
        // 持有独立错误信息——这个区别是**故意**向对端暴露的：区分信息本身
        // 不构成额外的暴力破解助力（真正限速的是节流阈值本身），暴露它是
        // 为了让发起方 UI 能给出准确提示，而非笼统地显示「配对失败」。
        if self.is_throttled(source) {
            return Err(LinkError::Throttled);
        }
        // 自配对守卫：发起方与本机持同一长期身份（典型场景：两个进程共享同一
        // 引擎数据库）——它们本就是同一台设备，直接拒绝，不消费配对码。
        if req.initiator_id_pub == self.identity.public_bytes() {
            return Err(LinkError::SelfPairing);
        }
        // 先只**查**码、不消费：码匹配只说明对方知道码，还不能证明 hello 是完整、未被篡改的。
        // 若此时就消费，持码（或窥屏到码）的人发一个签名错误的 hello 就能烧掉合法用户的码。
        {
            let mut codes = self.codes.lock().map_err(|_| LinkError::Unavailable)?;
            prune_codes(&mut codes);
            if !codes.iter().any(|c| c.code == req.code) {
                // 无匹配：仅记一次失败到解耦的全局节流器，**绝不**作废有效码（修复猜码 DoS）。
                drop(codes);
                self.record_failed_hello(source);
                return Err(LinkError::InvalidCode);
            }
        }

        // 验发起方身份签名：绑定其长期身份、临时公钥承诺，以及自报的
        // name/platform/app_version/initiator_addrs（防中间人篡改自报字段）。
        let message = hello_message(req);
        if !LinkIdentity::verify(&req.initiator_id_pub, &message, &req.initiator_sig) {
            // 签名错误的 hello 不消费码，但同样计入节流：否则持码者可以无限次地用错误签名试探。
            self.record_failed_hello(source);
            return Err(LinkError::BadSignature);
        }

        // 签名通过 → 消费匹配的有效码（消费即移除，杜绝重放）。两个并发的有效 hello 抢同一张码时
        // 只有先到者成功。
        //
        // 残余风险（评估为可接受，不做额外处理）：hello 请求本身明文传输，
        // 若攻击者已具备局域网内的流量可见性（中间人位置），可以原样重放
        // 抓到的 hello 抢先消费掉这个一次性码，导致真正的发起方随后收到
        // InvalidCode——效果仅是一次性拒绝服务式骚扰，用户重新生成码即可。
        // 攻击者拿不到发起方的临时私钥（hello 里只有承诺，临时公钥直到
        // reveal 才出现），推不出共享密钥 z / link_secret。
        {
            let mut codes = self.codes.lock().map_err(|_| LinkError::Unavailable)?;
            prune_codes(&mut codes);
            let Some(pos) = codes.iter().position(|c| c.code == req.code) else {
                return Err(LinkError::InvalidCode);
            };
            codes.remove(pos);
        }

        // 通过验签后才清洗自报文本：入册与展示用清洗后的值，转录仍按签名覆盖的原始值。
        let peer_name = {
            let cleaned = sanitize_text(&req.name, MAX_NAME_CHARS);
            if cleaned.is_empty() {
                fingerprint(&req.initiator_id_pub)
                    .chars()
                    .take(12)
                    .collect()
            } else {
                cleaned
            }
        };
        let peer_platform = sanitize_optional(req.platform.as_deref(), MAX_PLATFORM_CHARS);
        let peer_version = sanitize_optional(req.app_version.as_deref(), MAX_VERSION_CHARS);

        // 临时密钥与随机数在 hello 被验证之后、针对这一次会话全新生成——不随配对码
        // 复用，也先于揭示。
        let mut seed = [0u8; 32];
        rand::rng().fill_bytes(&mut seed);
        let eph_secret = StaticSecret::from(seed);
        let responder_eph_pub = PublicKey::from(&eph_secret).to_bytes();
        let mut nonce = [0u8; 32];
        rand::rng().fill_bytes(&mut nonce);
        let session_id = uuid::Uuid::new_v4().simple().to_string();
        let me = self.self_info();
        let info_digest =
            responder_info_digest(&me.name, me.platform.as_deref(), me.app_version.as_deref());

        {
            let mut sessions = self.sessions.lock().map_err(|_| LinkError::Unavailable)?;
            prune_sessions(&mut sessions);
            sessions.insert(
                session_id.clone(),
                ConfirmEntry {
                    initiator_id_pub: req.initiator_id_pub,
                    name: peer_name,
                    platform: peer_platform,
                    app_version: peer_version,
                    initiator_addrs,
                    request_control: req.request_control,
                    created: Instant::now(),
                    stage: SessionStage::AwaitingReveal {
                        commit: req.initiator_commit,
                        eph_secret,
                        nonce,
                        hello_digest: hash_transcript(&message),
                        responder_info_digest: info_digest,
                    },
                },
            );
        }

        Ok(HelloResponse {
            protocol_version: PAIRING_PROTOCOL_VERSION,
            session_id,
            responder_eph_pub,
            responder_nonce: nonce,
            responder_id_pub: self.identity.public_bytes(),
            name: me.name,
            platform: me.platform,
            app_version: me.app_version,
        })
    }

    /// 处理 `reveal`：核对发起方揭示的临时公钥/随机数与 hello 里的承诺一致 → ECDH →
    /// 构造完整转录 → 算出 SAS → 用身份私钥对转录签名。
    ///
    /// 承诺不符立即作废会话（[`LinkError::CommitmentMismatch`]），不留第二次尝试的机会；
    /// 超过 `REVEAL_TIMEOUT` 才到的 reveal 同样作废（`SessionExpired`）。只有仍在等待
    /// 揭示的会话才受理：重放的 reveal 不会生效，也不会误伤已经揭示的会话。
    pub fn handle_reveal(&self, req: &RevealRequest) -> LinkResult<RevealAccepted> {
        let entry = {
            let mut sessions = self.sessions.lock().map_err(|_| LinkError::Unavailable)?;
            prune_sessions(&mut sessions);
            let awaiting = sessions
                .get(&req.session_id)
                .is_some_and(|e| matches!(e.stage, SessionStage::AwaitingReveal { .. }));
            if !awaiting {
                return Err(LinkError::SessionExpired);
            }
            sessions.remove(&req.session_id)
        };
        let Some(ConfirmEntry {
            initiator_id_pub,
            name,
            platform,
            app_version,
            initiator_addrs,
            created,
            request_control,
            stage:
                SessionStage::AwaitingReveal {
                    commit,
                    eph_secret,
                    nonce,
                    hello_digest,
                    responder_info_digest,
                },
        }) = entry
        else {
            return Err(LinkError::SessionExpired);
        };
        if created.elapsed() > REVEAL_TIMEOUT {
            return Err(LinkError::SessionExpired);
        }
        if !verify_eph_commitment(&commit, &req.initiator_eph_pub, &req.initiator_nonce) {
            return Err(LinkError::CommitmentMismatch);
        }

        let z = eph_secret
            .diffie_hellman(&PublicKey::from(req.initiator_eph_pub))
            .to_bytes();
        // 纵深防御：x25519_dalek::PublicKey::from([u8; 32]) 不对输入做点
        // 校验，退化/小子群公钥会导致协商出全零共享密钥（与对端私钥无关）。
        // 拒绝退化点成本为零。
        if is_all_zero(&z) {
            return Err(LinkError::BadPayload("degenerate x25519 public key".into()));
        }
        let transcript = Transcript {
            hello_digest,
            initiator_id_pub,
            responder_id_pub: self.identity.public_bytes(),
            initiator_eph_pub: req.initiator_eph_pub,
            responder_eph_pub: PublicKey::from(&eph_secret).to_bytes(),
            initiator_nonce: req.initiator_nonce,
            responder_nonce: nonce,
            responder_info_digest,
        };
        let transcript_hash = transcript.hash();
        let sas = derive_sas(&z, &transcript_hash);
        let responder_sig = self.identity.sign(&transcript.responder_signing_message());

        {
            let mut sessions = self.sessions.lock().map_err(|_| LinkError::Unavailable)?;
            sessions.insert(
                req.session_id.clone(),
                ConfirmEntry {
                    initiator_id_pub,
                    name: name.clone(),
                    platform: platform.clone(),
                    app_version,
                    initiator_addrs,
                    request_control,
                    created,
                    stage: SessionStage::Revealed {
                        z,
                        transcript_hash,
                        announced: Instant::now(),
                        decision: None,
                    },
                },
            );
        }

        Ok(RevealAccepted {
            response: RevealResponse { responder_sig },
            sas,
            peer_name: name,
            peer_platform: platform,
            peer_id_pub: initiator_id_pub,
            request_control,
        })
    }

    /// 记录本机用户对某入站会话的核验结论——响应方 UI 让用户核对 SAS 后调用，
    /// 批准传 `true`、拒绝传 `false`；写入后唤醒正在等待的 `handle_confirm`。
    /// 会话不存在、已过期或尚未揭示（用户还看不到 SAS）→ `Err(LinkError::SessionExpired)`。
    pub fn set_local_decision(&self, session_id: &str, accept: bool) -> LinkResult<()> {
        {
            let mut sessions = self.sessions.lock().map_err(|_| LinkError::Unavailable)?;
            prune_sessions(&mut sessions);
            let entry = sessions
                .get_mut(session_id)
                .ok_or(LinkError::SessionExpired)?;
            let SessionStage::Revealed { decision, .. } = &mut entry.stage else {
                return Err(LinkError::SessionExpired);
            };
            *decision = Some(accept);
        }
        self.decision_notify.notify_waiters();
        Ok(())
    }

    /// 从会话表中移除一条会话——超时/拒绝/confirm 完成后清理，避免残留占用。移除后唤醒所有
    /// 等待决策的 `handle_confirm`：它们会发现会话已不在并立即返回，不必睡到各自的截止时间。
    fn remove_session(&self, session_id: &str) {
        if let Ok(mut sessions) = self.sessions.lock() {
            sessions.remove(session_id);
        }
        self.decision_notify.notify_waiters();
    }

    /// 处理 `confirm`：SAS 核对现在是**双向**的。`confirm=false`（发起方一侧
    /// 用户发现 SAS 不符或主动取消）→ 直接丢弃会话（并立即唤醒仍在等待本机决策的
    /// `confirm=true` 请求），返回 `None`。
    /// `confirm=true` 时不会立即放行：还必须等待响应方本机用户通过
    /// `Self::set_local_decision` 核对 SAS 并明确批准——响应方是这场配对能否
    /// 成立的最终把关人，而非一次性码验证通过就自动登记对端。最多等待
    /// `LOCAL_DECISION_TIMEOUT`（60 秒）：超时 → `Err(PairingTimeout)`；本机
    /// 用户拒绝 → `Err(RejectedByPeer)`；批准 → 登记并返回发起方设备记录。
    /// 本机用户**已经表态**的会话不再受 60 秒核验窗口约束（窗口只约束「等待用户表态」）：
    /// 用户及时批准、发起方的 confirm 稍后才到，配对仍然成立（会话总寿命 `SESSION_TTL` 兜底）。
    /// 尚未揭示就 `confirm=true` 是协议违规：会话作废并返回 `BadPayload`。
    pub async fn handle_confirm(
        &self,
        session_id: &str,
        confirm: bool,
    ) -> LinkResult<Option<PeerRecord>> {
        if !confirm {
            let existed = {
                let mut sessions = self.sessions.lock().map_err(|_| LinkError::Unavailable)?;
                prune_sessions(&mut sessions);
                sessions.remove(session_id).is_some()
            };
            self.decision_notify.notify_waiters();
            return if existed {
                Ok(None)
            } else {
                Err(LinkError::SessionExpired)
            };
        }

        // 决策截止时间锚定在揭示被受理的时刻（`IncomingPairing` 广播时刻），而不是
        // 本次 confirm 抵达时刻——与两端客户端「收到/广播 IncomingPairing 后从这一刻
        // 起算 60 秒倒计时」严格同源（不变量见 LOCAL_DECISION_TIMEOUT 上方注释）。
        // 发起方核对 SAS 到点 confirm 之间可能耗费任意时长：若仍从本次 confirm 抵达
        // 时刻重新起算，响应方 UI 弹窗早已按自己的倒计时自动关闭，backend 却还在
        // 傻等另一个 60s，用户在场也点不到已经消失的弹窗。
        let (existing_decision, deadline) = {
            let mut sessions = self.sessions.lock().map_err(|_| LinkError::Unavailable)?;
            prune_sessions(&mut sessions);
            let entry = sessions.get(session_id).ok_or(LinkError::SessionExpired)?;
            let revealed = match &entry.stage {
                SessionStage::Revealed {
                    announced,
                    decision,
                    ..
                } => Some((*decision, *announced + LOCAL_DECISION_TIMEOUT)),
                SessionStage::AwaitingReveal { .. } => None,
            };
            let Some(revealed) = revealed else {
                sessions.remove(session_id);
                return Err(LinkError::BadPayload(
                    "pairing confirm received before reveal".into(),
                ));
            };
            revealed
            // MutexGuard 在此作用域结束时 drop。
        };
        let decision = match existing_decision {
            // 已经表态（批准或拒绝）：直接采用，不再检查核验窗口。
            Some(decision) => decision,
            None => {
                // 窗口已过：两端客户端此刻也早已按同源倒计时自动关闭了 UI，不必再
                // 进入下面的等待循环——立即清理会话并返回超时，而不是傻等一个不
                // 可能再来的决策。
                if deadline.checked_duration_since(Instant::now()).is_none() {
                    self.remove_session(session_id);
                    return Err(LinkError::PairingTimeout);
                }
                self.await_decision(session_id, deadline).await?
            }
        };

        let entry = {
            let mut sessions = self.sessions.lock().map_err(|_| LinkError::Unavailable)?;
            sessions.remove(session_id)
        };
        let Some(ConfirmEntry {
            initiator_id_pub,
            name,
            platform,
            app_version,
            initiator_addrs,
            request_control,
            stage: SessionStage::Revealed {
                z, transcript_hash, ..
            },
            ..
        }) = entry
        else {
            return Err(LinkError::SessionExpired);
        };
        if !decision {
            return Err(LinkError::RejectedByPeer);
        }

        let now = now_unix();
        // 入册前再截断一次：地址已在 hello 里校验过，这里只是确保候选表有界。
        let candidates = initiator_addrs
            .iter()
            .take(MAX_DIRECT_CANDIDATES)
            .map(|a| PeerCandidate {
                kind: TransportKind::Direct,
                address: a.clone(),
            })
            .collect();
        Ok(Some(PeerRecord {
            fingerprint: fingerprint(&initiator_id_pub),
            identity_pub: initiator_id_pub.to_vec(),
            name,
            platform,
            link_secret: derive_link_key(&z, &transcript_hash),
            candidates,
            paired_at: now,
            last_seen_at: now,
            app_version,
            grants_control: request_control,
            ..PeerRecord::default()
        }))
    }

    /// 等待本机用户对 `session_id` 表态（批准 / 拒绝）。会话被移除（超时清理 / 发起方
    /// `confirm=false` / 本机拒绝后被收走）→ `SessionExpired`；超过 `deadline` →
    /// `PairingTimeout` 并清理会话。
    async fn await_decision(&self, session_id: &str, deadline: Instant) -> LinkResult<bool> {
        loop {
            // 先构造等待 future 再检查状态：只要 set_local_decision 的
            // notify_waiters() 发生在 `notified()` 构造之后（无论早于还是晚于
            // 随后的 await），该唤醒都不会丢失——这是 tokio::sync::Notify 对
            // notify_waiters 的文档保证，堵上「查到未决 → 决策写入并 notify →
            // 才开始 await」之间的经典竞态窗口。
            let notified = self.decision_notify.notified();
            tokio::pin!(notified);

            let existing = {
                let mut sessions = self.sessions.lock().map_err(|_| LinkError::Unavailable)?;
                prune_sessions(&mut sessions);
                let entry = sessions.get(session_id).ok_or(LinkError::SessionExpired)?;
                match &entry.stage {
                    SessionStage::Revealed { decision, .. } => *decision,
                    SessionStage::AwaitingReveal { .. } => None,
                }
                // MutexGuard 在此作用域结束时 drop，绝不带着它跨越下面的 await。
            };
            if let Some(decision) = existing {
                return Ok(decision);
            }

            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                self.remove_session(session_id);
                return Err(LinkError::PairingTimeout);
            };
            if tokio::time::timeout(remaining, notified).await.is_err() {
                self.remove_session(session_id);
                return Err(LinkError::PairingTimeout);
            }
            // 被唤醒：可能是本会话的决策落地，也可能是共享 Notify 上其它会话
            // 触发的假唤醒——回到循环顶部重新加锁读取，确认后再决断。
        }
    }

    /// 供后台 GC 定时调用：主动剪枝三张表，清理「半途放弃」的配对——例如
    /// hello 已成功但用户从未调用 `set_local_decision`/`handle_confirm`。
    /// 现状这三张表都只在「恰好又发生一次同类调用」时顺带清理，孤儿会话
    /// 会一直驻留到进程重启；提供这个入口让宿主可以定时主动回收。
    pub fn prune_expired(&self) {
        if let Ok(mut codes) = self.codes.lock() {
            prune_codes(&mut codes);
        }
        if let Ok(mut sessions) = self.sessions.lock() {
            prune_sessions(&mut sessions);
        }
        if let Ok(mut failures) = self.failures.lock() {
            prune_failures(&mut failures);
        }
    }
}

/// 配对**发起方**（正在添加设备的一侧）。跨三次 HTTP 往返（hello → reveal → confirm）有状态。
pub struct PairingInitiator {
    identity: LinkIdentity,
    eph_secret: StaticSecret,
    eph_pub: [u8; 32],
    nonce: [u8; 32],
    /// 本次配对是否请求「完全控制」对端；进入 hello 签名字节，配对完成后落成
    /// `PeerRecord::controllable`。
    request_control: bool,
    /// `build_hello` 之后填充：hello 被签名字节的摘要（转录里「配对码 + 发起方自报
    /// 信息」的来源）。
    hello_digest: Option<[u8; 32]>,
    /// `on_hello_response` 之后填充：`z` 与转录已就绪，但响应方身份签名尚未验证。
    pending: Option<Handshake>,
    /// `on_reveal_response` 验证响应方转录签名之后填充（confirm 阶段登记用）。
    verified: Option<Handshake>,
}

struct Handshake {
    z: [u8; 32],
    transcript: Transcript,
    /// 已清洗的响应方自报信息（转录里用的是 hello 回复里的原始值）。
    responder_name: String,
    responder_platform: Option<String>,
    responder_app_version: Option<String>,
    responder_addr: String,
}

impl PairingInitiator {
    /// 用本机身份新建一次发起方会话（生成临时 X25519 密钥与随机数）。
    #[must_use]
    pub fn new(identity: LinkIdentity) -> Self {
        let mut seed = [0u8; 32];
        rand::rng().fill_bytes(&mut seed);
        let eph_secret = StaticSecret::from(seed);
        let eph_pub = PublicKey::from(&eph_secret).to_bytes();
        let mut nonce = [0u8; 32];
        rand::rng().fill_bytes(&mut nonce);
        Self {
            identity,
            eph_secret,
            eph_pub,
            nonce,
            request_control: false,
            hello_digest: None,
            pending: None,
            verified: None,
        }
    }

    /// 让本次配对请求「完全控制」对端（默认不请求）。必须在 [`Self::build_hello`] 之前设置：
    /// 标志进入 hello 签名字节与转录，对端用户批准才生效。
    #[must_use]
    pub fn request_control(mut self, request: bool) -> Self {
        self.request_control = request;
        self
    }

    /// 构造 `hello` 请求：只带临时公钥与随机数的**承诺**，不带临时公钥本身。签名覆盖
    /// 配对码、发起方身份公钥、承诺，以及本次自报的 name/platform/app_version/
    /// initiator_addrs（请求控制时还有控制标志），见 `hello_message`。
    #[must_use]
    pub fn build_hello(
        &mut self,
        code: &str,
        self_info: &SelfInfo,
        initiator_addrs: Vec<String>,
    ) -> HelloRequest {
        let mut hello = HelloRequest {
            protocol_version: PAIRING_PROTOCOL_VERSION,
            code: code.to_string(),
            initiator_id_pub: self.identity.public_bytes(),
            initiator_commit: eph_commitment(&self.eph_pub, &self.nonce),
            initiator_sig: [0u8; 64],
            name: self_info.name.clone(),
            platform: self_info.platform.clone(),
            app_version: self_info.app_version.clone(),
            initiator_addrs,
            request_control: self.request_control,
        };
        let message = hello_message(&hello);
        self.hello_digest = Some(hash_transcript(&message));
        hello.initiator_sig = self.identity.sign(&message);
        hello
    }

    /// 处理响应方 `hello` 回复：核版本 → 自配对守卫 → 与已知发现指纹比对 → 计算 `z` 与
    /// 完整转录 → 产出要发给响应方的 `reveal`。响应方身份此刻还无法验证（它的转录签名
    /// 在 `reveal` 回复里），验证与 SAS 见 [`Self::on_reveal_response`]。
    ///
    /// `responder_addr` 是发起方本次拨通对端所用的 `ip:port`（存为回连候选）。
    /// `known_fingerprints` 是发起方事先从发现阶段（mDNS 广播或 `/ping`）得知的、该
    /// 地址上设备的指纹：非空时，响应方出示的身份公钥指纹必须落在其中，否则
    /// [`LinkError::IdentityMismatch`]；为空（没有发现记录）不做这项比对。
    pub fn on_hello_response(
        &mut self,
        resp: &HelloResponse,
        responder_addr: &str,
        known_fingerprints: &[String],
    ) -> LinkResult<RevealRequest> {
        if resp.protocol_version != PAIRING_PROTOCOL_VERSION {
            return Err(LinkError::UnsupportedVersion);
        }
        let hello_digest = self.hello_digest.ok_or(LinkError::SessionExpired)?;
        // 自配对守卫先于其它校验：身份相同即可决断（共享数据库的进程互连场景）。
        if resp.responder_id_pub == self.identity.public_bytes() {
            return Err(LinkError::SelfPairing);
        }
        if !known_fingerprints.is_empty() {
            let actual = fingerprint(&resp.responder_id_pub);
            if !known_fingerprints
                .iter()
                .any(|known| known.eq_ignore_ascii_case(&actual))
            {
                return Err(LinkError::IdentityMismatch(responder_addr.to_string()));
            }
        }
        let z = self
            .eph_secret
            .diffie_hellman(&PublicKey::from(resp.responder_eph_pub))
            .to_bytes();
        // 纵深防御：理由同 `PairingResponder::handle_reveal` 中的对称检查
        // ——拒绝退化点导致的全零共享密钥，成本为零。
        if is_all_zero(&z) {
            return Err(LinkError::BadPayload("degenerate x25519 public key".into()));
        }
        let transcript = Transcript {
            hello_digest,
            initiator_id_pub: self.identity.public_bytes(),
            responder_id_pub: resp.responder_id_pub,
            initiator_eph_pub: self.eph_pub,
            responder_eph_pub: resp.responder_eph_pub,
            initiator_nonce: self.nonce,
            responder_nonce: resp.responder_nonce,
            responder_info_digest: responder_info_digest(
                &resp.name,
                resp.platform.as_deref(),
                resp.app_version.as_deref(),
            ),
        };
        self.verified = None;
        self.pending = Some(Handshake {
            z,
            transcript,
            responder_name: sanitize_text(&resp.name, MAX_NAME_CHARS),
            responder_platform: sanitize_optional(resp.platform.as_deref(), MAX_PLATFORM_CHARS),
            responder_app_version: sanitize_optional(
                resp.app_version.as_deref(),
                MAX_VERSION_CHARS,
            ),
            responder_addr: responder_addr.to_string(),
        });
        Ok(RevealRequest {
            session_id: resp.session_id.clone(),
            initiator_eph_pub: self.eph_pub,
            initiator_nonce: self.nonce,
        })
    }

    /// 处理响应方 `reveal` 回复：用 hello 回复里出示的响应方身份公钥验证其对**完整转录**
    /// 的签名，通过后算出 SAS。返回本地应展示的 SAS。
    ///
    /// 无论验签成败，这次握手的待定状态都被消耗：失败后不能在同一会话上再试。
    pub fn on_reveal_response(&mut self, resp: &RevealResponse) -> LinkResult<String> {
        let handshake = self.pending.take().ok_or(LinkError::SessionExpired)?;
        if !LinkIdentity::verify(
            &handshake.transcript.responder_id_pub,
            &handshake.transcript.responder_signing_message(),
            &resp.responder_sig,
        ) {
            return Err(LinkError::BadSignature);
        }
        let sas = derive_sas(&handshake.z, &handshake.transcript.hash());
        self.verified = Some(handshake);
        Ok(sas)
    }

    /// SAS 核对通过后，产出要登记的响应方设备记录（confirm 成功分支调用）。
    pub fn finalize(&self) -> LinkResult<PeerRecord> {
        let n = self.verified.as_ref().ok_or(LinkError::SessionExpired)?;
        let now = now_unix();
        Ok(PeerRecord {
            fingerprint: fingerprint(&n.transcript.responder_id_pub),
            identity_pub: n.transcript.responder_id_pub.to_vec(),
            name: if n.responder_name.is_empty() {
                fingerprint(&n.transcript.responder_id_pub)
                    .chars()
                    .take(12)
                    .collect()
            } else {
                n.responder_name.clone()
            },
            platform: n.responder_platform.clone(),
            link_secret: derive_link_key(&n.z, &n.transcript.hash()),
            candidates: vec![PeerCandidate {
                kind: TransportKind::Direct,
                address: n.responder_addr.clone(),
            }],
            paired_at: now,
            last_seen_at: now,
            app_version: n.responder_app_version.clone(),
            controllable: self.request_control,
            ..PeerRecord::default()
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn self_info(name: &str) -> SelfInfo {
        SelfInfo {
            name: name.to_string(),
            platform: Some("linux".to_string()),
            app_version: Some("0.1.0".to_string()),
            cloud_device_id: None,
        }
    }

    /// 未签名的最小 hello（无自报平台 / 版本 / 地址，不请求控制）：测试按需覆盖字段后自行签名。
    fn bare_hello(code: &str, initiator_id_pub: [u8; 32], commit: [u8; 32]) -> HelloRequest {
        HelloRequest {
            protocol_version: PAIRING_PROTOCOL_VERSION,
            code: code.to_owned(),
            initiator_id_pub,
            initiator_commit: commit,
            initiator_sig: [0u8; 64],
            name: "L".to_owned(),
            platform: None,
            app_version: None,
            initiator_addrs: Vec::new(),
            request_control: false,
        }
    }

    /// 一次走完 hello → reveal 的握手产物。
    struct Run {
        hello_resp: HelloResponse,
        reveal: RevealRequest,
        accepted: RevealAccepted,
        initiator_sas: String,
    }

    fn run_handshake(
        responder: &PairingResponder,
        initiator: &mut PairingInitiator,
        addrs: Vec<String>,
    ) -> Run {
        let code = responder.generate_code();
        let hello = initiator.build_hello(&code, &self_info("Laptop"), addrs);
        let hello_resp = responder.handle_hello(&hello, None).unwrap();
        let reveal = initiator
            .on_hello_response(&hello_resp, "10.0.0.1:17800", &[])
            .unwrap();
        let accepted = responder.handle_reveal(&reveal).unwrap();
        let initiator_sas = initiator.on_reveal_response(&accepted.response).unwrap();
        Run {
            hello_resp,
            reveal,
            accepted,
            initiator_sas,
        }
    }

    /// 只走到响应方回出 hello 回复（尚未揭示）。
    fn hello_only(
        responder: &PairingResponder,
        initiator: &mut PairingInitiator,
    ) -> (HelloRequest, HelloResponse) {
        let code = responder.generate_code();
        let hello = initiator.build_hello(&code, &self_info("L"), vec![]);
        let hello_resp = responder.handle_hello(&hello, None).unwrap();
        (hello, hello_resp)
    }

    #[test]
    fn self_pairing_rejected_without_consuming_code() {
        // 双端同一身份（共享引擎数据库的两个进程）→ hello 直接拒绝，且配对码不被消费。
        let id = LinkIdentity::generate();
        let responder = PairingResponder::new(id.clone(), self_info("NAS"));
        let mut initiator = PairingInitiator::new(id.clone());

        let code = responder.generate_code();
        let hello = initiator.build_hello(&code, &self_info("NAS"), vec![]);
        assert!(matches!(
            responder.handle_hello(&hello, None),
            Err(LinkError::SelfPairing)
        ));

        // 码未被消费：换一个正常身份仍可用同一码完成 hello。
        let mut other = PairingInitiator::new(LinkIdentity::generate());
        let hello2 = other.build_hello(&code, &self_info("Laptop"), vec![]);
        let resp = responder.handle_hello(&hello2, None).unwrap();
        // 发起方若发现响应方就是自己，同样拒绝。
        assert!(matches!(
            initiator.on_hello_response(&resp, "127.0.0.1:1", &[]),
            Err(LinkError::SelfPairing)
        ));
    }

    #[tokio::test]
    async fn full_handshake_agrees_on_sas_and_link_key() {
        let resp_id = LinkIdentity::generate();
        let responder = PairingResponder::new(resp_id.clone(), self_info("NAS"));
        let init_id = LinkIdentity::generate();
        let mut initiator = PairingInitiator::new(init_id.clone());

        let run = run_handshake(&responder, &mut initiator, vec!["10.0.0.2:17800".into()]);
        // 双端 SAS 一致（无中间人）。
        assert_eq!(run.initiator_sas, run.accepted.sas);
        assert_eq!(run.initiator_sas.len(), 6);

        // 响应方本机用户核对 SAS 后批准，双端确认 → 各自登记对方。
        let session_id = &run.hello_resp.session_id;
        responder.set_local_decision(session_id, true).unwrap();
        let resp_side = responder
            .handle_confirm(session_id, true)
            .await
            .unwrap()
            .unwrap();
        let init_side = initiator.finalize().unwrap();

        // 响应方登记的是发起方身份；发起方登记的是响应方身份。
        assert_eq!(resp_side.fingerprint, init_id.fingerprint());
        assert_eq!(init_side.fingerprint, resp_id.fingerprint());
        // 关键：双方派生出**相同**链路密钥（ECDH 对称 + 同一份转录）。
        assert_eq!(resp_side.link_secret, init_side.link_secret);
        assert_eq!(resp_side.link_secret.len(), 32);
        // 候选端点各自记录了对端地址。
        assert_eq!(resp_side.direct_address(), Some("10.0.0.2:17800"));
        assert_eq!(init_side.direct_address(), Some("10.0.0.1:17800"));
        // 响应方自报信息经转录签名覆盖后落入发起方名册。
        assert_eq!(init_side.name, "NAS");
        assert_eq!(init_side.platform.as_deref(), Some("linux"));
    }

    #[test]
    fn wrong_code_is_rejected() {
        let responder = PairingResponder::new(LinkIdentity::generate(), self_info("NAS"));
        let mut initiator = PairingInitiator::new(LinkIdentity::generate());
        responder.generate_code();
        let hello = initiator.build_hello("000000", &self_info("L"), vec![]);
        // 除非恰好猜中，几乎必然 InvalidCode（用固定错码 000000 对真码概率 1e-6）。
        let real = responder.generate_code();
        if real != "000000" {
            assert!(matches!(
                responder.handle_hello(&hello, None),
                Err(LinkError::InvalidCode)
            ));
        }
    }

    #[test]
    fn code_is_single_use() {
        let responder = PairingResponder::new(LinkIdentity::generate(), self_info("NAS"));
        let mut initiator = PairingInitiator::new(LinkIdentity::generate());
        let code = responder.generate_code();
        let hello = initiator.build_hello(&code, &self_info("L"), vec![]);
        assert!(responder.handle_hello(&hello, None).is_ok());
        // 同码第二次 → 已消费 → InvalidCode。
        assert!(matches!(
            responder.handle_hello(&hello, None),
            Err(LinkError::InvalidCode)
        ));
    }

    #[test]
    fn tampered_initiator_signature_rejected() {
        let responder = PairingResponder::new(LinkIdentity::generate(), self_info("NAS"));
        let mut initiator = PairingInitiator::new(LinkIdentity::generate());
        let code = responder.generate_code();
        let mut hello = initiator.build_hello(&code, &self_info("L"), vec![]);
        hello.initiator_sig[0] ^= 0xff; // 篡改签名
        assert!(matches!(
            responder.handle_hello(&hello, None),
            Err(LinkError::BadSignature)
        ));
    }

    #[test]
    fn tampered_initiator_addrs_rejected() {
        // initiator_addrs 会被响应方写入回连候选长期生效：明文中间人改它必须被签名检测到。
        let responder = PairingResponder::new(LinkIdentity::generate(), self_info("NAS"));
        let mut initiator = PairingInitiator::new(LinkIdentity::generate());
        let code = responder.generate_code();
        let mut hello = initiator.build_hello(&code, &self_info("L"), vec!["10.0.0.2:1".into()]);
        hello.initiator_addrs = vec!["10.0.0.66:9999".into()]; // 中间人篡改回连地址
        assert!(matches!(
            responder.handle_hello(&hello, None),
            Err(LinkError::BadSignature)
        ));
    }

    #[test]
    fn tampered_initiator_name_rejected() {
        let responder = PairingResponder::new(LinkIdentity::generate(), self_info("NAS"));
        let mut initiator = PairingInitiator::new(LinkIdentity::generate());
        let code = responder.generate_code();
        let mut hello = initiator.build_hello(&code, &self_info("L"), vec![]);
        hello.name = "attacker-renamed".to_string();
        assert!(matches!(
            responder.handle_hello(&hello, None),
            Err(LinkError::BadSignature)
        ));
    }

    #[test]
    fn tampered_commitment_in_hello_rejected() {
        // 承诺在 hello 签名范围内：中间人不能把发起方的承诺换成自己的。
        let responder = PairingResponder::new(LinkIdentity::generate(), self_info("NAS"));
        let mut initiator = PairingInitiator::new(LinkIdentity::generate());
        let code = responder.generate_code();
        let mut hello = initiator.build_hello(&code, &self_info("L"), vec![]);
        hello.initiator_commit = eph_commitment(&[5u8; 32], &[6u8; 32]);
        assert!(matches!(
            responder.handle_hello(&hello, None),
            Err(LinkError::BadSignature)
        ));
    }

    #[test]
    fn reveal_with_substituted_ephemeral_key_rejected_and_burns_session() {
        let responder = PairingResponder::new(LinkIdentity::generate(), self_info("NAS"));
        let mut initiator = PairingInitiator::new(LinkIdentity::generate());
        let (_hello, hello_resp) = hello_only(&responder, &mut initiator);
        let mut reveal = initiator
            .on_hello_response(&hello_resp, "10.0.0.1:1", &[])
            .unwrap();
        let genuine = reveal.clone();
        // 攻击者想换一把自己挑选的临时公钥。
        let mut seed = [3u8; 32];
        seed[0] = 9;
        reveal.initiator_eph_pub = PublicKey::from(&StaticSecret::from(seed)).to_bytes();
        assert!(matches!(
            responder.handle_reveal(&reveal),
            Err(LinkError::CommitmentMismatch)
        ));
        // 会话已作废：连真正的揭示也无法再接上。
        assert!(matches!(
            responder.handle_reveal(&genuine),
            Err(LinkError::SessionExpired)
        ));
    }

    #[test]
    fn reveal_with_substituted_nonce_rejected() {
        let responder = PairingResponder::new(LinkIdentity::generate(), self_info("NAS"));
        let mut initiator = PairingInitiator::new(LinkIdentity::generate());
        let (_hello, hello_resp) = hello_only(&responder, &mut initiator);
        let mut reveal = initiator
            .on_hello_response(&hello_resp, "10.0.0.1:1", &[])
            .unwrap();
        reveal.initiator_nonce[0] ^= 0xff;
        assert!(matches!(
            responder.handle_reveal(&reveal),
            Err(LinkError::CommitmentMismatch)
        ));
    }

    #[test]
    fn reveal_for_unknown_or_replayed_session_is_ignored() {
        let responder = PairingResponder::new(LinkIdentity::generate(), self_info("NAS"));
        let mut initiator = PairingInitiator::new(LinkIdentity::generate());
        let run = run_handshake(&responder, &mut initiator, vec![]);

        let unknown = RevealRequest {
            session_id: "no-such-session".into(),
            ..run.reveal.clone()
        };
        assert!(matches!(
            responder.handle_reveal(&unknown),
            Err(LinkError::SessionExpired)
        ));
        // 重放同一条 reveal：不生效，且不误伤已经揭示的会话。
        assert!(matches!(
            responder.handle_reveal(&run.reveal),
            Err(LinkError::SessionExpired)
        ));
        responder
            .set_local_decision(&run.hello_resp.session_id, true)
            .unwrap();
    }

    #[test]
    fn late_reveal_rejected() {
        let responder = PairingResponder::new(LinkIdentity::generate(), self_info("NAS"));
        let mut initiator = PairingInitiator::new(LinkIdentity::generate());
        let (_hello, hello_resp) = hello_only(&responder, &mut initiator);
        let reveal = initiator
            .on_hello_response(&hello_resp, "10.0.0.1:1", &[])
            .unwrap();
        {
            let mut sessions = responder.sessions.lock().unwrap();
            let entry = sessions.get_mut(&hello_resp.session_id).unwrap();
            entry.created = Instant::now() - REVEAL_TIMEOUT - Duration::from_secs(1);
        }
        assert!(matches!(
            responder.handle_reveal(&reveal),
            Err(LinkError::SessionExpired)
        ));
    }

    #[tokio::test]
    async fn confirm_or_decision_before_reveal_is_rejected() {
        // 没有揭示就没有 SAS：此时既不能表态，也不能 confirm=true 放行。
        let responder = PairingResponder::new(LinkIdentity::generate(), self_info("NAS"));
        let mut initiator = PairingInitiator::new(LinkIdentity::generate());
        let (_hello, hello_resp) = hello_only(&responder, &mut initiator);
        assert!(matches!(
            responder.set_local_decision(&hello_resp.session_id, true),
            Err(LinkError::SessionExpired)
        ));
        assert!(matches!(
            responder.handle_confirm(&hello_resp.session_id, true).await,
            Err(LinkError::BadPayload(_))
        ));
        // 违规 confirm 已使会话作废。
        assert!(matches!(
            responder.handle_confirm(&hello_resp.session_id, true).await,
            Err(LinkError::SessionExpired)
        ));
    }

    #[test]
    fn other_protocol_versions_rejected_without_touching_code_or_throttle() {
        let responder = PairingResponder::new(LinkIdentity::generate(), self_info("NAS"));
        let mut initiator = PairingInitiator::new(LinkIdentity::generate());
        let source: Option<IpAddr> = Some("203.0.113.9".parse().unwrap());
        let code = responder.generate_code();
        let hello = initiator.build_hello(&code, &self_info("L"), vec![]);

        // 0 = 旧版对端（没有版本字段）；1 = 旧协议；大于当前版本 = 更新的协议。
        for version in [0, 1, PAIRING_PROTOCOL_VERSION + 1] {
            let mut stale = hello.clone();
            stale.protocol_version = version;
            for _ in 0..=MAX_FAILED_HELLOS {
                assert!(matches!(
                    responder.handle_hello(&stale, source),
                    Err(LinkError::UnsupportedVersion)
                ));
            }
        }
        // 版本不符既没有消耗配对码，也没有计入该来源的猜码节流。
        assert!(responder.handle_hello(&hello, source).is_ok());
    }

    #[test]
    fn initiator_rejects_peer_with_other_protocol_version() {
        let responder = PairingResponder::new(LinkIdentity::generate(), self_info("NAS"));
        let mut initiator = PairingInitiator::new(LinkIdentity::generate());
        let (_hello, mut hello_resp) = hello_only(&responder, &mut initiator);
        hello_resp.protocol_version = 1;
        assert!(matches!(
            initiator.on_hello_response(&hello_resp, "10.0.0.1:1", &[]),
            Err(LinkError::UnsupportedVersion)
        ));
    }

    #[test]
    fn responder_identity_must_match_discovery_fingerprint() {
        let resp_id = LinkIdentity::generate();
        let responder = PairingResponder::new(resp_id.clone(), self_info("NAS"));
        let other_fp = LinkIdentity::generate().fingerprint().to_string();

        // 发现阶段得知该地址是另一台设备：响应方身份对不上 → 拒绝，不揭示临时公钥。
        let mut initiator = PairingInitiator::new(LinkIdentity::generate());
        let (_hello, hello_resp) = hello_only(&responder, &mut initiator);
        assert!(matches!(
            initiator.on_hello_response(&hello_resp, "10.0.0.1:1", std::slice::from_ref(&other_fp)),
            Err(LinkError::IdentityMismatch(addr)) if addr == "10.0.0.1:1"
        ));

        // 指纹一致（大小写不敏感）通过；已知集合里混有过期指纹不妨碍命中。
        let mut initiator = PairingInitiator::new(LinkIdentity::generate());
        let (_hello, hello_resp) = hello_only(&responder, &mut initiator);
        let known = vec![other_fp, resp_id.fingerprint().to_ascii_uppercase()];
        assert!(
            initiator
                .on_hello_response(&hello_resp, "10.0.0.1:1", &known)
                .is_ok()
        );
    }

    #[test]
    fn responder_signature_covers_full_transcript() {
        // 响应方在 hello 回复里明文出示的每个转录成员，一旦被中间人改动，发起方都会在
        // 验证响应方转录签名时失败。
        type Tamper = fn(&mut HelloResponse);
        let tampers: [(&str, Tamper); 6] = [
            ("responder_eph_pub", |r| {
                r.responder_eph_pub = PublicKey::from(&StaticSecret::from([8u8; 32])).to_bytes();
            }),
            ("responder_nonce", |r| r.responder_nonce[0] ^= 0xff),
            ("name", |r| r.name = "evil".into()),
            ("platform", |r| r.platform = Some("plan9".into())),
            ("app_version", |r| r.app_version = Some("9.9.9".into())),
            ("responder_id_pub", |r| {
                r.responder_id_pub = LinkIdentity::generate().public_bytes();
            }),
        ];
        for (field, tamper) in tampers {
            let responder = PairingResponder::new(LinkIdentity::generate(), self_info("NAS"));
            let mut initiator = PairingInitiator::new(LinkIdentity::generate());
            let (_hello, mut hello_resp) = hello_only(&responder, &mut initiator);
            tamper(&mut hello_resp);
            let reveal = initiator
                .on_hello_response(&hello_resp, "10.0.0.1:1", &[])
                .unwrap();
            let accepted = responder.handle_reveal(&reveal).unwrap();
            assert!(
                matches!(
                    initiator.on_reveal_response(&accepted.response),
                    Err(LinkError::BadSignature)
                ),
                "tampering {field} must break the responder transcript signature"
            );
            assert!(
                initiator.finalize().is_err(),
                "a failed handshake must not yield a peer record ({field})"
            );
        }
    }

    #[test]
    fn initiator_identity_swap_breaks_responder_signature() {
        // 中间人把 hello 里的发起方身份换成自己（承诺、配对码、自报信息原样沿用，并用
        // 自己的私钥重新签名）：响应方受理，但它的转录里是攻击者的身份公钥，发起方
        // 据此验证响应方签名必然失败。
        let responder = PairingResponder::new(LinkIdentity::generate(), self_info("NAS"));
        let mut initiator = PairingInitiator::new(LinkIdentity::generate());
        let code = responder.generate_code();
        let hello = initiator.build_hello(&code, &self_info("L"), vec!["10.0.0.2:1".into()]);

        let attacker = LinkIdentity::generate();
        let mut forged = HelloRequest {
            initiator_id_pub: attacker.public_bytes(),
            ..hello
        };
        forged.initiator_sig = attacker.sign(&hello_message(&forged));
        let hello_resp = responder.handle_hello(&forged, None).unwrap();
        let reveal = initiator
            .on_hello_response(&hello_resp, "10.0.0.1:1", &[])
            .unwrap();
        let accepted = responder.handle_reveal(&reveal).unwrap();
        assert!(matches!(
            initiator.on_reveal_response(&accepted.response),
            Err(LinkError::BadSignature)
        ));
    }

    #[tokio::test]
    async fn mitm_between_two_honest_endpoints_shares_no_sas_or_link_key() {
        // 攻击者 M 同时扮演发起方（对真响应方 R）与响应方（对真发起方 I），各自完成
        // 一次合法握手。承诺-揭示迫使 M 在看到 I 的临时公钥之前就钉死面向 I 的临时值，
        // 两段会话的转录（身份、临时公钥、随机数）互不相同：
        // SAS 各自独立、链路密钥不同。
        let responder = PairingResponder::new(LinkIdentity::generate(), self_info("NAS"));
        let mut initiator = PairingInitiator::new(LinkIdentity::generate());
        let attacker = LinkIdentity::generate();
        let mut m_as_initiator = PairingInitiator::new(attacker.clone());
        let m_as_responder = PairingResponder::new(attacker.clone(), self_info("NAS"));

        // 用户在 I 上输入 R 展示的码；M 窥到了它，并让自己面向 I 的响应方接受同一个码。
        let code = responder.generate_code();
        m_as_responder.codes.lock().unwrap().push(CodeEntry {
            code: code.clone(),
            created: Instant::now(),
        });

        let hello_i = initiator.build_hello(&code, &self_info("Laptop"), vec![]);
        let challenge_m = m_as_responder.handle_hello(&hello_i, None).unwrap();

        let hello_m = m_as_initiator.build_hello(&code, &self_info("Laptop"), vec![]);
        let challenge_r = responder.handle_hello(&hello_m, None).unwrap();
        let reveal_m = m_as_initiator
            .on_hello_response(&challenge_r, "r", &[])
            .unwrap();
        let accepted_r = responder.handle_reveal(&reveal_m).unwrap();
        m_as_initiator
            .on_reveal_response(&accepted_r.response)
            .unwrap();

        let reveal_i = initiator.on_hello_response(&challenge_m, "m", &[]).unwrap();
        let accepted_m = m_as_responder.handle_reveal(&reveal_i).unwrap();
        let sas_i = initiator.on_reveal_response(&accepted_m.response).unwrap();

        // 两端展示的 SAS 不同（单次碰撞概率 1e-6）。
        assert_ne!(sas_i, accepted_r.sas);

        // 即便两端用户都点了批准，R 与 I 各自登记的也是不同的链路密钥与对端身份。
        responder
            .set_local_decision(&challenge_r.session_id, true)
            .unwrap();
        let record_r = responder
            .handle_confirm(&challenge_r.session_id, true)
            .await
            .unwrap()
            .unwrap();
        let record_i = initiator.finalize().unwrap();
        assert_ne!(record_r.link_secret, record_i.link_secret);
        assert_eq!(record_i.fingerprint, attacker.fingerprint());
        assert_eq!(record_r.fingerprint, attacker.fingerprint());
    }

    #[test]
    fn every_transcript_field_changes_hash_sas_and_link_key() {
        fn base() -> Transcript {
            Transcript {
                hello_digest: [1u8; 32],
                initiator_id_pub: [2u8; 32],
                responder_id_pub: [3u8; 32],
                initiator_eph_pub: [4u8; 32],
                responder_eph_pub: [5u8; 32],
                initiator_nonce: [6u8; 32],
                responder_nonce: [7u8; 32],
                responder_info_digest: [8u8; 32],
            }
        }
        let z = [42u8; 32];
        let reference = base();
        let ref_hash = reference.hash();
        let ref_sas = derive_sas(&z, &ref_hash);
        let ref_key = derive_link_key(&z, &ref_hash);

        type Mutate = fn(&mut Transcript);
        let mutations: [(&str, Mutate); 8] = [
            ("hello_digest", |t| t.hello_digest[0] ^= 1),
            ("initiator_id_pub", |t| t.initiator_id_pub[0] ^= 1),
            ("responder_id_pub", |t| t.responder_id_pub[0] ^= 1),
            ("initiator_eph_pub", |t| t.initiator_eph_pub[0] ^= 1),
            ("responder_eph_pub", |t| t.responder_eph_pub[0] ^= 1),
            ("initiator_nonce", |t| t.initiator_nonce[0] ^= 1),
            ("responder_nonce", |t| t.responder_nonce[0] ^= 1),
            ("responder_info_digest", |t| t.responder_info_digest[0] ^= 1),
        ];
        for (field, mutate) in mutations {
            let mut t = base();
            mutate(&mut t);
            assert_ne!(t.hash(), ref_hash, "{field} must be part of the transcript");
            assert_ne!(
                t.responder_signing_message(),
                reference.responder_signing_message(),
                "{field} must be covered by the responder signature"
            );
            assert_ne!(
                derive_sas(&z, &t.hash()),
                ref_sas,
                "{field} must change SAS"
            );
            assert_ne!(
                derive_link_key(&z, &t.hash()),
                ref_key,
                "{field} must change the link key"
            );
        }
    }

    #[test]
    fn hello_message_binds_code_and_uses_unambiguous_framing() {
        let id = [1u8; 32];
        let commit = [2u8; 32];
        let msg = |code: &str, platform: &str, version: &str| {
            hello_message(&HelloRequest {
                platform: Some(platform.to_owned()),
                app_version: Some(version.to_owned()),
                ..bare_hello(code, id, commit)
            })
        };
        assert_ne!(msg("111111", "a", "b"), msg("222222", "a", "b"));
        // 相邻变长字段的边界变化不能产生相同字节串。
        assert_ne!(msg("111111", "ab", "c"), msg("111111", "a", "bc"));
    }

    #[tokio::test]
    async fn confirm_false_registers_nothing() {
        let responder = PairingResponder::new(LinkIdentity::generate(), self_info("NAS"));
        let mut initiator = PairingInitiator::new(LinkIdentity::generate());
        let run = run_handshake(&responder, &mut initiator, vec![]);
        let session_id = &run.hello_resp.session_id;
        assert!(
            responder
                .handle_confirm(session_id, false)
                .await
                .unwrap()
                .is_none()
        );
        // 会话已消费 → 再次 confirm → SessionExpired。
        assert!(matches!(
            responder.handle_confirm(session_id, true).await,
            Err(LinkError::SessionExpired)
        ));
    }

    #[tokio::test]
    async fn local_rejection_yields_rejected_by_peer() {
        // 发起方一侧 confirm(true)（它自己已核对 SAS 通过），但响应方本机
        // 用户核对后拒绝——配对必须失败，且错误要能区分「对端拒绝」。
        let responder = PairingResponder::new(LinkIdentity::generate(), self_info("NAS"));
        let mut initiator = PairingInitiator::new(LinkIdentity::generate());
        let run = run_handshake(&responder, &mut initiator, vec![]);
        let session_id = &run.hello_resp.session_id;

        responder.set_local_decision(session_id, false).unwrap();
        assert!(matches!(
            responder.handle_confirm(session_id, true).await,
            Err(LinkError::RejectedByPeer)
        ));
    }

    #[tokio::test]
    async fn local_approval_completes_pairing() {
        let init_id = LinkIdentity::generate();
        let responder = PairingResponder::new(LinkIdentity::generate(), self_info("NAS"));
        let mut initiator = PairingInitiator::new(init_id.clone());
        let run = run_handshake(&responder, &mut initiator, vec![]);
        let session_id = &run.hello_resp.session_id;

        responder.set_local_decision(session_id, true).unwrap();
        let record = responder
            .handle_confirm(session_id, true)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(record.fingerprint, init_id.fingerprint());
    }

    #[tokio::test]
    async fn confirm_after_decision_window_times_out_immediately() {
        // 回归测试：决策截止时间锚定在揭示被受理的时刻（IncomingPairing 广播时）。
        // confirm 抵达时若这个窗口已经过去，必须立即返回 PairingTimeout、不进入
        // 等待循环——而不是从「本次 confirm 抵达」重新起算一整个
        // LOCAL_DECISION_TIMEOUT，那样响应方 UI 弹窗早已按同源倒计时自动
        // 关闭，backend 却还在傻等一个不会再来的决策。
        //
        // 直接回拨会话的 `announced` 模拟「发起方核对 SAS 耗时超过决策窗口」，
        // 避免测试真的等待 LOCAL_DECISION_TIMEOUT（60 秒）。
        let responder = PairingResponder::new(LinkIdentity::generate(), self_info("NAS"));
        let mut initiator = PairingInitiator::new(LinkIdentity::generate());
        let run = run_handshake(&responder, &mut initiator, vec![]);
        let session_id = &run.hello_resp.session_id;

        {
            let mut sessions = responder.sessions.lock().unwrap();
            let entry = sessions.get_mut(session_id).unwrap();
            let SessionStage::Revealed { announced, .. } = &mut entry.stage else {
                panic!("session must be revealed after a completed handshake");
            };
            *announced = Instant::now() - LOCAL_DECISION_TIMEOUT - Duration::from_secs(1);
        }

        let start = Instant::now();
        let result = responder.handle_confirm(session_id, true).await;
        assert!(matches!(result, Err(LinkError::PairingTimeout)));
        // 不阻塞：这是窗口检查的快速路径，不是凑巧在等待循环里等到超时——
        // 5 秒的余量远小于 LOCAL_DECISION_TIMEOUT（60 秒），调度抖动绰绰
        // 有余，但足以证明没有真的进入等待。
        assert!(start.elapsed() < Duration::from_secs(5));

        // 会话已被立即清理：随后即便本机用户才做出决策，也找不到会话了。
        assert!(matches!(
            responder.set_local_decision(session_id, true),
            Err(LinkError::SessionExpired)
        ));
    }

    #[test]
    fn set_local_decision_unknown_session_is_expired() {
        let responder = PairingResponder::new(LinkIdentity::generate(), self_info("NAS"));
        assert!(matches!(
            responder.set_local_decision("no-such-session", true),
            Err(LinkError::SessionExpired)
        ));
    }

    #[test]
    fn throttle_blocks_after_limit() {
        let responder = PairingResponder::new(LinkIdentity::generate(), self_info("NAS"));
        let mut initiator = PairingInitiator::new(LinkIdentity::generate());
        let attacker: Option<IpAddr> = Some("203.0.113.9".parse().unwrap());
        let real_code = responder.generate_code();
        // 固定错码：与随机生成的真码撞车概率仅 1e-6，这里直接避开而非依赖概率。
        let bad_code = if real_code == "999999" {
            "888888"
        } else {
            "999999"
        };

        // 同一来源连续 MAX_FAILED_HELLOS 次错码，喂满该来源的节流桶。
        for _ in 0..MAX_FAILED_HELLOS {
            let bad = initiator.build_hello(bad_code, &self_info("L"), vec![]);
            assert!(matches!(
                responder.handle_hello(&bad, attacker),
                Err(LinkError::InvalidCode)
            ));
        }

        // 第 11 次即便用正确、未过期的码也必须被节流拒绝——返回值必须是
        // Throttled 而不是 Ok(_)/InvalidCode，证明这次请求在查码、消费码、
        // 验签之前就被拒绝：正确码根本没被触碰。
        let good = initiator.build_hello(&real_code, &self_info("L"), vec![]);
        assert!(matches!(
            responder.handle_hello(&good, attacker),
            Err(LinkError::Throttled)
        ));

        // 节流是「读时窗」而非一次性消费的资源：仍在窗口内时，重复用正确码
        // 请求依旧稳定返回 Throttled，不会退化成第二次就侥幸放行。
        assert!(matches!(
            responder.handle_hello(&good, attacker),
            Err(LinkError::Throttled)
        ));
    }

    #[test]
    fn throttle_is_per_source() {
        // 回归测试：节流必须按来源分桶，而不是退化回全局单一计数器——否则
        // 任何能触达 hello 端点的主机只需刷满一个来源的失败次数，就能让
        // 全部来源（包括携带正确码的合法发起方）永久无法完成配对；`POST
        // /api/v1/link/pair/hello` 按设计又是免鉴权端点，这是可从公网触发
        // 的 DoS。
        let responder = PairingResponder::new(LinkIdentity::generate(), self_info("NAS"));
        let mut attacker_init = PairingInitiator::new(LinkIdentity::generate());
        let mut victim_init = PairingInitiator::new(LinkIdentity::generate());
        let attacker: Option<IpAddr> = Some("203.0.113.9".parse().unwrap());
        let victim: Option<IpAddr> = Some("198.51.100.7".parse().unwrap());
        let real_code = responder.generate_code();
        let bad_code = if real_code == "999999" {
            "888888"
        } else {
            "999999"
        };

        // 来源 A（攻击者）刷满自己的节流桶。
        for _ in 0..MAX_FAILED_HELLOS {
            let bad = attacker_init.build_hello(bad_code, &self_info("L"), vec![]);
            assert!(matches!(
                responder.handle_hello(&bad, attacker),
                Err(LinkError::InvalidCode)
            ));
        }
        // 来源 A 自己确实被节流了。
        let attacker_retry = attacker_init.build_hello(&real_code, &self_info("L"), vec![]);
        assert!(matches!(
            responder.handle_hello(&attacker_retry, attacker),
            Err(LinkError::Throttled)
        ));

        // 来源 B（合法发起方）用正确码依旧能成功配对——不受来源 A 的节流
        // 状态牵连。
        let hello = victim_init.build_hello(&real_code, &self_info("L"), vec![]);
        assert!(responder.handle_hello(&hello, victim).is_ok());
    }

    #[test]
    fn regenerating_code_invalidates_previous() {
        // 生成新码必须让旧码立即失效——同一响应方同一时刻只应有一个有效码，
        // 否则「刷新配对码」挤不掉已被窥屏泄露的旧码。
        let responder = PairingResponder::new(LinkIdentity::generate(), self_info("NAS"));
        let mut initiator = PairingInitiator::new(LinkIdentity::generate());
        let old_code = responder.generate_code();
        let new_code = responder.generate_code();
        assert_ne!(old_code, new_code, "极小概率随机撞码，重跑即可");
        let hello = initiator.build_hello(&old_code, &self_info("L"), vec![]);
        assert!(matches!(
            responder.handle_hello(&hello, None),
            Err(LinkError::InvalidCode)
        ));
    }

    #[test]
    fn degenerate_eph_pub_rejected_on_both_sides() {
        // 全零 32 字节是 X25519 的退化点：无论对端私钥是什么，协商出的共享
        // 密钥恒为全零。x25519_dalek::PublicKey::from([u8; 32]) 不做点校验，
        // handle_reveal 必须在 ECDH 之后显式拒绝。这里手工构造 hello 与 reveal（而非走
        // `PairingInitiator`，因为它内部随机生成临时公钥，拿不到全零值），让承诺与
        // 签名先通过，才能验证到达 ECDH 之后的拒绝分支。
        let responder = PairingResponder::new(LinkIdentity::generate(), self_info("NAS"));
        let initiator_id = LinkIdentity::generate();
        let code = responder.generate_code();
        let degenerate_eph_pub = [0u8; 32];
        let nonce = [7u8; 32];
        let commit = eph_commitment(&degenerate_eph_pub, &nonce);
        let mut hello = HelloRequest {
            name: "L".to_string(),
            ..bare_hello(&code, initiator_id.public_bytes(), commit)
        };
        hello.initiator_sig = initiator_id.sign(&hello_message(&hello));
        let hello_resp = responder.handle_hello(&hello, None).unwrap();
        let reveal = RevealRequest {
            session_id: hello_resp.session_id,
            initiator_eph_pub: degenerate_eph_pub,
            initiator_nonce: nonce,
        };
        assert!(matches!(
            responder.handle_reveal(&reveal),
            Err(LinkError::BadPayload(_))
        ));

        // 发起方对称：响应方回复里的退化临时公钥同样被拒绝。
        let responder = PairingResponder::new(LinkIdentity::generate(), self_info("NAS"));
        let mut initiator = PairingInitiator::new(LinkIdentity::generate());
        let (_hello, mut hello_resp) = hello_only(&responder, &mut initiator);
        hello_resp.responder_eph_pub = [0u8; 32];
        assert!(matches!(
            initiator.on_hello_response(&hello_resp, "10.0.0.1:1", &[]),
            Err(LinkError::BadPayload(_))
        ));
    }

    #[test]
    fn bad_signature_does_not_burn_the_pairing_code() {
        // 签名验证通过后才消费配对码：持码者发一个签名错误的 hello 不能烧掉合法用户的码。
        let responder = PairingResponder::new(LinkIdentity::generate(), self_info("NAS"));
        let mut attacker = PairingInitiator::new(LinkIdentity::generate());
        let code = responder.generate_code();
        let mut forged = attacker.build_hello(&code, &self_info("L"), vec![]);
        forged.initiator_sig[0] ^= 0xff;
        assert!(matches!(
            responder.handle_hello(&forged, None),
            Err(LinkError::BadSignature)
        ));
        // 码仍然有效：合法发起方照常完成 hello。
        let mut honest = PairingInitiator::new(LinkIdentity::generate());
        let hello = honest.build_hello(&code, &self_info("Laptop"), vec![]);
        assert!(responder.handle_hello(&hello, None).is_ok());
    }

    #[test]
    fn repeated_bad_signatures_are_throttled_per_source() {
        let responder = PairingResponder::new(LinkIdentity::generate(), self_info("NAS"));
        let mut attacker = PairingInitiator::new(LinkIdentity::generate());
        let source: Option<IpAddr> = Some("203.0.113.9".parse().unwrap());
        let code = responder.generate_code();
        for _ in 0..MAX_FAILED_HELLOS {
            let mut forged = attacker.build_hello(&code, &self_info("L"), vec![]);
            forged.initiator_sig[0] ^= 0xff;
            assert!(matches!(
                responder.handle_hello(&forged, source),
                Err(LinkError::BadSignature)
            ));
        }
        let good = attacker.build_hello(&code, &self_info("L"), vec![]);
        assert!(matches!(
            responder.handle_hello(&good, source),
            Err(LinkError::Throttled)
        ));
    }

    #[test]
    fn hello_inputs_are_validated_and_sanitized() {
        let responder = PairingResponder::new(LinkIdentity::generate(), self_info("NAS"));
        let mut initiator = PairingInitiator::new(LinkIdentity::generate());
        let code = responder.generate_code();

        // 回连地址：超过 4 条 / 不可解析 → BadPayload，且不消费配对码。
        let many: Vec<String> = (1..=5).map(|i| format!("10.0.0.{i}:17800")).collect();
        let hello = initiator.build_hello(&code, &self_info("L"), many);
        assert!(matches!(
            responder.handle_hello(&hello, None),
            Err(LinkError::BadPayload(_))
        ));
        let hello = initiator.build_hello(&code, &self_info("L"), vec!["ftp://evil".into()]);
        assert!(matches!(
            responder.handle_hello(&hello, None),
            Err(LinkError::BadPayload(_))
        ));

        // 名称 / 平台 / 版本：通过验签后清洗（控制字符、双向控制字符、超长）而不是原样入册。
        let dirty = SelfInfo {
            name: format!("Evil\u{202E}\u{0}{}", "x".repeat(200)),
            platform: Some("lin\nux".into()),
            app_version: Some("1.0\u{7}".into()),
            cloud_device_id: None,
        };
        let hello = initiator.build_hello(&code, &dirty, vec!["http://10.0.0.7:17800".into()]);
        let resp = responder.handle_hello(&hello, None).unwrap();
        let reveal = initiator
            .on_hello_response(&resp, "10.0.0.1:17800", &[])
            .unwrap();
        let accepted = responder.handle_reveal(&reveal).unwrap();
        assert!(accepted.peer_name.starts_with("Evilx"));
        assert_eq!(accepted.peer_name.chars().count(), MAX_NAME_CHARS);
        assert!(!accepted.peer_name.contains(['\u{202E}', '\u{0}']));
        assert_eq!(accepted.peer_platform.as_deref(), Some("linux"));
    }

    #[tokio::test]
    async fn confirmed_record_carries_sanitized_fields_and_canonical_candidates() {
        let responder = PairingResponder::new(LinkIdentity::generate(), self_info("NAS"));
        let mut initiator = PairingInitiator::new(LinkIdentity::generate());
        let code = responder.generate_code();
        let dirty = SelfInfo {
            name: "Lap\u{0}top".into(),
            platform: Some("macos".into()),
            app_version: Some("2.0.1".into()),
            cloud_device_id: None,
        };
        let hello = initiator.build_hello(
            &code,
            &dirty,
            vec!["http://10.0.0.7:17800".into(), "10.0.0.7:17800".into()],
        );
        let resp = responder.handle_hello(&hello, None).unwrap();
        let reveal = initiator
            .on_hello_response(&resp, "10.0.0.1:17800", &[])
            .unwrap();
        responder.handle_reveal(&reveal).unwrap();
        responder
            .set_local_decision(&resp.session_id, true)
            .unwrap();
        let record = responder
            .handle_confirm(&resp.session_id, true)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(record.name, "Laptop");
        assert_eq!(record.app_version.as_deref(), Some("2.0.1"));
        // 两种写法规范化为同一候选并去重。
        assert_eq!(record.candidates.len(), 1);
        assert_eq!(record.candidates[0].address, "10.0.0.7:17800");
        assert!(!record.revoked);
    }

    #[tokio::test]
    async fn approved_session_survives_a_late_confirm() {
        // 本机用户在 60 秒核验窗口内批准，发起方的 confirm 在窗口之后（但仍在会话 TTL 内）才到：
        // 配对仍然成立，不能因为 confirm 迟到而把已批准的结果丢掉。
        let responder = PairingResponder::new(LinkIdentity::generate(), self_info("NAS"));
        let mut initiator = PairingInitiator::new(LinkIdentity::generate());
        let run = run_handshake(&responder, &mut initiator, vec![]);
        let session_id = &run.hello_resp.session_id;
        responder.set_local_decision(session_id, true).unwrap();
        {
            let mut sessions = responder.sessions.lock().unwrap();
            let entry = sessions.get_mut(session_id).unwrap();
            let SessionStage::Revealed { announced, .. } = &mut entry.stage else {
                panic!("session must be revealed");
            };
            *announced = Instant::now() - LOCAL_DECISION_TIMEOUT - Duration::from_secs(30);
        }
        let record = responder.handle_confirm(session_id, true).await.unwrap();
        assert!(record.is_some(), "an approved pairing must not time out");
    }

    #[tokio::test]
    async fn locally_rejected_session_reports_rejection_even_after_the_window() {
        let responder = PairingResponder::new(LinkIdentity::generate(), self_info("NAS"));
        let mut initiator = PairingInitiator::new(LinkIdentity::generate());
        let run = run_handshake(&responder, &mut initiator, vec![]);
        let session_id = &run.hello_resp.session_id;
        responder.set_local_decision(session_id, false).unwrap();
        {
            let mut sessions = responder.sessions.lock().unwrap();
            let entry = sessions.get_mut(session_id).unwrap();
            let SessionStage::Revealed { announced, .. } = &mut entry.stage else {
                panic!("session must be revealed");
            };
            *announced = Instant::now() - LOCAL_DECISION_TIMEOUT - Duration::from_secs(30);
        }
        assert!(matches!(
            responder.handle_confirm(session_id, true).await,
            Err(LinkError::RejectedByPeer)
        ));
    }

    #[tokio::test]
    async fn decline_wakes_a_waiting_confirm_immediately() {
        // confirm=false 到达时，仍在等本机决策的 confirm=true 请求立即醒来，不必睡到核验窗口结束。
        let responder = Arc::new(PairingResponder::new(
            LinkIdentity::generate(),
            self_info("NAS"),
        ));
        let mut initiator = PairingInitiator::new(LinkIdentity::generate());
        let run = run_handshake(&responder, &mut initiator, vec![]);
        let session_id = run.hello_resp.session_id.clone();
        let waiter = {
            let responder = responder.clone();
            let session_id = session_id.clone();
            tokio::spawn(async move { responder.handle_confirm(&session_id, true).await })
        };
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            responder
                .handle_confirm(&session_id, false)
                .await
                .unwrap()
                .is_none()
        );
        let result = tokio::time::timeout(Duration::from_secs(5), waiter)
            .await
            .expect("the waiting confirm must wake up promptly")
            .unwrap();
        assert!(matches!(result, Err(LinkError::SessionExpired)));
    }

    #[test]
    fn throttle_buckets_ipv6_by_slash_64_and_normalizes_mapped_ipv4() {
        let a: IpAddr = "2001:db8:1:2:aaaa:bbbb:cccc:dddd".parse().unwrap();
        let b: IpAddr = "2001:db8:1:2:1111:2222:3333:4444".parse().unwrap();
        let other_net: IpAddr = "2001:db8:1:3:aaaa:bbbb:cccc:dddd".parse().unwrap();
        // 同一 /64 内轮换隐私地址：同一个桶。
        assert_eq!(failure_bucket_key(Some(a)), failure_bucket_key(Some(b)));
        assert_ne!(
            failure_bucket_key(Some(a)),
            failure_bucket_key(Some(other_net))
        );
        // IPv4-mapped IPv6 与 IPv4 是同一来源。
        let mapped: IpAddr = "::ffff:203.0.113.9".parse().unwrap();
        let plain: IpAddr = "203.0.113.9".parse().unwrap();
        assert_eq!(
            failure_bucket_key(Some(mapped)),
            failure_bucket_key(Some(plain))
        );
        assert_eq!(failure_bucket_key(None), UNKNOWN_SOURCE_KEY);

        // 行为层面：换 /64 内的地址不能绕过单来源限额。
        let responder = PairingResponder::new(LinkIdentity::generate(), self_info("NAS"));
        let mut initiator = PairingInitiator::new(LinkIdentity::generate());
        let real = responder.generate_code();
        let bad = if real == "999999" { "888888" } else { "999999" };
        for i in 0..MAX_FAILED_HELLOS {
            let rotating: IpAddr = format!("2001:db8:1:2::{:x}", i + 1).parse().unwrap();
            let hello = initiator.build_hello(bad, &self_info("L"), vec![]);
            assert!(matches!(
                responder.handle_hello(&hello, Some(rotating)),
                Err(LinkError::InvalidCode)
            ));
        }
        let fresh: IpAddr = "2001:db8:1:2:dead:beef:0:1".parse().unwrap();
        let good = initiator.build_hello(&real, &self_info("L"), vec![]);
        assert!(matches!(
            responder.handle_hello(&good, Some(fresh)),
            Err(LinkError::Throttled)
        ));
    }

    #[test]
    fn self_info_rename_applies_to_new_hellos_but_not_to_sessions_in_flight() {
        let responder = PairingResponder::new(LinkIdentity::generate(), self_info("NAS"));
        let mut initiator = PairingInitiator::new(LinkIdentity::generate());
        let (_hello, resp) = hello_only(&responder, &mut initiator);
        assert_eq!(resp.name, "NAS");
        // 握手进行中改名：已发出的 hello 回复里的名字是转录的一部分，reveal 仍要验签通过。
        responder.set_self_info(self_info("NAS-renamed"));
        let reveal = initiator
            .on_hello_response(&resp, "10.0.0.1:17800", &[])
            .unwrap();
        let accepted = responder.handle_reveal(&reveal).unwrap();
        initiator.on_reveal_response(&accepted.response).unwrap();
        // 之后的新会话用新名字。
        let (_hello, resp2) = hello_only(
            &responder,
            &mut PairingInitiator::new(LinkIdentity::generate()),
        );
        assert_eq!(resp2.name, "NAS-renamed");
    }

    // ── 控制请求（request_control）──────────────────────────────────────────

    /// 引入控制标志之前的 hello 被签名字节：按旧布局手工拼出，独立于 `hello_message`，
    /// 用来钉死「不请求控制时与旧版逐字节一致」。
    fn legacy_hello_bytes(req: &HelloRequest) -> Vec<u8> {
        let mut t = Vec::new();
        t.extend_from_slice(b"fluxdown-link-hello-v2");
        push_framed(&mut t, req.code.as_bytes());
        t.extend_from_slice(&req.initiator_id_pub);
        t.extend_from_slice(&req.initiator_commit);
        push_framed(&mut t, req.name.as_bytes());
        push_framed(&mut t, req.platform.as_deref().unwrap_or("").as_bytes());
        push_framed(&mut t, req.app_version.as_deref().unwrap_or("").as_bytes());
        t.extend_from_slice(&(req.initiator_addrs.len() as u32).to_le_bytes());
        for addr in &req.initiator_addrs {
            push_framed(&mut t, addr.as_bytes());
        }
        t
    }

    #[test]
    fn hello_bytes_without_control_request_match_the_legacy_layout() {
        let mut req = HelloRequest {
            platform: Some("macos".to_owned()),
            app_version: Some("1.2.3".to_owned()),
            initiator_addrs: vec!["10.0.0.2:17800".to_owned(), "[fe80::1]:1".to_owned()],
            ..bare_hello("123456", [3u8; 32], [4u8; 32])
        };
        let legacy = legacy_hello_bytes(&req);
        assert_eq!(hello_message(&req), legacy);

        // 请求控制：只在 addrs 之后追加 framed("fluxdown-link-control-v1")，其余字节不变。
        req.request_control = true;
        let mut expected = legacy;
        expected.extend_from_slice(&24u32.to_le_bytes());
        expected.extend_from_slice(b"fluxdown-link-control-v1");
        assert_eq!(hello_message(&req), expected);
    }

    #[test]
    fn control_flag_is_covered_by_the_hello_signature() {
        for request in [true, false] {
            let responder = PairingResponder::new(LinkIdentity::generate(), self_info("NAS"));
            let mut initiator =
                PairingInitiator::new(LinkIdentity::generate()).request_control(request);
            let code = responder.generate_code();
            let mut hello = initiator.build_hello(&code, &self_info("L"), vec![]);
            assert_eq!(hello.request_control, request);
            // 中间人增删控制标志：签名失败，且不烧掉配对码。
            hello.request_control = !request;
            assert!(
                matches!(
                    responder.handle_hello(&hello, None),
                    Err(LinkError::BadSignature)
                ),
                "flipping request_control from {request} must break the signature"
            );
            hello.request_control = request;
            assert!(responder.handle_hello(&hello, None).is_ok());
        }
    }

    #[tokio::test]
    async fn control_is_granted_only_when_requested_and_approved() {
        for (request, approve) in [(true, true), (true, false), (false, true), (false, false)] {
            let responder = PairingResponder::new(LinkIdentity::generate(), self_info("NAS"));
            let mut initiator =
                PairingInitiator::new(LinkIdentity::generate()).request_control(request);
            let run = run_handshake(&responder, &mut initiator, vec![]);
            assert_eq!(
                run.accepted.request_control, request,
                "the incoming request must tell the user whether control is requested"
            );
            let session_id = &run.hello_resp.session_id;
            responder.set_local_decision(session_id, approve).unwrap();
            let outcome = responder.handle_confirm(session_id, true).await;
            if !approve {
                assert!(matches!(outcome, Err(LinkError::RejectedByPeer)));
                continue;
            }
            let host_side = outcome.unwrap().unwrap();
            assert_eq!(
                host_side.grants_control, request,
                "grants_control needs both the request and the approval"
            );
            assert!(!host_side.controllable);
            let controller_side = initiator.finalize().unwrap();
            assert_eq!(controller_side.controllable, request);
            assert!(!controller_side.grants_control);
        }
    }
}
