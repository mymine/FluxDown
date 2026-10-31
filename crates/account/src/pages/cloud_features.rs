//! 云功能分组：配置同步开关（真实读写 `agent.sync.*`）+ 多设备协同状态行
//! （无独立开关，登录后自动可用，展示当前在线设备数）。

use fluxdown_protocol::{CloudDevice, SyncLocalOnlyParams, SyncStatusDto};
use fluxdown_ui_components::{
    ButtonVariant, FluxIcon, card, loading_button, tabular_numbers, tooltip_view,
};
use fluxdown_ui_i18n::Translator;
use fluxdown_ui_theme::{SemanticThemeTokens, active_theme};
use gpui::{
    App, ClickEvent, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement as _, Styled, div, prelude::FluentBuilder as _,
};
use gpui_component::{Disableable as _, Icon, IconNamed, h_flex, switch::Switch, v_flex};

use crate::errors::{ErrorContext, sync_reason_key};
use crate::link::now_unix_ms;
use crate::sync_scope::{SyncPhase, can_sync_now, relative_time, sync_groups, sync_phase};
use crate::view::AccountView;
use crate::{AccountCommand, t, t_with, ui};

/// 云功能分组的瞬时状态：登录态、断线只读。
#[derive(Clone, Copy)]
pub(crate) struct CloudFeaturesState {
    pub logged_in: bool,
    pub disabled: bool,
    /// 「立即同步」在途（含最短可见保持）。
    pub sync_now_busy: bool,
}

pub(crate) fn render(
    translator: &Translator,
    tokens: &SemanticThemeTokens,
    state: CloudFeaturesState,
    sync: &SyncStatusDto,
    devices: Option<&[CloudDevice]>,
    max_online: Option<u64>,
    cx: &mut Context<AccountView>,
) -> impl IntoElement {
    let CloudFeaturesState {
        logged_in,
        disabled,
        sync_now_busy,
    } = state;
    let heading = t(translator, "accountGroupCloudFeatures");
    let desc = t(translator, "accountCloudFeaturesDesc");

    div()
        .flex()
        .flex_col()
        .gap(tokens.spacing.sm)
        .child(ui::group_heading(heading, Some(desc), cx))
        .child(
            card(cx)
                .w_full()
                .flex()
                .flex_col()
                .child(config_sync_row(
                    translator,
                    tokens,
                    logged_in,
                    sync,
                    disabled,
                    sync_now_busy,
                    cx,
                ))
                .when(logged_in && sync.enabled, |card| {
                    card.child(ui::row_divider(cx))
                        .child(scope_block(translator, tokens, sync, disabled, cx))
                })
                .child(ui::row_divider(cx))
                .child(multi_device_row(
                    translator, tokens, logged_in, devices, max_online, cx,
                )),
        )
}

/// 行首图标块：`density.control` 见方的中性底 + `icon.lg` 图标。
fn row_icon(tokens: &SemanticThemeTokens, icon: impl IconNamed, cx: &App) -> impl IntoElement {
    div()
        .size(active_theme(cx).density().control)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(tokens.radius.md)
        .bg(tokens.colors.muted)
        .child(
            Icon::new(icon)
                .size(active_theme(cx).extended().icon.lg)
                .text_color(tokens.colors.muted_foreground),
        )
}

/// 行标题（sm MEDIUM）+ 说明（xs 二级文字）。
fn row_text(
    tokens: &SemanticThemeTokens,
    title: SharedString,
    subtitle: SharedString,
) -> impl IntoElement {
    v_flex()
        .flex_1()
        .min_w_0()
        .gap(tokens.spacing.xxs)
        .child(
            div()
                .text_size(tokens.typography.sm.size)
                .line_height(tokens.typography.sm.line_height)
                .font_weight(FontWeight::MEDIUM)
                .text_color(tokens.colors.foreground)
                .child(title),
        )
        .child(
            div()
                .text_size(tokens.typography.xs.size)
                .line_height(tokens.typography.xs.line_height)
                .text_color(tokens.colors.muted_foreground)
                .truncate()
                .child(subtitle),
        )
}

pub(crate) fn sync_subtitle(translator: &Translator, phase: SyncPhase) -> SharedString {
    match phase {
        SyncPhase::Disabled => t(translator, "cloudSyncDesc"),
        SyncPhase::Halted(reason) => {
            let reason = t(translator, sync_reason_key(reason));
            t_with(
                translator,
                "cloudSyncStatusHalted",
                &[("reason", reason.as_ref())],
            )
        }
        SyncPhase::Failed(reason) => {
            let reason = t(translator, sync_reason_key(reason));
            t_with(
                translator,
                "cloudSyncStatusError",
                &[("reason", reason.as_ref())],
            )
        }
        SyncPhase::Connecting => t(translator, "cloudSyncStatusConnecting"),
        SyncPhase::Syncing => t(translator, "cloudSyncStatusSyncing"),
        SyncPhase::Synced(None) => t(translator, "cloudSyncStatusSynced"),
        SyncPhase::Synced(Some(at)) => {
            let (key, count) = relative_time(now_unix_ms(), at);
            let time = t_with(translator, key, &[("n", &count.to_string())]);
            t_with(
                translator,
                "cloudSyncStatusSyncedAt",
                &[("time", time.as_ref())],
            )
        }
    }
}

fn config_sync_row(
    translator: &Translator,
    tokens: &SemanticThemeTokens,
    logged_in: bool,
    sync: &SyncStatusDto,
    disabled: bool,
    sync_now_busy: bool,
    cx: &mut Context<AccountView>,
) -> impl IntoElement {
    let title = t(translator, "cloudSyncTitle");
    let phase = sync_phase(logged_in, sync);
    let subtitle = sync_subtitle(translator, phase);
    let login_required_label = t(translator, "cloudSyncLoginRequired");
    let checked = sync.enabled;
    let switch_disabled = disabled || !logged_in;
    let sync_now_enabled = !disabled && can_sync_now(logged_in, sync);

    let switch = Switch::new("account-sync-enable")
        .checked(checked)
        .disabled(switch_disabled)
        .on_click(cx.listener(move |view, checked: &bool, _, cx| {
            let method = if *checked {
                fluxdown_protocol::method::AGENT_SYNC_ENABLE
            } else {
                fluxdown_protocol::method::AGENT_SYNC_DISABLE
            };
            let future = view.port(cx).execute(AccountCommand::Sync {
                method,
                params: serde_json::json!({}),
            });
            view.spawn_action(future, ErrorContext::Sync, cx);
        }));

    h_flex()
        .w_full()
        .items_center()
        .gap(tokens.spacing.md)
        .px(tokens.spacing.md)
        .py(tokens.spacing.sm)
        .child(row_icon(tokens, FluxIcon::RotateCw, cx))
        .child(row_text(tokens, title, subtitle))
        .when(logged_in && sync.enabled, |this| {
            this.child(
                loading_button(
                    "account-sync-now",
                    t(translator, "cloudSyncNow"),
                    ButtonVariant::Secondary,
                    sync_now_busy,
                    cx,
                )
                .disabled(!sync_now_enabled || sync_now_busy)
                .on_click(cx.listener(|view, _: &ClickEvent, _, cx| {
                    view.sync_now(cx);
                })),
            )
        })
        .child(if logged_in {
            switch.into_any_element()
        } else {
            div()
                .id("account-sync-login-required")
                .tooltip(move |window, cx| tooltip_view(login_required_label.clone(), window, cx))
                .child(switch)
                .into_any_element()
        })
}

/// 「在此设备同步的范围」：按键前缀分组的开关；关闭的组只留在本设备。
fn scope_block(
    translator: &Translator,
    tokens: &SemanticThemeTokens,
    sync: &SyncStatusDto,
    disabled: bool,
    cx: &mut Context<AccountView>,
) -> impl IntoElement {
    let groups = sync_groups(&sync.local_only_keys);
    let rows: Vec<_> = groups
        .iter()
        .enumerate()
        .map(|(index, state)| {
            let group = state.group;
            let mixed = state.is_partial().then(|| t(translator, "syncScopeMixed"));
            h_flex()
                .w_full()
                .items_center()
                .justify_between()
                .gap(tokens.spacing.md)
                .child(
                    v_flex()
                        .min_w_0()
                        .child(
                            div()
                                .text_size(tokens.typography.sm.size)
                                .line_height(tokens.typography.sm.line_height)
                                .text_color(tokens.colors.foreground)
                                .child(t(translator, group.label_key())),
                        )
                        .when_some(mixed, |column, mixed| {
                            column.child(
                                div()
                                    .text_size(tokens.typography.xs.size)
                                    .line_height(tokens.typography.xs.line_height)
                                    .text_color(tokens.colors.muted_foreground)
                                    .child(mixed),
                            )
                        }),
                )
                .child(
                    Switch::new(("account-sync-scope", index))
                        .checked(state.all_synced())
                        .disabled(disabled)
                        .on_click(cx.listener(move |view, checked: &bool, _, cx| {
                            let local_only_keys =
                                view.controller(cx).sync_status().local_only_keys.clone();
                            let Some(state) = sync_groups(&local_only_keys)
                                .into_iter()
                                .find(|state| state.group == group)
                            else {
                                return;
                            };
                            // 打开 = 参与同步（取消本机专属）；关闭 = 仅本机。
                            let make_local_only = !*checked;
                            let keys = state.keys_to_change(&local_only_keys, make_local_only);
                            if keys.is_empty() {
                                return;
                            }
                            let params = serde_json::to_value(SyncLocalOnlyParams {
                                keys,
                                local_only: make_local_only,
                            })
                            .unwrap_or_default();
                            let future = view.port(cx).execute(AccountCommand::Sync {
                                method: fluxdown_protocol::method::AGENT_SYNC_SET_LOCAL_ONLY,
                                params,
                            });
                            view.spawn_action(future, ErrorContext::Sync, cx);
                        })),
                )
        })
        .collect();
    v_flex()
        .w_full()
        .gap(tokens.spacing.sm)
        .px(tokens.spacing.md)
        .py(tokens.spacing.sm)
        .child(ui::group_heading(
            t(translator, "syncScopeTitle"),
            Some(t(translator, "syncScopeDesc")),
            cx,
        ))
        .children(rows)
}

fn multi_device_row(
    translator: &Translator,
    tokens: &SemanticThemeTokens,
    logged_in: bool,
    devices: Option<&[CloudDevice]>,
    max_online: Option<u64>,
    cx: &App,
) -> impl IntoElement {
    let title = t(translator, "multiDeviceTitle");
    let desc = t(translator, "multiDeviceDesc");
    let online_count = crate::device_list::online_count(devices.unwrap_or_default());
    let caption = active_theme(cx).extended().caption;

    h_flex()
        .w_full()
        .items_center()
        .gap(tokens.spacing.md)
        .px(tokens.spacing.md)
        .py(tokens.spacing.sm)
        .child(row_icon(tokens, FluxIcon::Network, cx))
        .child(row_text(tokens, title, desc))
        .when(logged_in, |this| {
            this.child(
                div()
                    .px(tokens.spacing.sm)
                    .py(tokens.spacing.xxs)
                    .rounded(active_theme(cx).components().badge_radius)
                    .bg(tokens.colors.muted)
                    .text_size(caption.size)
                    .line_height(caption.line_height)
                    .font_features(tabular_numbers())
                    .text_color(tokens.colors.muted_foreground)
                    .child(if devices.is_some() {
                        match max_online {
                            Some(max) => t_with(
                                translator,
                                "devicesOnlineCountOf",
                                &[
                                    ("count", &online_count.to_string()),
                                    ("max", &max.to_string()),
                                ],
                            ),
                            None => t_with(
                                translator,
                                "devicesOnlineCount",
                                &[("count", &online_count.to_string())],
                            ),
                        }
                    } else {
                        t(translator, "devicePresenceUnknown")
                    }),
            )
        })
}
