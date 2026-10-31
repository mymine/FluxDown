//! 接收确认窗口：取件码 / 链接 → 解析 → 确认（文件勾选、来源、保存目录）→ 开始接收。
//!
//! 窗口由 app 复用为单例（`retarget` 切换来源）。`resolveId` 是 agent 内存里的短时句柄，视图只把它当作
//! 一次确认的凭据保存；句柄失效且有原始输入时重试会重新解析。

use std::ops::Range;

use fluxdown_protocol::{RpcErrorData, ShareResolveDto, ShareResolveParams};
use fluxdown_ui_components::{
    BusyExt as _, ControlExt as _, FluxIcon, check_row, field_error, field_hint, field_label,
    tabular_numbers, tooltip_view,
};
use fluxdown_ui_theme::active_theme;
use gpui::{
    AnyElement, App, AppContext as _, Context, Div, Entity, Hsla, InteractiveElement as _,
    IntoElement, ParentElement, PathPromptOptions, Render, SharedString,
    StatefulInteractiveElement as _, Styled, Subscription, Window, div,
    prelude::FluentBuilder as _, uniform_list,
};
use gpui_component::{
    Disableable as _, Icon,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    scroll::ScrollableElement as _,
    spinner::Spinner,
    v_flex,
};

use crate::{
    errors::error_text,
    hub::ShareHub,
    model::{
        format_bytes, format_datetime,
        receive::{
            FailedStep, FileSelection, InputStage, RetryAction, SourceTone, classify_input,
            needs_turnstile, pickup_code_len, receive_params, retry_action, sender_name,
            single_line, source_lines,
        },
    },
    report::open_report_dialog,
    window_ui::{self, ellipsized, text},
};

/// 文件清单一次最多露出的行数（超出滚动）。
const FILE_LIST_MAX_ROWS: usize = 7;

/// 接收窗口的来源。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReceiveSource {
    /// 用户给出的取件码 / 分享链接（空串 = 只打开输入态）。
    Input(String),
    /// agent 已解析并等待确认的预览（`pendingReceive.resolveId`）。
    Pending(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// 可编辑输入。
    Input,
    Resolving,
    /// 确认框：文件 / 来源 / 保存目录。
    Preview,
    /// 已建好 N 个下载任务。
    Started(usize),
    /// 预览已失效（被取代 / 已处理）。
    Gone,
}

pub struct ShareReceiveView {
    hub: Entity<ShareHub>,
    input: Entity<InputState>,
    /// 输入框当前 placeholder；语言切换后在 render 中同步（InputState 只在构造时取一次）。
    placeholder: SharedString,
    phase: Phase,
    preview: Option<ShareResolveDto>,
    selection: FileSelection,
    /// 用户选的保存目录；`None` = daemon 默认目录。
    save_dir: Option<String>,
    /// 来自 `pendingReceive` 的预览 ID。
    pending_id: Option<String>,
    /// 最近一次成功发起解析的 `codeOrLink`，句柄失效时据此重新解析。
    origin: Option<String>,
    error: Option<(FailedStep, RpcErrorData)>,
    /// 取消待确认预览失败的文案。
    notice: Option<SharedString>,
    input_invalid: bool,
    /// 接收 / 取消请求在途。
    busy: bool,
    picking_dir: bool,
    /// 每次 `retarget` / 新解析自增：在途请求返回时发现不一致就丢弃结果。
    generation: u64,
    _subscriptions: Vec<Subscription>,
}

impl ShareReceiveView {
    pub fn new(
        hub: Entity<ShareHub>,
        source: ReceiveSource,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let translator = hub.read(cx).translator().clone();
        let placeholder = text(translator.read(cx), "sharePickupPlaceholder");
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder.clone()));
        let subscriptions = vec![
            cx.observe(&hub, |this, _, cx| this.on_hub_changed(cx)),
            cx.observe(&translator, |_, _, cx| cx.notify()),
            cx.subscribe_in(
                &input,
                window,
                |this: &mut Self, _, event: &InputEvent, _, cx| match event {
                    InputEvent::PressEnter { .. } => this.submit_input(cx),
                    InputEvent::Change => {
                        if this.input_invalid {
                            this.input_invalid = false;
                            cx.notify();
                        }
                    }
                    InputEvent::Focus | InputEvent::Blur => {}
                },
            ),
        ];
        let mut view = Self {
            hub,
            input,
            placeholder,
            phase: Phase::Input,
            preview: None,
            selection: FileSelection::default(),
            save_dir: None,
            pending_id: None,
            origin: None,
            error: None,
            notice: None,
            input_invalid: false,
            busy: false,
            picking_dir: false,
            generation: 0,
            _subscriptions: subscriptions,
        };
        view.retarget(source, window, cx);
        view
    }

    /// 换一个来源（app 复用同一个窗口）。相同的待确认预览正在确认时保留用户的勾选与目录。
    pub fn retarget(&mut self, source: ReceiveSource, window: &mut Window, cx: &mut Context<Self>) {
        if let ReceiveSource::Pending(id) = &source
            && self.phase == Phase::Preview
            && self.pending_id.as_deref() == Some(id.as_str())
        {
            cx.notify();
            return;
        }
        self.generation += 1;
        self.preview = None;
        self.selection = FileSelection::default();
        self.save_dir = None;
        self.pending_id = None;
        self.origin = None;
        self.error = None;
        self.notice = None;
        self.input_invalid = false;
        self.busy = false;
        self.picking_dir = false;
        self.phase = Phase::Input;
        match source {
            ReceiveSource::Input(value) => {
                self.input
                    .update(cx, |input, cx| input.set_value(value.clone(), window, cx));
                let code_len = pickup_code_len(&self.hub.read(cx).state().policy_summary);
                match classify_input(&value, code_len) {
                    InputStage::Empty => self.input.update(cx, |input, cx| input.focus(window, cx)),
                    InputStage::Invalid => self.input_invalid = true,
                    InputStage::Valid(parsed) => self.resolve(parsed.as_wire().to_owned(), cx),
                }
            }
            ReceiveSource::Pending(id) => {
                self.input
                    .update(cx, |input, cx| input.set_value(String::new(), window, cx));
                let pending = self
                    .hub
                    .read(cx)
                    .pending_receive()
                    .filter(|pending| pending.resolve_id == id)
                    .cloned();
                match pending {
                    Some(pending) => {
                        self.pending_id = Some(id);
                        self.set_preview(pending);
                    }
                    None => self.phase = Phase::Gone,
                }
            }
        }
        cx.notify();
    }

    fn set_preview(&mut self, preview: ShareResolveDto) {
        self.selection = FileSelection::all(preview.files.len());
        self.save_dir = None;
        self.preview = Some(preview);
        self.phase = Phase::Preview;
    }

    /// 待确认预览被取代 / 已被别处处理：确认中的窗口转为失效态（接收 / 取消在途时不打断）。
    fn on_hub_changed(&mut self, cx: &mut Context<Self>) {
        if self.phase == Phase::Preview
            && !self.busy
            && let Some(id) = self.pending_id.as_deref()
            && self
                .hub
                .read(cx)
                .pending_receive()
                .map(|pending| pending.resolve_id.as_str())
                != Some(id)
        {
            self.phase = Phase::Gone;
        }
        cx.notify();
    }

    /// 用输入框当前内容发起解析。
    fn submit_input(&mut self, cx: &mut Context<Self>) {
        if self.phase != Phase::Input || self.hub.read(cx).is_stale() {
            return;
        }
        let value = self.input.read(cx).value().to_string();
        let code_len = pickup_code_len(&self.hub.read(cx).state().policy_summary);
        match classify_input(&value, code_len) {
            InputStage::Empty => {}
            InputStage::Invalid => {
                self.input_invalid = true;
                self.error = None;
            }
            InputStage::Valid(parsed) => {
                self.input_invalid = false;
                self.resolve(parsed.as_wire().to_owned(), cx);
            }
        }
        cx.notify();
    }

    fn resolve(&mut self, code_or_link: String, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.origin = Some(code_or_link.clone());
        self.error = None;
        self.notice = None;
        self.phase = Phase::Resolving;
        let params = ShareResolveParams {
            code_or_link,
            turnstile_token: None,
        };
        let future = self.hub.read(cx).api().resolve(&params);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = future.await;
            let Ok(()) = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                match result {
                    Ok(preview) => this.set_preview(preview),
                    Err(error) => {
                        this.phase = Phase::Input;
                        this.error = Some((FailedStep::Resolve, error));
                    }
                }
                cx.notify();
            }) else {
                // 窗口已关闭，不再回写异步结果。
                return;
            };
        })
        .detach();
    }

    fn start_receive(&mut self, cx: &mut Context<Self>) {
        if self.busy
            || self.phase != Phase::Preview
            || self.picking_dir
            || !self.selection.any_selected()
            || self.hub.read(cx).is_stale()
        {
            return;
        }
        let Some(preview) = &self.preview else {
            return;
        };
        let params = receive_params(
            &preview.resolve_id,
            self.save_dir.as_deref(),
            &self.selection,
        );
        self.busy = true;
        self.error = None;
        self.notice = None;
        let generation = self.generation;
        let future = self.hub.read(cx).api().receive(&params);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = future.await;
            let Ok(()) = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.busy = false;
                match result {
                    Ok(received) => this.phase = Phase::Started(received.task_ids.len()),
                    Err(error) => this.error = Some((FailedStep::Receive, error)),
                }
                cx.notify();
            }) else {
                // 窗口已关闭，不再回写异步结果。
                return;
            };
        })
        .detach();
    }

    /// 「重试」：按失败步骤与句柄是否失效决定重新解析还是重新接收。
    fn retry(&mut self, cx: &mut Context<Self>) {
        let Some((step, error)) = self.error.clone() else {
            return;
        };
        match retry_action(step, &error, self.origin.is_some()) {
            RetryAction::Resolve => match self.origin.clone() {
                Some(origin) if self.phase != Phase::Input => self.resolve(origin, cx),
                _ => self.submit_input(cx),
            },
            RetryAction::Receive => self.start_receive(cx),
        }
    }

    /// 取消：待确认预览要先告诉 agent 丢弃（成功后关窗，失败留在窗口里提示）；其余直接关窗。
    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pending = match (self.phase, self.pending_id.clone()) {
            (Phase::Preview, Some(id)) if !self.busy => Some(id),
            _ => None,
        };
        let Some(id) = pending else {
            window.remove_window();
            return;
        };
        self.busy = true;
        self.notice = None;
        let generation = self.generation;
        let future = self.hub.read(cx).api().dismiss_pending(&id);
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = future.await;
            let Ok(()) = this.update_in(cx, |this, window, cx| {
                if this.generation != generation {
                    return;
                }
                this.busy = false;
                match result {
                    Ok(_) => window.remove_window(),
                    Err(error) => {
                        let message = error_text(this.hub.read(cx).translator().read(cx), &error);
                        this.notice = Some(SharedString::from(message));
                    }
                }
                cx.notify();
            }) else {
                // 窗口已关闭，不再回写异步结果。
                return;
            };
        })
        .detach();
    }

    fn pick_dir(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.picking_dir || self.busy {
            return;
        }
        self.picking_dir = true;
        cx.notify();
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        let generation = self.generation;
        cx.spawn_in(window, async move |this, cx| {
            let picked = match receiver.await {
                Ok(Ok(Some(paths))) => paths.first().map(|path| path.display().to_string()),
                _ => None,
            };
            let Ok(()) = this.update_in(cx, |this, _, cx| {
                if this.generation != generation {
                    return;
                }
                this.picking_dir = false;
                if picked.is_some() {
                    this.save_dir = picked;
                }
                cx.notify();
            }) else {
                // 窗口已关闭，不再回写异步结果。
                return;
            };
        })
        .detach();
    }

    fn toggle_all(&mut self, cx: &mut Context<Self>) {
        let all = self.selection.all_selected();
        self.selection.set_all(!all);
        cx.notify();
    }
}

fn source_color(tone: SourceTone, cx: &App) -> Hsla {
    let theme = active_theme(cx);
    match tone {
        SourceTone::Good => theme.extended().colors.success,
        SourceTone::Warn => theme.extended().colors.warning,
        SourceTone::Unknown => theme.tokens().colors.muted_foreground,
    }
}

impl ShareReceiveView {
    fn sync_placeholder(
        &mut self,
        translator_text: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.placeholder != translator_text {
            self.placeholder = translator_text.clone();
            self.input.update(cx, |input, cx| {
                input.set_placeholder(translator_text, window, cx)
            });
        }
    }

    /// 解析 / 接收失败的错误行；人机验证另给提示。
    fn render_error(
        &self,
        translator: &fluxdown_ui_i18n::Translator,
        cx: &Context<Self>,
    ) -> Option<Div> {
        let tokens = active_theme(cx).tokens();
        let (_, error) = self.error.as_ref()?;
        Some(
            v_flex()
                .w_full()
                .gap(tokens.spacing.xs)
                .child(field_error(error_text(translator, error), cx))
                .when(needs_turnstile(error), |this| {
                    this.child(field_hint(text(translator, "shareTurnstileHint"), cx))
                }),
        )
    }

    fn render_input_phase(
        &self,
        translator: &fluxdown_ui_i18n::Translator,
        stale: bool,
        cx: &Context<Self>,
    ) -> Div {
        let theme = active_theme(cx);
        let tokens = theme.tokens();
        let empty = self.input.read(cx).value().trim().is_empty();
        let retry = self.error.is_some();
        v_flex()
            .size_full()
            .min_h_0()
            .child(window_ui::header(
                FluxIcon::Share2,
                text(translator, "shareReceiveTitle"),
                None,
                None,
                text(translator, "close"),
                cx,
            ))
            .child(
                div().flex_1().min_h_0().w_full().child(
                    div()
                        .id("share-receive-input-body")
                        .size_full()
                        .overflow_y_scrollbar()
                        .child(
                            v_flex()
                                .w_full()
                                .gap(tokens.spacing.sm)
                                .p(tokens.spacing.lg)
                                .child(Input::new(&self.input).control(cx).w_full().disabled(stale))
                                .when(self.input_invalid, |this| {
                                    this.child(field_error(
                                        text(translator, "shareInputInvalid"),
                                        cx,
                                    ))
                                })
                                .when_some(self.render_error(translator, cx), |this, error| {
                                    this.child(error)
                                })
                                .when(stale, |this| {
                                    this.child(field_hint(
                                        text(translator, "localServiceDisconnected"),
                                        cx,
                                    ))
                                }),
                        ),
                ),
            )
            .child(
                window_ui::footer_frame(cx).child(div()).child(
                    h_flex()
                        .gap(tokens.spacing.sm)
                        .child(
                            Button::new("share-receive-cancel")
                                .outline()
                                .label(text(translator, "cancel"))
                                .control(cx)
                                .on_click(|_, window, _| window.remove_window()),
                        )
                        .child(
                            Button::new("share-receive-submit")
                                .primary()
                                .label(text(
                                    translator,
                                    if retry { "shareRetry" } else { "shareReceive" },
                                ))
                                .control(cx)
                                .disabled(empty || stale)
                                .on_click(cx.listener(|this, _, _, cx| this.submit_input(cx))),
                        ),
                ),
            )
    }

    fn render_resolving(
        &self,
        translator: &fluxdown_ui_i18n::Translator,
        cx: &Context<Self>,
    ) -> Div {
        let tokens = active_theme(cx).tokens();
        v_flex()
            .size_full()
            .min_h_0()
            .child(window_ui::header(
                FluxIcon::Share2,
                text(translator, "shareReceiveTitle"),
                None,
                None,
                text(translator, "close"),
                cx,
            ))
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .items_center()
                    .justify_center()
                    .gap(tokens.spacing.md)
                    .child(Spinner::new())
                    .child(
                        div()
                            .text_size(tokens.typography.sm.size)
                            .line_height(tokens.typography.sm.line_height)
                            .text_color(tokens.colors.muted_foreground)
                            .child(text(translator, "shareResolving")),
                    ),
            )
            .child(
                window_ui::footer_frame(cx).child(div()).child(
                    Button::new("share-resolving-cancel")
                        .outline()
                        .label(text(translator, "cancel"))
                        .control(cx)
                        .on_click(|_, window, _| window.remove_window()),
                ),
            )
    }

    /// 单行文件项：勾选框 + 文件名（中间省略）+ 大小。
    fn render_file_row(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(file) = self
            .preview
            .as_ref()
            .and_then(|preview| preview.files.get(index))
        else {
            return div().into_any_element();
        };
        let theme = active_theme(cx);
        let tokens = theme.tokens();
        let translator = self.hub.read(cx).translator().read(cx).clone();
        let name = if file.name.trim().is_empty() {
            text(&translator, "shareUntitled")
        } else {
            SharedString::from(single_line(&file.name))
        };
        let tip = name.clone();
        let view = cx.entity();
        check_row(
            ("share-receive-file", index),
            self.selection.is_selected(index),
            h_flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .gap(tokens.spacing.md)
                .child(
                    Icon::new(FluxIcon::File)
                        .size(theme.extended().icon.sm)
                        .text_color(tokens.colors.muted_foreground),
                )
                .child(
                    div()
                        .id(("share-receive-file-name", index))
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis_middle()
                        .tooltip(move |window, cx| tooltip_view(tip.clone(), window, cx))
                        .child(name),
                )
                .child(
                    div()
                        .flex_none()
                        .font_features(tabular_numbers())
                        .text_size(tokens.typography.xs.size)
                        .line_height(tokens.typography.xs.line_height)
                        .text_color(tokens.colors.muted_foreground)
                        .child(format_bytes(file.size)),
                ),
            move |_, _, cx| {
                view.update(cx, |this, cx| {
                    this.selection.toggle(index);
                    cx.notify();
                });
            },
            cx,
        )
        .w_full()
        .h(theme.density().control)
        .into_any_element()
    }

    fn render_files(
        &self,
        preview: &ShareResolveDto,
        translator: &fluxdown_ui_i18n::Translator,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = active_theme(cx);
        let tokens = theme.tokens();
        let extended = theme.extended();
        let count = preview.files.len();
        let visible_rows = count.clamp(1, FILE_LIST_MAX_ROWS);
        let list_height = theme.density().control * visible_rows as f32;
        let all = self.selection.all_selected();
        let view = cx.entity();
        let summary = translator.text_with(
            "shareSelectedSummary",
            &[
                ("count", &self.selection.selected_count().to_string()),
                (
                    "size",
                    &format_bytes(self.selection.selected_bytes(&preview.files)),
                ),
            ],
        );
        v_flex()
            .w_full()
            .gap(tokens.spacing.xs)
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .gap(tokens.spacing.md)
                    .child(
                        check_row(
                            "share-receive-select-all",
                            all,
                            text(translator, "shareSelectAll"),
                            move |_, _, cx| view.update(cx, |this, cx| this.toggle_all(cx)),
                            cx,
                        )
                        .flex_none(),
                    )
                    .child(
                        ellipsized(summary)
                            .font_features(tabular_numbers())
                            .text_size(tokens.typography.xs.size)
                            .line_height(tokens.typography.xs.line_height)
                            .text_color(tokens.colors.muted_foreground),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .h(list_height)
                    .overflow_hidden()
                    .rounded(tokens.radius.md)
                    .border(extended.stroke.thin)
                    .border_color(extended.colors.hairline)
                    .child(
                        uniform_list(
                            ("share-receive-files", self.generation as usize),
                            count,
                            cx.processor(move |this, range: Range<usize>, _, cx| {
                                range
                                    .map(|index| this.render_file_row(index, cx))
                                    .collect::<Vec<_>>()
                            }),
                        )
                        .size_full(),
                    ),
            )
    }

    fn render_sources(
        &self,
        preview: &ShareResolveDto,
        translator: &fluxdown_ui_i18n::Translator,
        cx: &Context<Self>,
    ) -> Div {
        let tokens = active_theme(cx).tokens();
        v_flex()
            .w_full()
            .gap(tokens.spacing.xs + tokens.spacing.xxs)
            .child(field_label(text(translator, "shareSourcesTitle"), cx))
            .child(
                h_flex()
                    .w_full()
                    .flex_wrap()
                    .gap_x(tokens.spacing.lg)
                    .gap_y(tokens.spacing.xs)
                    .children(source_lines(&preview.sources).into_iter().map(|line| {
                        h_flex()
                            .items_center()
                            .gap(tokens.spacing.xs + tokens.spacing.xxs)
                            .text_size(tokens.typography.sm.size)
                            .line_height(tokens.typography.sm.line_height)
                            .text_color(tokens.colors.foreground)
                            .child(
                                div()
                                    .flex_none()
                                    .size(tokens.spacing.sm)
                                    .rounded(tokens.radius.full)
                                    .bg(source_color(line.tone, cx)),
                            )
                            .child(match line.count {
                                Some(count) => SharedString::from(
                                    translator
                                        .text_with(line.key, &[("count", &count.to_string())]),
                                ),
                                None => text(translator, line.key),
                            })
                    })),
            )
    }

    fn render_save_dir(
        &self,
        translator: &fluxdown_ui_i18n::Translator,
        cx: &Context<Self>,
    ) -> Div {
        let theme = active_theme(cx);
        let tokens = theme.tokens();
        let extended = theme.extended();
        let custom = self
            .save_dir
            .as_ref()
            .map(|dir| SharedString::from(single_line(dir)));
        let shown = custom
            .clone()
            .unwrap_or_else(|| text(translator, "shareSaveDefault"));
        let tip = shown.clone();
        let is_custom = custom.is_some();
        let reset_label = text(translator, "shareSaveDefault");
        let busy = self.busy;
        v_flex()
            .w_full()
            .gap(tokens.spacing.xs + tokens.spacing.xxs)
            .child(field_label(text(translator, "shareSaveTo"), cx))
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(tokens.spacing.sm)
                    .child(
                        div()
                            .id("share-receive-save-dir")
                            .flex_1()
                            .min_w_0()
                            .h(theme.density().control)
                            .flex()
                            .items_center()
                            .px(tokens.spacing.sm + tokens.spacing.xxs)
                            .rounded(theme.components().button_radius)
                            .border(extended.stroke.thin)
                            .border_color(tokens.colors.border)
                            .bg(tokens.colors.muted)
                            .text_size(tokens.typography.sm.size)
                            .line_height(tokens.typography.sm.line_height)
                            .text_color(if is_custom {
                                tokens.colors.foreground
                            } else {
                                tokens.colors.muted_foreground
                            })
                            .tooltip(move |window, cx| tooltip_view(tip.clone(), window, cx))
                            .child(
                                div()
                                    .min_w_0()
                                    .w_full()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis_middle()
                                    .child(shown),
                            ),
                    )
                    .when(is_custom, |this| {
                        this.child(
                            Button::new("share-receive-save-reset")
                                .ghost()
                                .icon(FluxIcon::X)
                                .tooltip(reset_label)
                                .control(cx)
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.save_dir = None;
                                    cx.notify();
                                })),
                        )
                    })
                    .child(
                        Button::new("share-receive-choose-dir")
                            .outline()
                            .icon(FluxIcon::Folder)
                            .label(text(translator, "shareChooseFolder"))
                            .control(cx)
                            .disabled(busy || self.picking_dir)
                            .on_click(cx.listener(|this, _, window, cx| this.pick_dir(window, cx))),
                    ),
            )
    }

    fn render_preview(
        &self,
        preview: &ShareResolveDto,
        translator: &fluxdown_ui_i18n::Translator,
        stale: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let tokens = active_theme(cx).tokens().clone();
        let sender = sender_name(&preview.sender_device_name).map_or_else(
            || text(translator, "shareSenderUnknown"),
            |name| SharedString::from(single_line(name)),
        );
        let from = SharedString::from(
            translator.text_with("shareFromDevice", &[("device", sender.as_ref())]),
        );
        let title = {
            let headline = single_line(preview.title.trim());
            if headline.trim().is_empty() {
                preview
                    .files
                    .first()
                    .filter(|file| !file.name.trim().is_empty())
                    .map_or_else(
                        || text(translator, "shareUntitled"),
                        |file| SharedString::from(single_line(&file.name)),
                    )
            } else {
                SharedString::from(headline)
            }
        };
        let meta = SharedString::from(format!(
            "{} · {} · {}",
            from,
            translator.text_with(
                "shareFilesCount",
                &[("count", &preview.files.len().to_string())]
            ),
            format_bytes(preview.total_bytes),
        ));
        let expires = SharedString::from(translator.text_with(
            "shareExpiresAt",
            &[("time", &format_datetime(&preview.expires_at))],
        ));
        let can_start = self.selection.any_selected() && !self.busy && !self.picking_dir && !stale;
        let retry = self.error.is_some();
        let busy = self.busy;

        let files = self.render_files(preview, translator, cx);
        let sources = self.render_sources(preview, translator, cx);
        let save_dir = self.render_save_dir(translator, cx);
        let error = self.render_error(translator, cx);
        let tokens_body = &tokens;

        v_flex()
            .size_full()
            .min_h_0()
            .child(window_ui::header(
                if preview.files.len() > 1 {
                    FluxIcon::Folders
                } else {
                    FluxIcon::File
                },
                title,
                Some(meta),
                None,
                text(translator, "close"),
                cx,
            ))
            .child(
                div().flex_1().min_h_0().w_full().child(
                    div()
                        .id("share-receive-body")
                        .size_full()
                        .overflow_y_scrollbar()
                        .child(
                            v_flex()
                                .w_full()
                                .gap(tokens_body.spacing.lg)
                                .p(tokens_body.spacing.lg)
                                .child(
                                    ellipsized(expires)
                                        .text_size(tokens.typography.xs.size)
                                        .line_height(tokens.typography.xs.line_height)
                                        .text_color(tokens.colors.muted_foreground),
                                )
                                .child(files)
                                .child(sources)
                                .child(save_dir)
                                .when_some(error, |this, error| this.child(error))
                                .when_some(self.notice.clone(), |this, notice| {
                                    this.child(field_error(notice, cx))
                                })
                                .when(stale, |this| {
                                    this.child(field_hint(
                                        text(translator, "localServiceDisconnected"),
                                        cx,
                                    ))
                                }),
                        ),
                ),
            )
            .child(
                window_ui::footer_frame(cx)
                    .child({
                        let hub = self.hub.clone();
                        let share_id = preview.share_id.clone();
                        Button::new("share-receive-report")
                            .ghost()
                            .label(text(translator, "shareReport"))
                            .control(cx)
                            .disabled(stale || share_id.is_empty())
                            .on_click(move |_, window, cx| {
                                open_report_dialog(&hub, share_id.clone(), window, cx);
                            })
                    })
                    .child(
                        h_flex()
                            .gap(tokens.spacing.sm)
                            .child(
                                Button::new("share-receive-cancel")
                                    .outline()
                                    .label(text(translator, "cancel"))
                                    .control(cx)
                                    .disabled(busy && self.pending_id.is_none())
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.cancel(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("share-receive-start")
                                    .primary()
                                    .label(text(
                                        translator,
                                        if retry {
                                            "shareRetry"
                                        } else {
                                            "shareStartReceive"
                                        },
                                    ))
                                    .control(cx)
                                    .busy(busy)
                                    .disabled(!can_start && !(retry && !busy && !stale))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if this.error.is_some() {
                                            this.retry(cx);
                                        } else {
                                            this.start_receive(cx);
                                        }
                                    })),
                            ),
                    ),
            )
    }
}

impl Render for ShareReceiveView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let translator = self.hub.read(cx).translator().read(cx).clone();
        let stale = self.hub.read(cx).is_stale();
        self.sync_placeholder(text(&translator, "sharePickupPlaceholder"), window, cx);
        let tokens = active_theme(cx).tokens().clone();
        let extended_colors = active_theme(cx).extended().colors;
        let root = v_flex()
            .size_full()
            .min_h_0()
            .bg(tokens.colors.surface)
            .text_color(tokens.colors.foreground);
        match self.phase {
            Phase::Input => root.child(self.render_input_phase(&translator, stale, cx)),
            Phase::Resolving => root.child(self.render_resolving(&translator, cx)),
            Phase::Preview => match self.preview.clone() {
                Some(preview) => root.child(self.render_preview(&preview, &translator, stale, cx)),
                None => root,
            },
            Phase::Started(count) => root
                .child(window_ui::header(
                    FluxIcon::Share2,
                    text(&translator, "shareReceiveTitle"),
                    None,
                    None,
                    text(&translator, "close"),
                    cx,
                ))
                .child(
                    div().flex_1().min_h_0().child(window_ui::notice(
                        FluxIcon::CircleCheck,
                        extended_colors.success,
                        SharedString::from(
                            translator
                                .text_with("shareReceiveStarted", &[("count", &count.to_string())]),
                        ),
                        cx,
                    )),
                )
                .child(
                    window_ui::footer_frame(cx).child(div()).child(
                        Button::new("share-receive-done")
                            .primary()
                            .label(text(&translator, "close"))
                            .control(cx)
                            .on_click(|_, window, _| window.remove_window()),
                    ),
                ),
            Phase::Gone => root
                .child(window_ui::header(
                    FluxIcon::Share2,
                    text(&translator, "shareReceiveTitle"),
                    None,
                    None,
                    text(&translator, "close"),
                    cx,
                ))
                .child(div().flex_1().min_h_0().child(window_ui::notice(
                    FluxIcon::CircleAlert,
                    extended_colors.warning,
                    text(&translator, "shareReceiveGone"),
                    cx,
                )))
                .child(
                    window_ui::footer_frame(cx).child(div()).child(
                        Button::new("share-receive-gone-close")
                            .outline()
                            .label(text(&translator, "close"))
                            .control(cx)
                            .on_click(|_, window, _| window.remove_window()),
                    ),
                ),
        }
    }
}
