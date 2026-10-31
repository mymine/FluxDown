//! 分享面板与接收确认窗口共用的窗口骨架：标题栏、底栏、居中提示与单行省略文本。

use fluxdown_ui_components::{FluxIcon, tabular_numbers, toolbar_action_button};
use fluxdown_ui_i18n::Translator;
use fluxdown_ui_theme::active_theme;
use gpui::{
    App, Div, FontWeight, Hsla, ParentElement as _, SharedString, Styled as _, div,
    prelude::FluentBuilder as _,
};
use gpui_component::{Icon, h_flex, v_flex};

use crate::model::CountLabel;

/// 标题栏图标块边长（基准 px，随字号缩放）。
const ICON_TILE: f32 = 36.;
/// 居中提示文字的最大宽度（基准 px，随字号缩放）。
pub(crate) const CENTER_MAX_WIDTH: f32 = 380.;

pub(crate) fn text(translator: &Translator, key: &str) -> SharedString {
    SharedString::from(translator.text(key).to_owned())
}

/// 带 `{count}` 占位的计数文案。
pub(crate) fn count_text(translator: &Translator, label: CountLabel) -> SharedString {
    SharedString::from(translator.text_with(label.key, &[("count", &label.count.to_string())]))
}

/// 单行省略的文本容器（需要放进可收缩的父级里才会真正截断）。
pub(crate) fn ellipsized(content: impl Into<SharedString>) -> Div {
    div()
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .child(content.into())
}

/// 标题栏：图标块 + 标题 / 副标题 + 状态芯片 + 关闭。
pub(crate) fn header(
    icon: FluxIcon,
    title: SharedString,
    subtitle: Option<SharedString>,
    chip: Option<(SharedString, Hsla)>,
    close_label: SharedString,
    cx: &App,
) -> Div {
    let theme = active_theme(cx);
    let tokens = theme.tokens();
    let extended = theme.extended();
    let tile = theme.text_extent(ICON_TILE);
    h_flex()
        .w_full()
        .flex_none()
        .items_center()
        .gap(tokens.spacing.md)
        .px(tokens.spacing.lg)
        .py(tokens.spacing.md)
        .border_b(extended.stroke.thin)
        .border_color(extended.colors.hairline)
        .child(
            div()
                .size(tile)
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .rounded(tokens.radius.md)
                .bg(tokens.colors.muted)
                .child(
                    Icon::new(icon)
                        .size(extended.icon.lg)
                        .text_color(tokens.colors.muted_foreground),
                ),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap(tokens.spacing.xxs)
                .child(
                    ellipsized(title)
                        .text_size(tokens.typography.md.size)
                        .line_height(tokens.typography.md.line_height)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(tokens.colors.foreground),
                )
                .when_some(subtitle, |this, subtitle| {
                    this.child(
                        ellipsized(subtitle)
                            .text_size(tokens.typography.xs.size)
                            .line_height(tokens.typography.xs.line_height)
                            .font_features(tabular_numbers())
                            .text_color(tokens.colors.muted_foreground),
                    )
                }),
        )
        .when_some(chip, |this, (label, color)| {
            this.child(
                div()
                    .flex_none()
                    .px(tokens.spacing.sm)
                    .py(tokens.spacing.xxs)
                    .rounded(tokens.radius.full)
                    .bg(color.opacity(0.14))
                    .text_size(tokens.typography.xs.size)
                    .line_height(tokens.typography.xs.line_height)
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(color)
                    .child(label),
            )
        })
        .child(
            toolbar_action_button(
                "share-window-close",
                close_label,
                Icon::new(FluxIcon::X).size(extended.icon.md),
                false,
                false,
                cx,
            )
            .on_click(|_, window, _| window.remove_window()),
        )
}

/// 底栏：上方 hairline 分隔，左右两组操作。
pub(crate) fn footer_frame(cx: &App) -> Div {
    let theme = active_theme(cx);
    let tokens = theme.tokens();
    h_flex()
        .w_full()
        .flex_none()
        .items_center()
        .justify_between()
        .gap(tokens.spacing.md)
        .px(tokens.spacing.lg)
        .py(tokens.spacing.md)
        .border_t(theme.extended().stroke.thin)
        .border_color(theme.extended().colors.hairline)
}

/// 窗口内居中的图标 + 一段说明（失败 / 已结束 / 已开始）。
pub(crate) fn notice(icon: FluxIcon, color: Hsla, message: SharedString, cx: &App) -> Div {
    let theme = active_theme(cx);
    let tokens = theme.tokens();
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .gap(tokens.spacing.md)
        .p(tokens.spacing.xl)
        .child(
            Icon::new(icon)
                .size(theme.extended().icon.lg)
                .text_color(color),
        )
        .child(
            div()
                .max_w(theme.text_extent(CENTER_MAX_WIDTH))
                .text_center()
                .text_size(tokens.typography.sm.size)
                .line_height(tokens.typography.sm.line_height)
                .text_color(tokens.colors.foreground)
                .child(message),
        )
}
