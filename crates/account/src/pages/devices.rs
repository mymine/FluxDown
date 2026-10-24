//! 设备分组：云账号受信任设备（在线状态 / 重命名 / 删除 / 详情 / 管理全部）
//! 与局域网已配对设备（在线 / 解除配对 / 刷新），以及「添加设备」入口。

use fluxdown_protocol::{CloudConnectionState, CloudDevice, LinkDeviceInfo};
use fluxdown_ui_components::{
    Button as BaseButton, ButtonVariant, FluxIcon, IconControlExt as _, button, card,
};
use fluxdown_ui_i18n::Translator;
use fluxdown_ui_theme::{SemanticThemeTokens, active_theme};
use gpui::{
    App, ClickEvent, Context, Entity, FontWeight, IntoElement, ParentElement, SharedString, Styled,
    div, prelude::FluentBuilder as _,
};
use gpui_component::{
    Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    spinner::Spinner,
    v_flex,
};

use crate::device_list::{platform_label_key, summarize};
use crate::dialogs::device::{
    confirm_delete, confirm_unpair, disconnect_device, open_detail, open_manage_all, open_rename,
};
use crate::host::AccountHost;
use crate::view::AccountView;
use crate::{t, t_with, ui};

pub(crate) struct DevicesState<'a> {
    pub host: &'a Entity<AccountHost>,
    pub devices: &'a [CloudDevice],
    pub linked: &'a [LinkDeviceInfo],
    pub logged_in: bool,
    pub disabled: bool,
    pub refreshing: bool,
    pub linked_refreshing: bool,
}

pub(crate) fn render(
    translator: &Translator,
    tokens: &SemanticThemeTokens,
    state: &DevicesState<'_>,
    cx: &mut Context<AccountView>,
) -> impl IntoElement {
    let mut column = div().flex().flex_col().gap(tokens.spacing.xl);
    if state.logged_in {
        column = column.child(render_cloud(translator, tokens, state, cx));
    }
    column.child(render_linked(translator, tokens, state, cx))
}

fn add_device_button(
    translator: &Translator,
    host: &Entity<AccountHost>,
    disabled: bool,
    cx: &App,
) -> BaseButton {
    let host = host.clone();
    button(
        "account-devices-add",
        t(translator, "addDeviceEntry"),
        ButtonVariant::Secondary,
        cx,
    )
    .disabled(disabled)
    .on_click(move |_, window, cx| crate::dialogs::add_device::open(&host, window, cx))
}

fn render_cloud(
    translator: &Translator,
    tokens: &SemanticThemeTokens,
    state: &DevicesState<'_>,
    cx: &mut Context<AccountView>,
) -> impl IntoElement {
    let title = t(translator, "accountDevicesTitle");
    let desc = t(translator, "accountDevicesDesc");
    let controller = &state.host.read(cx).controller;
    let retry_label = t(
        translator,
        if controller.cloud_connected() {
            "accountDevicesRetry"
        } else {
            "cloudConnectionRetry"
        },
    );
    let connection = controller.cloud_connection();
    let status_key = if controller.is_stale() {
        "localServiceDisconnected"
    } else {
        match connection.state {
            CloudConnectionState::Connected => "cloudConnectionConnected",
            CloudConnectionState::Connecting => "cloudConnectionConnecting",
            CloudConnectionState::Reconnecting => "cloudConnectionReconnecting",
            CloudConnectionState::Disconnected => "cloudConnectionDisconnected",
        }
    };
    let error_key = (!controller.is_stale()
        && !controller.cloud_connected()
        && (connection.last_error.is_some() || connection.last_error_reason.is_some()))
    .then(|| {
        connection
            .last_error_reason
            .and_then(|reason| {
                crate::errors::reason_key(reason, crate::errors::ErrorContext::General)
            })
            .unwrap_or("accountErrorNetwork")
    });
    let empty_label = t(translator, "accountDevicesEmpty");
    let extended = active_theme(cx).extended().clone();
    let (visible, hidden) = summarize(
        state.devices,
        !controller.is_stale() && controller.cloud_connected(),
    );
    let host = state.host.clone();
    let disabled = state.disabled;
    let refreshing = state.refreshing;
    let total = state.devices.len();
    let waiting = controller.online_limit_waiting();
    let max_online = controller.max_online_devices();

    let rows: Vec<_> = visible
        .iter()
        .enumerate()
        .map(|(index, device)| {
            device_row(
                state.host,
                "account-devices",
                device,
                index > 0,
                translator,
                cx,
            )
            .into_any_element()
        })
        .collect();

    div()
        .flex()
        .flex_col()
        .gap(tokens.spacing.sm)
        .child(
            h_flex()
                .w_full()
                .justify_between()
                .items_center()
                .child(ui::group_heading(title, Some(desc), cx))
                .child(
                    h_flex()
                        .gap(tokens.spacing.sm)
                        .child(add_device_button(translator, &host, disabled, cx))
                        .child(
                            button(
                                "account-devices-refresh",
                                retry_label,
                                ButtonVariant::Secondary,
                                cx,
                            )
                            .gap(tokens.spacing.xs)
                            .disabled(disabled || refreshing)
                            .when(refreshing, |this| {
                                this.child(
                                    Spinner::new()
                                        .with_size(extended.icon.sm)
                                        .color(tokens.colors.muted_foreground),
                                )
                            })
                            .on_click(cx.listener(
                                move |view, _: &ClickEvent, window: &mut gpui::Window, cx| {
                                    view.refresh_devices(window, cx);
                                },
                            )),
                        ),
                ),
        )
        .child(ui::group_heading(
            t(translator, status_key),
            Some(t(translator, "cloudConnectionStatusHint")),
            cx,
        ))
        .children(
            error_key
                .filter(|_| !waiting)
                .map(|key| ui::group_heading(t(translator, key), None, cx)),
        )
        .children(waiting.then(|| online_limit_notice(translator, tokens, state, max_online, cx)))
        .child(card(cx).w_full().map(|card| {
            if rows.is_empty() {
                card.p(tokens.spacing.lg).flex().justify_center().child(
                    div()
                        .text_size(tokens.typography.xs.size)
                        .line_height(tokens.typography.xs.line_height)
                        .text_color(tokens.colors.muted_foreground)
                        .child(empty_label),
                )
            } else {
                card.flex()
                    .flex_col()
                    .children(rows)
                    .when(hidden > 0, |card| {
                        let host = host.clone();
                        card.child(ui::row_divider(cx)).child(
                            h_flex()
                                .w_full()
                                .justify_center()
                                .py(tokens.spacing.xs)
                                .child(
                                    button(
                                        "account-devices-manage-all",
                                        t_with(
                                            translator,
                                            "accountDevicesManageAll",
                                            &[("count", &total.to_string())],
                                        ),
                                        ButtonVariant::Link,
                                        cx,
                                    )
                                    .on_click(
                                        move |_, window, cx| {
                                            open_manage_all(&host, window, cx);
                                        },
                                    ),
                                ),
                        )
                    })
            }
        }))
}

/// 在线名额已满、本机等待时的提示：说明上限，并列出占用名额的其他在线设备，可逐个让其下线。
fn online_limit_notice(
    translator: &Translator,
    tokens: &SemanticThemeTokens,
    state: &DevicesState<'_>,
    max_online: Option<u64>,
    cx: &App,
) -> impl IntoElement {
    let text = match max_online {
        Some(max) => t_with(
            translator,
            "cloudOnlineLimitNotice",
            &[("max", &max.to_string())],
        ),
        None => t(translator, "cloudOnlineLimitNoticeNoMax"),
    };
    let rows: Vec<_> = state
        .devices
        .iter()
        .filter(|device| device.is_online && !device.is_current)
        .map(|device| {
            let host = state.host.clone();
            let target = device.clone();
            h_flex()
                .w_full()
                .items_center()
                .justify_between()
                .gap(tokens.spacing.md)
                .px(tokens.spacing.md)
                .child(
                    div()
                        .min_w_0()
                        .text_size(tokens.typography.sm.size)
                        .line_height(tokens.typography.sm.line_height)
                        .text_color(tokens.colors.foreground)
                        .truncate()
                        .child(device.name.clone()),
                )
                .child(
                    button(
                        SharedString::from(format!(
                            "account-online-limit-disconnect-{}",
                            device.id
                        )),
                        t(translator, "accountDeviceDisconnectAction"),
                        ButtonVariant::Secondary,
                        cx,
                    )
                    .on_click(move |_, window, cx| {
                        disconnect_device(&host, &target, window, cx);
                    }),
                )
                .into_any_element()
        })
        .collect();
    v_flex()
        .w_full()
        .gap(tokens.spacing.sm)
        .child(ui::notice(ui::NoticeTone::Warning, text, cx))
        .when(!rows.is_empty(), |this| {
            this.child(
                div()
                    .px(tokens.spacing.md)
                    .text_size(tokens.typography.xs.size)
                    .line_height(tokens.typography.xs.line_height)
                    .text_color(tokens.colors.muted_foreground)
                    .child(t(translator, "cloudOnlineLimitListTitle")),
            )
            .children(rows)
        })
}

fn render_linked(
    translator: &Translator,
    tokens: &SemanticThemeTokens,
    state: &DevicesState<'_>,
    cx: &mut Context<AccountView>,
) -> impl IntoElement {
    let title = t(translator, "linkedDevicesTitle");
    let desc = t(translator, "linkedDevicesDesc");
    let empty_label = t(translator, "linkedDevicesEmpty");
    let refresh_label = t(translator, "linkedDeviceRefresh");
    let extended = active_theme(cx).extended().clone();
    let host = state.host.clone();
    let disabled = state.disabled;
    let refreshing = state.linked_refreshing;
    let mut linked = state.linked.to_vec();
    linked.sort_by(|a, b| {
        b.online
            .cmp(&a.online)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    let rows: Vec<_> = linked
        .iter()
        .enumerate()
        .map(|(index, device)| {
            linked_row(state.host, device, index > 0, translator, cx).into_any_element()
        })
        .collect();

    div()
        .flex()
        .flex_col()
        .gap(tokens.spacing.sm)
        .child(
            h_flex()
                .w_full()
                .justify_between()
                .items_center()
                .child(ui::group_heading(title, Some(desc), cx))
                .child(
                    h_flex()
                        .gap(tokens.spacing.sm)
                        .when(!state.logged_in, |this| {
                            this.child(add_device_button(translator, &host, disabled, cx))
                        })
                        .child(
                            button(
                                "account-linked-refresh",
                                refresh_label,
                                ButtonVariant::Secondary,
                                cx,
                            )
                            .gap(tokens.spacing.xs)
                            .disabled(disabled || refreshing)
                            .when(refreshing, |this| {
                                this.child(
                                    Spinner::new()
                                        .with_size(extended.icon.sm)
                                        .color(tokens.colors.muted_foreground),
                                )
                            })
                            .on_click(cx.listener(
                                move |view, _: &ClickEvent, _: &mut gpui::Window, cx| {
                                    view.refresh_linked(cx);
                                },
                            )),
                        ),
                ),
        )
        .child(card(cx).w_full().map(|card| {
            if rows.is_empty() {
                card.p(tokens.spacing.lg).flex().justify_center().child(
                    div()
                        .text_size(tokens.typography.xs.size)
                        .line_height(tokens.typography.xs.line_height)
                        .text_color(tokens.colors.muted_foreground)
                        .child(empty_label),
                )
            } else {
                card.flex().flex_col().children(rows)
            }
        }))
}

fn presence_dot(online: bool, cx: &App) -> impl IntoElement {
    let theme = active_theme(cx);
    div()
        .flex_none()
        .size(gpui::px(8.))
        .rounded_full()
        .bg(if online {
            theme.extended().colors.success
        } else {
            theme.extended().colors.text_tertiary
        })
}

fn platform_text(translator: &Translator, platform: Option<&str>) -> String {
    let platform = platform.unwrap_or_default();
    platform_label_key(platform)
        .map_or_else(|| platform.to_owned(), |key| t(translator, key).to_string())
}

/// 云设备一行：在线点 + 名称（本机徽标）+ 平台/版本，右侧详情 / 重命名 / 删除。
pub(crate) fn device_row(
    host: &Entity<AccountHost>,
    scope: &'static str,
    device: &CloudDevice,
    with_divider: bool,
    translator: &Translator,
    cx: &App,
) -> impl IntoElement {
    let theme = active_theme(cx);
    let tokens = theme.tokens().clone();
    let extended = theme.extended().clone();
    let badge_radius = theme.components().badge_radius;
    let subtitle = [
        platform_text(translator, device.platform.as_deref()),
        device.app_version.clone().unwrap_or_default(),
    ]
    .into_iter()
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join(" · ");
    let id = device.id.clone();
    let element_id = |kind: &'static str| {
        gpui::ElementId::from(SharedString::from(format!("{scope}-{kind}-{id}")))
    };
    let (detail_host, rename_host, delete_host, disconnect_host) =
        (host.clone(), host.clone(), host.clone(), host.clone());
    let (detail_device, rename_device, delete_device, disconnect_target) = (
        device.clone(),
        device.clone(),
        device.clone(),
        device.clone(),
    );
    let can_disconnect = host.read(cx).controller.can_disconnect_device(device);
    let status = t(
        translator,
        host.read(cx).controller.device_presence_key(device),
    );

    div()
        .w_full()
        .when(with_divider, |this| this.child(ui::row_divider(cx)))
        .child(
            h_flex()
                .w_full()
                .items_center()
                .justify_between()
                .gap(tokens.spacing.md)
                .px(tokens.spacing.md)
                .py(tokens.spacing.sm)
                .child(
                    h_flex()
                        .min_w_0()
                        .items_center()
                        .gap(tokens.spacing.sm)
                        .child(presence_dot(
                            host.read(cx).controller.device_presence(device) == Some(true),
                            cx,
                        ))
                        .child(
                            v_flex()
                                .min_w_0()
                                .gap(tokens.spacing.xxs)
                                .child(
                                    h_flex()
                                        .items_center()
                                        .gap(tokens.spacing.sm)
                                        .child(
                                            div()
                                                .text_size(tokens.typography.sm.size)
                                                .line_height(tokens.typography.sm.line_height)
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(tokens.colors.foreground)
                                                .truncate()
                                                .child(device.name.clone()),
                                        )
                                        .when(device.is_current, |this| {
                                            this.child(
                                                div()
                                                    .px(tokens.spacing.xs)
                                                    .py(tokens.spacing.xxs)
                                                    .rounded(badge_radius)
                                                    .bg(tokens.colors.accent)
                                                    .text_size(extended.caption.size)
                                                    .line_height(extended.caption.line_height)
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .text_color(tokens.colors.accent_foreground)
                                                    .child(t(translator, "accountDeviceCurrent")),
                                            )
                                        }),
                                )
                                .child(
                                    div()
                                        .text_size(tokens.typography.xs.size)
                                        .line_height(tokens.typography.xs.line_height)
                                        .text_color(tokens.colors.muted_foreground)
                                        .truncate()
                                        .child(if subtitle.is_empty() {
                                            status.to_string()
                                        } else {
                                            format!("{status} · {subtitle}")
                                        }),
                                ),
                        ),
                )
                .child(
                    h_flex()
                        .flex_none()
                        .gap(tokens.spacing.xs)
                        .when(can_disconnect, |this| {
                            this.child(
                                Button::new(element_id("disconnect"))
                                    .ghost()
                                    .control_icon(cx)
                                    .icon(FluxIcon::Power)
                                    .tooltip(t(translator, "accountDeviceDisconnectAction"))
                                    .on_click(move |_, window, cx| {
                                        disconnect_device(
                                            &disconnect_host,
                                            &disconnect_target,
                                            window,
                                            cx,
                                        );
                                    }),
                            )
                        })
                        .child(
                            Button::new(element_id("detail"))
                                .ghost()
                                .control_icon(cx)
                                .icon(FluxIcon::Info)
                                .tooltip(t(translator, "accountDeviceDetailTitle"))
                                .on_click(move |_, window, cx| {
                                    open_detail(&detail_host, &detail_device, window, cx);
                                }),
                        )
                        .child(
                            Button::new(element_id("rename"))
                                .ghost()
                                .control_icon(cx)
                                .icon(FluxIcon::Pen)
                                .tooltip(t(translator, "accountDeviceRenameTitle"))
                                .on_click(move |_, window, cx| {
                                    open_rename(&rename_host, &rename_device, window, cx);
                                }),
                        )
                        .child(
                            Button::new(element_id("delete"))
                                .ghost()
                                .control_icon(cx)
                                .icon(FluxIcon::Trash2)
                                .tooltip(t(translator, "accountDeviceDeleteAction"))
                                .on_click(move |_, window, cx| {
                                    confirm_delete(&delete_host, &delete_device, window, cx);
                                }),
                        ),
                ),
        )
}

/// 小徽标（与云设备行的「本机」徽标同款）。
fn tag_badge(text: SharedString, cx: &App) -> impl IntoElement {
    let theme = active_theme(cx);
    let tokens = theme.tokens().clone();
    let extended = theme.extended().clone();
    div()
        .flex_none()
        .px(tokens.spacing.xs)
        .py(tokens.spacing.xxs)
        .rounded(theme.components().badge_radius)
        .bg(tokens.colors.accent)
        .text_size(extended.caption.size)
        .line_height(extended.caption.line_height)
        .font_weight(FontWeight::MEDIUM)
        .text_color(tokens.colors.accent_foreground)
        .child(text)
}

/// 局域网已配对设备一行：在线点 + 名称（备注名优先）+ 标签（对方已解除 / 同一账号）+ 平台，
/// 右侧「重命名」「解除配对」。对方已解除配对的记录只能删除。
fn linked_row(
    host: &Entity<AccountHost>,
    device: &LinkDeviceInfo,
    with_divider: bool,
    translator: &Translator,
    cx: &App,
) -> impl IntoElement {
    let tokens = active_theme(cx).tokens().clone();
    let stale = host.read(cx).controller.is_stale();
    let same_account = device.cloud_device_id.as_deref().is_some_and(|cloud_id| {
        host.read(cx)
            .controller
            .devices()
            .iter()
            .any(|cloud| cloud.id == cloud_id)
    });
    let status_key = if device.revoked {
        "linkedDeviceRevokedDesc"
    } else if stale {
        "devicePresenceUnknown"
    } else if device.online {
        "deviceOnline"
    } else {
        "deviceOffline"
    };
    let subtitle = [
        t(translator, status_key).to_string(),
        platform_text(translator, device.platform.as_deref()),
        device.app_version.clone().unwrap_or_default(),
    ]
    .into_iter()
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join(" · ");
    let host = host.clone();
    let rename_host = host.clone();
    let target = device.clone();
    let rename_target = device.clone();
    let remove_id = gpui::ElementId::from(SharedString::from(format!(
        "account-linked-remove-{}",
        device.fingerprint
    )));
    let rename_id = gpui::ElementId::from(SharedString::from(format!(
        "account-linked-rename-{}",
        device.fingerprint
    )));

    div()
        .w_full()
        .when(with_divider, |this| this.child(ui::row_divider(cx)))
        .child(
            h_flex()
                .w_full()
                .items_center()
                .justify_between()
                .gap(tokens.spacing.md)
                .px(tokens.spacing.md)
                .py(tokens.spacing.sm)
                .child(
                    h_flex()
                        .min_w_0()
                        .items_center()
                        .gap(tokens.spacing.sm)
                        .child(presence_dot(!stale && !device.revoked && device.online, cx))
                        .child(
                            v_flex()
                                .min_w_0()
                                .gap(tokens.spacing.xxs)
                                .child(
                                    h_flex()
                                        .items_center()
                                        .gap(tokens.spacing.sm)
                                        .child(
                                            div()
                                                .min_w_0()
                                                .text_size(tokens.typography.sm.size)
                                                .line_height(tokens.typography.sm.line_height)
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(tokens.colors.foreground)
                                                .truncate()
                                                .child(device.name.clone()),
                                        )
                                        .when(device.revoked, |this| {
                                            this.child(tag_badge(
                                                t(translator, "linkedDeviceRevokedTag"),
                                                cx,
                                            ))
                                        })
                                        .when(same_account, |this| {
                                            this.child(tag_badge(
                                                t(translator, "linkedDeviceSameAccountTag"),
                                                cx,
                                            ))
                                        })
                                        .when(device.grants_control && !device.revoked, |this| {
                                            this.child(tag_badge(
                                                t(translator, "linkControlGrantedTag"),
                                                cx,
                                            ))
                                        }),
                                )
                                .child(
                                    div()
                                        .text_size(tokens.typography.xs.size)
                                        .line_height(tokens.typography.xs.line_height)
                                        .text_color(tokens.colors.muted_foreground)
                                        .truncate()
                                        .child(subtitle),
                                ),
                        ),
                )
                .child(
                    h_flex()
                        .flex_none()
                        .gap(tokens.spacing.xs)
                        .when(!device.revoked, |this| {
                            this.child(
                                Button::new(rename_id)
                                    .ghost()
                                    .control_icon(cx)
                                    .icon(FluxIcon::Pen)
                                    .tooltip(t(translator, "linkedDeviceRename"))
                                    .on_click(move |_, window, cx| {
                                        crate::dialogs::device::open_link_rename(
                                            &rename_host,
                                            &rename_target,
                                            window,
                                            cx,
                                        );
                                    }),
                            )
                        })
                        .child(
                            button(
                                remove_id,
                                t(translator, "linkedDeviceRemove"),
                                ButtonVariant::Secondary,
                                cx,
                            )
                            .on_click(move |_, window, cx| {
                                confirm_unpair(&host, &target, window, cx);
                            }),
                        ),
                ),
        )
}
