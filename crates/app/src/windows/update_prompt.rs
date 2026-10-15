//! 「发现新版本」提醒窗口：独立的辅助窗口，不依赖主窗口存在。开窗时机见
//! [`crate::update_notices`]；内容与动作见 [`UpdatePromptView`]。
//!
//! 层级与下载完成窗口一致：托盘驻留时由 agent 拉起的界面常是后台应用，提醒连同应用一起置前。

use fluxdown_ui_settings::{
    UPDATE_PROMPT_WINDOW_HEIGHT, UPDATE_PROMPT_WINDOW_WIDTH, UpdatePromptEvent, UpdatePromptView,
};
use fluxdown_ui_shell::{AuxiliaryWindowView, auxiliary_window_options};
use gpui::{App, AppContext as _, Bounds, WindowBounds, WindowKind, px, size};
use gpui_component::Root;

use crate::{
    app::Desktop,
    windows::{WindowKey, WindowRegistry, bring_to_front},
};

const TITLE_KEY: &str = "softwareUpdate";

/// 打开（或置前）提醒窗口，居中主窗口所在显示器（无主窗口时为主显示器）。
pub fn open(cx: &mut App) {
    let desktop = Desktop::global(cx);
    let translator = desktop.translator.clone();
    let store = desktop.settings_store.clone();
    let display_id = WindowRegistry::main_display_id(cx);
    let mut options = auxiliary_window_options(translator.read(cx).text(TITLE_KEY).to_owned());
    options.display_id = display_id;
    options.window_min_size = None;
    options.kind = WindowKind::Floating;
    options.is_resizable = false;
    options.window_bounds = Some(WindowBounds::Windowed(Bounds::centered(
        display_id,
        size(
            px(UPDATE_PROMPT_WINDOW_WIDTH),
            px(UPDATE_PROMPT_WINDOW_HEIGHT),
        ),
        cx,
    )));

    WindowRegistry::open_or_focus(cx, WindowKey::UpdatePrompt, options, move |window, cx| {
        let view = cx.new(|cx| UpdatePromptView::new(translator.clone(), store, cx));
        let window_view = cx.new(|cx| {
            AuxiliaryWindowView::new(translator.clone(), TITLE_KEY, view.clone().into(), cx)
                .resizable(false)
        });
        cx.new(|cx| {
            let root = Root::new(window_view, window, cx);
            cx.subscribe_in(&view, window, |_, _, event, window, _| match event {
                UpdatePromptEvent::Close => window.remove_window(),
            })
            .detach();
            root
        })
    });

    if let Some(handle) = WindowRegistry::handle(cx, &WindowKey::UpdatePrompt)
        && let Err(error) = handle.update(cx, |_, window, cx| bring_to_front(window, cx))
    {
        log::debug!("view or window released before lifecycle update: {error:#}");
    }
}
