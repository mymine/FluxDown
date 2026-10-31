//! FluxMesh（临时文件分享）：agent 侧的 `fluxdown_share` 宿主。设计见 `docs/temp-share-design.md`。
//!
//! - [`ShareService`]（`service.rs` / `lifecycle.rs` / `ops.rs`）：状态投影、端点生命周期、
//!   发送（哈希 → 云端登记 → 发送方登记表）、接收（resolve → 回环适配器 → 普通 daemon 下载任务）、
//!   全部 `agent.share.*` 方法与分享链接出口（[`crate::share_link::ShareLinkSink`]）。
//! - `cloud.rs`：FluxCloud `/share/*` 线上形态与错误映射（HTTP 调用在 `cloud/api.rs`）。
//! - `store.rs`：iroh 私钥（独立 0600 文件）、本机设置、分享登记、接收记录的小文件持久化。
//! - `telemetry.rs`：上报出口（有界队列 + 退避后台发送）与会话诊断簿。
//! - `logs.rs`：iroh tracing 内存环形缓冲（诊断页）。
//! - `source.rs`：接收会话的 HTTP 区间来源（云端副本 CDN / 网关桥接），`switchToBridge` 的数据面。
//! - `swarm.rs`：共享（多设备按内容互相取块）——内容表同步、共享生命周期、云端登记与对端查询。
//! - `upload.rs` / `upload_job.rs` / `upload_state.rs`：云端暂存（`convertToCloud` / `deleteCloudCopy`）——
//!   服务侧装配与启动续传、上传引擎（方案 → 秒传证明 → 分片 PUT → complete）、断点文件。

mod cloud;
mod compliance;
mod convert;
mod exit;
mod guest;
mod lifecycle;
pub mod logs;
mod ops;
mod service;
mod source;
mod store;
mod swarm;
mod telemetry;
mod time;
mod upload;
mod upload_job;
mod upload_state;
mod usage;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod source_tests;
#[cfg(test)]
mod swarm_tests;
#[cfg(test)]
mod upload_tests;

pub use service::{ShareService, ShareServiceParts};
pub use store::StoreError;
