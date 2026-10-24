//! 任务表状态列的「待确认」角标：任务正等用户决定「文件已存在」怎么处理。点击重新打开
//! （或置前）聚合的冲突窗口；角标代替该行的状态文案，其余行完全不受影响。

use fluxdown_ui_components::{FluxIcon, tooltip_view};
use fluxdown_ui_theme::active_theme;
use gpui::{
    AnyElement, App, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, WeakEntity, div,
};
use gpui_component::{Icon, h_flex};

use crate::{pages::downloads::DownloadView, strings::DownloadStrings};

/// 角标：与状态列主文案同字号档（xs）。
pub(crate) fn conflict_badge(
    task_id: &str,
    strings: &DownloadStrings,
    host: Option<&WeakEntity<DownloadView>>,
    cx: &App,
) -> AnyElement {
    let theme = active_theme(cx);
    let tokens = theme.tokens();
    let warning = theme.extended().colors.warning;
    let text = &tokens.typography.xs;
    let tooltip = strings.conflict_pending_tooltip.clone();
    let host = host.cloned();
    h_flex()
        .id(SharedString::from(format!("file-conflict-badge-{task_id}")))
        .flex_none()
        .items_center()
        .gap(tokens.spacing.xxs)
        .px(tokens.spacing.xs)
        .rounded(tokens.radius.sm)
        .bg(warning.opacity(0.14))
        .text_color(warning)
        .text_size(text.size)
        .line_height(text.line_height)
        .cursor_pointer()
        .hover(move |style| style.bg(warning.opacity(0.24)))
        .tooltip(move |window, cx| tooltip_view(tooltip.clone(), window, cx))
        // 不让点击落到行上（选中 / 双击打开详情）。
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            let Some(host) = host.as_ref() else {
                return;
            };
            let Ok(()) = host.update(cx, |view, cx| view.open_file_conflicts(window, cx)) else {
                // 视图已释放，结束这次回调而不再更新状态。
                return;
            };
        })
        .child(Icon::new(FluxIcon::CircleAlert).size(theme.extended().icon.sm))
        .child(
            div()
                .min_w_0()
                .truncate()
                .child(strings.conflict_pending.clone()),
        )
        .into_any_element()
}
