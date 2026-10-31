//! 分享页与传输诊断视图共用的小件：文字样式、徽标、键值行、空状态、区块标题、截断文本。
//! 颜色 / 字号 / 间距一律取主题 token；用户或设备给出的文本一律单行省略并可悬停看全文。

use fluxdown_ui_components::{FluxIcon, tabular_numbers, tooltip_view};
use fluxdown_ui_i18n::Translator;
use fluxdown_ui_theme::active_theme;
use gpui::{
    AnyElement, AnyView, App, Div, ElementId, FontWeight, Hsla, InteractiveElement as _,
    IntoElement, ParentElement as _, SharedString, Stateful, StatefulInteractiveElement as _,
    Styled as _, Window, div, prelude::FluentBuilder as _,
};
use gpui_component::{Icon, h_flex, v_flex};

/// 键值行标签列宽（默认字号下的 px，随字体缩放）。
const KV_LABEL_WIDTH: f32 = 128.;

/// 查表为 `SharedString`。
pub(crate) fn text(translator: &Translator, key: &str) -> SharedString {
    SharedString::from(translator.text(key).to_owned())
}

/// 带 `{name}` 占位的查表。
pub(crate) fn text_with(
    translator: &Translator,
    key: &str,
    arguments: &[(&str, &str)],
) -> SharedString {
    SharedString::from(translator.text_with(key, arguments))
}

/// 元信息文字：xs + 三级文字色。
pub(crate) fn meta_text(cx: &App) -> Div {
    let theme = active_theme(cx);
    let xs = theme.tokens().typography.xs;
    div()
        .text_size(xs.size)
        .line_height(xs.line_height)
        .text_color(theme.extended().colors.text_tertiary)
}

/// 正文文字：sm + 前景色。
pub(crate) fn body_text(cx: &App) -> Div {
    let tokens = active_theme(cx).tokens();
    div()
        .text_size(tokens.typography.sm.size)
        .line_height(tokens.typography.sm.line_height)
        .text_color(tokens.colors.foreground)
}

/// 等宽小字（日志 / JSON / 指标键）。
pub(crate) fn mono_text(cx: &App) -> Div {
    let tokens = active_theme(cx).tokens();
    div()
        .font_family(tokens.typography.mono.clone())
        .text_size(tokens.typography.xs.size)
        .line_height(tokens.typography.xs.line_height)
        .text_color(tokens.colors.foreground)
}

/// 单行截断文本：超出省略，悬停显示全文。`id` 必须在同一视图内唯一。
pub(crate) fn clipped(id: impl Into<ElementId>, value: impl Into<SharedString>) -> Stateful<Div> {
    let value: SharedString = value.into();
    let full = value.clone();
    div()
        .id(id)
        .min_w_0()
        .truncate()
        .tooltip(move |window, cx| tooltip_view(full.clone(), window, cx))
        .child(value)
}

/// 中性小徽标；`tone` 只用于需要提示的状态。文字被截断时悬停显示全文。
pub(crate) fn chip(
    id: impl Into<ElementId>,
    value: impl Into<SharedString>,
    tone: Option<Hsla>,
    cx: &App,
) -> Stateful<Div> {
    let theme = active_theme(cx);
    let tokens = theme.tokens();
    let caption = theme.extended().caption;
    let value: SharedString = value.into();
    let full = value.clone();
    div()
        .id(id)
        .flex_none()
        .max_w_full()
        .truncate()
        .px(tokens.spacing.xs + tokens.spacing.xxs)
        .py(gpui::px(1.))
        .rounded(tokens.radius.sm)
        .bg(tokens.colors.muted)
        .text_size(caption.size)
        .line_height(caption.line_height)
        .font_weight(FontWeight::MEDIUM)
        .text_color(tone.unwrap_or(tokens.colors.muted_foreground))
        .tooltip(move |window, cx| tooltip_view(full.clone(), window, cx))
        .child(value)
}

/// 区块标题行：左侧标题（+ 可选计数），右侧放操作。
pub(crate) fn section_header(
    title: SharedString,
    count: Option<SharedString>,
    trailing: Option<AnyElement>,
    cx: &App,
) -> Div {
    let theme = active_theme(cx);
    let tokens = theme.tokens();
    h_flex()
        .w_full()
        .items_center()
        .gap(tokens.spacing.sm)
        .min_h(theme.density().control)
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_size(tokens.typography.sm.size)
                .line_height(tokens.typography.sm.line_height)
                .font_weight(FontWeight::MEDIUM)
                .text_color(tokens.colors.foreground)
                .child(title),
        )
        .when_some(count, |row, count| {
            row.child(
                div()
                    .flex_none()
                    .font_features(tabular_numbers())
                    .child(meta_text(cx).child(count)),
            )
        })
        .child(div().flex_1())
        .when_some(trailing, |row, trailing| row.child(trailing))
}

/// 键值行：固定宽度标签列 + 可伸缩值列（值列 `min_w_0` 才能真正截断）。
pub(crate) fn kv_row(label: SharedString, value: impl IntoElement, cx: &App) -> Div {
    let theme = active_theme(cx);
    let tokens = theme.tokens();
    h_flex()
        .w_full()
        .items_start()
        .gap(tokens.spacing.md)
        .child(
            div()
                .w(theme.text_extent(KV_LABEL_WIDTH))
                .flex_none()
                .truncate()
                .text_size(tokens.typography.sm.size)
                .line_height(tokens.typography.sm.line_height)
                .text_color(tokens.colors.muted_foreground)
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(tokens.typography.sm.size)
                .line_height(tokens.typography.sm.line_height)
                .text_color(tokens.colors.foreground)
                .child(value),
        )
}

/// 带标签的小指标（`RTT 24 ms`）：标签三级色，数值前景色等宽数字。
pub(crate) fn fact(label: SharedString, value: impl Into<SharedString>, cx: &App) -> Div {
    let theme = active_theme(cx);
    let tokens = theme.tokens();
    h_flex()
        .flex_none()
        .items_baseline()
        .gap(tokens.spacing.xs)
        .child(meta_text(cx).child(label))
        .child(
            div()
                .font_features(tabular_numbers())
                .text_size(tokens.typography.xs.size)
                .line_height(tokens.typography.xs.line_height)
                .text_color(tokens.colors.foreground)
                .child(value.into()),
        )
}

/// 空状态：32px 图标 + 标题 + 可选说明，居中。
pub(crate) fn empty_state(
    icon: FluxIcon,
    icon_color: Option<Hsla>,
    title: SharedString,
    description: Option<SharedString>,
    cx: &App,
) -> Div {
    let theme = active_theme(cx);
    let tokens = theme.tokens();
    v_flex()
        .w_full()
        .items_center()
        .justify_center()
        .gap(tokens.spacing.sm)
        .px(tokens.spacing.lg)
        .py(tokens.spacing.xl)
        .child(
            Icon::new(icon)
                .size(theme.extended().icon.lg * 1.5)
                .text_color(icon_color.unwrap_or(theme.extended().colors.text_tertiary)),
        )
        .child(
            div()
                .text_center()
                .text_size(tokens.typography.sm.size)
                .line_height(tokens.typography.sm.line_height)
                .font_weight(FontWeight::MEDIUM)
                .text_color(tokens.colors.foreground)
                .child(title),
        )
        .when_some(description, |this, description| {
            this.child(
                div()
                    .max_w(theme.text_extent(480.))
                    .text_center()
                    .text_size(tokens.typography.xs.size)
                    .line_height(tokens.typography.xs.line_height)
                    .text_color(tokens.colors.muted_foreground)
                    .child(description),
            )
        })
}

/// 悬浮提示构造器（供 `StatefulInteractiveElement::tooltip` / 图标按钮使用）。
pub(crate) fn tooltip_builder(
    label: SharedString,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    move |window, cx| tooltip_view(label.clone(), window, cx)
}

/// 一条细进度条：左侧标签、右侧数值文字，下方 4px 轨道。`ratio = None` 不画轨道（不限）；
/// `exhausted` 时数值与轨道变危险色。样式与设置页 `cloud_push::meter_row` 一致（share 不依赖 settings）。
pub(crate) fn meter_row(
    id: impl Into<ElementId>,
    label: SharedString,
    value: SharedString,
    ratio: Option<f32>,
    exhausted: bool,
    cx: &App,
) -> Div {
    let tokens = active_theme(cx).tokens().clone();
    let (fill, value_color) = if exhausted {
        (tokens.colors.destructive, tokens.colors.destructive)
    } else {
        (tokens.colors.primary, tokens.colors.foreground)
    };
    v_flex()
        .w_full()
        .gap(tokens.spacing.xs)
        .child(
            h_flex()
                .w_full()
                .items_baseline()
                .justify_between()
                .gap(tokens.spacing.sm)
                .child(body_text(cx).flex_shrink_0().child(label))
                .child(
                    meta_text(cx)
                        .min_w_0()
                        .truncate()
                        .font_features(tabular_numbers())
                        .text_color(value_color)
                        .when(exhausted, |this| this.font_weight(FontWeight::MEDIUM))
                        .child(value),
                ),
        )
        .children(ratio.map(|ratio| {
            div()
                .id(id)
                .w_full()
                .h(gpui::px(4.))
                .rounded_full()
                .bg(tokens.colors.muted)
                .overflow_hidden()
                .child(
                    div()
                        .h_full()
                        .w(gpui::relative(ratio.clamp(0., 1.)))
                        .rounded_full()
                        .bg(fill),
                )
        }))
}
