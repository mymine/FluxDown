//! FluxMesh状态中枢：整个应用唯一一份 `AgentSnapshot.share` 的投影。
//!
//! 分享页、分享面板、接收窗口、设置页与诊断视图都观察同一个 `ShareHub`（跨窗口共享实体），
//! 由 app 把 agent 会话的快照 / 事件喂进来；视图只读状态、经 [`ShareApi`] 发命令。

use std::collections::HashMap;

use fluxdown_protocol::{
    AgentEvent, AgentSnapshot, ServiceEvent, ShareDto, ShareHashingDto, ShareResolveDto,
    ShareStateDto, ShareTaskFactsDto,
};
use fluxdown_ui_i18n::Translator;
use gpui::{Context, Entity, EventEmitter};

use crate::{host::ShareHost, port::ShareApi};

/// hub 对外发出的一次性事件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShareHubEvent {
    /// agent 解析出一个等待用户确认的接收预览（链接 / 深链 / 剪贴板打开）；同一个 `resolveId` 只宣告一次。
    /// app 据此打开接收确认窗口。
    PendingReceive { resolve_id: String },
}

/// 一份分享上正在进行的云端副本操作：面板 / 列表里被点的按钮据此转圈。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CloudOp {
    /// 转为云端暂存（含上传失败后的重试）。
    Convert,
    /// 删除云端副本（含取消上传）。
    Delete,
}

pub struct ShareHub {
    api: ShareApi,
    translator: Entity<Translator>,
    state: ShareStateDto,
    /// 与 agent 的会话已断开：状态是最后一次所见，命令不可用。
    stale: bool,
    /// agent 当前有账号会话（云端暂存的转换 / 删除要求已登录，游客会被拒）。
    signed_in: bool,
    host: ShareHost,
    /// 待网络诊断视图吸收的定位请求：外层 `Some` = 有请求，内层 = 要聚焦的会话 ID。
    diagnostics_request: Option<Option<String>>,
    /// 已宣告过的待确认接收，避免同一个预览反复弹窗。
    announced_pending: Option<String>,
    /// 进行中的云端副本操作（分享 ID → 操作）：RPC 发出时登记，成败后撤销。
    cloud_ops: HashMap<String, CloudOp>,
}

impl EventEmitter<ShareHubEvent> for ShareHub {}

impl ShareHub {
    pub fn new(translator: Entity<Translator>, api: ShareApi) -> Self {
        Self {
            api,
            translator,
            state: ShareStateDto::default(),
            stale: true,
            signed_in: false,
            host: ShareHost::default(),
            diagnostics_request: None,
            announced_pending: None,
            cloud_ops: HashMap::new(),
        }
    }

    /// 该分享上进行中的云端副本操作。
    #[must_use]
    pub(crate) fn cloud_op(&self, share_id: &str) -> Option<CloudOp> {
        self.cloud_ops.get(share_id).copied()
    }

    /// 登记 / 撤销（`None`）某分享上进行中的云端副本操作，并通知观察者重绘。
    pub(crate) fn set_cloud_op(
        &mut self,
        share_id: &str,
        op: Option<CloudOp>,
        cx: &mut Context<Self>,
    ) {
        match op {
            Some(op) => {
                self.cloud_ops.insert(share_id.to_owned(), op);
            }
            None => {
                self.cloud_ops.remove(share_id);
            }
        }
        cx.notify();
    }

    /// 注入窗口入口（app 在创建窗口能力时调用）。
    pub fn set_host(&mut self, host: ShareHost) {
        self.host = host;
    }

    #[must_use]
    pub fn host(&self) -> &ShareHost {
        &self.host
    }

    #[must_use]
    pub fn api(&self) -> ShareApi {
        self.api.clone()
    }

    #[must_use]
    pub fn translator(&self) -> &Entity<Translator> {
        &self.translator
    }

    #[must_use]
    pub fn state(&self) -> &ShareStateDto {
        &self.state
    }

    #[must_use]
    pub fn is_stale(&self) -> bool {
        self.stale
    }

    /// 本构建编入FluxMesh且服务可用：为 `false` 时全部FluxMesh入口隐藏。
    #[must_use]
    pub fn available(&self) -> bool {
        self.state.available
    }

    /// 可发起分享（已登录，或云端允许游客发起）。
    #[must_use]
    pub fn can_send(&self) -> bool {
        self.state.available && self.state.can_send
    }

    /// 已登录账号：转为云端暂存 / 删除云端副本的前提。
    #[must_use]
    pub fn signed_in(&self) -> bool {
        self.signed_in
    }

    #[must_use]
    pub fn share(&self, id: &str) -> Option<&ShareDto> {
        self.state.shares.iter().find(|share| share.id == id)
    }

    #[must_use]
    pub fn task_facts(&self, task_id: &str) -> Option<&ShareTaskFactsDto> {
        self.state.task_facts.get(task_id)
    }

    /// agent 持有的待确认接收预览。
    #[must_use]
    pub fn pending_receive(&self) -> Option<&ShareResolveDto> {
        self.state.pending_receive.as_ref()
    }

    /// 创建分享时正在做块哈希的任务。
    #[must_use]
    pub fn hashing(&self) -> &[ShareHashingDto] {
        &self.state.hashing
    }

    /// 请求网络诊断页定位（由 app 的 `open_diagnostics` 调用；`session_id` = 要聚焦的会话）。
    pub fn request_diagnostics(&mut self, session_id: Option<String>, cx: &mut Context<Self>) {
        self.diagnostics_request = Some(session_id);
        cx.notify();
    }

    /// 取走待处理的诊断定位请求（网络诊断视图创建 / 观察时调用）。
    pub fn take_diagnostics_request(&mut self) -> Option<Option<String>> {
        self.diagnostics_request.take()
    }

    /// 状态整段替换后：新出现的待确认接收只宣告一次；预览消失则允许同一 ID 之外的新预览再次宣告。
    fn absorb_pending(&mut self, cx: &mut Context<Self>) {
        let current = self
            .state
            .pending_receive
            .as_ref()
            .map(|pending| pending.resolve_id.clone());
        match current {
            Some(resolve_id) if self.announced_pending.as_deref() != Some(resolve_id.as_str()) => {
                self.announced_pending = Some(resolve_id.clone());
                cx.emit(ShareHubEvent::PendingReceive { resolve_id });
            }
            Some(_) => {}
            None => self.announced_pending = None,
        }
    }

    pub fn replace_snapshot(&mut self, snapshot: &AgentSnapshot, cx: &mut Context<Self>) {
        self.stale = false;
        self.signed_in = snapshot.session.is_some();
        self.state.clone_from(&snapshot.share);
        self.absorb_pending(cx);
        cx.notify();
    }

    pub fn apply_event(&mut self, event: &ServiceEvent, cx: &mut Context<Self>) {
        let ServiceEvent::Agent(event) = event else {
            return;
        };
        match event {
            AgentEvent::SessionChanged(session) => {
                let signed_in = session.is_some();
                if self.signed_in != signed_in {
                    self.signed_in = signed_in;
                    cx.notify();
                }
            }
            AgentEvent::ShareChanged(state) => {
                if self.state != *state {
                    self.state.clone_from(state);
                    self.absorb_pending(cx);
                    cx.notify();
                }
            }
            AgentEvent::ShareProgress(progress) => {
                // 只在传输进行中到达且已被合并；直接应用并通知，不做整体比较。
                self.state.apply_progress(progress);
                cx.notify();
            }
            _ => {}
        }
    }

    pub fn mark_stale(&mut self, cx: &mut Context<Self>) {
        if !self.stale {
            self.stale = true;
            cx.notify();
        }
    }
}
