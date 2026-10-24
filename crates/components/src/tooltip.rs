//! 文本悬浮提示：gpui-component 的 `Tooltip::new` 不限宽，长文本（插件描述、错误原因、完整路径）
//! 会撑成一整行横跨窗口。全应用的文本提示统一经这里构造，超过上限时按宽度折行。

use fluxdown_ui_theme::active_theme;
use gpui::{AnyView, App, ParentElement as _, SharedString, Styled as _, Window, div};
use gpui_component::tooltip::Tooltip;

/// 提示正文的最大宽度（默认字号下的 px，随界面缩放与字号同比放大）。
const TOOLTIP_MAX_WIDTH: f32 = 320.;

/// 构造文本悬浮提示视图，用于 `.tooltip(move |window, cx| tooltip_view(..))`。
pub fn tooltip_view(text: impl Into<SharedString>, window: &mut Window, cx: &mut App) -> AnyView {
    let text = text.into();
    Tooltip::element(move |_, cx| {
        div()
            .max_w(active_theme(cx).text_extent(TOOLTIP_MAX_WIDTH))
            .child(text.clone())
    })
    .build(window, cx)
}
