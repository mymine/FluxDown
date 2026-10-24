//! 本机服务之间经数据目录交换的文件契约：端点发布文件与 agent 启动失败记录。
//!
//! 这些文件只在同一台机器、同一用户的数据目录内读写（daemon → agent、agent → 桌面界面），
//! 不经网络传输、不含任何凭据；内容为 camelCase JSON，写入方一律原子替换。
//!
//! - 端点文件让监听方在默认端口不可用（被占用 / 落在系统保留区间）时回退到系统分配的
//!   端口，读取方每次连接前重读，不必猜端口。
//! - 启动失败记录让拉起方在子进程反复退出时给出具体原因，而不只是「连不上」。

use serde::{Deserialize, Serialize};

/// agent UI Gateway 端点：发布在 `agent.token` 同目录。
pub const GATEWAY_ENDPOINT_FILE: &str = "gateway-endpoint.json";

/// daemon 控制面端点：发布在 `daemon.token` 同目录。daemon 取得进程租约后先删除旧文件，
/// 控制面绑定成功后写入实际地址，退出时删除；旧版 daemon 从不发布。
pub const DAEMON_ENDPOINT_FILE: &str = "daemon-endpoint.json";

/// agent 启动失败记录：写在 `agent.token` 同目录；agent 开始提供服务后删除。
pub const AGENT_STARTUP_FAILURE_FILE: &str = "agent-startup-failure.json";

/// 已发布的本机 RPC 端点（无凭据）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalEndpoint {
    /// `ws://<回环地址>:<端口>/rpc`。
    pub rpc_url: String,
}

/// agent 启动失败所处的环节。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentStartupFailureKind {
    /// UI Gateway 无法监听：固定地址被占用 / 系统保留，或回退端口同样不可用。
    GatewayBind,
    /// agent 数据目录不可读写（权限、只读卷、磁盘已满）。
    DataDir,
    /// 其它启动错误；也承接新版本写入、本版本尚不认识的环节。
    #[serde(other)]
    Other,
}

/// agent 启动失败记录。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStartupFailure {
    pub kind: AgentStartupFailureKind,
    /// 完整错误链，仅供诊断展示。
    pub detail: String,
    /// 写入时刻（Unix 毫秒）：读取方据此排除早于本轮拉起的旧记录。
    pub at_ms: u64,
    /// 写入方 agent 版本。
    pub version: String,
}

#[cfg(test)]
mod tests {
    use super::{AgentStartupFailure, AgentStartupFailureKind};

    /// 新版 agent 写入的未知环节在旧版界面里按「其它」展示，而不是整条记录解析失败。
    #[test]
    fn unknown_failure_kind_reads_as_other() {
        let record: AgentStartupFailure = serde_json::from_str(
            r#"{"kind":"someFutureStage","detail":"boom","atMs":7,"version":"9.9.9"}"#,
        )
        .expect("record with an unknown kind");
        assert_eq!(record.kind, AgentStartupFailureKind::Other);
        assert_eq!(record.detail, "boom");
        assert_eq!(record.at_ms, 7);
    }
}
