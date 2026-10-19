//! 账户页共用的版式小积木：行分隔线、分组标题（卡片 / 对话框标题直接用 `fluxdown_ui_components::{card, dialog_title}`）。
//!
//! 规则与全应用一致：分隔线用 hairline；
//! 分组标题 sm MEDIUM + xs 二级说明。

use fluxdown_ui_components::FluxIcon;
use fluxdown_ui_theme::active_theme;
use gpui::{App, Div, FontWeight, ParentElement, SharedString, Styled, div};
use gpui_component::{Icon, h_flex};

/// 卡片内部的行间分隔线（hairline，`stroke.thin` 粗）。
pub(crate) fn row_divider(cx: &App) -> Div {
    let extended = active_theme(cx).extended();
    div()
        .w_full()
        .h(extended.stroke.thin)
        .bg(extended.colors.hairline)
}

/// 卡片组上方的分组标题：标题 sm MEDIUM + 可选 xs 二级说明。
pub(crate) fn group_heading(title: SharedString, desc: Option<SharedString>, cx: &App) -> Div {
    let tokens = active_theme(cx).tokens();
    let typography = &tokens.typography;
    div()
        .flex()
        .flex_col()
        .gap(tokens.spacing.xxs)
        .child(
            div()
                .text_size(typography.sm.size)
                .line_height(typography.sm.line_height)
                .font_weight(FontWeight::MEDIUM)
                .text_color(tokens.colors.foreground)
                .child(title),
        )
        .children(desc.map(|desc| {
            div()
                .text_size(typography.xs.size)
                .line_height(typography.xs.line_height)
                .text_color(tokens.colors.muted_foreground)
                .child(desc)
        }))
}

/// 提示条语气。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NoticeTone {
    Info,
    Warning,
    Danger,
}

/// 内联提示条：语气色 8% 底 + 35% 描边 + 图标 + 正文（sm）。购买对话框与推介页共用。
pub(crate) fn notice(tone: NoticeTone, text: SharedString, cx: &App) -> Div {
    let theme = active_theme(cx);
    let tokens = theme.tokens();
    let extended = theme.extended();
    let (color, icon) = match tone {
        NoticeTone::Info => (extended.colors.info, FluxIcon::Info),
        NoticeTone::Warning => (extended.colors.warning, FluxIcon::CircleAlert),
        NoticeTone::Danger => (tokens.colors.destructive, FluxIcon::CircleAlert),
    };
    let line = tokens.typography.sm.line_height;
    h_flex()
        .w_full()
        .items_start()
        .gap(tokens.spacing.sm)
        .px(tokens.spacing.md)
        .py(tokens.spacing.sm)
        .rounded(tokens.radius.md)
        .border(extended.stroke.thin)
        .border_color(color.opacity(0.35))
        .bg(color.opacity(0.08))
        .child(
            div()
                .flex_none()
                .h(line)
                .flex()
                .items_center()
                .child(Icon::new(icon).size(extended.icon.md).text_color(color)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(tokens.typography.sm.size)
                .line_height(line)
                .text_color(tokens.colors.foreground)
                .child(text),
        )
}
