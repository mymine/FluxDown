//! 「我的分享」表的列宽、行高与表头。

use fluxdown_ui_theme::active_theme;
use gpui::{
    AnyElement, App, Div, FontWeight, ParentElement as _, Pixels, SharedString, Styled as _, div,
    prelude::FluentBuilder as _,
};
use gpui_component::h_flex;

use super::ShareView;
use crate::manage_ui::meta_text;

/// 列宽（默认字号下的 px，随字体缩放）。
pub(super) const COL_CLAIMS: f32 = 112.;
pub(super) const COL_STATUS: f32 = 92.;

/// 列宽集合。
pub(super) struct Columns {
    pub(super) claims: Pixels,
    pub(super) status: Pixels,
    /// 行内按钮：复制 / 二维码 / 停止·删除 / 更多，四个图标按钮 + 间距。
    pub(super) share_actions: Pixels,
    /// 行首复选框列。
    pub(super) select: Pixels,
}

pub(super) fn columns(cx: &App) -> Columns {
    let theme = active_theme(cx);
    let spacing = theme.tokens().spacing;
    Columns {
        claims: theme.text_extent(COL_CLAIMS),
        status: theme.text_extent(COL_STATUS),
        share_actions: theme.density().control * 4. + spacing.xxs * 3.,
        select: theme.density().control,
    }
}

/// 列表行高：两行文字（标题 + 元信息）加上下内边距。
pub(super) fn row_height(cx: &App) -> Pixels {
    let theme = active_theme(cx);
    let tokens = theme.tokens();
    (tokens.typography.sm.line_height + tokens.typography.xs.line_height + tokens.spacing.xxs)
        .max(theme.density().task_row)
        + tokens.spacing.sm
}

/// 文本非空则原样返回，否则占位。
pub(super) fn or_placeholder(value: &str) -> String {
    if value.trim().is_empty() {
        crate::model::diagnostics::PLACEHOLDER.to_owned()
    } else {
        value.to_owned()
    }
}

impl ShareView {
    /// 表头行：与数据行共用同一组列宽，左右内边距一致。
    pub(super) fn header_row(
        &self,
        cells: Vec<(SharedString, Option<Pixels>)>,
        trailing: Pixels,
        leading: Option<AnyElement>,
        cx: &App,
    ) -> Div {
        let theme = active_theme(cx);
        let tokens = theme.tokens();
        let mut row = h_flex()
            .w_full()
            .flex_none()
            .items_center()
            .gap(tokens.spacing.md)
            .px(tokens.spacing.sm)
            .pb(tokens.spacing.xs)
            .border_b_1()
            .border_color(theme.extended().colors.hairline);
        if let Some(leading) = leading {
            row = row.child(leading);
        }
        for (label, width) in cells {
            row = row.child(
                meta_text(cx)
                    .truncate()
                    .font_weight(FontWeight::MEDIUM)
                    .map(|cell| match width {
                        Some(width) => cell.w(width).flex_none(),
                        None => cell.flex_1().min_w_0(),
                    })
                    .child(label),
            );
        }
        row.child(div().w(trailing).flex_none())
    }
}
