//! 退出流程：完全退出前，若有实时分享仍有接收方没收完，按「FluxMesh」设置的退出处理询问 / 自动转为
//! 云端暂存，并在阻塞进度框里等上传完成后再退出。决策本身是纯函数（[`crate::model::exit`]）。
//!
//! app 在真正退出的两条路径（菜单 / ⌘Q、非驻留模式关主窗口）上先问 [`needs_exit_gate`]：为 `false`
//! 就直接退出，不需要窗口；为 `true` 再用 [`run_exit_gate`] 弹对话框，决定后回调 `proceed`。

use std::rc::Rc;

use fluxdown_ui_components::{ControlExt as _, tabular_numbers};
use fluxdown_ui_i18n::Translator;
use fluxdown_ui_theme::active_theme;
use gpui::{
    AnyWindowHandle, App, AppContext as _, Context, Entity, Global, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Window, div,
    prelude::FluentBuilder as _,
};
use gpui_component::{
    WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::{DialogClose, DialogFooter},
    progress::Progress,
    spinner::Spinner,
    v_flex,
};

use crate::{
    errors::{error_text, reason_key},
    hub::ShareHub,
    model::{
        exit::{ExitCandidates, ExitDecision, ExitWait, decide_exit, exit_wait, wait_percent},
        format_bytes, format_percent, format_rate,
    },
    window_ui::text,
};

/// 对话框内容宽度（基准 px，随字号缩放）。
const DIALOG_WIDTH: f32 = 420.;

/// 退出后继续执行的动作（由 app 提供：完全退出）。
pub type ExitContinuation = Rc<dyn Fn(&mut App)>;

/// 已有退出对话框在显示：同一窗口再次触发退出（重复 ⌘Q）不叠第二个。
#[derive(Default)]
struct ExitDialogOpen(Option<AnyWindowHandle>);

impl Global for ExitDialogOpen {}

fn decision(hub: &ShareHub) -> ExitDecision {
    // 与 agent 断开时状态是旧的、转换也发不出去：不拦退出。
    // FluxMesh 未启用 / 不可用时没有任何分享需要处理。
    if hub.is_stale() || !hub.available() {
        return ExitDecision::Quit;
    }
    let state = hub.state();
    decide_exit(state, hub.signed_in(), state.local_config.exit_behavior)
}

/// 退出前是否需要先处理FluxMesh（询问或自动转换）。为 `false` 时调用方应直接退出。
#[must_use]
pub fn needs_exit_gate(hub: &Entity<ShareHub>, cx: &App) -> bool {
    decision(hub.read(cx)) != ExitDecision::Quit
}

/// 在 `window` 上执行退出前的FluxMesh处理；处理完成（或用户选择直接退出）时调用 `proceed`，
/// 用户取消则什么都不做。没有需要处理的分享时立即 `proceed`。
pub fn run_exit_gate(
    hub: &Entity<ShareHub>,
    window: &mut Window,
    cx: &mut App,
    proceed: ExitContinuation,
) {
    let handle = window.window_handle();
    if cx.default_global::<ExitDialogOpen>().0 == Some(handle) && window.has_active_dialog(cx) {
        return;
    }
    let decided = decision(hub.read(cx));
    match decided {
        ExitDecision::Quit => proceed(cx),
        ExitDecision::Prompt(candidates) => open_prompt(hub, candidates, window, cx, proceed),
        ExitDecision::Convert(candidates) => {
            start_conversion(hub, candidates.share_ids, window, cx, proceed);
        }
    }
}

fn mark_open(window: &Window, cx: &mut App) {
    cx.default_global::<ExitDialogOpen>().0 = Some(window.window_handle());
}

fn mark_closed(cx: &mut App) {
    cx.default_global::<ExitDialogOpen>().0 = None;
}

/// `ExitBehavior::Prompt`：三选一——转为云端暂存后退出 / 直接退出 / 取消。
fn open_prompt(
    hub: &Entity<ShareHub>,
    candidates: ExitCandidates,
    window: &mut Window,
    cx: &mut App,
    proceed: ExitContinuation,
) {
    let translator = hub.read(cx).translator().read(cx).clone();
    let title = text(&translator, "shareExitTitle");
    let description = SharedString::from(translator.text_with(
        "shareExitPrompt",
        &[("count", &candidates.receivers.to_string())],
    ));
    let convert = text(&translator, "shareExitConvertQuit");
    let quit_now = text(&translator, "shareExitQuitNow");
    let cancel = text(&translator, "cancel");
    let hub = hub.clone();
    let share_ids = candidates.share_ids;
    mark_open(window, cx);
    window.open_dialog(cx, move |dialog, _, cx| {
        let hub = hub.clone();
        let share_ids = share_ids.clone();
        let proceed_convert = Rc::clone(&proceed);
        let proceed_quit = Rc::clone(&proceed);
        let spacing = active_theme(cx).tokens().spacing;
        let description = description.clone();
        dialog
            .title(fluxdown_ui_components::dialog_title(title.clone(), cx))
            .w(active_theme(cx).text_extent(DIALOG_WIDTH))
            .content(move |content, _, cx| {
                let tokens = active_theme(cx).tokens().clone();
                content.min_h_0().child(
                    div()
                        .w_full()
                        .text_size(tokens.typography.sm.size)
                        .line_height(tokens.typography.sm.line_height)
                        .text_color(tokens.colors.muted_foreground)
                        .child(description.clone()),
                )
            })
            .on_close(|_, _, cx| mark_closed(cx))
            .footer(
                DialogFooter::new()
                    .gap(spacing.sm)
                    .child(
                        Button::new("share-exit-quit")
                            .outline()
                            .label(quit_now.clone())
                            .control(cx)
                            .on_click(move |_, window, cx| {
                                window.close_dialog(cx);
                                mark_closed(cx);
                                proceed_quit(cx);
                            }),
                    )
                    .child(
                        DialogClose::new().child(
                            Button::new("share-exit-cancel")
                                .outline()
                                .label(cancel.clone())
                                .control(cx),
                        ),
                    )
                    .child(
                        Button::new("share-exit-convert")
                            .primary()
                            .label(convert.clone())
                            .control(cx)
                            .on_click(move |_, window, cx| {
                                window.close_dialog(cx);
                                mark_closed(cx);
                                let hub = hub.clone();
                                let share_ids = share_ids.clone();
                                let proceed = Rc::clone(&proceed_convert);
                                // 等当前对话框收起后再开进度框，避免两者叠在同一帧。
                                window.defer(cx, move |window, cx| {
                                    start_conversion(&hub, share_ids, window, cx, proceed);
                                });
                            }),
                    ),
            )
    });
}

/// 转换进度框的内容：聚合上传进度，随 hub 的 `ShareChanged` 刷新，全部就绪后调用 `proceed`。
struct ExitProgress {
    hub: Entity<ShareHub>,
    share_ids: Vec<String>,
    /// 还没返回的 `convertToCloud` 调用数；归零前不判断完成（hub 里还是转换前的旧状态）。
    calls_pending: usize,
    /// 第一个失败的调用的错误文案。
    call_error: Option<SharedString>,
    proceed: ExitContinuation,
    finished: bool,
}

impl ExitProgress {
    fn new(
        hub: Entity<ShareHub>,
        share_ids: Vec<String>,
        proceed: ExitContinuation,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&hub, |this, _, cx| this.check(cx)).detach();
        let translator = hub.read(cx).translator().clone();
        cx.observe(&translator, |_, _, cx| cx.notify()).detach();
        Self {
            hub,
            calls_pending: share_ids.len(),
            share_ids,
            call_error: None,
            proceed,
            finished: false,
        }
    }

    /// 所有调用返回、没有错误且每个副本都已就绪：退出。
    fn check(&mut self, cx: &mut Context<Self>) {
        cx.notify();
        if self.finished || self.calls_pending > 0 || self.call_error.is_some() {
            return;
        }
        let wait = exit_wait(self.hub.read(cx).state(), &self.share_ids);
        if wait == ExitWait::Ready {
            self.finished = true;
            let proceed = Rc::clone(&self.proceed);
            proceed(cx);
        }
    }

    fn call_finished(&mut self, error: Option<SharedString>, cx: &mut Context<Self>) {
        self.calls_pending = self.calls_pending.saturating_sub(1);
        if self.call_error.is_none() {
            self.call_error = error;
        }
        self.check(cx);
    }
}

impl Render for ExitProgress {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = active_theme(cx);
        let tokens = theme.tokens().clone();
        let hub = self.hub.read(cx);
        let translator = hub.translator().read(cx);
        let wait = exit_wait(hub.state(), &self.share_ids);

        let (headline, detail, percent, failed) = progress_texts(
            translator,
            self.call_error.as_ref(),
            self.calls_pending,
            wait,
        );
        v_flex()
            .w_full()
            .min_w_0()
            .gap(tokens.spacing.sm)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(tokens.spacing.sm)
                    .when(!failed && !self.finished, |this| this.child(Spinner::new()))
                    .child(
                        div()
                            .min_w_0()
                            .text_size(tokens.typography.sm.size)
                            .line_height(tokens.typography.sm.line_height)
                            .text_color(if failed {
                                tokens.colors.destructive
                            } else {
                                tokens.colors.foreground
                            })
                            .child(headline),
                    ),
            )
            .when(!failed, |this| {
                this.child(
                    Progress::new("share-exit-progress")
                        .w_full()
                        .color(tokens.colors.primary)
                        .value(percent.unwrap_or(0.)),
                )
            })
            .child(
                div()
                    .w_full()
                    .font_features(tabular_numbers())
                    .text_size(tokens.typography.xs.size)
                    .line_height(tokens.typography.xs.line_height)
                    .text_color(tokens.colors.muted_foreground)
                    .child(detail),
            )
    }
}

/// 进度框三段文案：`(标题, 说明, 进度百分比, 是否失败)`。
fn progress_texts(
    translator: &Translator,
    call_error: Option<&SharedString>,
    calls_pending: usize,
    wait: ExitWait,
) -> (SharedString, SharedString, Option<f32>, bool) {
    if let Some(error) = call_error {
        return (
            text(translator, "shareExitFailedTitle"),
            error.clone(),
            None,
            true,
        );
    }
    match wait {
        ExitWait::Failed(reason) => (
            text(translator, "shareExitFailedTitle"),
            text(translator, reason_key(reason)),
            None,
            true,
        ),
        ExitWait::Ready if calls_pending == 0 => (
            text(translator, "shareExitDone"),
            text(translator, "shareExitDoneHint"),
            Some(100.),
            false,
        ),
        ExitWait::Ready => (
            text(translator, "shareExitProgressTitle"),
            text(translator, "shareExitStarting"),
            None,
            false,
        ),
        ExitWait::Uploading {
            done_bytes,
            total_bytes,
            rate_bps,
            paused,
        } => {
            let percent = wait_percent(done_bytes, total_bytes);
            let fraction = percent.map_or(0., |percent| f64::from(percent) / 100.);
            let amount = translator.text_with(
                "shareStageProgress",
                &[
                    ("done", &format_bytes(done_bytes)),
                    ("total", &format_bytes(total_bytes)),
                    ("percent", &format_percent(fraction)),
                    ("rate", &format_rate(rate_bps)),
                ],
            );
            let mut detail = amount;
            if paused {
                detail.push_str(" · ");
                detail.push_str(translator.text("shareStagePaused"));
            }
            (
                text(translator, "shareExitProgressTitle"),
                SharedString::from(detail),
                percent,
                false,
            )
        }
    }
}

/// 转换并等待：对每个分享调用 `convertToCloud`，弹阻塞进度框；全部就绪后退出。进度框里
/// 「直接退出」放弃等待，「取消退出」收起对话框（已开始的上传继续，可在分享面板里看）。
fn start_conversion(
    hub: &Entity<ShareHub>,
    share_ids: Vec<String>,
    window: &mut Window,
    cx: &mut App,
    proceed: ExitContinuation,
) {
    let (api, translator) = {
        let hub = hub.read(cx);
        (hub.api(), hub.translator().read(cx).clone())
    };
    let progress = {
        let hub = hub.clone();
        let share_ids = share_ids.clone();
        let proceed = Rc::clone(&proceed);
        cx.new(move |cx| ExitProgress::new(hub, share_ids, proceed, cx))
    };

    let title = text(&translator, "shareExitProgressTitle");
    let quit_now = text(&translator, "shareExitQuitNow");
    let cancel = text(&translator, "shareExitCancelQuit");
    mark_open(window, cx);
    let progress_view = progress.clone();
    window.open_dialog(cx, move |dialog, _, cx| {
        let progress = progress_view.clone();
        let proceed = Rc::clone(&proceed);
        let spacing = active_theme(cx).tokens().spacing;
        dialog
            .title(fluxdown_ui_components::dialog_title(title.clone(), cx))
            .w(active_theme(cx).text_extent(DIALOG_WIDTH))
            .close_button(false)
            .overlay_closable(false)
            .keyboard(false)
            .content(move |content, _, _| content.min_h_0().child(progress.clone()))
            .footer(
                DialogFooter::new()
                    .gap(spacing.sm)
                    .child(
                        Button::new("share-exit-progress-quit")
                            .outline()
                            .label(quit_now.clone())
                            .control(cx)
                            .on_click(move |_, window, cx| {
                                window.close_dialog(cx);
                                mark_closed(cx);
                                proceed(cx);
                            }),
                    )
                    .child(
                        DialogClose::new().child(
                            Button::new("share-exit-progress-cancel")
                                .primary()
                                .label(cancel.clone())
                                .control(cx),
                        ),
                    ),
            )
            .on_close(|_, _, cx| mark_closed(cx))
    });

    for id in share_ids {
        let future = api.convert_to_cloud(&id, None);
        let translator = translator.clone();
        let progress = progress.downgrade();
        cx.spawn(async move |cx| {
            let error = future
                .await
                .err()
                .map(|error| SharedString::from(error_text(&translator, &error)));
            let Ok(()) = progress.update(cx, |this, cx| this.call_finished(error, cx)) else {
                // 进度框已关闭（取消退出 / 直接退出），不再回写。
                return;
            };
        })
        .detach();
    }
}
