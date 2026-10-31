//! FluxMesh（临时文件分享）capability：分享页、分享面板、接收确认、网络诊断与额度视图，以及「FluxMesh」设置页。
//!
//! 状态只有一份：[`ShareHub`] 投影 `AgentSnapshot.share`（跨窗口共享实体，由 app 把会话快照 / 事件
//! 喂进来）；视图观察它、经 [`ShareApi`] 调 `agent.share.*`。窗口由 app 创建，本 crate 通过
//! [`ShareHost`] 回调请求打开，不依赖 downloads / settings 等其它 capability；网络诊断 / 额度视图由
//! app 作为内容槽位注入设置窗口（设置 › FluxMesh › 网络诊断、设置 › 账户）。

mod actions;
mod appeal;
mod cloud_ui;
mod errors;
mod exit_ui;
mod host;
mod hub;
mod manage_ui;
pub mod model;
mod nested_scroll;
mod network;
mod page;
mod panel;
mod port;
mod qr;
mod receive;
mod report;
mod session_view;
mod settings_view;
mod task_diagnostics;
mod usage_sync;
mod usage_view;
mod window_ui;

pub use actions::{stop_providing, switch_to_bridge};
pub use errors::{error_key, error_text};
pub use exit_ui::{ExitContinuation, needs_exit_gate, run_exit_gate};
pub use host::{
    AccountOpener, DiagnosticsOpener, ManageOpener, PanelOpener, ReceiveOpener, ShareHost,
};
pub use hub::{ShareHub, ShareHubEvent};
pub use model::{PanelTarget, panel_key};
pub use network::ShareNetworkView;
pub use page::ShareView;
pub use panel::SharePanelView;
pub use port::{PortFuture, ShareApi, SharePort};
pub use receive::{ReceiveSource, ShareReceiveView};
pub use settings_view::ShareSettingsView;
pub use task_diagnostics::ShareDiagnosticsView;
pub use usage_view::ShareUsageView;
