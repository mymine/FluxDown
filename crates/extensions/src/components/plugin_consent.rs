//! 插件授权对话框：列出请求的权限与网络主机、风险等级；`full` 风险需勾选「我已了解」。
//!
//! 同一组件服务两处：
//! - [`ConsentAction::Grant`]：已安装但待授权的插件，确定后调用 `daemon.plugin.consent`；
//! - [`ConsentAction::Confirm`]：市场安装 / 更新前的确认，确定后交给调用方继续安装。

use std::{rc::Rc, sync::Arc};

use fluxdown_protocol::PluginDto;
use fluxdown_ui_components::{
    BusyExt as _, ControlExt as _, FluxIcon, check_row, dialog_scroll_body,
};
use fluxdown_ui_i18n::Translator;
use fluxdown_ui_theme::active_theme;
use gpui::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement, Render, SharedString,
    Styled, Window, prelude::FluentBuilder as _,
};
use gpui_component::{
    Disableable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::DialogFooter,
    v_flex,
};

use super::plugin_detail::permission_label;
use crate::{ExtensionsPort, controller::grant_plugin_consent, error_text, pages::Frame, ui};

/// 对话框展示内容。
pub struct ConsentRequest {
    pub title: SharedString,
    pub intro: SharedString,
    pub ok_label: SharedString,
    /// `standard|elevated|full`。
    pub risk: String,
    pub permissions: Vec<String>,
    pub network_hosts: Vec<String>,
}

impl ConsentRequest {
    /// 已安装插件的授权请求：展示 pending 部分（插件当前请求集合与已授权集合的差）。
    pub fn for_plugin(plugin: &PluginDto, translator: &Translator) -> Self {
        Self {
            title: translator
                .text_with("pluginConsentTitle", &[("name", &plugin.name)])
                .into(),
            intro: translator.text("pluginConsentIntro").to_owned().into(),
            ok_label: translator.text("pluginConsentGrant").to_owned().into(),
            risk: plugin.risk.clone(),
            permissions: plugin.consent_pending.clone(),
            network_hosts: plugin.consent_pending_hosts.clone(),
        }
    }
}

/// 确认回调：返回 `false` 表示未提交（对话框保持打开）。
pub type ConfirmCallback = Rc<dyn Fn(&mut Window, &mut App) -> bool>;

/// 点击确定后的动作。
pub enum ConsentAction {
    /// 调用 `daemon.plugin.consent`（参数为插件当前完整请求集合）。
    Grant {
        port: Arc<dyn ExtensionsPort>,
        plugin: Box<PluginDto>,
    },
    /// 交给调用方继续（如市场安装）。
    Confirm(ConfirmCallback),
}

/// 只有 `full` 风险需要额外勾选确认。
pub fn needs_acknowledge(risk: &str) -> bool {
    risk == "full"
}

struct ConsentDialog {
    translator: Entity<Translator>,
    request: ConsentRequest,
    action: ConsentAction,
    acknowledged: bool,
    granting: bool,
    error: Option<SharedString>,
}

impl ConsentDialog {
    fn can_confirm(&self) -> bool {
        !self.granting && (!needs_acknowledge(&self.request.risk) || self.acknowledged)
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_confirm() {
            return;
        }
        match &self.action {
            ConsentAction::Confirm(on_confirm) => {
                if on_confirm(window, cx) {
                    window.close_dialog(cx);
                }
            }
            ConsentAction::Grant { port, plugin } => {
                self.granting = true;
                self.error = None;
                cx.notify();
                let future = grant_plugin_consent(port, plugin);
                cx.spawn_in(window, async move |this, cx| {
                    let result = future.await;

                    let Ok(()) = this.update_in(cx, |this, window, cx| {
                        this.granting = false;
                        match result {
                            Ok(_) => window.close_dialog(cx),
                            Err(error) => {
                                let translator = this.translator.read(cx);
                                let message = error_text(translator, &error);
                                this.error = Some(
                                    translator
                                        .text_with("pluginConsentFailed", &[("message", &message)])
                                        .into(),
                                );
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
    }
}

impl Render for ConsentDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = active_theme(cx);
        let translator = self.translator.read(cx);
        let frame = Frame {
            translator,
            tokens: theme.tokens(),
            extended: theme.extended(),
            components: theme.components(),
            stale: false,
        };
        let tokens = frame.tokens;
        let request = &self.request;
        let full = needs_acknowledge(&request.risk);
        let state = cx.entity();

        let permission_rows = request
            .permissions
            .iter()
            .map(|permission| {
                let (name, description) = permission_label(translator, permission);
                v_flex()
                    .w_full()
                    .gap(tokens.spacing.xxs)
                    .child(ui::title_text(name, frame))
                    .child(ui::meta_text(description, frame))
            })
            .collect::<Vec<_>>();
        let hosts = if request.network_hosts.is_empty() {
            vec![ui::meta_text(
                translator.text("pluginConsentNoNetworkHosts").to_owned(),
                frame,
            )]
        } else {
            request
                .network_hosts
                .iter()
                .map(|host| ui::body_text(host.clone(), frame))
                .collect()
        };

        v_flex()
            .w_full()
            .gap(tokens.spacing.sm)
            .child(ui::meta_text(request.intro.clone(), frame))
            .children(ui::risk_pill("plugin-consent-risk", &request.risk, frame))
            .children(permission_rows)
            .child(ui::title_text(
                translator.text("pluginConsentNetworkHosts").to_owned(),
                frame,
            ))
            .children(hosts)
            .when(full, |this| {
                this.child(ui::status_line(
                    FluxIcon::CircleAlert,
                    tokens.colors.destructive,
                    translator.text("pluginConsentFullTrustWarning").to_owned(),
                    frame,
                ))
                .child(ui::meta_text(
                    translator.text("pluginConsentNativeNotAudited").to_owned(),
                    frame,
                ))
                .child(check_row(
                    "plugin-consent-ack",
                    self.acknowledged,
                    translator.text("pluginConsentAcknowledge").to_owned(),
                    move |checked, _, cx| {
                        state.update(cx, |this, cx| {
                            this.acknowledged = checked;
                            cx.notify();
                        });
                    },
                    cx,
                ))
            })
            .when_some(self.error.clone(), |this, error| {
                this.child(ui::status_line(
                    FluxIcon::CircleAlert,
                    tokens.colors.destructive,
                    error,
                    frame,
                ))
            })
    }
}

/// 打开授权对话框。
pub fn open_consent_dialog(
    request: ConsentRequest,
    action: ConsentAction,
    translator: Entity<Translator>,
    window: &mut Window,
    cx: &mut App,
) {
    let cancel = SharedString::from(translator.read(cx).text("cancel").to_owned());
    let title = request.title.clone();
    let ok_label = request.ok_label.clone();
    let state = cx.new(|_| ConsentDialog {
        translator,
        request,
        action,
        acknowledged: false,
        granting: false,
        error: None,
    });
    window.open_dialog(cx, move |dialog, _, cx| {
        let (granting, can_confirm) = {
            let state = state.read(cx);
            (state.granting, state.can_confirm())
        };
        let content = state.clone();
        let confirm = state.clone();
        dialog
            .title(fluxdown_ui_components::dialog_title(title.clone(), cx))
            .w(active_theme(cx).text_extent(520.))
            .overlay_closable(!granting)
            .content(move |root, _, cx| {
                root.min_h_0().child(dialog_scroll_body(
                    "plugin-consent-body",
                    None,
                    content.clone(),
                    cx,
                ))
            })
            .footer(
                DialogFooter::new()
                    .gap(active_theme(cx).tokens().spacing.sm)
                    .child(
                        Button::new("plugin-consent-cancel")
                            .outline()
                            .control(cx)
                            .label(cancel.clone())
                            .disabled(granting)
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("plugin-consent-ok")
                            .primary()
                            .control(cx)
                            .label(ok_label.clone())
                            .busy(granting)
                            .disabled(!can_confirm)
                            .on_click(move |_, window, cx| {
                                confirm.update(cx, |this, cx| this.confirm(window, cx));
                            }),
                    ),
            )
    });
}

#[cfg(test)]
mod tests {
    use super::needs_acknowledge;

    #[test]
    fn only_full_trust_requires_acknowledgement() {
        assert!(needs_acknowledge("full"));
        assert!(!needs_acknowledge("elevated"));
        assert!(!needs_acknowledge("standard"));
        assert!(!needs_acknowledge(""));
    }
}
