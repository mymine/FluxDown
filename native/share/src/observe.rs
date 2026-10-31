//! 宿主可订阅的事件与上报出口。

use std::fmt;
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::types::{NetSnapshot, PathKind, SessionReport, SessionRole, SessionSummary};

/// 宿主实现的上报出口（把数据 POST 给 FluxCloud）。
///
/// 方法在 iroh 任务里同步调用，**不得阻塞**：实现应只入队，由宿主自己的任务做网络请求与重试。
pub trait ShareTelemetrySink: Send + Sync + 'static {
    /// 网络状态发生变化时的快照（`POST /share/telemetry/net`）。相同状态不会重复送达。
    fn net_snapshot(&self, snapshot: &NetSnapshot);

    /// 会话报告（`POST /share/telemetry/sessions`）：传输进行中按 `reportIntervalSecs` 一条，
    /// 终态一条 `is_final == true`。空闲时不产生任何报告。
    fn session_report(&self, report: &SessionReport);
}

/// 会话事件（宿主 UI 与分享管理使用）。
#[derive(Debug, Clone)]
pub enum SessionEvent {
    /// 新会话建立（发送方：对端已通过票据验证；接收方：握手与 Hello 完成）。
    Opened {
        /// 会话 ID。
        session_id: String,
        /// 分享 ID。
        share_id: String,
        /// 本机角色。
        role: SessionRole,
        /// 对端 endpoint ID（z32）。
        peer_endpoint_id: String,
    },
    /// 承载路径发生变化（首次选定、打洞成功、回落中继）。
    PathChanged {
        /// 会话 ID。
        session_id: String,
        /// 新路径类别。
        kind: PathKind,
        /// 对端传输地址。
        remote: String,
    },
    /// 传输进度，仅在传输进行中按上报间隔发出。
    Progress {
        /// 会话 ID。
        session_id: String,
        /// 会话累计字节。
        bytes: u64,
        /// 最近一个采样周期的速率（字节 / 秒）。
        rate_bps: f64,
        /// 当前路径类别。
        path_kind: PathKind,
    },
    /// 会话结束。
    Closed {
        /// 会话 ID。
        session_id: String,
        /// 异常终止原因；正常结束为 `None`。
        error: Option<String>,
        /// 终态摘要。
        summary: Box<SessionSummary>,
    },
    /// 分享方读源文件时发现文件已被删除或与登记的大小 / 修改时间不符（接收方得到 `changed`）。
    /// 同一分享可能重复发出，宿主应幂等处理。
    SourceChanged {
        /// 分享 ID。
        share_id: String,
    },
    /// 共享提供的完整文件已被删除 / 修改（大小或修改时间不符）。宿主应停止提供该内容；可能重复发出。
    ContentChanged {
        /// 文件根哈希（64 位小写 hex）。
        root_hash: String,
    },
}

/// 事件通道类型：宿主持有接收端。
pub type SessionEventSender = mpsc::UnboundedSender<SessionEvent>;

/// 事件 / 上报出口的集合。全部可选。
#[derive(Clone, Default)]
pub struct Observers {
    /// 会话事件通道。
    pub events: Option<SessionEventSender>,
    /// 上报出口。
    pub telemetry: Option<Arc<dyn ShareTelemetrySink>>,
}

impl fmt::Debug for Observers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Observers")
            .field("events", &self.events.is_some())
            .field("telemetry", &self.telemetry.is_some())
            .finish()
    }
}

impl Observers {
    pub(crate) fn emit(&self, event: SessionEvent) {
        if let Some(tx) = &self.events
            && tx.send(event).is_err()
        {
            tracing::debug!("session event receiver is gone");
        }
    }

    pub(crate) fn report_session(&self, report: &SessionReport) {
        if let Some(sink) = &self.telemetry {
            sink.session_report(report);
        }
    }

    pub(crate) fn report_net(&self, snapshot: &NetSnapshot) {
        if let Some(sink) = &self.telemetry {
            sink.net_snapshot(snapshot);
        }
    }
}
