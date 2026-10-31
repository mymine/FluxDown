//! FluxMesh「分享」入口（下载页一侧）：右键菜单 / 选择条 / 停靠详情面板 / 命令面板都走这里。
//!
//! 可见性与禁用原因由 [`crate::model::share::share_control`] 统一判定（见
//! [`crate::model::TaskStore::share_control`]）；点击只把任务 id 交给宿主的
//! [`super::downloads::DownloadHostActions::share_tasks`]，分享面板由 app 打开。

use fluxdown_ui_components::FluxIcon;
use gpui::{AnyElement, App, Context, Styled as _, Window};
use gpui_component::Icon;

use crate::{
    actions::ShareSelected,
    model::{RowKey, share::ShareControl},
    pages::downloads::DownloadView,
};

impl DownloadView {
    /// 当前表格选区的「分享」入口呈现。
    pub(crate) fn selected_share_control(&self, cx: &App) -> ShareControl {
        let keys = self.table_state.read(cx).delegate().selected_keys();
        self.controller.store().share_control(&keys)
    }

    /// 选区可分享（命令面板 / 菜单据此启用「分享」）。
    pub fn can_share_selected(&self, cx: &App) -> bool {
        self.selected_share_control(cx).is_enabled()
    }

    /// 把这些任务交给宿主打开分享面板；入口当前不可用（隐藏 / 禁用）时什么也不做。
    pub(crate) fn share_keys(&self, keys: &[RowKey], window: &mut Window, cx: &mut App) {
        if !self.controller.store().share_control(keys).is_enabled() {
            return;
        }
        let Some(open) = self.host.share_tasks.clone() else {
            return;
        };
        let task_ids = keys.iter().map(|key| key.task_id().to_owned()).collect();
        open(task_ids, window, cx);
    }

    pub(crate) fn share_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let keys = self.table_state.read(cx).delegate().selected_keys();
        self.share_keys(&keys, window, cx);
    }

    pub(crate) fn on_share_selected(
        &mut self,
        _: &ShareSelected,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.share_selected(window, cx);
    }

    /// 任务行角标上的「切换桥接」。
    pub(crate) fn share_switch_to_bridge(
        &self,
        task_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(open) = self.host.share_switch_to_bridge.clone() {
            open(task_id, window, cx);
        }
    }

    /// 任务行角标上的「额度已用完 / 需登录」chip：打开设置 › 账户的额度。
    pub(crate) fn share_open_usage(
        &self,
        task_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(open) = self.host.share_open_usage.clone() {
            open(task_id, window, cx);
        }
    }

    /// 任务右键「停止共享」：按任务当前事实取接收记录 ID 交给宿主；记录已不在共享时什么也不做。
    pub(crate) fn share_stop_providing(
        &self,
        task_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(stop) = self.host.share_stop_providing.clone() else {
            return;
        };
        let record_id = self
            .controller
            .store()
            .share()
            .providing_record(task_id)
            .map(str::to_owned);
        if let Some(record_id) = record_id {
            stop(record_id, window, cx);
        }
    }

    /// 图标型「分享」按钮：隐藏时返回 `None`；禁用时保留悬浮提示并说明原因。
    pub(crate) fn share_button(
        &self,
        id: &'static str,
        control: ShareControl,
        icon_size: gpui::Pixels,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !control.is_visible() {
            return None;
        }
        let label = match control.block() {
            Some(block) => self.strings.share_disabled(block),
            None => self.strings.share.clone(),
        };
        Some(self.icon_action_with_state(
            id,
            label,
            Icon::new(FluxIcon::Share2).size(icon_size),
            false,
            !control.is_enabled(),
            on_click,
            cx,
        ))
    }
}
