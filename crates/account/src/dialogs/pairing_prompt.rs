//! 入站局域网配对确认框：对端输入了本机配对码后，双方核对 SAS，本机确认或拒绝。
//!
//! 由 app 在收到 `AccountHostEvent::PairingRequested` 时于某个窗口弹出；请求被对端取消 /
//! 过期 / 已被其他窗口处理（从快照消失）时自动关闭。必须明确选择：不响应 Esc 与点击遮罩。

use fluxdown_protocol::{LinkApproveParams, LinkPairingRequestDto, method};
use fluxdown_ui_components::{BusyExt as _, ControlExt as _, field_error, field_hint};
use fluxdown_ui_theme::active_theme;
use gpui::{
    App, AppContext as _, ClickEvent, Context, Entity, FontWeight, Global, IntoElement,
    ParentElement, Render, SharedString, Styled, Window, div, prelude::FluentBuilder as _,
};
use gpui_component::{
    Disableable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use std::collections::VecDeque;

use crate::errors::{ErrorContext, error_text};
use crate::host::AccountHost;
use crate::link::{group_sas, now_unix_ms, seconds_until};
use crate::verification::spawn_ticker;
use crate::{AccountCommand, t, t_with};

struct PairingPrompt {
    host: Entity<AccountHost>,
    request: LinkPairingRequestDto,
    /// 本机截止时刻（收到时刻 + `ttlMs`）；到点自动拒绝。
    deadline_ms: i64,
    remaining: u64,
    busy: bool,
    /// 已向 agent 提交过决定（含超时自动拒绝），不再重复提交。
    answered: bool,
    error: Option<SharedString>,
}

/// 同一时刻只显示一个确认框：gpui-component 的 `close_dialog` 只能弹出栈顶，
/// 多个请求并存时无法按句柄关闭指定弹窗，所以其余请求排队，前一个关闭后再依次打开。
#[derive(Default)]
struct PromptQueue {
    active: Option<String>,
    waiting: VecDeque<String>,
}

impl Global for PromptQueue {}

/// 为入站请求 `session_id` 打开确认框（已有确认框时排队）；请求已经不在快照里则什么也不做。
pub fn open(host: &Entity<AccountHost>, session_id: &str, window: &mut Window, cx: &mut App) {
    if host.read(cx).pairing_request(session_id).is_none() {
        return;
    }
    let queue = cx.default_global::<PromptQueue>();
    if queue.active.as_deref() == Some(session_id) {
        return;
    }
    if queue.active.is_some() {
        if !queue.waiting.iter().any(|id| id == session_id) {
            queue.waiting.push_back(session_id.to_owned());
        }
        return;
    }
    open_now(host, session_id, window, cx);
}

fn open_now(host: &Entity<AccountHost>, session_id: &str, window: &mut Window, cx: &mut App) {
    let Some(request) = host.read(cx).pairing_request(session_id) else {
        return;
    };
    cx.default_global::<PromptQueue>().active = Some(session_id.to_owned());
    let title = t(host.read(cx).translator().read(cx), "incomingPairingTitle");
    let view = cx.new(|cx| PairingPrompt::new(host.clone(), request, window, cx));
    window.open_dialog(cx, move |dialog, _, cx| {
        let view = view.clone();
        dialog
            .title(fluxdown_ui_components::dialog_title(title.clone(), cx))
            .w(active_theme(cx).text_extent(480.))
            .close_button(false)
            .overlay_closable(false)
            .keyboard(false)
            .content(move |content, _, _| content.child(view.clone()))
    });
}

impl PairingPrompt {
    fn new(
        host: Entity<AccountHost>,
        request: LinkPairingRequestDto,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let deadline_ms = host
            .read(cx)
            .pairing_deadline_ms(&request.session_id)
            .unwrap_or(request.expires_at_unix_ms);
        let remaining = seconds_until(deadline_ms, now_unix_ms());
        let session_id = request.session_id.clone();
        // 承载窗口被关闭 / 实体被释放而请求仍未处理：清空队列的 active，否则之后所有请求都只会排队；
        // 并把本请求与排队中的请求重新抛给 app，在仍存在（或重新打开）的窗口里再弹。
        let released_id = session_id.clone();
        let released_host = host.clone();
        cx.on_release(move |this: &mut Self, cx| {
            if this.answered {
                return;
            }
            let queue = cx.default_global::<PromptQueue>();
            if queue.active.as_deref() == Some(released_id.as_str()) {
                queue.active = None;
            }
            let mut ids = vec![released_id.clone()];
            ids.extend(queue.waiting.drain(..));
            released_host.update(cx, |host, cx| host.reemit_pairing_requests(ids, cx));
        })
        .detach();
        cx.observe_in(&host, window, move |this, host, window, cx| {
            if host.read(cx).pairing_request(&session_id).is_none() {
                this.finish(window, cx);
            }
        })
        .detach();
        spawn_ticker(window, cx, |this: &mut Self, window, cx| {
            this.remaining = seconds_until(this.deadline_ms, now_unix_ms());
            // 到期仍无人处理：自动拒绝（agent 侧也会过期，这里让请求方立刻得到结果）。
            if this.remaining == 0 {
                this.respond(false, window, cx);
            }
            cx.notify();
            !this.answered
        });
        Self {
            host,
            request,
            deadline_ms,
            remaining,
            busy: false,
            answered: false,
            error: None,
        }
    }

    fn t(&self, key: &str, cx: &App) -> SharedString {
        t(self.host.read(cx).translator().read(cx), key)
    }

    /// 关闭自己的确认框并轮到队列里的下一个请求；幂等。
    fn finish(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.answered {
            return;
        }
        self.answered = true;
        window.close_dialog(cx);
        cx.default_global::<PromptQueue>().active = None;
        while let Some(next) = cx.default_global::<PromptQueue>().waiting.pop_front() {
            open_now(&self.host, &next, window, cx);
            if cx.default_global::<PromptQueue>().active.is_some() {
                break;
            }
        }
    }

    fn respond(&mut self, accept: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.answered {
            return;
        }
        self.busy = true;
        self.error = None;
        cx.notify();
        if accept {
            // 批准只是提交本机决定；「已配对」提示等已配对列表出现对端指纹后再给。
            let fingerprint = self.request.peer_fingerprint.clone();
            let name = self.request.peer_name.clone();
            self.host
                .update(cx, |host, _| host.await_pairing(&fingerprint, &name));
        }
        let params = serde_json::to_value(LinkApproveParams {
            session_id: self.request.session_id.clone(),
            accept,
        })
        .unwrap_or_default();
        let future = self.host.read(cx).port().execute(AccountCommand::Link {
            method: method::AGENT_LINK_APPROVE,
            params,
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = future.await;
            let Ok(()) = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(_) => {
                        this.finish(window, cx);
                    }
                    Err(error) => {
                        if accept {
                            let fingerprint = this.request.peer_fingerprint.clone();
                            this.host
                                .update(cx, |host, _| host.cancel_awaited_pairing(&fingerprint));
                        }
                        this.error = Some(error_text(
                            this.host.read(cx).translator().read(cx),
                            &error,
                            ErrorContext::Pairing,
                        ));
                    }
                }
                cx.notify();
            }) else {
                // 对话框或窗口已释放，停止回写异步结果。
                return;
            };
        })
        .detach();
    }
}

impl Render for PairingPrompt {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = active_theme(cx).tokens().clone();
        let extended = active_theme(cx).extended().clone();
        let translator = self.host.read(cx).translator().read(cx).clone();
        let from = t_with(
            &translator,
            "incomingPairingFrom",
            &[("device", &self.request.peer_name)],
        );
        let countdown = t_with(
            &translator,
            "incomingPairingCountdown",
            &[("seconds", &self.remaining.to_string())],
        );
        let platform = self.request.peer_platform.clone();
        v_flex()
            .w_full()
            .gap(tokens.spacing.lg)
            .child(
                v_flex()
                    .gap(tokens.spacing.xxs)
                    .child(
                        div()
                            .text_size(tokens.typography.sm.size)
                            .line_height(tokens.typography.sm.line_height)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(tokens.colors.foreground)
                            .child(from),
                    )
                    .when_some(platform, |column, platform| {
                        column.child(field_hint(platform, cx))
                    })
                    .child(field_hint(self.t("incomingPairingHint", cx), cx)),
            )
            .when(self.request.requests_control, |column| {
                column.child(crate::ui::notice(
                    crate::ui::NoticeTone::Danger,
                    SharedString::from(format!(
                        "{}\n{}",
                        self.t("linkControlRequestTag", cx),
                        self.t("linkControlRequestWarning", cx)
                    )),
                    cx,
                ))
            })
            .child(
                div()
                    .w_full()
                    .flex()
                    .justify_center()
                    .py(tokens.spacing.md)
                    .rounded(tokens.radius.md)
                    .bg(tokens.colors.muted)
                    .text_size(extended.title.size * 2.)
                    .line_height(extended.title.line_height * 2.)
                    .font_weight(FontWeight::SEMIBOLD)
                    .font_features(fluxdown_ui_components::tabular_numbers())
                    .text_color(tokens.colors.foreground)
                    .child(group_sas(&self.request.sas)),
            )
            .child(field_hint(countdown, cx))
            .when_some(self.error.clone(), |column, error| {
                column.child(field_error(error, cx))
            })
            .child(
                h_flex()
                    .w_full()
                    .pt(tokens.spacing.sm)
                    .justify_end()
                    .gap(tokens.spacing.sm)
                    .child(
                        Button::new("incoming-pairing-reject")
                            .outline()
                            .label(self.t("incomingPairingReject", cx))
                            .control(cx)
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.respond(false, window, cx);
                            })),
                    )
                    .child(
                        Button::new("incoming-pairing-accept")
                            .primary()
                            .label(if self.request.requests_control {
                                self.t("linkControlApprove", cx)
                            } else {
                                self.t("incomingPairingAccept", cx)
                            })
                            .control(cx)
                            .busy(self.busy)
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.respond(true, window, cx);
                            })),
                    ),
            )
    }
}
