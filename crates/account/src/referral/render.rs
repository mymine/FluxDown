//! 推介页渲染：分段标签 + 说明 / 我的推荐码 / 收益记录三个面板。

use fluxdown_protocol::{
    CloudReferralCode, CloudReferralCodesResult, CloudReferralRecord, CloudReferralRecordsResult,
    CloudReferralSummary,
};
use fluxdown_ui_components::{
    ButtonVariant, ControlExt as _, FluxIcon, button, caption_number, card, icon_button,
    segmented_tabs, tabular_numbers, tooltip_view,
};
use fluxdown_ui_theme::{ExtendedTokens, SemanticThemeTokens, active_theme};
use gpui::{
    ClickEvent, Context, FontWeight, Hsla, InteractiveElement as _, IntoElement, ParentElement,
    Render, SharedString, StatefulInteractiveElement as _, Styled, Window, div,
    prelude::FluentBuilder as _, relative,
};
use gpui_component::{
    Icon, IconName, h_flex, input::Input, scroll::ScrollableElement as _, skeleton::Skeleton,
    text::TextView, v_flex,
};

use super::{Load, ReferralView, Tab, codes_full, page_count};
use crate::billing::{MAX_REFERRAL_CODES, format_local_date, format_minor};
use crate::ui::{NoticeTone, notice};
use crate::{t_with, ui};

/// 列表加载骨架行数。
const SKELETON_ROWS: usize = 3;
/// 推介奖励金额固定以人民币展示。
const CURRENCY: &str = "CNY";
/// 说明正文行高（与关于页更新说明一致的阅读行距）。
const DESCRIPTION_LINE_HEIGHT: f32 = 1.6;

/// 局部滚动区：占满剩余高度并在区内滚动；头部 / 统计 / 分页保持固定可见。
/// 必须 `flex_1`：滚动容器不以内容高度作为伸缩基准，按内容排布（`flex_initial`）会塌成 0 高。
fn scroll_region(id: &'static str, child: impl IntoElement) -> impl IntoElement {
    v_flex()
        .id(id)
        .flex_1()
        .min_h_0()
        .w_full()
        .overflow_y_scrollbar()
        .child(child)
}

impl Render for ReferralView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_loaded(window, cx);
        let theme = active_theme(cx);
        let tokens = theme.tokens().clone();
        let extended = theme.extended().clone();
        let labels: Vec<SharedString> = Tab::ALL
            .iter()
            .map(|tab| self.text(tab.label_key(), cx))
            .collect();
        let selected = Tab::ALL
            .iter()
            .position(|tab| *tab == self.tab)
            .unwrap_or(0);
        let view = cx.entity().downgrade();
        let tabs = segmented_tabs(
            "referral-tabs",
            labels,
            selected,
            move |index, window, cx| {
                let Some(tab) = Tab::ALL.get(index).copied() else {
                    return;
                };
                if let Err(error) = view.update(cx, |this, cx| this.set_tab(tab, window, cx)) {
                    log::debug!("referral view released before tab switch: {error:#}");
                }
            },
            cx,
        );
        let panel = match self.tab {
            Tab::Intro => self.render_intro(&tokens, &extended, cx).into_any_element(),
            Tab::Codes => self.render_codes(&tokens, &extended, cx).into_any_element(),
            Tab::Records => self
                .render_records(&tokens, &extended, cx)
                .into_any_element(),
        };
        // 设置宿主以「占满高度」挂载本页：标签固定在顶部，各面板自管局部滚动。
        v_flex()
            .size_full()
            .min_h_0()
            .gap(tokens.spacing.lg)
            .child(div().flex().flex_none().child(tabs))
            .child(v_flex().flex_1().min_h_0().w_full().child(panel))
    }
}

impl ReferralView {
    fn failed_block(
        &self,
        message: SharedString,
        retry_id: &'static str,
        tokens: &SemanticThemeTokens,
        on_retry: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        v_flex()
            .w_full()
            .items_center()
            .gap(tokens.spacing.md)
            .py(tokens.spacing.xl)
            .child(notice(NoticeTone::Danger, message, cx))
            .child(
                button(
                    retry_id,
                    self.text("accountDevicesRetry", cx),
                    ButtonVariant::Secondary,
                    cx,
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    on_retry(this, window, cx);
                })),
            )
    }

    fn skeleton_rows(&self, tokens: &SemanticThemeTokens, cx: &gpui::App) -> impl IntoElement {
        let theme = active_theme(cx);
        let rows: Vec<_> = (0..SKELETON_ROWS)
            .map(|_| {
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(tokens.spacing.md)
                    .px(tokens.spacing.md)
                    .py(tokens.spacing.sm)
                    .child(
                        v_flex()
                            .flex_1()
                            .gap(tokens.spacing.xs)
                            .child(
                                Skeleton::new()
                                    .w(theme.text_extent(140.))
                                    .h(tokens.typography.md.line_height)
                                    .rounded(tokens.radius.sm),
                            )
                            .child(
                                Skeleton::new()
                                    .w(theme.text_extent(220.))
                                    .h(tokens.typography.xs.line_height)
                                    .rounded(tokens.radius.sm),
                            ),
                    )
                    .into_any_element()
            })
            .collect();
        card(cx).w_full().children(rows)
    }

    fn empty_state(
        &self,
        text: SharedString,
        tokens: &SemanticThemeTokens,
        extended: &ExtendedTokens,
    ) -> impl IntoElement {
        v_flex()
            .w_full()
            .items_center()
            .gap(tokens.spacing.sm)
            .py(tokens.spacing.xxl)
            .child(
                Icon::new(FluxIcon::Gift)
                    .size(extended.icon.lg * 2.)
                    .text_color(extended.colors.text_tertiary),
            )
            .child(
                div()
                    .text_size(tokens.typography.sm.size)
                    .line_height(tokens.typography.sm.line_height)
                    .text_color(tokens.colors.muted_foreground)
                    .child(text),
            )
    }

    // ───────────────────────── 说明 ─────────────────────────

    fn render_intro(
        &self,
        tokens: &SemanticThemeTokens,
        extended: &ExtendedTokens,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        match &self.summary {
            Load::Idle | Load::Loading => v_flex()
                .w_full()
                .gap(tokens.spacing.md)
                .child(self.skeleton_rows(tokens, cx))
                .into_any_element(),
            Load::Failed(message) => self
                .failed_block(
                    message.clone(),
                    "referral-summary-retry",
                    tokens,
                    |this, window, cx| this.load_summary(window, cx),
                    cx,
                )
                .into_any_element(),
            Load::Ready(summary) => self
                .intro_body(summary, tokens, extended, cx)
                .into_any_element(),
        }
    }

    fn intro_body(
        &self,
        summary: &CloudReferralSummary,
        tokens: &SemanticThemeTokens,
        extended: &ExtendedTokens,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let translator = self.translator(cx);
        let stats = h_flex()
            .w_full()
            .gap(tokens.spacing.md)
            .child(stat_card(
                self.text("accountReferralStatPending", cx),
                format_minor(summary.pending_reward_minor, CURRENCY),
                tokens,
                cx,
            ))
            .child(stat_card(
                self.text("accountReferralStatPaid", cx),
                format_minor(summary.paid_reward_minor, CURRENCY),
                tokens,
                cx,
            ))
            .child(stat_card(
                self.text("accountReferralStatInvited", cx),
                summary.invited_count.to_string(),
                tokens,
                cx,
            ));
        let rules = (!summary.rules.is_empty()).then(|| {
            let rows: Vec<_> = summary
                .rules
                .iter()
                .enumerate()
                .map(|(index, rule)| {
                    let text = t_with(
                        &translator,
                        "accountReferralRuleItem",
                        &[
                            ("planName", &rule.plan_name),
                            ("percent", &rule.reward_percent.to_string()),
                            ("discount", &format_minor(rule.discount_minor, CURRENCY)),
                        ],
                    );
                    v_flex()
                        .w_full()
                        .when(index > 0, |this| this.child(ui::row_divider(cx)))
                        .child(
                            div()
                                .px(tokens.spacing.md)
                                .py(tokens.spacing.sm)
                                .text_size(tokens.typography.sm.size)
                                .line_height(tokens.typography.sm.line_height)
                                .text_color(tokens.colors.foreground)
                                .child(text),
                        )
                        .into_any_element()
                })
                .collect();
            v_flex()
                .w_full()
                .gap(tokens.spacing.sm)
                .child(ui::group_heading(
                    self.text("accountReferralRulesTitle", cx),
                    None,
                    cx,
                ))
                .child(card(cx).w_full().children(rows))
        });
        let contact = (!summary.contact.trim().is_empty()).then(|| {
            div()
                .text_size(tokens.typography.xs.size)
                .line_height(tokens.typography.xs.line_height)
                .text_color(extended.colors.text_tertiary)
                .child(t_with(
                    &translator,
                    "accountReferralRedeemHint",
                    &[("contact", summary.contact.trim())],
                ))
        });
        let description = summary.description.trim();
        let top = v_flex()
            .w_full()
            .flex_none()
            .gap(tokens.spacing.lg)
            .when(!summary.enabled, |this| {
                this.child(notice(
                    NoticeTone::Warning,
                    self.text("accountReferralDisabledNotice", cx),
                    cx,
                ))
            })
            .child(stats)
            .when(!summary.reward_enabled, |this| {
                this.child(notice(
                    NoticeTone::Info,
                    self.text("accountReferralRewardDisabledNotice", cx),
                    cx,
                ))
            });
        let layout = v_flex()
            .size_full()
            .min_h_0()
            .gap(tokens.spacing.lg)
            .child(top);
        if description.is_empty() {
            // 无说明正文：规则与兑付提示整体放进局部滚动区。
            return layout.child(scroll_region(
                "referral-intro-scroll",
                v_flex()
                    .w_full()
                    .gap(tokens.spacing.lg)
                    .children(rules)
                    .children(contact),
            ));
        }
        // 说明正文（云端下发 Markdown）占满剩余高度并在卡片内滚动；规则与兑付提示固定在下方。
        layout
            .child(
                card(cx)
                    .w_full()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .child(scroll_region(
                        "referral-intro-description",
                        div()
                            .w_full()
                            .flex_none()
                            .p(tokens.spacing.md)
                            .text_size(tokens.typography.sm.size)
                            .line_height(relative(DESCRIPTION_LINE_HEIGHT))
                            .text_color(tokens.colors.foreground)
                            .child(
                                TextView::markdown("referral-description", description.to_owned())
                                    .selectable(true)
                                    .w_full(),
                            ),
                    )),
            )
            .child(
                v_flex()
                    .w_full()
                    .flex_none()
                    .gap(tokens.spacing.sm)
                    .children(rules)
                    .children(contact),
            )
    }

    // ───────────────────────── 我的推荐码 ─────────────────────────

    fn render_codes(
        &self,
        tokens: &SemanticThemeTokens,
        extended: &ExtendedTokens,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let translator = self.translator(cx);
        let full = matches!(&self.codes.load, Load::Ready(codes) if codes_full(codes));
        let create_enabled = self.can_create_code(cx);
        let header = h_flex()
            .w_full()
            .items_center()
            .gap(tokens.spacing.md)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(tokens.spacing.xxs)
                    .child(
                        div()
                            .text_size(tokens.typography.sm.size)
                            .line_height(tokens.typography.sm.line_height)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(tokens.colors.foreground)
                            .child(self.text("accountReferralTabCode", cx)),
                    )
                    .when(full, |this| {
                        this.child(
                            div()
                                .text_size(tokens.typography.xs.size)
                                .line_height(tokens.typography.xs.line_height)
                                .text_color(tokens.colors.muted_foreground)
                                .child(t_with(
                                    &translator,
                                    "accountReferralCodeLimit",
                                    &[("max", &MAX_REFERRAL_CODES.to_string())],
                                )),
                        )
                    }),
            )
            .child(
                button(
                    "referral-code-create",
                    self.text("accountReferralCodeCreate", cx),
                    ButtonVariant::Primary,
                    cx,
                )
                .disabled(!create_enabled)
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.open_create_dialog(window, cx);
                })),
            );

        let body = match &self.codes.load {
            Load::Idle | Load::Loading => self.skeleton_rows(tokens, cx).into_any_element(),
            Load::Failed(message) => self
                .failed_block(
                    message.clone(),
                    "referral-codes-retry",
                    tokens,
                    |this, window, cx| this.load_codes(window, cx),
                    cx,
                )
                .into_any_element(),
            Load::Ready(codes) if codes.items.is_empty() => self
                .empty_state(self.text("accountReferralCodeEmpty", cx), tokens, extended)
                .into_any_element(),
            Load::Ready(codes) => self
                .codes_list(codes, tokens, extended, cx)
                .into_any_element(),
        };

        v_flex()
            .size_full()
            .min_h_0()
            .gap(tokens.spacing.md)
            .child(div().w_full().flex_none().child(header))
            .when(!self.enabled(), |this| {
                this.child(div().w_full().flex_none().child(notice(
                    NoticeTone::Warning,
                    self.text("accountReferralDisabledNotice", cx),
                    cx,
                )))
            })
            .child(body)
    }

    fn codes_list(
        &self,
        codes: &CloudReferralCodesResult,
        tokens: &SemanticThemeTokens,
        extended: &ExtendedTokens,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let disconnected = self.disconnected(cx);
        let rows: Vec<_> = codes
            .items
            .iter()
            .enumerate()
            .map(|(index, code)| {
                self.code_row(index, code, disconnected, tokens, extended, cx)
                    .into_any_element()
            })
            .collect();
        v_flex()
            .w_full()
            .flex_1()
            .min_h_0()
            .gap(tokens.spacing.md)
            .child(scroll_region(
                "referral-codes-scroll",
                card(cx).w_full().flex_none().children(rows),
            ))
            .child(div().w_full().flex_none().child(self.pager(
                Tab::Codes,
                self.codes.page,
                page_count(codes.total),
                tokens,
                cx,
            )))
    }

    fn code_row(
        &self,
        index: usize,
        code: &CloudReferralCode,
        disconnected: bool,
        tokens: &SemanticThemeTokens,
        extended: &ExtendedTokens,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let translator = self.translator(cx);
        let stats = t_with(
            &translator,
            "accountReferralCodeStats",
            &[
                ("count", &code.paid_order_count.to_string()),
                ("amount", &format_minor(code.reward_minor, CURRENCY)),
            ],
        );
        let copy_label = self.text("accountReferralCodeCopy", cx);
        let delete_label = self.text("delete", cx);
        let deleting = self.codes.deleting.as_deref() == Some(code.id.as_str());
        let copy_value = code.code.clone();
        let delete_id = code.id.clone();
        let copy_tip = copy_label.clone();
        let delete_tip = delete_label.clone();
        v_flex()
            .w_full()
            .when(index > 0, |this| this.child(ui::row_divider(cx)))
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(tokens.spacing.md)
                    .px(tokens.spacing.md)
                    .py(tokens.spacing.sm)
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(tokens.spacing.xxs)
                            .child(
                                div()
                                    .font_family(tokens.typography.mono.clone())
                                    .text_size(tokens.typography.xl.size)
                                    .line_height(tokens.typography.xl.line_height)
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .font_features(tabular_numbers())
                                    .text_color(tokens.colors.foreground)
                                    .child(code.code.clone()),
                            )
                            .child(caption_number(stats, cx)),
                    )
                    .child(
                        icon_button(
                            SharedString::from(format!("referral-copy-{}", code.id)),
                            copy_label,
                            Icon::new(FluxIcon::Copy)
                                .size(extended.icon.md)
                                .text_color(tokens.colors.muted_foreground),
                            ButtonVariant::Ghost,
                            cx,
                        )
                        .tooltip(move |window, cx| tooltip_view(copy_tip.clone(), window, cx))
                        .on_click(cx.listener(
                            move |this, _: &ClickEvent, window, cx| {
                                this.copy_code(&copy_value, window, cx);
                            },
                        )),
                    )
                    .child(
                        icon_button(
                            SharedString::from(format!("referral-delete-{}", code.id)),
                            delete_label,
                            Icon::new(FluxIcon::Trash2)
                                .size(extended.icon.md)
                                .text_color(tokens.colors.destructive),
                            ButtonVariant::Ghost,
                            cx,
                        )
                        .disabled(disconnected || deleting)
                        .tooltip(move |window, cx| tooltip_view(delete_tip.clone(), window, cx))
                        .on_click(cx.listener(
                            move |this, _: &ClickEvent, window, cx| {
                                this.confirm_delete(delete_id.clone(), window, cx);
                            },
                        )),
                    ),
            )
    }

    // ───────────────────────── 收益记录 ─────────────────────────

    fn render_records(
        &self,
        tokens: &SemanticThemeTokens,
        extended: &ExtendedTokens,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let searching = !self.records.term.is_empty();
        let body = match &self.records.load {
            Load::Idle | Load::Loading => self.skeleton_rows(tokens, cx).into_any_element(),
            Load::Failed(message) => self
                .failed_block(
                    message.clone(),
                    "referral-records-retry",
                    tokens,
                    |this, window, cx| this.load_records(window, cx),
                    cx,
                )
                .into_any_element(),
            Load::Ready(records) if records.items.is_empty() => self
                .empty_state(
                    self.text(
                        if searching {
                            "accountReferralSearchEmpty"
                        } else {
                            "accountReferralRecordsEmpty"
                        },
                        cx,
                    ),
                    tokens,
                    extended,
                )
                .into_any_element(),
            Load::Ready(records) => self
                .records_list(records, tokens, extended, cx)
                .into_any_element(),
        };
        v_flex()
            .size_full()
            .min_h_0()
            .gap(tokens.spacing.md)
            .child(
                div().w_full().flex_none().child(
                    Input::new(&self.search_input).control(cx).w_full().prefix(
                        Icon::new(FluxIcon::Search)
                            .size(extended.icon.md)
                            .text_color(extended.colors.text_tertiary),
                    ),
                ),
            )
            .child(body)
    }

    fn records_list(
        &self,
        records: &CloudReferralRecordsResult,
        tokens: &SemanticThemeTokens,
        extended: &ExtendedTokens,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let rows: Vec<_> = records
            .items
            .iter()
            .enumerate()
            .map(|(index, record)| {
                self.record_row(index, record, tokens, extended, cx)
                    .into_any_element()
            })
            .collect();
        v_flex()
            .w_full()
            .flex_1()
            .min_h_0()
            .gap(tokens.spacing.md)
            .child(scroll_region(
                "referral-records-scroll",
                card(cx).w_full().flex_none().children(rows),
            ))
            .child(div().w_full().flex_none().child(self.pager(
                Tab::Records,
                self.records.page,
                page_count(records.total),
                tokens,
                cx,
            )))
    }

    fn record_row(
        &self,
        index: usize,
        record: &CloudReferralRecord,
        tokens: &SemanticThemeTokens,
        extended: &ExtendedTokens,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let translator = self.translator(cx);
        let (status_key, status_color): (&str, Hsla) = match record.status.as_str() {
            "paid" => ("accountReferralStatusPaid", extended.colors.success),
            "revoked" => (
                "accountReferralStatusRevoked",
                tokens.colors.muted_foreground,
            ),
            _ => ("accountReferralStatusPending", extended.colors.warning),
        };
        let components = active_theme(cx).components();
        let xs = tokens.typography.xs;
        let meta = {
            let mut parts = Vec::new();
            if let Some(code) = record
                .referral_code
                .as_deref()
                .filter(|code| !code.is_empty())
            {
                parts.push(code.to_owned());
            }
            parts.push(format_local_date(&record.created_at));
            parts.join(" · ")
        };
        v_flex()
            .w_full()
            .when(index > 0, |this| this.child(ui::row_divider(cx)))
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(tokens.spacing.md)
                    .px(tokens.spacing.md)
                    .py(tokens.spacing.sm)
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(tokens.spacing.xxs)
                            .child(
                                div()
                                    .truncate()
                                    .text_size(tokens.typography.sm.size)
                                    .line_height(tokens.typography.sm.line_height)
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(tokens.colors.foreground)
                                    .child(record.buyer_label.clone()),
                            )
                            .child(caption_number(meta, cx)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(xs.size)
                            .line_height(xs.line_height)
                            .font_features(tabular_numbers())
                            .text_color(tokens.colors.muted_foreground)
                            .child(t_with(
                                &translator,
                                "accountReferralRecordOrderAmount",
                                &[("amount", &format_minor(record.order_amount_minor, CURRENCY))],
                            )),
                    )
                    .child(
                        v_flex()
                            .flex_none()
                            .items_end()
                            .gap(tokens.spacing.xxs)
                            .child(
                                div()
                                    .text_size(tokens.typography.md.size)
                                    .line_height(tokens.typography.md.line_height)
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .font_features(tabular_numbers())
                                    .text_color(extended.colors.accent_text)
                                    .child(format!(
                                        "+{}",
                                        format_minor(record.reward_minor, CURRENCY)
                                    )),
                            )
                            .child(
                                div()
                                    .text_size(xs.size)
                                    .line_height(xs.line_height)
                                    .font_features(tabular_numbers())
                                    .text_color(extended.colors.text_tertiary)
                                    .child(format!("{}%", record.reward_percent)),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .px(tokens.spacing.sm)
                            .py(tokens.spacing.xxs)
                            .rounded(components.badge_radius)
                            .bg(status_color.opacity(0.14))
                            .text_size(extended.caption.size)
                            .line_height(extended.caption.line_height)
                            .text_color(status_color)
                            .child(self.text(status_key, cx)),
                    ),
            )
    }

    // ───────────────────────── 分页 ─────────────────────────

    fn pager(
        &self,
        tab: Tab,
        page: u32,
        pages: u32,
        tokens: &SemanticThemeTokens,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let translator = self.translator(cx);
        let extended = active_theme(cx).extended().clone();
        let indicator = t_with(
            &translator,
            "accountReferralPageIndicator",
            &[("x", &page.to_string()), ("y", &pages.to_string())],
        );
        let go =
            move |this: &mut Self, target: u32, window: &mut Window, cx: &mut Context<Self>| {
                match tab {
                    Tab::Codes => this.go_codes_page(target, window, cx),
                    Tab::Records => this.go_records_page(target, window, cx),
                    Tab::Intro => {}
                }
            };
        h_flex()
            .w_full()
            .items_center()
            .justify_center()
            .gap(tokens.spacing.md)
            .when(pages <= 1, |this| this.hidden())
            .child(
                icon_button(
                    SharedString::from(format!("referral-prev-{tab:?}")),
                    indicator.clone(),
                    Icon::new(IconName::ChevronLeft)
                        .size(extended.icon.md)
                        .text_color(tokens.colors.muted_foreground),
                    ButtonVariant::Ghost,
                    cx,
                )
                .disabled(page <= 1)
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    go(this, page.saturating_sub(1), window, cx);
                })),
            )
            .child(
                div()
                    .text_size(tokens.typography.xs.size)
                    .line_height(tokens.typography.xs.line_height)
                    .font_features(tabular_numbers())
                    .text_color(tokens.colors.muted_foreground)
                    .child(indicator.clone()),
            )
            .child(
                icon_button(
                    SharedString::from(format!("referral-next-{tab:?}")),
                    indicator,
                    Icon::new(IconName::ChevronRight)
                        .size(extended.icon.md)
                        .text_color(tokens.colors.muted_foreground),
                    ButtonVariant::Ghost,
                    cx,
                )
                .disabled(page >= pages)
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    go(this, page + 1, window, cx);
                })),
            )
    }
}

/// 统计卡：二级文字标题 + 大号等宽数字。
fn stat_card(
    label: SharedString,
    value: String,
    tokens: &SemanticThemeTokens,
    cx: &gpui::App,
) -> impl IntoElement {
    card(cx)
        .flex_1()
        .min_w_0()
        .p(tokens.spacing.md)
        .flex()
        .flex_col()
        .gap(tokens.spacing.xs)
        .child(
            div()
                .text_size(tokens.typography.xs.size)
                .line_height(tokens.typography.xs.line_height)
                .text_color(tokens.colors.muted_foreground)
                .child(label),
        )
        .child(
            div()
                .text_size(tokens.typography.xl.size)
                .line_height(tokens.typography.xl.line_height)
                .font_weight(FontWeight::SEMIBOLD)
                .font_features(tabular_numbers())
                .text_color(tokens.colors.foreground)
                .child(value),
        )
}
