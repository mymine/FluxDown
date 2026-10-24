//! `/rpc` 会话的凭据记录与撤销后的重新校验。
//!
//! 每条会话记住建连时通过的凭据种类（[`RpcCredential`]）。以下事件之后，会话重新校验自己的凭据，
//! 失效即由 `run_socket` 关闭 WebSocket（客户端重连会拿到 401）：
//!
//! - 设备凭据：名册里的设备记录被删除 / 撤销 / 重新配对（`link_secret` 变化）/ 失去控制授权
//!   （[`LinkService::subscribe_roster`](crate::link::LinkService::subscribe_roster)）；
//! - 访问密钥：密钥被改（`agent.gateway.patch` 的 `userToken` / `regenerateUserToken`）——用旧密钥
//!   认证的会话断开，发起修改的那条会话除外（它记住的值同步更新为新值，见
//!   [`SessionAuth::rebind_access_key`]）。
//!
//! `agent.token` 与进程内连接不依赖名册或访问密钥，永不被这些事件撤销。

use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::watch;

/// 撤销时发给客户端的 WebSocket 关闭码（策略违规）：区别于 4009（事件落后，立即重连）与
/// `service-quit`（服务退出，停止重连）——客户端重连会在升级阶段得到 401。
pub(super) const CLOSE_CODE_CREDENTIAL_REVOKED: u16 = 1008;
/// 与 [`CLOSE_CODE_CREDENTIAL_REVOKED`] 一同发送的关闭原因。
pub(super) const CLOSE_REASON_CREDENTIAL_REVOKED: &str = "credential-revoked";

/// 一条 `/rpc` 会话建连时通过的凭据。
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum RpcCredential {
    /// `agent.token`（`agent.token` 文件内容，仅本机进程持有）或进程内连接。
    Agent,
    /// server 模式的访问密钥；记住所呈值，密钥被改后用旧值认证的会话必须断开。
    AccessKey(String),
    /// 设备控制凭据（局域网配对得到）；名册变化后按名册重新校验。
    Device { token: String, fingerprint: String },
}

/// 凭据明文不进日志 / 断言输出。
impl fmt::Debug for RpcCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Agent => f.write_str("Agent"),
            Self::AccessKey(_) => f.write_str("AccessKey(..)"),
            Self::Device { fingerprint, .. } => f
                .debug_struct("Device")
                .field("fingerprint", fingerprint)
                .finish_non_exhaustive(),
        }
    }
}

/// 会话当前记住的凭据（访问密钥会话在自己改密钥后改写它）。
pub(crate) struct SessionAuth {
    credential: Mutex<RpcCredential>,
}

impl SessionAuth {
    pub(crate) fn new(credential: RpcCredential) -> Arc<Self> {
        Arc::new(Self {
            credential: Mutex::new(credential),
        })
    }

    pub(crate) fn credential(&self) -> RpcCredential {
        self.credential
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// 本会话自己改了访问密钥：把记住的值更新为新值，使它不被随后的撤销检查踢掉。只对
    /// 以访问密钥认证的会话生效；其它凭据种类不受访问密钥影响。
    pub(crate) fn rebind_access_key(&self, key: &str) {
        let mut credential = self
            .credential
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if matches!(*credential, RpcCredential::AccessKey(_)) {
            *credential = RpcCredential::AccessKey(key.to_owned());
        }
    }
}

/// 一条会话的连接级上下文：请求分发据此决定「本机来源」放行范围，并把改密钥的那条会话
/// 标记出来。
#[derive(Clone)]
pub(crate) struct SessionContext {
    /// 能否在主机上打开 / 定位任务产物（`agent.platform.*`）。建连时一次判定：桌面宿主的 `agent.token`
    /// 与进程内连接为 `true`；server 模式见 `ServerHandle::local_platform_permitted`；设备凭据
    /// 会话恒为 `false`（远程手机不能触发主机本地平台能力）。
    pub(crate) allow_local_platform: bool,
    pub(crate) auth: Arc<SessionAuth>,
}

impl SessionContext {
    pub(crate) fn new(allow_local_platform: bool, credential: RpcCredential) -> Self {
        Self {
            allow_local_platform,
            auth: SessionAuth::new(credential),
        }
    }

    /// 进程内可信调用方（嵌入式 App、托盘、剪贴板监听）。
    pub(crate) fn local(allow_local_platform: bool) -> Self {
        Self::new(allow_local_platform, RpcCredential::Agent)
    }
}

/// 等待订阅的变更信号；没有订阅（`None`）或发送端已关闭时永久挂起，让 `tokio::select!` 的该分支
/// 永不触发。
pub(super) async fn wait_changed(receiver: &mut Option<watch::Receiver<u64>>) {
    if let Some(receiver) = receiver
        && receiver.changed().await.is_ok()
    {
        return;
    }
    std::future::pending::<()>().await;
}
