use std::collections::BTreeMap;

use fluxdown_ui_components::{FluxIcon, tooltip_view};
use fluxdown_ui_theme::active_theme;
use gpui::{
    AnyElement, App, Context, Entity, InteractiveElement as _, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement as _, Styled, WeakEntity, div,
};
use gpui_component::{
    Disableable as _, Icon, h_flex,
    menu::{DropdownMenu as _, PopupMenu, PopupMenuItem},
};

use fluxdown_protocol::proxy_rules::enabled_proxy_rule_count;

use super::status_bar::{WidgetPlacement, compact_tooltip, status_button, status_button_content};
use crate::{controller::DownloadsCommand, pages::downloads::DownloadView};

const PROXY_MODES: [(&str, &str); 4] = [
    ("none", "proxyModeNone"),
    ("system", "proxyModeSystem"),
    ("manual", "proxyModeManual"),
    ("auto", "proxyModeAuto"),
];

fn current_proxy_mode(value: &str) -> &str {
    match value {
        "system" | "manual" | "auto" => value,
        _ => "none",
    }
}

// 返回已配置的服务器与端口，供菜单展示与执行前检查共用；不分配端点字符串。
fn saved_proxy_server<'a>(host: &'a str, port: &str) -> Option<(&'a str, u16)> {
    let authority = host.split_once("://").map_or(host, |(_, host)| host);
    let authority = authority.split(['/', '?', '#']).next().unwrap_or_default();
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host)
        .trim();
    let host = if host.starts_with('[') {
        host.strip_prefix('[')?.split_once(']')?.0
    } else if host.matches(':').count() == 1 {
        host.split_once(':').map_or(host, |(host, _)| host)
    } else {
        host
    };
    let port = port.trim().parse::<u16>().ok().filter(|port| *port > 0)?;
    (!host.is_empty()).then_some((host, port))
}

// 仅格式化服务器端点，不读取认证字段；即便 host 含 URL userinfo，也不把它展示出来。
fn saved_proxy_endpoint(host: &str, port: &str) -> Option<SharedString> {
    let (host, port) = saved_proxy_server(host, port)?;
    Some(SharedString::from(if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }))
}

fn proxy_menu(menu: PopupMenu, entity: &Entity<DownloadView>, cx: &App) -> PopupMenu {
    let this = entity.read(cx);
    let current_mode = current_proxy_mode(this.controller.config_str("proxy_mode"));
    let disabled = this.controller.is_stale();
    let endpoint = saved_proxy_endpoint(
        this.controller.config_str("proxy_host"),
        this.controller.config_str("proxy_port"),
    );
    let rule_count = enabled_proxy_rule_count(this.controller.config_str("proxy_rules"));
    let translator = this.translator.read(cx);
    let manual_configured = endpoint.is_some();
    let manual_hint = endpoint.unwrap_or_else(|| {
        SharedString::from(translator.text("proxyConfigureInSettings").to_owned())
    });
    let mut menu = menu.max_w(active_theme(cx).density().status_control * 12.);
    for (mode, key) in PROXY_MODES {
        let label = SharedString::from(translator.text(key).to_owned());
        let item = if mode == "manual" {
            let endpoint = manual_hint.clone();
            PopupMenuItem::element(move |_, cx| {
                let theme = active_theme(cx);
                let caption = &theme.extended().caption;
                let tooltip_endpoint = endpoint.clone();
                h_flex()
                    .w_full()
                    .min_w_0()
                    .gap(theme.tokens().spacing.sm)
                    .child(div().flex_none().child(label.clone()))
                    .child(
                        div()
                            .id("status-proxy-manual-endpoint")
                            .min_w_0()
                            .flex_1()
                            .max_w(theme.density().status_control * 8.)
                            .truncate()
                            .text_size(caption.size)
                            .line_height(caption.line_height)
                            .text_color(theme.tokens().colors.muted_foreground)
                            .tooltip(move |window, cx| {
                                tooltip_view(tooltip_endpoint.clone(), window, cx)
                            })
                            .child(endpoint.clone()),
                    )
            })
        } else {
            PopupMenuItem::new(label)
        };
        let view = entity.downgrade();
        menu = menu.item(
            item.checked(mode == current_mode)
                .disabled(disabled || (mode == "manual" && !manual_configured))
                .on_click(move |_, _, cx| {
                    let Ok(()) = view.update(cx, |this, cx| {
                        this.execute_proxy_mode_patch(mode, cx);
                    }) else {
                        // 视图已释放，无需再更新状态。
                        return;
                    };
                }),
        );
    }
    let rules_hint = (rule_count > 0).then(|| {
        SharedString::from(translator.text_with(
            "proxyRulesActiveHint",
            &[("count", &rule_count.to_string())],
        ))
    });
    if let Some(hint) = rules_hint {
        menu = menu.separator().item(
            PopupMenuItem::element(move |_, cx| {
                let theme = active_theme(cx);
                let caption = &theme.extended().caption;
                h_flex()
                    .w_full()
                    .min_w_0()
                    .gap(theme.tokens().spacing.sm)
                    .items_center()
                    .child(
                        Icon::new(FluxIcon::Info)
                            .size(theme.extended().icon.sm)
                            .flex_none(),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .text_size(caption.size)
                            .line_height(caption.line_height)
                            .text_color(theme.tokens().colors.muted_foreground)
                            .child(hint.clone()),
                    )
            })
            .disabled(true),
        );
    }
    menu
}

impl DownloadView {
    fn execute_proxy_mode_patch(&mut self, mode: &'static str, cx: &mut Context<Self>) {
        // 菜单打开后也可能断线，执行时以控制器的最新连接态为准。
        if self.controller.is_stale() {
            self.last_error = Some(self.strings.disconnected.clone());
            cx.notify();
            return;
        }
        // 菜单打开后配置可能被另一窗口清空，按当前配置拒绝切到空手动代理。
        if mode == "manual"
            && saved_proxy_server(
                self.controller.config_str("proxy_host"),
                self.controller.config_str("proxy_port"),
            )
            .is_none()
        {
            self.last_error = Some(SharedString::from(
                self.translator
                    .read(cx)
                    .text("proxyConfigureInSettings")
                    .to_owned(),
            ));
            cx.notify();
            return;
        }
        self.execute_status_config_command(
            DownloadsCommand::PatchConfig {
                values: BTreeMap::from([("proxy_mode".to_owned(), mode.to_owned())]),
                expected_revision: self.controller.config_revision(),
            },
            cx,
        );
    }

    /// 代理模式控件：地球图标 + 当前模式 + 下拉箭头；紧凑摆放只留图标，模式并入悬浮提示。
    pub(super) fn render_proxy_control(
        &self,
        placement: WidgetPlacement,
        view: WeakEntity<Self>,
        cx: &App,
    ) -> AnyElement {
        let mode = current_proxy_mode(self.controller.config_str("proxy_mode"));
        let label_key = PROXY_MODES
            .iter()
            .find(|(value, _)| *value == mode)
            .map_or("proxyModeNone", |(_, key)| *key);
        let translator = self.translator.read(cx);
        let label = SharedString::from(translator.text(label_key).to_owned());
        let title = SharedString::from(translator.text("settingsCatProxy").to_owned());
        let rule_count = enabled_proxy_rule_count(self.controller.config_str("proxy_rules"));
        let rules_hint = (rule_count > 0).then(|| {
            SharedString::from(translator.text_with(
                "proxyRulesActiveHint",
                &[("count", &rule_count.to_string())],
            ))
        });
        let theme = active_theme(cx);
        let icon_size = theme.extended().icon.sm;
        let control_height = theme.density().status_control;
        let content = status_button_content(Some(FluxIcon::Globe), None, None, cx);
        let (content, tooltip) = if placement.compact {
            (content, compact_tooltip(&title, &label))
        } else {
            let mut content = content
                .min_w_0()
                .child(div().min_w_0().truncate().child(label));
            if rule_count > 0 {
                let caption = &theme.extended().caption;
                let tokens = theme.tokens();
                content = content.child(
                    div()
                        .flex_none()
                        .px(tokens.spacing.xs)
                        .rounded(tokens.radius.sm)
                        .bg(tokens.colors.muted)
                        .text_color(tokens.colors.muted_foreground)
                        .text_size(caption.size)
                        .line_height(caption.line_height)
                        .child(SharedString::from(rule_count.to_string())),
                );
            }
            (
                content.child(Icon::new(FluxIcon::ChevronDown).size(icon_size).flex_none()),
                title,
            )
        };
        let tooltip = match rules_hint {
            Some(hint) => SharedString::from(format!("{tooltip} · {hint}")),
            None => tooltip,
        };

        status_button("status-proxy-mode", cx)
            .max_w(control_height * 7.)
            .disabled(self.controller.is_stale())
            .child(content)
            .tooltip(tooltip)
            .dropdown_menu_with_anchor(placement.menu_anchor, move |menu, window, cx| {
                let Some(entity) = view.upgrade() else {
                    return menu;
                };
                let this = entity.read(cx);
                let mut state = (
                    this.controller.config_revision(),
                    this.controller.is_stale(),
                );
                // 只在配置版本 / 连接态变化时重建已打开菜单，速度刷新不分配菜单项。
                cx.observe_in(&entity, window, move |menu, entity, window, cx| {
                    let this = entity.read(cx);
                    let current_state = (
                        this.controller.config_revision(),
                        this.controller.is_stale(),
                    );
                    if state != current_state {
                        state = current_state;
                        menu.rebuild(window, cx, |menu, _, cx| proxy_menu(menu, &entity, cx));
                    }
                })
                .detach();
                proxy_menu(menu, &entity, cx)
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{saved_proxy_endpoint, saved_proxy_server};

    #[test]
    fn endpoint_excludes_url_credentials_and_non_server_parts() {
        assert_eq!(
            saved_proxy_endpoint(
                "https://alice:secret@proxy.example/path?token=secret",
                "8080"
            )
            .as_deref(),
            Some("proxy.example:8080")
        );
        assert_eq!(
            saved_proxy_endpoint("alice:secret@proxy.example", "1080").as_deref(),
            Some("proxy.example:1080")
        );
        assert_eq!(
            saved_proxy_endpoint("http://alice:secret@proxy.example:3128/path", "8080").as_deref(),
            Some("proxy.example:8080")
        );
    }

    #[test]
    fn endpoint_formats_ipv6_with_single_pair_of_brackets() {
        assert_eq!(
            saved_proxy_endpoint("::1", "1080").as_deref(),
            Some("[::1]:1080")
        );
        assert_eq!(
            saved_proxy_endpoint("[2001:db8::1]", "8080").as_deref(),
            Some("[2001:db8::1]:8080")
        );
        assert_eq!(
            saved_proxy_endpoint("http://[2001:db8::1]:3128/path", "8080").as_deref(),
            Some("[2001:db8::1]:8080")
        );
    }

    #[test]
    fn manual_proxy_requires_server_and_valid_port() {
        for (host, port) in [
            ("", "8080"),
            ("   ", "8080"),
            ("proxy.example", ""),
            ("proxy.example", "0"),
            ("proxy.example", "-1"),
            ("proxy.example", "65536"),
            ("proxy.example", "invalid"),
        ] {
            assert_eq!(saved_proxy_server(host, port), None);
        }
        assert_eq!(
            saved_proxy_server("proxy.example", "8080"),
            Some(("proxy.example", 8080))
        );
        assert_eq!(saved_proxy_server("::1", "65535"), Some(("::1", 65535)));
    }
}
