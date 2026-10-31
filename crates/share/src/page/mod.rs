//! FluxMesh 分享页（活动栏路由内容）：顶部一行摘要 + 「我的分享」列表，没有页签。
//!
//! 状态只读 [`ShareHub`]。接收到的文件是普通下载任务（接收信息在任务详情「传输诊断」里），完整额度视图在
//! 设置 › 账户，网络诊断在设置 › FluxMesh；取件码 / 链接输入在新建下载对话框与命令面板「接收分享」。
//!
//! 按职责拆分：`summary`（摘要行：最受限额度 / 贡献 / 共享开关）、`mine`（我的分享列表与行内操作）、
//! `table`（列表的列宽与表头）。

mod mine;
mod summary;
mod table;

use std::collections::{HashMap, HashSet};

use fluxdown_ui_components::FluxIcon;
use fluxdown_ui_i18n::Translator;
use fluxdown_ui_theme::active_theme;
use gpui::{Context, Entity, IntoElement, ParentElement as _, Render, Styled as _, Window};
use gpui_component::v_flex;

use crate::{
    hub::ShareHub,
    manage_ui::{empty_state, text},
    usage_sync::UsageSync,
};

/// FluxMesh 分享页（活动栏路由内容）。
pub struct ShareView {
    hub: Entity<ShareHub>,
    translator: Entity<Translator>,
    /// 摘要行额度的「只在可见时」取数状态。
    usage_sync: UsageSync,
    /// 摘要行「共享已接收的文件」开关：提交后到 hub 回流之前先显示的乐观值。
    provide_override: Option<bool>,
    /// 开关的 `setLocalConfig` 请求在途：期间开关不可再点。
    provide_inflight: bool,
    /// 正在提交操作的分享 ID 及其操作（停止 / 开始 / 删除），防重入，并让点击的按钮转圈。
    busy: HashMap<String, crate::model::ShareAction>,
    /// 「我的分享」多选集合（分享 ID）；分享从列表消失时在 `absorb_hub` 里剔除。
    selected: HashSet<String>,
}

impl ShareView {
    /// 创建分享页：`hub` 是跨窗口共享的FluxMesh状态；列表的选择 / 防重入状态只在本视图内持有。
    pub fn new(hub: Entity<ShareHub>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let translator = hub.read(cx).translator().clone();
        cx.observe(&hub, |this, _, cx| this.absorb_hub(cx)).detach();
        cx.observe(&translator, |_, _, cx| cx.notify()).detach();
        Self {
            hub,
            translator,
            usage_sync: UsageSync::new(),
            provide_override: None,
            provide_inflight: false,
            busy: HashMap::new(),
            selected: HashSet::new(),
        }
    }

    /// hub 变化：剔除已消失分享的选择；开关乐观值在请求结束后交还给 hub 的值。
    fn absorb_hub(&mut self, cx: &mut Context<Self>) {
        if !self.selected.is_empty() {
            let state = self.hub.read(cx).state();
            self.selected
                .retain(|id| state.shares.iter().any(|share| &share.id == id));
        }
        if !self.provide_inflight {
            self.provide_override = None;
        }
        cx.notify();
    }
}

// ───────────────────────── 渲染 ─────────────────────────

impl Render for ShareView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = active_theme(cx);
        let tokens = theme.tokens().clone();
        let translator = self.translator.read(cx).clone();

        let root = v_flex().size_full().min_h_0();
        if !self.hub.read(cx).available() {
            return root.child(empty_state(
                FluxIcon::Share2,
                None,
                text(&translator, "shareNavTitle"),
                Some(text(&translator, "errReasonShareUnavailable")),
                cx,
            ));
        }

        root.px(tokens.spacing.lg)
            .pt(tokens.spacing.md)
            .gap(tokens.spacing.md)
            .child(self.render_summary(&translator, cx))
            .child(self.render_mine(&translator, cx))
    }
}
