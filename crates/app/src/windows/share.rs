//! 分享面板窗口：每个任务集 / 分享一个辅助窗口（同一组任务只会有一个面板）。

use fluxdown_ui_share::{PanelTarget, SharePanelView, panel_key};
use fluxdown_ui_shell::{AuxiliaryWindowView, auxiliary_window_options};
use gpui::{App, AppContext as _, Bounds, WindowBounds, px, size};
use gpui_component::Root;

use crate::{
    app::Desktop,
    windows::{WindowKey, WindowRegistry, bring_to_front},
};

const PANEL_TITLE_KEY: &str = "sharePanelTitle";
const PANEL_WINDOW_SIZE: gpui::Size<gpui::Pixels> = size(px(520.), px(680.));
const PANEL_WINDOW_MIN_SIZE: gpui::Size<gpui::Pixels> = size(px(440.), px(480.));

/// 打开或聚焦分享面板。`Tasks` 目标会让面板向 agent 创建（或取回已有的）分享。
pub fn open(cx: &mut App, target: PanelTarget) {
    let desktop = Desktop::global(cx);
    let translator = desktop.translator.clone();
    let hub = desktop.share_hub.clone();
    let key = panel_key(&target, &hub.read(cx).state().shares);
    let display_id = WindowRegistry::main_display_id(cx);
    let mut options =
        auxiliary_window_options(translator.read(cx).text(PANEL_TITLE_KEY).to_owned());
    options.display_id = display_id;
    options.window_min_size = Some(PANEL_WINDOW_MIN_SIZE);
    options.window_bounds = Some(WindowBounds::Windowed(Bounds::centered(
        display_id,
        PANEL_WINDOW_SIZE,
        cx,
    )));

    let window_key = WindowKey::Share(key);
    WindowRegistry::open_or_focus(cx, window_key.clone(), options, move |window, cx| {
        let panel = cx.new(|cx| SharePanelView::new(hub, target, window, cx));
        let window_view =
            cx.new(|cx| AuxiliaryWindowView::new(translator, PANEL_TITLE_KEY, panel.into(), cx));
        cx.new(|cx| Root::new(window_view, window, cx))
    });

    if let Some(handle) = WindowRegistry::handle(cx, &window_key)
        && let Err(error) = handle.update(cx, |_, window, cx| bring_to_front(window, cx))
    {
        log::debug!("view or window released before lifecycle update: {error:#}");
    }
}
