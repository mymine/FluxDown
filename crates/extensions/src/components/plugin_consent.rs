//! 插件授权对话框：列出请求的权限与网络主机、风险等级；`full` 风险需勾选「我已了解」。
//!
//! 同一组件服务两处：
//! - [`ConsentAction::Grant`]：已安装但待授权的插件，确定后调用 `daemon.plugin.consent`；
//! - [`ConsentAction::Confirm`]：市场安装 / 更新前的确认，确定后交给调用方继续安装。

use std::{rc::Rc, sync::Arc};

use fluxdown_protocol::{PluginDto, PluginPageScriptDeclDto};
use fluxdown_ui_components::{
    BusyExt as _, ControlExt as _, FluxIcon, check_row, dialog_scroll_body,
};
use fluxdown_ui_i18n::Translator;
use fluxdown_ui_theme::active_theme;
use gpui::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement, Render, SharedString,
    Styled, Subscription, Window, prelude::FluentBuilder as _,
};
use gpui_component::{
    Disableable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::DialogFooter,
    v_flex,
};

use super::{
    plugin_detail::{page_script_rows, permission_label},
    plugin_live::LivePlugins,
};
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
    /// 插件声明的页面脚本（标题 + 匹配站点）；市场安装前确认不携带。
    pub page_scripts: Vec<PluginPageScriptDeclDto>,
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
            page_scripts: plugin.contributions.page_scripts.clone(),
        }
    }
}

/// 确认回调：返回 `false` 表示未提交（对话框保持打开）。
pub type ConfirmCallback = Rc<dyn Fn(&mut Window, &mut App) -> bool>;

/// 点击确定后的动作。
pub enum ConsentAction {
    /// 调用 `daemon.plugin.consent`（参数为插件当前完整请求集合）；`live` 让对话框跟随最新列表。
    Grant {
        port: Arc<dyn ExtensionsPort>,
        plugin: Box<PluginDto>,
        live: Entity<LivePlugins>,
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
    /// 已被跟随逻辑关闭；异步结果不得再关一次（会误关下面的对话框）。
    closed: bool,
    error: Option<SharedString>,
    _live: Option<Subscription>,
}

/// 两份插件快照展示给用户、并由 daemon 按「完整集合相等」校验的授权请求是否一致。
fn same_request(a: &PluginDto, b: &PluginDto) -> bool {
    a.risk == b.risk
        && a.permissions == b.permissions
        && a.network_hosts == b.network_hosts
        && a.consent_pending == b.consent_pending
        && a.consent_pending_hosts == b.consent_pending_hosts
        && a.contributions.page_scripts == b.contributions.page_scripts
}

/// 授权请求是否还有待授权项。
fn has_pending(plugin: &PluginDto) -> bool {
    !plugin.consent_pending.is_empty() || !plugin.consent_pending_hosts.is_empty()
}

impl ConsentDialog {
    fn can_confirm(&self) -> bool {
        !self.granting && (!needs_acknowledge(&self.request.risk) || self.acknowledged)
    }

    /// 跟随实时列表：插件已卸载 / 已无待授权项 → 关闭；请求集合变化 → 刷新展示并重置勾选
    /// （daemon 只接受与当前请求完全相等的集合，旧快照提交必被拒绝）。自己的授权请求在途时
    /// 不处理，由请求结果决定关闭或展示错误。
    fn follow_live(
        &mut self,
        live: &Entity<LivePlugins>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.granting || self.closed {
            return;
        }
        let ConsentAction::Grant { plugin, .. } = &mut self.action else {
            return;
        };
        let Some(current) = live
            .read(cx)
            .plugin(&plugin.identity)
            .filter(|current| has_pending(current))
            .cloned()
        else {
            self.closed = true;
            window.close_dialog(cx);
            return;
        };
        if same_request(plugin, &current) {
            return;
        }
        self.request = ConsentRequest::for_plugin(&current, self.translator.read(cx));
        **plugin = current;
        self.acknowledged = false;
        self.error = None;
        cx.notify();
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_confirm() || self.closed {
            return;
        }
        match &self.action {
            ConsentAction::Confirm(on_confirm) => {
                if on_confirm(window, cx) {
                    self.closed = true;
                    window.close_dialog(cx);
                }
            }
            ConsentAction::Grant { port, plugin, live } => {
                self.granting = true;
                self.error = None;
                cx.notify();
                let future = grant_plugin_consent(port, plugin);
                let live = live.clone();
                cx.spawn_in(window, async move |this, cx| {
                    let result = future.await;

                    let Ok(()) = this.update_in(cx, |this, window, cx| {
                        this.granting = false;
                        match result {
                            Ok(_) if !this.closed => {
                                this.closed = true;
                                window.close_dialog(cx);
                            }
                            Ok(_) => {}
                            Err(error) => {
                                let translator = this.translator.read(cx);
                                let message = error_text(translator, &error);
                                this.error = Some(
                                    translator
                                        .text_with("pluginConsentFailed", &[("message", &message)])
                                        .into(),
                                );
                                // 期间列表可能已变（授权集合被改 / 已在别处授权 / 已卸载）。
                                this.follow_live(&live, window, cx);
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
            .when(!request.page_scripts.is_empty(), |this| {
                this.child(ui::title_text(
                    translator.text("pluginConsentPageScripts").to_owned(),
                    frame,
                ))
                .children(page_script_rows(&request.page_scripts, frame))
            })
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

/// 打开授权对话框。`Grant` 动作的对话框跟随实时插件列表（见 `ConsentDialog::follow_live`）。
pub fn open_consent_dialog(
    request: ConsentRequest,
    action: ConsentAction,
    translator: Entity<Translator>,
    window: &mut Window,
    cx: &mut App,
) {
    let cancel = SharedString::from(translator.read(cx).text("cancel").to_owned());
    let ok_label = request.ok_label.clone();
    let live = match &action {
        ConsentAction::Grant { live, .. } => Some(live.clone()),
        ConsentAction::Confirm(_) => None,
    };
    let state = cx.new(|_| ConsentDialog {
        translator,
        request,
        action,
        acknowledged: false,
        granting: false,
        closed: false,
        error: None,
        _live: None,
    });
    if let Some(live) = live {
        state.update(cx, |this, cx| {
            this._live = Some(cx.observe_in(&live, window, |this, live, window, cx| {
                this.follow_live(&live, window, cx);
            }));
        });
    }
    window.open_dialog(cx, move |dialog, _, cx| {
        let (granting, can_confirm, title) = {
            let state = state.read(cx);
            (
                state.granting,
                state.can_confirm(),
                state.request.title.clone(),
            )
        };
        let content = state.clone();
        let confirm = state.clone();
        dialog
            .title(fluxdown_ui_components::dialog_title(title, cx))
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
    use fluxdown_protocol::PluginDto;

    use super::{has_pending, needs_acknowledge, same_request};

    fn plugin() -> PluginDto {
        serde_json::from_value(serde_json::json!({
            "identity": "demo@1",
            "name": "Demo",
            "version": "1.0.0",
            "enabled": true,
            "devMode": false,
            "disabledReason": "None",
            "settings": [],
            "settingsValues": {},
            "permissions": ["tasks"],
            "consentPending": ["tasks"],
        }))
        .expect("plugin fixture")
    }

    #[test]
    fn only_full_trust_requires_acknowledgement() {
        assert!(needs_acknowledge("full"));
        assert!(!needs_acknowledge("elevated"));
        assert!(!needs_acknowledge("standard"));
        assert!(!needs_acknowledge(""));
    }

    #[test]
    fn request_identity_covers_everything_the_daemon_checks() {
        let base = plugin();
        assert!(same_request(&base, &base.clone()));

        let mut renamed = base.clone();
        renamed.name = "Other".to_owned();
        assert!(same_request(&base, &renamed));

        let mut more_permissions = base.clone();
        more_permissions.permissions.push("ytdlp".to_owned());
        assert!(!same_request(&base, &more_permissions));

        let mut more_hosts = base.clone();
        more_hosts.network_hosts.push("example.test".to_owned());
        assert!(!same_request(&base, &more_hosts));

        let mut pending_changed = base.clone();
        pending_changed
            .consent_pending_hosts
            .push("example.test".to_owned());
        assert!(!same_request(&base, &pending_changed));
    }

    #[test]
    fn pending_means_permissions_or_hosts_outstanding() {
        let mut plugin = plugin();
        assert!(has_pending(&plugin));
        plugin.consent_pending.clear();
        assert!(!has_pending(&plugin));
        plugin.consent_pending_hosts.push("example.test".to_owned());
        assert!(has_pending(&plugin));
    }
}
