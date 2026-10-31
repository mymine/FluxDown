//! 接收确认窗口：单例辅助窗口。取件码 / 链接 / 深链 / 剪贴板打开的分享都进同一个窗口，
//! 已开时把新的来源交给现有视图（`retarget`）。

use fluxdown_ui_share::{ReceiveSource, ShareReceiveView};
use fluxdown_ui_shell::{AuxiliaryWindowView, auxiliary_window_options};
use gpui::{App, AppContext as _, Bounds, WindowBounds, px, size};
use gpui_component::Root;

use crate::{
    app::Desktop,
    windows::{WindowKey, WindowRegistry, bring_to_front},
};

const RECEIVE_TITLE_KEY: &str = "shareReceiveTitle";
const RECEIVE_WINDOW_SIZE: gpui::Size<gpui::Pixels> = size(px(520.), px(600.));
const RECEIVE_WINDOW_MIN_SIZE: gpui::Size<gpui::Pixels> = size(px(440.), px(420.));

/// 打开或聚焦接收确认窗口并交给它新的来源。
pub fn open(cx: &mut App, source: ReceiveSource) {
    let desktop = Desktop::global(cx);
    let translator = desktop.translator.clone();
    let hub = desktop.share_hub.clone();
    let display_id = WindowRegistry::main_display_id(cx);
    let mut options =
        auxiliary_window_options(translator.read(cx).text(RECEIVE_TITLE_KEY).to_owned());
    options.display_id = display_id;
    options.window_min_size = Some(RECEIVE_WINDOW_MIN_SIZE);
    options.window_bounds = Some(WindowBounds::Windowed(Bounds::centered(
        display_id,
        RECEIVE_WINDOW_SIZE,
        cx,
    )));

    let opened_source = source.clone();
    let opened =
        WindowRegistry::open_or_focus(cx, WindowKey::ShareReceive, options, move |window, cx| {
            let view = cx.new(|cx| ShareReceiveView::new(hub, opened_source, window, cx));
            Desktop::global_mut(cx).share_receive = Some(view.downgrade());
            let window_view = cx
                .new(|cx| AuxiliaryWindowView::new(translator, RECEIVE_TITLE_KEY, view.into(), cx));
            cx.new(|cx| Root::new(window_view, window, cx))
        });
    let Some(handle) = WindowRegistry::handle(cx, &WindowKey::ShareReceive) else {
        return;
    };
    // 窗口已开：把新来源交给现有视图（新建窗口的视图已经带着来源创建）。
    if opened.is_none()
        && let Some(view) = Desktop::global(cx)
            .share_receive
            .as_ref()
            .and_then(gpui::WeakEntity::upgrade)
        && let Err(error) = handle.update(cx, |_, window, cx| {
            view.update(cx, |view, cx| view.retarget(source, window, cx));
        })
    {
        log::debug!("view or window released before lifecycle update: {error:#}");
    }
    if let Err(error) = handle.update(cx, |_, window, cx| bring_to_front(window, cx)) {
        log::debug!("view or window released before lifecycle update: {error:#}");
    }
}
