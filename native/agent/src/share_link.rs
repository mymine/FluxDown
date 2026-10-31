//! 分享链接（`fluxdown://share/<slug>`、`https://fluxdown.com/s/<slug>`）的路由出口。
//!
//! 捕获入口（系统打开链接、剪贴板、浏览器扩展接管）不认识FluxMesh；它们只把分享链接交给这里的
//! [`ShareLinkSink`]。服务编入时由FluxMesh服务实现（解析后在 `ShareStateDto.pendingReceive` 里等 UI 确认），
//! 未编入时没有 sink，捕获按「不支持」拒绝——分享链接绝不会落成一条损坏的下载任务。

use async_trait::async_trait;
use fluxdown_protocol::RpcErrorData;

/// 接收分享链接的出口。
#[async_trait]
pub trait ShareLinkSink: Send + Sync {
    /// 解析链接并登记待确认的接收预览；不创建任何下载任务。
    async fn open_link(&self, link: &str) -> Result<(), RpcErrorData>;

    /// 免打扰接收：解析链接并直接接收全部文件（不经确认预览）。`save_dir` 缺省用 daemon 默认目录。
    async fn receive_link(&self, link: &str, save_dir: Option<&str>) -> Result<(), RpcErrorData>;
}
