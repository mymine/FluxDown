//! 插件桌面界面窗口：contributes.views 中 `location == window` 且带 `desktop` 的脚本，
//! 每个视图一个窗口。
//!
//! 层级：`Root`（FluxDown 浮层）→ `ShellRoot`（脚本的对话框 / sheet / toast）→
//! `AuxiliaryWindowView`（FluxDown 标题栏）→ 脚本视图。尺寸取 manifest，未声明时用默认值。

use fluxdown_protocol::PluginDto;
use fluxdown_ui_plugin_ui::PluginWindowSpec;
use fluxdown_ui_shell::{AuxiliaryWindowView, auxiliary_window_options};
use gpui::{AnyView, App, AppContext as _, Bounds, SharedString, WindowBounds, px, size};
use gpui_component::Root;

use crate::{
    app::Desktop,
    windows::{WindowKey, WindowRegistry},
};

const DEFAULT_WIDTH: f32 = 960.;
const DEFAULT_HEIGHT: f32 = 640.;
/// 插件可声明的最小尺寸下限（与引擎 manifest 校验 `[240, 4096]` 一致）。
const MIN_EDGE: f32 = 240.;

/// 打开（或聚焦）插件某个 window 视图的桌面界面。视图没有可用桌面入口时什么也不做。
pub fn open(cx: &mut App, plugin: &PluginDto, view_id: &str) {
    let spec = plugin
        .contributions
        .views
        .iter()
        .find(|view| view.id == view_id)
        .and_then(|view| PluginWindowSpec::from_view(plugin, view));
    let Some(spec) = spec else {
        log::warn!(
            "plugin {} has no loadable desktop interface",
            plugin.identity
        );
        return;
    };
    let desktop = Desktop::global(cx);
    let translator = desktop.translator.clone();
    let title = SharedString::from(format!("{}: {}", spec.name, spec.view_title));

    let width = spec.entry.width.unwrap_or(DEFAULT_WIDTH);
    let height = spec.entry.height.unwrap_or(DEFAULT_HEIGHT);
    let window_size = size(px(width), px(height));
    let mut options = auxiliary_window_options(title.clone());
    options.window_min_size = Some(size(px(width.min(MIN_EDGE)), px(height.min(MIN_EDGE))));
    options.window_bounds = Some(WindowBounds::Windowed(Bounds::centered(
        None,
        window_size,
        cx,
    )));
    options.is_resizable = true;
    options.is_minimizable = true;

    let key = WindowKey::Plugin(format!("{}/{}", spec.identity, spec.view_id));
    WindowRegistry::open_or_focus(cx, key, options, move |window, cx| {
        let content: AnyView = match fluxdown_ui_plugin_ui::load_desktop_view(&spec, window, cx) {
            Ok(view) => view.into(),
            Err(error) => fluxdown_ui_plugin_ui::load_failure_view(&error, cx),
        };
        let chrome = cx.new(|cx| {
            let mut chrome =
                AuxiliaryWindowView::new(translator.clone(), "pluginWindowTitle", content, cx);
            chrome.set_title(Some(title.clone()), cx);
            chrome
        });
        window.set_window_title(&title);
        // `ShellRoot::find` 只认窗口根自身或 `Root` 的直接内层：脚本的对话框 / sheet / toast
        // 才能找到宿主，故 `ShellRoot` 必须紧贴在 `Root` 下、包住 FluxDown 标题栏。
        let shell_root = fluxdown_ui_plugin_ui::wrap_shell_root(chrome.into(), window, cx);
        cx.new(|cx| Root::new(shell_root, window, cx))
    });
}
