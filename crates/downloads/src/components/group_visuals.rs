//! 任务组的公共视觉件：文件夹图标（带成员数徽标）与计数行。任务表组行与停靠面板的
//! 组概览共用。

use fluxdown_ui_components::{FluxIcon, tabular_numbers};
use fluxdown_ui_theme::active_theme;
use gpui::{
    AnyElement, App, Div, FontWeight, IntoElement, ParentElement, Pixels, SharedString, Styled,
    div, prelude::FluentBuilder as _, px,
};
use gpui_component::{Icon, h_flex};

use crate::strings::GroupCountTone;

/// 计数段之间的分隔。
const SEPARATOR: &str = " · ";

/// 组文件夹图标：折叠为合上的文件夹、展开为打开的文件夹（品牌色），右下角徽标为成员数。
/// 外框比图标本身各边多出徽标溢出的空间，行内对齐时按 `size` 计算即可。
pub(crate) fn group_folder_icon(
    expanded: bool,
    count: usize,
    size: Pixels,
    cx: &App,
) -> AnyElement {
    let theme = active_theme(cx);
    let tokens = theme.tokens();
    let colors = &tokens.colors;
    let badge_text = if count > 999 {
        SharedString::from("999+")
    } else {
        SharedString::from(count.to_string())
    };
    let badge_height = (size * 0.55).max(px(12.));
    div()
        .relative()
        .flex_none()
        .size(size)
        .child(
            Icon::new(if expanded {
                FluxIcon::FolderOpen
            } else {
                FluxIcon::Folder
            })
            .size(size)
            .text_color(colors.primary),
        )
        .child(
            h_flex()
                .absolute()
                .right(-(badge_height * 0.35))
                .bottom(-(badge_height * 0.25))
                .h(badge_height)
                .min_w(badge_height)
                .px(px(3.))
                .justify_center()
                .rounded_full()
                .border_1()
                .border_color(colors.surface)
                .bg(colors.primary)
                .text_color(colors.primary_foreground)
                .text_size(badge_height * 0.7)
                .line_height(badge_height)
                .font_weight(FontWeight::SEMIBOLD)
                .font_features(tabular_numbers())
                .child(badge_text),
        )
        .into_any_element()
}

/// 计数行：各段按语气着色、以 ` · ` 连接，单行截断。
pub(crate) fn group_counts_line(segments: Vec<(GroupCountTone, SharedString)>, cx: &App) -> Div {
    let theme = active_theme(cx);
    let extended = theme.extended();
    let plain = extended.colors.text_tertiary;
    let mut line = h_flex()
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .font_features(tabular_numbers())
        .text_color(plain);
    for (ix, (tone, text)) in segments.into_iter().enumerate() {
        if ix > 0 {
            line = line.child(div().flex_none().child(SEPARATOR));
        }
        let color = match tone {
            GroupCountTone::Plain => plain,
            GroupCountTone::Active => extended.colors.status_downloading,
            GroupCountTone::Failed => extended.colors.status_failed,
        };
        line = line.child(
            div()
                .flex_none()
                .text_color(color)
                // 失败段加粗一档，突出需要处理的成员。
                .when(tone == GroupCountTone::Failed, |this| {
                    this.font_weight(FontWeight::MEDIUM)
                })
                .child(text),
        );
    }
    line
}
