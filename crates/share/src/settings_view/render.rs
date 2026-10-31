//! 页面装配：不可用 / 断连提示 + 四个分组。

use fluxdown_ui_theme::active_theme;
use gpui::{
    Context, IntoElement, ParentElement as _, Render, Styled as _, Window,
    prelude::FluentBuilder as _,
};
use gpui_component::v_flex;

use super::{
    ShareSettingsView,
    groups::GroupCtx,
    rows::{meta_text, text},
};

impl Render for ShareSettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = active_theme(cx).tokens().clone();
        let translator = self.translator.read(cx).clone();
        let (enabled, available, stale, policy) = {
            let hub = self.hub.read(cx);
            (
                hub.state().local_config.enabled,
                hub.available(),
                hub.is_stale(),
                hub.state().policy_summary.clone(),
            )
        };
        let group = GroupCtx {
            translator: &translator,
            policy: &policy,
        };
        let switch = self.enable_card(&group, window, cx);
        if !enabled {
            // 实验性功能默认关闭：只留总开关。
            return switch;
        }
        if !available {
            return v_flex()
                .w_full()
                .gap(tokens.spacing.lg)
                .child(switch)
                .child(
                    meta_text(cx)
                        .w_full()
                        .py(tokens.spacing.md)
                        .child(text(&translator, "shareSetUnavailable")),
                )
                .into_any_element();
        }
        let defaults = self.defaults_group(&group, window, cx);
        let network = self.network_group(&group, window, cx);
        let transfer = self.transfer_group(&group, window, cx);
        let diagnostics = self.diagnostics_group(&group, window, cx);
        v_flex()
            .w_full()
            .gap(tokens.spacing.lg)
            .when(stale, |this| {
                this.child(
                    meta_text(cx)
                        .text_color(tokens.colors.destructive)
                        .child(text(&translator, "shareSetStale")),
                )
            })
            .child(switch)
            .child(defaults)
            .child(network)
            .child(transfer)
            .child(diagnostics)
            .into_any_element()
    }
}
