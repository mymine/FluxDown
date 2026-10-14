//! 独立下载进度 / 完成窗口：任务进度与自动完成集合共用 FluxDown 辅助窗口和业务视图。
//! 标题随进度与导航刷新，可调整大小、可最小化；共用设备本地的尺寸、位置与显示器记忆。
//! 开关时机由 [`crate::progress_windows`] 与 [`crate::completion_windows`] 决定。
//!
//! 层级：界面常是后台应用（下载由浏览器捕获 / 静默建成时浏览器在前台），只在本应用内排序
//! 的窗口会压在浏览器下面。
//! - 进度窗口（用户刚开始的下载）：连同应用一起置前（`bring_to_front`）。
//! - 后台完成窗口：不抢焦点，macOS / Windows 用 `WindowKind::PopUp` 浮在所有应用之上
//!   （macOS 为不激活应用的浮动面板，Windows 为 `WS_EX_TOPMOST`）；Linux X11 的 PopUp 是
//!   不受窗口管理器管理的 override-redirect（不能拖动），这里保持普通窗口并请求注意。

use std::{sync::Arc, time::Duration};

use fluxdown_ui_downloads::{
    COMPLETION_WINDOW_PREF, PROGRESS_WINDOW_INITIAL_HEIGHT, PROGRESS_WINDOW_WIDTH,
    ProgressWindowEvent, ProgressWindowPrefs, ProgressWindowView,
};
use fluxdown_ui_shell::{AuxiliaryWindowView, auxiliary_window_options};
use gpui::{App, AppContext as _, Context, Entity, SharedString, Window, WindowKind, px, size};
use gpui_component::Root;

use crate::{
    app::Desktop,
    downloads_port::AgentDownloadsPort,
    session::attach,
    windows::{RememberedWindow, WindowKey, WindowRegistry},
};

/// 打开文件 / 文件夹后等待对方接管前台的上限。
const HANDOFF_CLOSE_TIMEOUT: Duration = Duration::from_millis(1500);

/// 打开（或置前）任务的进度窗口。`activate = false` 时不抢焦点（后台完成弹窗）。
pub fn open(cx: &mut App, task_id: String, activate: bool) {
    // The root owns the view; manual progress callers do not retain another handle.
    drop(open_window(
        cx,
        WindowKey::Progress(task_id.clone()),
        task_id,
        activate,
        None,
    ));
}

/// 自动完成复用原有进度窗口，不激活应用，也不聚焦已存在的完成窗口。
pub(crate) fn open_completion(
    cx: &mut App,
    task_ids: Vec<String>,
    total: usize,
) -> Option<Entity<ProgressWindowView>> {
    if WindowRegistry::is_open(cx, &WindowKey::Completion) {
        return None;
    }
    let first = task_ids.first()?.clone();
    open_window(
        cx,
        WindowKey::Completion,
        first,
        false,
        Some((task_ids, total)),
    )
}

fn open_window(
    cx: &mut App,
    key: WindowKey,
    task_id: String,
    activate: bool,
    completed: Option<(Vec<String>, usize)>,
) -> Option<Entity<ProgressWindowView>> {
    let desktop = Desktop::global(cx);
    let translator = desktop.translator.clone();
    let session = desktop.session.clone();
    let client = desktop.client.clone();
    let title = translator.read(cx).text("progressWindowTitle").to_owned();
    let settings = desktop.settings_store.clone();
    let show_completion =
        ProgressWindowPrefs::from_preferences(settings.read(cx).preferences()).completion;

    let mut options = auxiliary_window_options(title);
    let min_size = size(
        px(PROGRESS_WINDOW_WIDTH),
        px(PROGRESS_WINDOW_INITIAL_HEIGHT),
    );
    options.window_min_size = Some(min_size);
    options.is_resizable = true;
    options.is_minimizable = true;
    WindowRegistry::restore_bounds(RememberedWindow::Progress, &mut options, min_size, cx);
    options.focus = activate;
    if !activate && cfg!(any(target_os = "macos", target_os = "windows")) {
        options.kind = WindowKind::PopUp;
    }

    let mut opened_view = None;
    let completion = key == WindowKey::Completion;
    let opened = WindowRegistry::open_or_focus(cx, key.clone(), options, |window, cx| {
        let port = Arc::new(AgentDownloadsPort::new(client.clone()));
        let view = cx.new(|cx| {
            cx.observe(&settings, |view: &mut ProgressWindowView, settings, cx| {
                let completion =
                    ProgressWindowPrefs::from_preferences(settings.read(cx).preferences())
                        .completion;
                view.set_show_completion(completion, cx);
            })
            .detach();
            ProgressWindowView::new(
                translator.clone(),
                task_id.clone(),
                port,
                show_completion,
                cx,
            )
        });
        attach(&session, &view, cx);
        if let Some((task_ids, total)) = completed {
            view.update(cx, |view, cx| view.append_completed(task_ids, total, cx));
        }
        let window_view = cx.new(|cx| {
            AuxiliaryWindowView::new(
                translator.clone(),
                "progressWindowTitle",
                view.clone().into(),
                cx,
            )
        });
        if let Some(title) = view.read(cx).title() {
            window.set_window_title(&title);
            window_view.update(cx, |chrome, cx| {
                chrome.set_title(Some(SharedString::from(title)), cx);
            });
        }

        let root = cx.new(|cx| {
            let root = Root::new(window_view.clone(), window, cx);
            cx.observe_in(&view, window, move |_, view, window, cx| {
                let Some(title) = view.read(cx).title() else {
                    return;
                };
                window.set_window_title(&title);
                window_view.update(cx, |chrome, cx| {
                    chrome.set_title(Some(SharedString::from(title)), cx);
                });
            })
            .detach();
            cx.subscribe_in(&view, window, move |_, _, event, window, cx| match event {
                ProgressWindowEvent::Close => window.remove_window(),
                ProgressWindowEvent::HandedOff => {
                    if completion {
                        crate::completion_windows::window_closing(cx);
                    }
                    close_after_handoff(window, cx);
                }
                ProgressWindowEvent::ShowCompletionChanged(value) => {
                    Desktop::set_pref(cx, COMPLETION_WINDOW_PREF, (*value).into());
                }
            })
            .detach();
            root
        });
        WindowRegistry::persist_bounds(RememberedWindow::Progress, client, &root, window, cx);
        opened_view = Some(view);
        root
    });
    let handle = opened
        .map(Into::into)
        .or_else(|| WindowRegistry::handle(cx, &key));
    if let Some(handle) = handle
        && let Err(error) = handle.update(cx, |_, window, cx| {
            if activate {
                crate::windows::bring_to_front(window, cx);
            } else {
                #[cfg(any(target_os = "linux", target_os = "freebsd"))]
                window.request_attention();
            }
        })
    {
        log::debug!("view or window released before lifecycle update: {error:#}");
    }
    opened_view
}

/// 打开的文件 / 文件夹接管前台后再关窗。本应用仍在前台时直接关闭当前窗口，系统会把本应用
/// 的下一个窗口（通常是主窗口）提到前面、压住刚打开的程序；等窗口失活（对方已成为前台）再关，
/// 超时兜底（对方未抢前台时也不让窗口滞留）。
fn close_after_handoff(window: &mut Window, cx: &mut Context<Root>) {
    if !window.is_window_active() {
        window.remove_window();
        return;
    }
    cx.observe_window_activation(window, |_, window, _| {
        if !window.is_window_active() {
            window.remove_window();
        }
    })
    .detach();
    cx.spawn_in(window, async move |_, cx| {
        cx.background_executor().timer(HANDOFF_CLOSE_TIMEOUT).await;
        if let Err(error) = cx.update(|window, _| window.remove_window()) {
            log::debug!("view or window released before lifecycle update: {error:#}");
        }
    })
    .detach();
}
