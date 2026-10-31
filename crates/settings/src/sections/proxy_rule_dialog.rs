//! 站点代理规则新增 / 编辑对话框：字段、服务端检测与保存。
//!
//! 检测走 `daemon.config.proxyRoute`（引擎是唯一的校验 / 匹配实现，客户端不在本地判断）：
//! 失焦与「检测」只发送草稿一条规则；保存前发送保存后的完整列表，有 issue 就不写入。

use fluxdown_protocol::proxy_rules::{ProxyRuleSpec, encode_proxy_rules, new_proxy_rule};
use fluxdown_protocol::{ProxyRouteResponse, RpcErrorData, method};
use fluxdown_ui_components::{
    BusyHold, ButtonVariant, ControlExt as _, FluxIcon, choice_chip, dialog_scroll_body,
    dialog_title, field_error, form, form_field, input_with_action, loading_button, segmented_tabs,
};
use fluxdown_ui_i18n::Translator;
use fluxdown_ui_theme::active_theme;
use gpui::{
    App, AppContext as _, ClickEvent, Context, Div, Entity, IntoElement, ParentElement, Render,
    SharedString, Styled, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    Icon, WindowExt as _,
    button::Button,
    h_flex,
    input::{Input, InputEvent, InputState},
    v_flex,
};
use serde_json::{Value, json};

use super::category_dialog::unix_ms;
use super::proxy_rules::{
    confirm_delete, issue_text, manual_endpoint, manual_incomplete, mutate_rules, read_rules,
    upsert,
};
use crate::store::{SettingsStore, rpc_error_text};
use crate::ui::{danger_ghost_button, dialog_footer};

#[derive(Clone, Copy, PartialEq, Eq)]
enum MatchKind {
    Domain,
    Regex,
}

impl MatchKind {
    fn wire(self) -> &'static str {
        match self {
            Self::Domain => "domain",
            Self::Regex => "regex",
        }
    }

    fn from_wire(wire: &str) -> Self {
        if wire == "regex" {
            Self::Regex
        } else {
            Self::Domain
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    Direct,
    System,
    Manual,
    Custom,
}

impl Action {
    const ALL: [Self; 4] = [Self::Direct, Self::System, Self::Manual, Self::Custom];

    fn wire(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::System => "system",
            Self::Manual => "manual",
            Self::Custom => "custom",
        }
    }

    fn from_wire(wire: &str) -> Self {
        match wire {
            "system" => Self::System,
            "manual" => Self::Manual,
            "custom" => Self::Custom,
            _ => Self::Direct,
        }
    }

    fn label_key(self) -> &'static str {
        match self {
            Self::Direct => "proxyRuleActionDirect",
            Self::System => "proxyRuleActionSystem",
            Self::Manual => "proxyRuleActionManual",
            Self::Custom => "proxyRuleActionCustom",
        }
    }
}

/// 一次检测发出时的上下文，回调里用来判定结果是否仍然有效。
struct CheckTicket {
    then_save: bool,
    /// 草稿在被检测列表中的下标。
    position: usize,
    /// 发出时的输入代数；期间有编辑则丢弃结果。
    generation: u64,
    has_sample: bool,
    draft: ProxyRuleSpec,
}

pub(crate) struct ProxyRuleDialog {
    store: Entity<SettingsStore>,
    translator: Translator,
    existing: Option<ProxyRuleSpec>,
    /// 新建时在 `open` 里生成一次，之后每次检测与保存都用它。
    draft_id: String,
    enabled: bool,
    name: Entity<InputState>,
    pattern: Entity<InputState>,
    proxy: Entity<InputState>,
    sample: Entity<InputState>,
    match_kind: MatchKind,
    action: Action,
    name_error: Option<SharedString>,
    pattern_error: Option<SharedString>,
    proxy_error: Option<SharedString>,
    form_error: Option<SharedString>,
    try_result: Option<bool>,
    /// 进行中的检测：`Some(true)` 为保存前检测，`Some(false)` 为「试一试」；转圈只落在发起的按钮上。
    checking: Option<bool>,
    /// 「试一试」的最短可见转圈（本机路由判定常在一帧内返回）。
    try_hold: BusyHold,
    /// 任一输入变化时 +1。
    edit_generation: u64,
}

/// 打开新增（`existing = None`）或编辑对话框。
pub(crate) fn open(
    store: Entity<SettingsStore>,
    translator: Translator,
    existing: Option<ProxyRuleSpec>,
    window: &mut Window,
    cx: &mut App,
) {
    let title = SharedString::from(
        translator
            .text(if existing.is_some() {
                "proxyRuleEdit"
            } else {
                "proxyRuleNew"
            })
            .to_owned(),
    );
    let view = cx.new(|cx| ProxyRuleDialog::new(store, translator, existing, window, cx));
    let pattern = view.read(cx).pattern.clone();
    window.open_dialog(cx, move |dialog, _, cx| {
        let view = view.clone();
        dialog
            .title(dialog_title(title.clone(), cx))
            .w(active_theme(cx).text_extent(560.))
            .content(move |content, _, _| content.min_h_0().child(view.clone()))
    });
    pattern.update(cx, |input, cx| input.focus(window, cx));
}

impl ProxyRuleDialog {
    fn new(
        store: Entity<SettingsStore>,
        translator: Translator,
        existing: Option<ProxyRuleSpec>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let base = existing
            .clone()
            .unwrap_or_else(|| new_proxy_rule(format!("pr_{}", unix_ms())));
        let match_kind = MatchKind::from_wire(&base.match_kind);
        let text = |key: &str| SharedString::from(translator.text(key).to_owned());
        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(base.name.clone())
                .placeholder(text("proxyRuleNamePlaceholder"))
        });
        let pattern = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(base.pattern.clone())
                .placeholder(text(pattern_placeholder_key(match_kind)))
        });
        let proxy = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(base.proxy.clone())
                .placeholder(text("proxyRuleProxyUrlPlaceholder"))
        });
        let sample =
            cx.new(|cx| InputState::new(window, cx).placeholder(text("proxyRouteUrlPlaceholder")));

        for input in [&name, &pattern, &proxy, &sample] {
            cx.subscribe_in(
                input,
                window,
                |this, input, event: &InputEvent, window, cx| {
                    this.on_input_event(input, event, window, cx);
                },
            )
            .detach();
        }

        Self {
            store,
            translator,
            draft_id: base.id.clone(),
            enabled: base.enabled,
            action: Action::from_wire(&base.action),
            existing,
            name,
            pattern,
            proxy,
            sample,
            match_kind,
            name_error: None,
            pattern_error: None,
            proxy_error: None,
            form_error: None,
            try_result: None,
            checking: None,
            try_hold: BusyHold::default(),
            edit_generation: 0,
        }
    }

    fn t(&self, key: &str) -> SharedString {
        SharedString::from(self.translator.text(key).to_owned())
    }

    fn on_input_event(
        &mut self,
        input: &Entity<InputState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => {
                self.edit_generation += 1;
                if *input == self.name {
                    self.name_error = None;
                } else if *input == self.pattern {
                    self.pattern_error = None;
                    self.try_result = None;
                } else if *input == self.proxy {
                    self.proxy_error = None;
                    self.try_result = None;
                } else {
                    self.try_result = None;
                }
                self.form_error = None;
                cx.notify();
            }
            InputEvent::Blur => {
                // 失焦检测只针对已有内容的字段，避免没填就切走时立刻报错。
                let filled = if *input == self.pattern {
                    !input.read(cx).value().trim().is_empty()
                } else if *input == self.proxy {
                    self.action == Action::Custom && !input.read(cx).value().trim().is_empty()
                } else {
                    false
                };
                if filled {
                    self.check(false, window, cx);
                }
            }
            InputEvent::PressEnter { .. } if *input == self.sample => {
                self.check(false, window, cx);
            }
            _ => {}
        }
    }

    fn draft(&self, cx: &App) -> ProxyRuleSpec {
        ProxyRuleSpec {
            id: self.draft_id.clone(),
            enabled: self.enabled,
            name: self.name.read(cx).value().trim().to_owned(),
            match_kind: self.match_kind.wire().to_owned(),
            pattern: self.pattern.read(cx).value().trim().to_owned(),
            action: self.action.wire().to_owned(),
            proxy: if self.action == Action::Custom {
                self.proxy.read(cx).value().trim().to_owned()
            } else {
                String::new()
            },
        }
    }

    fn generic_error(&self, detail: &str) -> SharedString {
        SharedString::from(
            self.translator
                .text_with("proxyRuleErrGeneric", &[("detail", detail)]),
        )
    }

    /// 向 daemon 检测草稿。`then_save` 为真时检测保存后的完整列表，通过才写入并关闭。
    ///
    /// 不使用 `SettingsStore::call_with`：它在断线时直接返回且不回调，`checking` 会一直卡住，
    /// 还会改写整页的错误 / 提示 / busy 状态。
    fn check(&mut self, then_save: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.checking.is_some() {
            return;
        }
        let mut draft = self.draft(cx);
        let sample = self.sample.read(cx).value().trim().to_owned();
        let (url, list, position) = if then_save {
            let next = upsert(read_rules(self.store.read(cx)), draft.clone());
            let position = next
                .iter()
                .position(|rule| rule.id == draft.id)
                .unwrap_or_default();
            (String::new(), next, position)
        } else {
            // 「试一试」看的是匹配，不受草稿的启用状态影响。
            draft.enabled = true;
            (sample.clone(), vec![draft.clone()], 0)
        };
        let rules = match encode_proxy_rules(&list) {
            Ok(rules) => rules,
            Err(error) => {
                self.form_error = Some(self.generic_error(&error.to_string()));
                cx.notify();
                return;
            }
        };
        let future = self.store.read(cx).raw_call(
            method::DAEMON_CONFIG_PROXY_ROUTE,
            json!({ "url": url, "rules": rules }),
        );
        let ticket = CheckTicket {
            then_save,
            position,
            generation: self.edit_generation,
            has_sample: !sample.is_empty(),
            draft: self.draft(cx),
        };
        self.checking = Some(then_save);
        if !then_save {
            self.try_hold.start(cx);
        }
        self.form_error = None;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = future.await;
            let Ok(()) = this.update_in(cx, |this, window, cx| {
                this.finish_check(result, &ticket, window, cx);
            }) else {
                // 对话框已关闭，结束回调，不再更新状态。
                return;
            };
        })
        .detach();
    }

    fn finish_check(
        &mut self,
        result: Result<Value, RpcErrorData>,
        ticket: &CheckTicket,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.checking = None;
        let response = match result {
            Ok(value) => serde_json::from_value::<ProxyRouteResponse>(value)
                .map_err(|error| self.generic_error(&error.to_string())),
            Err(error) => Err(SharedString::from(rpc_error_text(&self.translator, &error))),
        };
        let response = match response {
            Ok(response) => response,
            Err(text) => {
                self.form_error = Some(text);
                cx.notify();
                return;
            }
        };
        // 检测期间又改了内容：这份结果对应的是旧文本，丢弃。
        if ticket.generation != self.edit_generation {
            cx.notify();
            return;
        }
        self.name_error = None;
        self.pattern_error = None;
        self.proxy_error = None;
        self.form_error = None;
        self.try_result = None;
        let mut blocked = false;
        for issue in &response.issues {
            blocked = true;
            let text = SharedString::from(issue_text(&self.translator, issue));
            if issue.index as usize != ticket.position {
                if self.form_error.is_none() {
                    self.form_error =
                        Some(self.generic_error(&format!("#{} {}", issue.index + 1, issue.code)));
                }
                continue;
            }
            let slot = match issue.field.as_str() {
                "pattern" => &mut self.pattern_error,
                "proxy" => &mut self.proxy_error,
                "name" => &mut self.name_error,
                _ => &mut self.form_error,
            };
            slot.get_or_insert(text);
        }
        if !ticket.then_save && !blocked && ticket.has_sample {
            self.try_result = Some(response.matched.is_some());
        }
        if ticket.then_save && !blocked {
            let draft = ticket.draft.clone();
            self.store.update(cx, |store, cx| {
                mutate_rules(store, move |list| Some(upsert(list, draft.clone())), cx);
            });
            window.close_dialog(cx);
            return;
        }
        cx.notify();
    }

    fn set_match_kind(&mut self, kind: MatchKind, window: &mut Window, cx: &mut Context<Self>) {
        if self.match_kind == kind {
            return;
        }
        self.match_kind = kind;
        self.pattern_error = None;
        self.try_result = None;
        self.edit_generation += 1;
        let placeholder = self.t(pattern_placeholder_key(kind));
        self.pattern.update(cx, |input, cx| {
            input.set_placeholder(placeholder, window, cx)
        });
        cx.notify();
    }

    fn set_action(&mut self, action: Action, cx: &mut Context<Self>) {
        if self.action == action {
            return;
        }
        self.action = action;
        self.proxy_error = None;
        self.try_result = None;
        self.edit_generation += 1;
        cx.notify();
    }

    // ───────────────────────── 渲染 ─────────────────────────

    fn render_match_kind(&self, cx: &mut Context<Self>) -> Div {
        let view = cx.entity();
        form_field(
            self.t("proxyRuleMatch"),
            h_flex().child(segmented_tabs(
                "proxy-rule-match",
                [
                    self.t("proxyRuleMatchDomain"),
                    self.t("proxyRuleMatchRegex"),
                ],
                match self.match_kind {
                    MatchKind::Domain => 0,
                    MatchKind::Regex => 1,
                },
                move |index, window, cx| {
                    view.update(cx, |this, cx| {
                        let kind = if index == 1 {
                            MatchKind::Regex
                        } else {
                            MatchKind::Domain
                        };
                        this.set_match_kind(kind, window, cx);
                    });
                },
                cx,
            )),
            None,
            cx,
        )
    }

    fn render_pattern(&self, cx: &mut Context<Self>) -> Div {
        let (label, hint) = match self.match_kind {
            MatchKind::Domain => ("proxyRuleMatchDomain", "proxyRulePatternDomainHint"),
            MatchKind::Regex => ("proxyRuleMatchRegex", "proxyRulePatternRegexHint"),
        };
        let tokens = active_theme(cx).tokens().clone();
        form_field(
            self.t(label),
            Input::new(&self.pattern)
                .control(cx)
                .w_full()
                .font_family(tokens.typography.mono.clone()),
            Some(self.t(hint)),
            cx,
        )
        .when_some(self.pattern_error.clone(), |field, error| {
            field.child(field_error(error, cx))
        })
    }

    fn render_action(&self, cx: &mut Context<Self>) -> Div {
        let theme = active_theme(cx);
        let tokens = theme.tokens().clone();
        let extended = theme.extended().clone();
        let mut chips = h_flex().flex_wrap().gap(tokens.spacing.sm);
        for action in Action::ALL {
            chips = chips.child(
                choice_chip(
                    SharedString::from(format!("proxy-rule-action-{}", action.wire())),
                    self.t(action.label_key()),
                    self.action == action,
                    cx,
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.set_action(action, cx);
                })),
            );
        }
        let hint_line = |text: SharedString, warning: bool| {
            div()
                .text_size(tokens.typography.xs.size)
                .line_height(tokens.typography.xs.line_height)
                .text_color(if warning {
                    extended.colors.warning
                } else {
                    tokens.colors.muted_foreground
                })
                .child(text)
        };
        let hint = match self.action {
            Action::Direct => hint_line(self.t("proxyRuleActionDirectHint"), false),
            Action::System => hint_line(self.t("proxyRuleActionSystemHint"), false),
            Action::Manual => {
                let store = self.store.read(cx);
                if manual_incomplete(
                    &store.daemon_str("proxy_host"),
                    &store.daemon_str("proxy_port"),
                ) {
                    hint_line(self.t("proxyRuleActionManualMissing"), true)
                } else {
                    hint_line(
                        SharedString::from(self.translator.text_with(
                            "proxyRuleActionManualHint",
                            &[("endpoint", &manual_endpoint(store))],
                        )),
                        false,
                    )
                }
            }
            Action::Custom => div(),
        };
        form_field(
            self.t("proxyRuleAction"),
            v_flex().gap(tokens.spacing.sm).child(chips).child(hint),
            None,
            cx,
        )
    }

    fn render_proxy_url(&self, cx: &mut Context<Self>) -> Div {
        let tokens = active_theme(cx).tokens().clone();
        form_field(
            self.t("proxyRuleProxyUrl"),
            Input::new(&self.proxy)
                .control(cx)
                .w_full()
                .font_family(tokens.typography.mono.clone()),
            Some(self.t("proxyRuleProxyUrlHint")),
            cx,
        )
        .when_some(self.proxy_error.clone(), |field, error| {
            field.child(field_error(error, cx))
        })
    }

    fn render_try(&self, cx: &mut Context<Self>) -> Div {
        let theme = active_theme(cx);
        let tokens = theme.tokens().clone();
        let extended = theme.extended().clone();
        let empty = self.sample.read(cx).value().trim().is_empty();
        let trying = self.checking == Some(false) || self.try_hold.is_held();
        let action = loading_button(
            "proxy-rule-try",
            self.t("proxyRouteCheck"),
            ButtonVariant::Secondary,
            trying,
            cx,
        )
        .disabled(self.checking.is_some() || trying || empty)
        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
            this.check(false, window, cx);
        }));
        let result = self.try_result.map(|hit| {
            let (color, text) = if hit {
                (extended.colors.success, self.t("proxyRuleTryHit"))
            } else {
                (extended.colors.text_tertiary, self.t("proxyRuleTryMiss"))
            };
            h_flex()
                .gap(tokens.spacing.xs)
                .items_center()
                .text_size(tokens.typography.xs.size)
                .line_height(tokens.typography.xs.line_height)
                .text_color(color)
                .when(hit, |line| {
                    line.child(Icon::new(FluxIcon::CircleCheck).size(extended.icon.sm))
                })
                .child(text)
        });
        form_field(
            self.t("proxyRuleTry"),
            v_flex()
                .gap(tokens.spacing.sm)
                .child(input_with_action(
                    Input::new(&self.sample).control(cx).w_full(),
                    action,
                    cx,
                ))
                .children(result),
            None,
            cx,
        )
    }

    fn render_footer(&self, cx: &mut Context<Self>) -> Div {
        let delete = self.existing.is_some().then(|| {
            danger_ghost_button("proxy-rule-dialog-delete", self.t("delete"), cx)
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    if let Some(rule) = this.existing.clone() {
                        confirm_delete(
                            this.store.clone(),
                            &this.translator,
                            &rule,
                            true,
                            window,
                            cx,
                        );
                    }
                }))
                .into_any_element()
        });
        dialog_footer(
            delete,
            [
                Button::new("proxy-rule-dialog-cancel")
                    .outline()
                    .label(self.t("cancel"))
                    .control(cx)
                    .on_click(|_, window, cx| window.close_dialog(cx))
                    .into_any_element(),
                loading_button(
                    "proxy-rule-dialog-save",
                    self.t("proxyRuleSave"),
                    ButtonVariant::Primary,
                    self.checking == Some(true),
                    cx,
                )
                .disabled(self.checking.is_some())
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.check(true, window, cx);
                }))
                .into_any_element(),
            ],
            cx,
        )
    }
}

fn pattern_placeholder_key(kind: MatchKind) -> &'static str {
    match kind {
        MatchKind::Domain => "proxyRulePatternDomainPlaceholder",
        MatchKind::Regex => "proxyRulePatternRegexPlaceholder",
    }
}

impl Render for ProxyRuleDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = active_theme(cx).tokens().clone();
        let mut body = form(cx)
            .child(form_field(
                self.t("proxyRuleName"),
                Input::new(&self.name).control(cx).w_full(),
                None,
                cx,
            ))
            .child(self.render_match_kind(cx))
            .child(self.render_pattern(cx))
            .child(self.render_action(cx));
        if self.action == Action::Custom {
            body = body.child(self.render_proxy_url(cx));
        }
        body = body.child(self.render_try(cx));
        if let Some(error) = self.name_error.clone() {
            body = body.child(field_error(error, cx));
        }
        div()
            .flex()
            .flex_col()
            .w_full()
            .min_h_0()
            .gap(tokens.spacing.lg)
            .child(dialog_scroll_body(
                "proxy-rule-dialog-body",
                Some(px(480.)),
                body,
                cx,
            ))
            .when_some(self.form_error.clone(), |this, error| {
                this.child(field_error(error, cx))
            })
            .child(self.render_footer(cx).pt(tokens.spacing.sm))
    }
}
