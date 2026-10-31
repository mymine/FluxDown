//! 分享页顶部摘要行：最受限的一项额度、贡献等级与本月贡献、「共享已接收的文件」开关、额度详情入口。
//!
//! 完整额度视图在设置 › 账户；开关与设置 › FluxMesh 是同一份 `auto_provide` 配置，提交走同一个
//! [`prepare_commit`]（整体替换 `agent.share.setLocalConfig`）。

use fluxdown_ui_components::{ButtonVariant, button};
use fluxdown_ui_i18n::Translator;
use fluxdown_ui_theme::active_theme;
use gpui::{
    Context, Div, InteractiveElement as _, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div,
};
use gpui_component::{Disableable as _, h_flex, switch::Switch};

use super::ShareView;
use crate::{
    actions::notify_error,
    errors::error_text,
    manage_ui::{body_text, chip, clipped, meta_text, text, text_with, tooltip_builder},
    model::{
        format_datetime,
        settings_form::prepare_commit,
        usage::{format_quota_bytes, level_label, meters, most_constrained},
    },
};

impl ShareView {
    pub(super) fn open_account(&self, window: &mut Window, cx: &mut gpui::App) {
        let open = self.hub.read(cx).host().open_account.clone();
        if let Some(open) = open {
            open(window, cx);
        }
    }

    /// 开关当前值：请求在途 / 刚提交时用乐观值，否则读 hub 里的配置（`None` = 开）。
    fn auto_provide_on(&self, cx: &gpui::App) -> bool {
        self.provide_override.unwrap_or_else(|| {
            self.hub
                .read(cx)
                .state()
                .local_config
                .auto_provide
                .unwrap_or(true)
        })
    }

    /// 提交「共享已接收的文件」：以 hub 里的配置为底稿改一个字段并整体提交；失败弹错误通知并回退显示。
    fn set_auto_provide(&mut self, value: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.provide_inflight || self.hub.read(cx).is_stale() {
            return;
        }
        let base = self.hub.read(cx).state().local_config.clone();
        let mut next = base.clone();
        next.auto_provide = Some(value);
        let offset = chrono::Local::now().offset().local_minus_utc();
        let Some(next) = prepare_commit(&base, next, offset) else {
            return;
        };
        self.provide_override = Some(value);
        self.provide_inflight = true;
        let future = self.hub.read(cx).api().set_local_config(&next);
        let translator = self.translator.read(cx).clone();
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let result = future.await;
            let failure = result
                .as_ref()
                .err()
                .map(|error| error_text(&translator, error));
            let Ok(()) = this.update(cx, |this, cx| {
                this.provide_inflight = false;
                if failure.is_some() {
                    this.provide_override = None;
                }
                cx.notify();
            }) else {
                // 分享页已释放，结果无处展示。
                return;
            };
            if let Some(message) = failure {
                notify_error(handle, cx, message);
            }
        })
        .detach();
        cx.notify();
    }

    /// 摘要行的额度文案：不限只显示「不限」，其余 `流量种类 · 剩余 / 已用完`；没有任何可展示的额度为 `None`。
    fn quota_line(&self, translator: &Translator, cx: &gpui::App) -> Option<SharedString> {
        let usage = self.hub.read(cx).state().usage.clone()?;
        let views = meters(&usage);
        let meter = most_constrained(&views)?;
        if meter.unlimited {
            return Some(text(translator, "shareUsageUnlimited"));
        }
        let value = if meter.exhausted {
            text_with(
                translator,
                "shareUsageExhausted",
                &[("time", &format_datetime(&usage.reset_at))],
            )
        } else {
            text_with(
                translator,
                "shareUsageRemaining",
                &[
                    ("remaining", &format_quota_bytes(meter.remaining)),
                    ("allowance", &format_quota_bytes(meter.allowance)),
                ],
            )
        };
        Some(SharedString::from(format!(
            "{} · {value}",
            translator.text(meter.label_key)
        )))
    }

    pub(super) fn render_summary(
        &mut self,
        translator: &Translator,
        cx: &mut Context<Self>,
    ) -> Div {
        let hub = self.hub.clone();
        self.usage_sync
            .refresh_if_needed(&hub, |this: &mut Self| &mut this.usage_sync, cx);

        let tokens = active_theme(cx).tokens().clone();
        let (stale, signed_in) = {
            let hub = self.hub.read(cx);
            (hub.is_stale(), hub.signed_in())
        };
        let usage = self.hub.read(cx).state().usage.clone();
        let quota = self.quota_line(translator, cx);

        let mut row = h_flex()
            .w_full()
            .flex_none()
            .items_center()
            .gap(tokens.spacing.md)
            .px(tokens.spacing.sm);

        // 额度；未登录 / 无数据时给登录入口。
        if let Some(quota) = quota {
            row = row.child(
                meta_text(cx)
                    .min_w_0()
                    .child(clipped("share-summary-quota", quota)),
            );
        } else if usage.is_none() && (!signed_in || self.usage_sync.loaded) {
            row = row.child(
                button(
                    "share-summary-login",
                    text(translator, "shareUsageLogin"),
                    ButtonVariant::Link,
                    cx,
                )
                .on_click(cx.listener(|this, _, window, cx| this.open_account(window, cx))),
            );
        }

        // 贡献：等级 + 本月贡献；无数据时省略。
        if let Some(usage) = usage.as_ref() {
            let contribution = &usage.contribution;
            row = row
                .child(chip(
                    "share-summary-level",
                    level_label(contribution.level, contribution.level_max),
                    None,
                    cx,
                ))
                .child(meta_text(cx).min_w_0().child(clipped(
                    "share-summary-contrib",
                    text_with(
                        translator,
                        "shareUsageContribMonth",
                        &[("bytes", &format_quota_bytes(contribution.month_bytes))],
                    ),
                )));
        }

        // 「共享已接收的文件」开关（悬浮提示 = 说明）；请求在途或与 agent 断开时禁用。
        let checked = self.auto_provide_on(cx);
        let toggle = h_flex()
            .id("share-summary-provide")
            .flex_none()
            .items_center()
            .gap(tokens.spacing.sm)
            .tooltip(tooltip_builder(text(translator, "shareSetAutoProvideDesc")))
            .child(body_text(cx).child(text(translator, "shareSetAutoProvide")))
            .child(
                Switch::new("share-summary-provide-switch")
                    .checked(checked)
                    .disabled(stale || self.provide_inflight)
                    .on_click(cx.listener(|this, checked: &bool, window, cx| {
                        this.set_auto_provide(*checked, window, cx);
                    })),
            );

        row.child(div().flex_1()).child(toggle).child(
            button(
                "share-summary-details",
                text(translator, "shareSummaryDetails"),
                ButtonVariant::Link,
                cx,
            )
            .on_click(cx.listener(|this, _, window, cx| this.open_account(window, cx))),
        )
    }
}
