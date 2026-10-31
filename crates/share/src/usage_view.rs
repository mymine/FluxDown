//! 额度与贡献视图（嵌在设置 › 账户页滚动内容里）：三种流量的本月额度、额度明细、贡献等级与赚取。
//!
//! 数据来自 `ShareStateDto.usage`（agent 经云端推送 / 事件刷新）；视图只在渲染（可见）时经
//! [`UsageSync`] 节流调用 `agent.share.usageRefresh`，hub 变化只重绘不取数，所以空闲时没有任何请求。
//! 不自管滚动；未登录 / 暂无数据时渲染空元素（购买 / 升级 / 登录入口由账户页自己提供）。

use fluxdown_protocol::{ShareTrafficKind, ShareUsageDto};
use fluxdown_ui_components::{BusyHold, ButtonVariant, FluxIcon, card, loading_icon_button};
use fluxdown_ui_i18n::Translator;
use fluxdown_ui_theme::active_theme;
use gpui::{
    AnyElement, App, Context, Div, Entity, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, prelude::FluentBuilder as _,
};
use gpui_component::{Icon, h_flex, v_flex};

use crate::{
    hub::ShareHub,
    manage_ui::{
        body_text, chip, clipped, meta_text, meter_row, section_header, text, text_with,
        tooltip_builder,
    },
    model::{
        format_datetime,
        usage::{boost_label, format_quota_bytes, grant_source_key, kind_key, level_label, meters},
    },
    usage_sync::UsageSync,
};

/// 明细表「剩余」「到期」列宽（默认字号下的 px）。
const COL_REMAINING_WIDTH: f32 = 96.;
const COL_EXPIRES_WIDTH: f32 = 128.;

/// 额度与贡献视图。
pub struct ShareUsageView {
    hub: Entity<ShareHub>,
    translator: Entity<Translator>,
    sync: UsageSync,
    /// 用户点「刷新」后的最短转圈保持。
    refresh_hold: BusyHold,
}

impl ShareUsageView {
    /// 创建额度视图；`hub` 是跨窗口共享的FluxMesh状态。
    pub fn new(hub: Entity<ShareHub>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let translator = hub.read(cx).translator().clone();
        // hub 变化（含刚取到的额度）只重绘，不触发新的取数。
        cx.observe(&hub, |_, _, cx| cx.notify()).detach();
        cx.observe(&translator, |_, _, cx| cx.notify()).detach();
        Self {
            hub,
            translator,
            sync: UsageSync::new(),
            refresh_hold: BusyHold::default(),
        }
    }

    fn refresh_clicked(&mut self, cx: &mut Context<Self>) {
        self.refresh_hold.start(cx);
        self.refresh_now(cx);
    }

    fn refresh_now(&mut self, cx: &mut Context<Self>) {
        let hub = self.hub.clone();
        self.sync
            .refresh_now(&hub, |this: &mut Self| &mut this.sync, cx);
    }
}

impl Render for ShareUsageView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let hub = self.hub.clone();
        self.sync
            .refresh_if_needed(&hub, |this: &mut Self| &mut this.sync, cx);
        let tokens = active_theme(cx).tokens().clone();
        let translator = self.translator.read(cx).clone();
        let state = self.hub.read(cx).state();
        let usage = state.usage.clone();
        let load_failed = self.sync.failed || state.usage_error.is_some();

        // 未登录 / 暂无数据：渲染空元素，不占位。
        let Some(usage) = usage else {
            return div();
        };
        let mut page = v_flex()
            .w_full()
            .gap(tokens.spacing.lg)
            .child(self.render_header(&usage, &translator, cx))
            .child(self.render_meters(&usage, &translator, cx));
        if load_failed {
            page = page.child(
                meta_text(cx)
                    .text_color(tokens.colors.destructive)
                    .child(text(&translator, "shareUsageLoadFailed")),
            );
        }
        if !usage.grants.is_empty() {
            page = page.child(render_grants(&usage, &translator, cx));
        }
        div()
            .w_full()
            .child(page.child(render_contribution(&usage, &translator, cx)))
    }
}

impl ShareUsageView {
    fn render_header(
        &self,
        usage: &ShareUsageDto,
        translator: &Translator,
        cx: &mut Context<Self>,
    ) -> Div {
        let tokens = active_theme(cx).tokens().clone();
        let reset = format_datetime(&usage.reset_at);
        let title = text_with(
            translator,
            "shareUsageMonth",
            &[("month", &usage.month), ("time", &reset)],
        );
        let label = text(translator, "shareNetRefresh");
        let loading = self.sync.loading || self.refresh_hold.is_held();
        let refresh: AnyElement = loading_icon_button(
            "share-usage-refresh",
            label.clone(),
            Icon::new(FluxIcon::RotateCw).size(active_theme(cx).extended().icon.md),
            ButtonVariant::Ghost,
            loading,
            cx,
        )
        .tooltip(tooltip_builder(label))
        .disabled(loading)
        .on_click(cx.listener(|this, _, _, cx| this.refresh_clicked(cx)))
        .into_any_element();
        v_flex()
            .w_full()
            .gap(tokens.spacing.xs)
            .child(section_header(title, None, Some(refresh), cx))
            .when(!usage.enforced.swarm, |this| {
                this.child(meta_text(cx).child(text(translator, "shareUsageNotEnforced")))
            })
    }

    fn render_meters(
        &self,
        usage: &ShareUsageDto,
        translator: &Translator,
        cx: &mut Context<Self>,
    ) -> Div {
        let tokens = active_theme(cx).tokens().clone();
        let reset = format_datetime(&usage.reset_at);
        let mut block = card(cx)
            .p(tokens.spacing.md)
            .flex()
            .flex_col()
            .gap(tokens.spacing.md);
        for meter in meters(usage) {
            let id = format!("share-usage-meter-{}", meter_suffix(meter.label_key));
            let value = if meter.unlimited {
                text(translator, "shareUsageUnlimited")
            } else if meter.exhausted {
                text_with(translator, "shareUsageExhausted", &[("time", &reset)])
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
            let ratio = (!meter.unlimited).then_some(meter.ratio);
            block = block.child(meter_row(
                SharedString::from(id),
                text(translator, meter.label_key),
                value,
                ratio,
                meter.exhausted,
                cx,
            ));
        }
        block
    }
}

/// `shareUsageKindSwarm` → `swarm`（元素 id 后缀）。
fn meter_suffix(label_key: &str) -> String {
    label_key
        .strip_prefix("shareUsageKind")
        .unwrap_or(label_key)
        .to_lowercase()
}

fn kind_name(translator: &Translator, kind: ShareTrafficKind) -> SharedString {
    kind_key(kind).map_or_else(SharedString::default, |key| text(translator, key))
}

fn render_grants(usage: &ShareUsageDto, translator: &Translator, cx: &App) -> Div {
    let tokens = active_theme(cx).tokens().clone();
    let header = |key: &str| meta_text(cx).child(text(translator, key));
    let mut table = card(cx)
        .p(tokens.spacing.md)
        .flex()
        .flex_col()
        .gap(tokens.spacing.sm)
        .child(
            h_flex()
                .w_full()
                .gap(tokens.spacing.sm)
                .child(header("shareUsageColSource").flex_1().min_w_0())
                .child(header("shareUsageColKind").flex_1().min_w_0())
                .child(
                    header("shareUsageColRemaining")
                        .w(gpui::px(COL_REMAINING_WIDTH))
                        .flex_none(),
                )
                .child(
                    header("shareUsageColExpires")
                        .w(gpui::px(COL_EXPIRES_WIDTH))
                        .flex_none(),
                ),
        );
    for (index, grant) in usage.grants.iter().enumerate() {
        let remaining = grant.bytes.saturating_sub(grant.used_bytes);
        let expires = grant.expires_at.as_deref().map_or_else(
            || text(translator, "shareUsageGrantNoExpiry"),
            |at| SharedString::from(format_datetime(at)),
        );
        table = table.child(
            h_flex()
                .w_full()
                .gap(tokens.spacing.sm)
                .child(
                    clipped(
                        SharedString::from(format!("share-usage-grant-source-{index}")),
                        text(translator, grant_source_key(grant.source)),
                    )
                    .flex_1()
                    .min_w_0()
                    .text_size(tokens.typography.sm.size),
                )
                .child(
                    clipped(
                        SharedString::from(format!("share-usage-grant-kind-{index}")),
                        kind_name(translator, grant.kind),
                    )
                    .flex_1()
                    .min_w_0()
                    .text_size(tokens.typography.sm.size),
                )
                .child(
                    clipped(
                        SharedString::from(format!("share-usage-grant-left-{index}")),
                        format_quota_bytes(remaining),
                    )
                    .w(gpui::px(COL_REMAINING_WIDTH))
                    .flex_none()
                    .text_size(tokens.typography.sm.size),
                )
                .child(
                    clipped(
                        SharedString::from(format!("share-usage-grant-expires-{index}")),
                        expires,
                    )
                    .w(gpui::px(COL_EXPIRES_WIDTH))
                    .flex_none()
                    .text_size(tokens.typography.sm.size),
                ),
        );
    }
    v_flex()
        .w_full()
        .gap(tokens.spacing.xs)
        .child(section_header(
            text(translator, "shareUsageGrantsTitle"),
            None,
            None,
            cx,
        ))
        .child(table)
}

fn render_contribution(usage: &ShareUsageDto, translator: &Translator, cx: &App) -> Div {
    let tokens = active_theme(cx).tokens().clone();
    let contribution = &usage.contribution;

    let mut level_row = h_flex()
        .w_full()
        .items_center()
        .gap(tokens.spacing.sm)
        .child(chip(
            "share-usage-level",
            level_label(contribution.level, contribution.level_max),
            None,
            cx,
        ));
    if let Some(next) = contribution.next_level_bytes {
        let gap = format_quota_bytes(next.saturating_sub(contribution.lifetime_bytes));
        level_row = level_row.child(meta_text(cx).min_w_0().truncate().child(text_with(
            translator,
            "shareUsageNextLevel",
            &[("bytes", &gap)],
        )));
    }

    let mut block = card(cx)
        .p(tokens.spacing.md)
        .flex()
        .flex_col()
        .gap(tokens.spacing.sm)
        .child(level_row)
        .child(body_text(cx).child(text_with(
            translator,
            "shareUsageContribMonth",
            &[("bytes", &format_quota_bytes(contribution.month_bytes))],
        )));
    if let Some(boost) = boost_label(contribution.boost_permille) {
        block = block.child(body_text(cx).child(text_with(
            translator,
            "shareUsageBoost",
            &[("boost", &boost)],
        )));
    }
    for earn in &contribution.earn {
        if kind_key(earn.kind).is_none() {
            continue;
        }
        let kind = kind_name(translator, earn.kind);
        let earned = format_quota_bytes(earn.earned_this_month);
        let line = if earn.cap_bytes == 0 {
            text_with(
                translator,
                "shareUsageEarnedNoCap",
                &[("kind", &kind), ("earned", &earned)],
            )
        } else {
            text_with(
                translator,
                "shareUsageEarned",
                &[
                    ("kind", &kind),
                    ("earned", &earned),
                    ("cap", &format_quota_bytes(earn.cap_bytes)),
                    ("months", &earn.valid_months.to_string()),
                ],
            )
        };
        block = block.child(body_text(cx).child(line));
    }
    if contribution.priority_active {
        let line = if contribution.priority {
            text(translator, "shareUsagePriorityOn")
        } else if contribution.priority_threshold_bytes > 0 {
            text_with(
                translator,
                "shareUsagePriorityOffContrib",
                &[
                    (
                        "bytes",
                        &format_quota_bytes(contribution.priority_threshold_bytes),
                    ),
                    ("days", &contribution.priority_window_days.to_string()),
                ],
            )
        } else {
            text(translator, "shareUsagePriorityOffPlan")
        };
        block = block.child(meta_text(cx).child(line));
    }

    v_flex()
        .w_full()
        .gap(tokens.spacing.xs)
        .child(section_header(
            text(translator, "shareUsageContributionTitle"),
            None,
            None,
            cx,
        ))
        .child(block)
}
