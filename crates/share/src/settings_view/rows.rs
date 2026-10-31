//! 行 / 分组（复刻设置页外观）。

use fluxdown_ui_components::{FluxIcon, card, tooltip_view};
use fluxdown_ui_i18n::Translator;
use fluxdown_ui_theme::active_theme;
use gpui::{
    AnyElement, App, Div, ElementId, FontWeight, InteractiveElement as _, IntoElement as _,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window, canvas,
    div, prelude::FluentBuilder as _, px,
};
use gpui_component::{Icon, v_flex};

use super::ROW_MIN_PADDING;

pub(super) fn body_text(cx: &App) -> Div {
    let tokens = active_theme(cx).tokens().clone();
    div()
        .text_size(tokens.typography.sm.size)
        .line_height(tokens.typography.sm.line_height)
        .text_color(tokens.colors.foreground)
}

pub(super) fn meta_text(cx: &App) -> Div {
    let tokens = active_theme(cx).tokens().clone();
    div()
        .text_size(tokens.typography.xs.size)
        .line_height(tokens.typography.xs.line_height)
        .text_color(tokens.colors.muted_foreground)
}

pub(super) fn caption_heading(cx: &App) -> Div {
    let theme = active_theme(cx);
    let extended = theme.extended();
    div()
        .text_size(extended.caption.size)
        .line_height(extended.caption.line_height)
        .font_weight(FontWeight::MEDIUM)
        .text_color(extended.colors.text_tertiary)
}

/// 描述按当前文字样式排成单行时是否超出 `available`。
pub(super) fn description_overflows(
    description: &SharedString,
    available: gpui::Pixels,
    window: &Window,
) -> bool {
    let style = window.text_style();
    let font_size = style.font_size.to_pixels(window.rem_size());
    let run = style.to_run(description.len());
    let line = window
        .text_system()
        .shape_line(description.clone(), font_size, &[run], None);
    line.width > available
}

/// 一行设置：标题 + 单行描述（超出时截断，完整内容收进标题旁信息图标的提示）在左，控件在右；
/// `vertical` 时控件占满标题下方整行（多行输入）。`error` 显示在描述下方。
pub(super) struct RowSpec<'a> {
    key: &'a str,
    title: SharedString,
    description: SharedString,
    error: Option<SharedString>,
    control: AnyElement,
    vertical: bool,
}

impl<'a> RowSpec<'a> {
    pub(super) fn new(
        translator: &Translator,
        key: &'a str,
        title_key: &str,
        desc_key: &str,
        control: AnyElement,
    ) -> Self {
        Self {
            key,
            title: text(translator, title_key),
            description: text(translator, desc_key),
            error: None,
            control,
            vertical: false,
        }
    }

    pub(super) fn error(mut self, error: Option<SharedString>) -> Self {
        self.error = error;
        self
    }

    pub(super) fn vertical(mut self) -> Self {
        self.vertical = true;
        self
    }
}

/// 翻译文案转 `SharedString`。
pub(super) fn text(translator: &Translator, key: &str) -> SharedString {
    SharedString::from(translator.text(key).to_owned())
}

pub(super) fn setting_row(spec: RowSpec, window: &mut Window, cx: &mut App) -> AnyElement {
    let RowSpec {
        key,
        title,
        description,
        error,
        control,
        vertical,
    } = spec;
    let theme = active_theme(cx);
    let tokens = theme.tokens().clone();
    let extended = theme.extended().clone();
    let row_min_height = theme.density().control + px(ROW_MIN_PADDING);
    let title_min_width = theme.text_extent(160.);
    let overflow = window.use_keyed_state(
        ElementId::from(SharedString::from(format!("share-set-{key}-overflow"))),
        cx,
        |_, _| false,
    );
    let overflows = *overflow.read(cx);

    let mut title_row = div()
        .flex()
        .items_center()
        .gap(tokens.spacing.xs)
        .child(body_text(cx).truncate().child(title));
    if overflows {
        let text = description.clone();
        title_row = title_row.child(
            div()
                .id(ElementId::from(SharedString::from(format!(
                    "share-set-{key}-info"
                ))))
                .flex()
                .flex_none()
                .items_center()
                .tooltip(move |window, cx| tooltip_view(text.clone(), window, cx))
                .child(
                    Icon::new(FluxIcon::Info)
                        .size(extended.icon.sm)
                        .text_color(extended.colors.text_tertiary),
                ),
        );
    }
    let label = v_flex()
        .gap(tokens.spacing.xxs)
        .min_w_0()
        .child(title_row)
        .child(
            meta_text(cx)
                .relative()
                .truncate()
                .child(description.clone())
                .child(
                    canvas(
                        move |bounds, window, cx| {
                            let now =
                                description_overflows(&description, bounds.size.width, window);
                            if *overflow.read(cx) != now {
                                overflow.update(cx, |value, cx| {
                                    *value = now;
                                    cx.notify();
                                });
                            }
                        },
                        |_, (), _, _| {},
                    )
                    .absolute()
                    .size_full(),
                ),
        )
        .when_some(error, |this, error| {
            this.child(
                meta_text(cx)
                    .text_color(tokens.colors.destructive)
                    .child(error),
            )
        });

    let row = div()
        .id(ElementId::from(SharedString::from(format!(
            "share-set-{key}"
        ))))
        .w_full()
        .px(tokens.spacing.lg);
    if vertical {
        row.py(tokens.spacing.md).child(
            v_flex()
                .w_full()
                .gap(tokens.spacing.sm)
                .child(label)
                .child(div().w_full().child(control)),
        )
    } else {
        row.py(tokens.spacing.sm)
            .flex()
            .items_center()
            .min_h(row_min_height)
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(tokens.spacing.lg)
                    // 标题列保底 160px；控件列可收缩，避免宽控件把描述挤成一列。
                    .child(div().flex_1().min_w(title_min_width).child(label))
                    .child(div().min_w_0().child(control)),
            )
    }
    .into_any_element()
}

/// 无标题的行卡片：行之间用发丝线分隔。
pub(super) fn setting_card(rows: Vec<AnyElement>, cx: &App) -> AnyElement {
    let theme = active_theme(cx);
    let tokens = theme.tokens().clone();
    let hairline = theme.extended().colors.hairline;
    let mut body = card(cx).flex().flex_col().w_full().overflow_hidden();
    for (index, row) in rows.into_iter().enumerate() {
        if index > 0 {
            body = body.child(
                div()
                    .w_full()
                    .h(px(1.))
                    .pl(tokens.spacing.lg)
                    .child(div().size_full().bg(hairline)),
            );
        }
        body = body.child(row);
    }
    body.into_any_element()
}

/// 一组设置：组标题（+ 可选说明）+ 一张行间发丝线分隔的卡片。
pub(super) fn setting_group(
    title: SharedString,
    subtitle: Option<SharedString>,
    rows: Vec<AnyElement>,
    cx: &App,
) -> AnyElement {
    let tokens = active_theme(cx).tokens().clone();
    let body = setting_card(rows, cx);
    v_flex()
        .w_full()
        .child(
            v_flex()
                .px(tokens.spacing.xs)
                .pb(tokens.spacing.sm)
                .gap(tokens.spacing.xxs)
                .child(caption_heading(cx).child(title))
                .children(subtitle.map(|subtitle| meta_text(cx).child(subtitle))),
        )
        .child(body)
        .into_any_element()
}
