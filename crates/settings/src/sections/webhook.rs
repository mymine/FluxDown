//! Webhook：端点列表（daemon `webhook.endpoints` JSON）与投递记录。

use fluxdown_protocol::method;
use fluxdown_ui_components::{
    ButtonVariant, DialogIntent, FluxIcon, button, dialog_title, loading_button, tabular_numbers,
};
use fluxdown_ui_theme::active_theme;
use gpui::{
    App, Context, InteractiveElement as _, IntoElement as _, ParentElement, SharedString, Styled,
    Window,
};
use gpui_component::{Disableable as _, WindowExt as _, h_flex, v_flex};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

use super::{SectionContext, webhook_dialog};
use crate::store::{SettingsStore, rpc_error_text};
use crate::ui::{
    SettingsRow, SettingsSection, body_text, empty_state, meta_text, row_button, row_danger_button,
    row_loading_button,
};

pub(crate) const ENDPOINTS_KEY: &str = "webhook.endpoints";
/// 行内「测试」结果：`{ endpointId, success, text }`，只渲染在发起的那一行。
const TEST_RESULT_KEY: &str = "webhook_test_result";

/// 与 `engine::webhook::EndpointSpec` 同 wire 形状、同宽松解析规则：类型不符的字段回退
/// 默认值，数组 / 映射里的非字符串项丢弃（Web `parseEndpoint` 逐字段一致）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct EndpointSpec {
    #[serde(deserialize_with = "lenient::string")]
    pub id: String,
    #[serde(deserialize_with = "lenient::string")]
    pub name: String,
    #[serde(deserialize_with = "lenient::string")]
    pub preset: String,
    #[serde(deserialize_with = "lenient::string")]
    pub url: String,
    #[serde(default = "default_true", deserialize_with = "lenient::bool_or_true")]
    pub enabled: bool,
    #[serde(deserialize_with = "lenient::strings")]
    pub events: Vec<String>,
    #[serde(deserialize_with = "lenient::string")]
    pub queue_id: String,
    #[serde(deserialize_with = "lenient::string_map")]
    pub headers: BTreeMap<String, String>,
    #[serde(deserialize_with = "lenient::string")]
    pub body_template: String,
    #[serde(deserialize_with = "lenient::string")]
    pub sign_secret: String,
    #[serde(deserialize_with = "lenient::bool_or_false")]
    pub allow_http: bool,
    #[serde(deserialize_with = "lenient::bool_or_false")]
    pub use_proxy: bool,
}

fn default_true() -> bool {
    true
}

mod lenient {
    use std::collections::BTreeMap;

    use serde::{Deserialize, Deserializer};
    use serde_json::Value;

    pub(super) fn string<'de, D: Deserializer<'de>>(de: D) -> Result<String, D::Error> {
        Ok(match Value::deserialize(de)? {
            Value::String(value) => value,
            _ => String::new(),
        })
    }

    pub(super) fn bool_or_true<'de, D: Deserializer<'de>>(de: D) -> Result<bool, D::Error> {
        Ok(Value::deserialize(de)?.as_bool().unwrap_or(true))
    }

    pub(super) fn bool_or_false<'de, D: Deserializer<'de>>(de: D) -> Result<bool, D::Error> {
        Ok(Value::deserialize(de)?.as_bool().unwrap_or(false))
    }

    pub(super) fn strings<'de, D: Deserializer<'de>>(de: D) -> Result<Vec<String>, D::Error> {
        Ok(match Value::deserialize(de)? {
            Value::Array(items) => items
                .into_iter()
                .filter_map(|item| match item {
                    Value::String(value) => Some(value),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        })
    }

    pub(super) fn string_map<'de, D: Deserializer<'de>>(
        de: D,
    ) -> Result<BTreeMap<String, String>, D::Error> {
        Ok(match Value::deserialize(de)? {
            Value::Object(map) => map
                .into_iter()
                .filter_map(|(key, value)| match value {
                    Value::String(value) => Some((key, value)),
                    _ => None,
                })
                .collect(),
            _ => BTreeMap::new(),
        })
    }
}

/// 事件 wire 名（与引擎 `WebhookEventKind::wire()` 逐字一致）。
pub(crate) const WEBHOOK_EVENTS: &[(&str, &str)] = &[
    ("task.created", "webhookEventCreated"),
    ("task.started", "webhookEventStarted"),
    ("task.completed", "webhookEventCompleted"),
    ("task.failed", "webhookEventFailed"),
    ("task.paused", "webhookEventPaused"),
    ("queue.started", "webhookEventQueueStarted"),
    ("queue.drained", "webhookEventQueueDrained"),
];

/// 仅本机 webhook 可订阅、FluxCloud 不认识的事件（agent 上报前也会丢弃）。
const LOCAL_ONLY_EVENTS: &[&str] = &["queue.started"];

/// 云端推送渠道可订阅的事件：[`WEBHOOK_EVENTS`] 去掉本机专属事件，保持原顺序。
pub(crate) fn cloud_events() -> impl Iterator<Item = &'static (&'static str, &'static str)> {
    WEBHOOK_EVENTS
        .iter()
        .filter(|(wire, _)| !LOCAL_ONLY_EVENTS.contains(wire))
}

/// 配置串 → 端点列表：空串 / 非 JSON 数组视为空列表，数组里的非对象元素跳过。
pub(crate) fn parse_endpoints(raw: &str) -> Vec<EndpointSpec> {
    if raw.trim().is_empty() {
        return Vec::new();
    }
    serde_json::from_str::<Vec<Value>>(raw)
        .map(|items| {
            items
                .into_iter()
                .filter(Value::is_object)
                .filter_map(|item| serde_json::from_value(item).ok())
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn read_endpoints(store: &SettingsStore) -> Vec<EndpointSpec> {
    parse_endpoints(&store.daemon_str(ENDPOINTS_KEY))
}

/// 读-改-写端点列表：`edit` 每次都基于服务端最新列表计算（冲突重取重放），
/// 返回 `None` 表示无需写入。不会覆盖别处同时写入的端点。
pub(crate) fn mutate_endpoints(
    store: &mut SettingsStore,
    edit: impl Fn(Vec<EndpointSpec>) -> Option<Vec<EndpointSpec>> + 'static,
    cx: &mut Context<SettingsStore>,
) {
    store.mutate_daemon(
        ENDPOINTS_KEY,
        move |raw| edit(parse_endpoints(raw)).and_then(|list| serde_json::to_string(&list).ok()),
        cx,
    );
}

/// 新增或按 id 覆盖。
pub(crate) fn upsert_endpoint(
    store: &mut SettingsStore,
    draft: EndpointSpec,
    cx: &mut Context<SettingsStore>,
) {
    clear_test_result(store, &draft.id, cx);
    mutate_endpoints(
        store,
        move |mut list| {
            match list.iter_mut().find(|entry| entry.id == draft.id) {
                Some(slot) => *slot = draft.clone(),
                None => list.push(draft.clone()),
            }
            Some(list)
        },
        cx,
    );
}

/// 端点的配置变了 / 被删了，旧的行内测试结果不再代表它。
fn clear_test_result(
    store: &mut SettingsStore,
    endpoint_id: &str,
    cx: &mut Context<SettingsStore>,
) {
    let owned = store
        .transient(TEST_RESULT_KEY)
        .and_then(|result| result.get("endpointId"))
        .and_then(Value::as_str)
        == Some(endpoint_id);
    if owned {
        store.set_transient(TEST_RESULT_KEY, Value::Null, cx);
    }
}

/// 删除前确认（与 Web `confirmDialog(webhookRowDeleteConfirm)` 一致）。
fn confirm_delete(
    store: gpui::Entity<SettingsStore>,
    translator: &fluxdown_ui_i18n::Translator,
    endpoint: &EndpointSpec,
    window: &mut Window,
    cx: &mut App,
) {
    let title = SharedString::from(translator.text("webhookRowDeleteConfirm").to_owned());
    let description = SharedString::from(endpoint.name.clone());
    let ok = SharedString::from(translator.text("webhookRowDelete").to_owned());
    let cancel = SharedString::from(translator.text("cancel").to_owned());
    let id = endpoint.id.clone();
    window.open_alert_dialog(cx, move |alert, _, cx| {
        let store = store.clone();
        let id = id.clone();
        alert
            .title(dialog_title(title.clone(), cx))
            .description(description.clone())
            .footer(fluxdown_ui_components::dialog_footer(
                Some(cancel.clone()),
                ok.clone(),
                DialogIntent::Destructive,
                cx,
            ))
            .on_ok(move |_, _, cx| {
                let id = id.clone();
                store.update(cx, |store, cx| {
                    clear_test_result(store, &id, cx);
                    mutate_endpoints(
                        store,
                        move |list| {
                            list.iter()
                                .any(|entry| entry.id == id)
                                .then(|| list.into_iter().filter(|entry| entry.id != id).collect())
                        },
                        cx,
                    );
                });
                true
            })
    });
}

/// 「发送测试」回执 → `(成功, 文案)`；行内测试与对话框测试共用同一套措辞。
pub(crate) fn test_result_text(
    translator: &fluxdown_ui_i18n::Translator,
    result: Result<Value, fluxdown_protocol::RpcErrorData>,
) -> (bool, String) {
    let value = match result {
        Ok(value) => value,
        Err(error) => {
            return (
                false,
                translator.text_with(
                    "webhookTestFail",
                    &[("error", &rpc_error_text(translator, &error))],
                ),
            );
        }
    };
    let success = value
        .get("success")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let status = value.get("statusCode").and_then(Value::as_i64).unwrap_or(0);
    if success {
        let status = if status == 0 {
            "OK".to_owned()
        } else {
            status.to_string()
        };
        let latency = value
            .get("latencyMs")
            .and_then(Value::as_i64)
            .unwrap_or(0)
            .to_string();
        return (
            true,
            translator.text_with("webhookTestOk", &[("status", &status), ("ms", &latency)]),
        );
    }
    let error = value
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let detail = if error.is_empty() {
        format!("HTTP {status}")
    } else {
        error.to_owned()
    };
    (
        false,
        translator.text_with("webhookTestFail", &[("error", &detail)]),
    )
}

/// 自托管标签下的端点列表。页头分段标签已标明「自托管」，分区不再重复标题；
/// 空状态的「改用云端推送」链接经 `try_cloud` 切到云端标签（`None` = 云端未开放，不显示）。
pub(crate) fn endpoints_group(
    ctx: &SectionContext,
    try_cloud: Option<crate::push_view::WindowAction>,
    _cx: &mut App,
) -> SettingsSection {
    SettingsSection::new()
        .subtitle(ctx.t("webhookSemantics"))
        .row(endpoints_item(ctx, try_cloud))
}

fn endpoints_item(
    ctx: &SectionContext,
    try_cloud: Option<crate::push_view::WindowAction>,
) -> SettingsRow {
    let store = ctx.store();
    let translator = ctx.translator.clone();
    let empty_title = ctx.t("webhookEmptyTitle");
    let empty_desc = ctx.t("webhookEmptyDescSelfHosted");
    let try_cloud_label = ctx.t("webhookTryCloud");
    let add = ctx.t("webhookAddEndpoint");
    let edit = ctx.t("webhookRowEdit");
    let test = ctx.t("webhookRowTest");
    let delete = ctx.t("webhookRowDelete");
    let disabled_label = ctx.t("webhookHealthDisabled");
    SettingsRow::custom(move |disabled, _key, _window, cx: &mut App| {
        let disabled = disabled || !store.read(cx).daemon_connected();
        let theme = active_theme(cx);
        let tokens = theme.tokens();
        let extended = theme.extended().colors;
        let endpoints = read_endpoints(store.read(cx));
        let deliveries = store.read(cx).webhook_deliveries().to_vec();
        let mut column = v_flex().w_full().gap(tokens.spacing.xxs);
        if endpoints.is_empty() {
            column = column.child(empty_state(
                FluxIcon::Webhook,
                empty_title.clone(),
                empty_desc.clone(),
                cx,
            ));
            if let Some(try_cloud) = try_cloud.clone() {
                column = column.child(
                    h_flex()
                        .w_full()
                        .justify_center()
                        .pb(tokens.spacing.sm)
                        .child(
                            button(
                                "webhook-try-cloud",
                                try_cloud_label.clone(),
                                ButtonVariant::Link,
                                cx,
                            )
                            .on_click(move |_, window, cx| try_cloud(window, cx)),
                        ),
                );
            }
        }
        for endpoint in &endpoints {
            let health = deliveries
                .iter()
                .filter(|delivery| delivery.endpoint_id == endpoint.id)
                .max_by_key(|delivery| delivery.timestamp_ms)
                .map_or_else(
                    || translator.text("webhookHealthNone").to_owned(),
                    |delivery| {
                        if delivery.success {
                            translator.text_with(
                                "webhookHealthOk",
                                &[("time", &format!("{}ms", delivery.latency_ms))],
                            )
                        } else {
                            let detail = if delivery.error.is_empty() {
                                format!("HTTP {}", delivery.status_code)
                            } else {
                                delivery.error.clone()
                            };
                            translator.text_with("webhookHealthFail", &[("detail", &detail)])
                        }
                    },
                );
            let test_result = store
                .read(cx)
                .transient(TEST_RESULT_KEY)
                .filter(|result| {
                    result.get("endpointId").and_then(Value::as_str) == Some(endpoint.id.as_str())
                })
                .map(|result| {
                    (
                        result
                            .get("success")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                        result
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                    )
                });
            let toggle_store = store.clone();
            let toggle_id = endpoint.id.clone();
            let edit_store = store.clone();
            let edit_translator = translator.clone();
            let edit_endpoint = endpoint.clone();
            let test_store = store.clone();
            let test_endpoint = endpoint.clone();
            let delete_store = store.clone();
            let delete_translator = translator.clone();
            let delete_endpoint = endpoint.clone();
            let events = endpoint.events.join(", ");
            column = column.child(
                h_flex()
                    .id(SharedString::from(format!("webhook-row-{}", endpoint.id)))
                    .w_full()
                    .items_center()
                    .gap(tokens.spacing.sm)
                    .px(tokens.spacing.sm)
                    .py(tokens.spacing.xs)
                    .rounded(tokens.radius.md)
                    .hover(move |style| style.bg(extended.row_hover))
                    .child(
                        gpui_component::switch::Switch::new(SharedString::from(format!(
                            "webhook-enabled-{}",
                            endpoint.id
                        )))
                        .checked(endpoint.enabled)
                        .disabled(disabled)
                        .on_click(move |checked: &bool, _, cx| {
                            let id = toggle_id.clone();
                            let checked = *checked;
                            toggle_store.update(cx, |store, cx| {
                                mutate_endpoints(
                                    store,
                                    move |mut list| {
                                        let entry = list.iter_mut().find(|entry| entry.id == id)?;
                                        if entry.enabled == checked {
                                            return None;
                                        }
                                        entry.enabled = checked;
                                        Some(list)
                                    },
                                    cx,
                                );
                            });
                        }),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(tokens.spacing.xxs)
                            .child(body_text(cx).child(SharedString::from(endpoint.name.clone())))
                            .child(meta_text(cx).truncate().child(SharedString::from(format!(
                                "{} · {}",
                                endpoint.url, events
                            ))))
                            .child(meta_text(cx).text_color(extended.text_tertiary).child(
                                if endpoint.enabled {
                                    SharedString::from(health)
                                } else {
                                    disabled_label.clone()
                                },
                            ))
                            .children(test_result.map(|(success, text)| {
                                meta_text(cx)
                                    .text_color(if success {
                                        extended.success
                                    } else {
                                        tokens.colors.destructive
                                    })
                                    .child(SharedString::from(text))
                            })),
                    )
                    .child(
                        row_button(
                            SharedString::from(format!("webhook-edit-{}", endpoint.id)),
                            edit.clone(),
                            ButtonVariant::Secondary,
                            cx,
                        )
                        .disabled(disabled)
                        .on_click(move |_, window, cx| {
                            webhook_dialog::open(
                                edit_store.clone(),
                                edit_translator.clone(),
                                Some(edit_endpoint.clone()),
                                window,
                                cx,
                            );
                        }),
                    )
                    .child(
                        row_loading_button(
                            SharedString::from(format!("webhook-test-{}", endpoint.id)),
                            test.clone(),
                            ButtonVariant::Secondary,
                            store.read(cx).is_busy_tagged("webhookTest", &endpoint.id),
                            cx,
                        )
                        .disabled(disabled || store.read(cx).is_busy("webhookTest"))
                        .on_click({
                            let translator = translator.clone();
                            move |_, _, cx| {
                                let params = serde_json::to_value(&test_endpoint)
                                    .unwrap_or_else(|_| json!({}));
                                let translator = translator.clone();
                                let endpoint_id = test_endpoint.id.clone();
                                test_store.update(cx, |store, cx| {
                                    store.set_transient(TEST_RESULT_KEY, Value::Null, cx);
                                    store.call_with(
                                        "webhookTest",
                                        method::DAEMON_WEBHOOK_TEST,
                                        params,
                                        cx,
                                        move |store, result, cx| {
                                            let (success, text) =
                                                test_result_text(&translator, result);
                                            store.set_transient(
                                                TEST_RESULT_KEY,
                                                json!({
                                                    "endpointId": endpoint_id,
                                                    "success": success,
                                                    "text": text,
                                                }),
                                                cx,
                                            );
                                        },
                                    );
                                    store.tag_busy("webhookTest", test_endpoint.id.clone());
                                });
                            }
                        }),
                    )
                    .child(
                        row_danger_button(
                            SharedString::from(format!("webhook-delete-{}", endpoint.id)),
                            delete.clone(),
                            cx,
                        )
                        .disabled(disabled)
                        .on_click(move |_, window, cx| {
                            confirm_delete(
                                delete_store.clone(),
                                &delete_translator,
                                &delete_endpoint,
                                window,
                                cx,
                            );
                        }),
                    ),
            );
        }
        let add_store = store.clone();
        let add_translator = translator.clone();
        column
            .child(
                h_flex().w_full().pt(tokens.spacing.sm).justify_end().child(
                    button("webhook-add", add.clone(), ButtonVariant::Primary, cx)
                        .disabled(disabled)
                        .on_click(move |_, window, cx| {
                            webhook_dialog::open(
                                add_store.clone(),
                                add_translator.clone(),
                                None,
                                window,
                                cx,
                            );
                        }),
                ),
            )
            .into_any_element()
    })
    .keywords([ctx.t("notifyGroupWebhook"), ctx.t("webhookAddEndpoint")])
}

pub(crate) fn delivery_log_group(ctx: &SectionContext, _cx: &mut App) -> SettingsSection {
    let store = ctx.store();
    let translator = ctx.translator.clone();
    let empty = ctx.t("webhookLogEmpty");
    let clear = ctx.t("webhookLogClear");
    let simulate = ctx.t("webhookLogSimulate");
    let row = SettingsRow::custom(move |disabled, _key, _window, cx: &mut App| {
        let disabled = disabled || !store.read(cx).daemon_connected();
        let theme = active_theme(cx);
        let tokens = theme.tokens();
        let hairline = theme.extended().colors.hairline;
        let deliveries = store.read(cx).webhook_deliveries().to_vec();
        let clear_store = store.clone();
        let simulate_store = store.clone();
        let mut column = v_flex().w_full().gap(tokens.spacing.xs);
        if let Some(text) = store
            .read(cx)
            .transient("webhook_simulate_result")
            .and_then(serde_json::Value::as_str)
        {
            column = column.child(meta_text(cx).child(SharedString::from(text.to_owned())));
        }
        if deliveries.is_empty() {
            column = column.child(meta_text(cx).child(empty.clone()));
        }
        for delivery in deliveries.iter().take(50) {
            let status = if delivery.success {
                format!("{} · {}ms", delivery.status_code, delivery.latency_ms)
            } else if delivery.error.is_empty() {
                format!("HTTP {}", delivery.status_code)
            } else {
                delivery.error.clone()
            };
            let attempts =
                translator.text_with("webhookAttempts", &[("n", &delivery.attempts.to_string())]);
            column =
                column.child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .justify_between()
                        .gap(tokens.spacing.md)
                        .py(tokens.spacing.xs)
                        .border_b_1()
                        .border_color(hairline)
                        .child(
                            v_flex()
                                .min_w_0()
                                .gap(tokens.spacing.xxs)
                                .child(body_text(cx).child(SharedString::from(format!(
                                    "{} · {}",
                                    delivery.endpoint_name, delivery.event
                                ))))
                                .child(meta_text(cx).truncate().child(SharedString::from(
                                    format!("{} · {}", status, attempts),
                                ))),
                        )
                        .child(
                            meta_text(cx)
                                .flex_none()
                                .font_features(tabular_numbers())
                                .text_color(if delivery.success {
                                    tokens.colors.muted_foreground
                                } else {
                                    tokens.colors.destructive
                                })
                                .child(SharedString::from(super::subscription::format_unix(
                                    delivery.timestamp_ms / 1000,
                                ))),
                        ),
                );
        }
        column
            .child(
                h_flex()
                    .w_full()
                    .justify_end()
                    .gap(tokens.spacing.sm)
                    .child(
                        loading_button(
                            "webhook-simulate",
                            if store.read(cx).is_busy("webhookSimulate") {
                                SharedString::from(translator.text("webhookLogPending").to_owned())
                            } else {
                                simulate.clone()
                            },
                            ButtonVariant::Secondary,
                            simulate_store.read(cx).is_busy("webhookSimulate"),
                            cx,
                        )
                        .disabled(disabled || simulate_store.read(cx).is_busy("webhookSimulate"))
                        .on_click({
                            let translator = translator.clone();
                            move |_, _, cx| {
                                let translator = translator.clone();
                                simulate_store.update(cx, |store, cx| {
                                    if store.is_busy("webhookSimulate") {
                                        return;
                                    }
                                    store.set_transient(
                                        "webhook_simulate_result",
                                        json!(translator.text("webhookLogPending")),
                                        cx,
                                    );
                                    store.call_with(
                                        "webhookSimulate",
                                        method::DAEMON_WEBHOOK_SIMULATE,
                                        json!({}),
                                        cx,
                                        move |store, result, cx| {
                                            let text = match result {
                                                Ok(value) => match serde_json::from_value::<
                                                    fluxdown_protocol::WebhookSimulateResponse,
                                                >(
                                                    value
                                                ) {
                                                    Ok(response) if response.dispatched == 0 => {
                                                        translator
                                                            .text("webhookSimulateNoTarget")
                                                            .to_owned()
                                                    }
                                                    Ok(response) => translator.text_with(
                                                        "webhookSimulateDispatched",
                                                        &[("n", &response.dispatched.to_string())],
                                                    ),
                                                    Err(error) => translator.text_with(
                                                        "webhookTestFail",
                                                        &[("error", &error.to_string())],
                                                    ),
                                                },
                                                Err(error) => translator.text_with(
                                                    "webhookTestFail",
                                                    &[(
                                                        "error",
                                                        &rpc_error_text(&translator, &error),
                                                    )],
                                                ),
                                            };
                                            store.set_transient(
                                                "webhook_simulate_result",
                                                json!(text),
                                                cx,
                                            );
                                        },
                                    );
                                });
                            }
                        }),
                    )
                    .child(
                        button(
                            "webhook-clear-log",
                            clear.clone(),
                            ButtonVariant::Secondary,
                            cx,
                        )
                        .disabled(disabled || deliveries.is_empty())
                        .on_click(move |_, _, cx| {
                            clear_store.update(cx, |store, cx| {
                                store.call_simple(
                                    "webhookClear",
                                    method::DAEMON_WEBHOOK_CLEAR_DELIVERIES,
                                    json!({}),
                                    None,
                                    cx,
                                );
                            });
                        }),
                    ),
            )
            .into_any_element()
    })
    .keywords([ctx.t("webhookDeliveryLog")]);
    SettingsSection::new()
        .title(ctx.t("webhookDeliveryLog"))
        .subtitle(ctx.t("webhookLogSubtitle"))
        .row(row)
}

#[cfg(test)]
mod tests {
    use super::{WEBHOOK_EVENTS, cloud_events, parse_endpoints};

    /// 本机 webhook 订阅全部引擎事件；云端渠道不提供 FluxCloud 不认识的 `queue.started`，
    /// 其余事件顺序不变。
    #[test]
    fn cloud_events_drop_only_local_only_queue_started() {
        assert!(
            WEBHOOK_EVENTS
                .iter()
                .any(|(wire, _)| *wire == "queue.started")
        );
        let cloud: Vec<&str> = cloud_events().map(|(wire, _)| *wire).collect();
        assert!(!cloud.contains(&"queue.started"));
        let expected: Vec<&str> = WEBHOOK_EVENTS
            .iter()
            .map(|(wire, _)| *wire)
            .filter(|wire| *wire != "queue.started")
            .collect();
        assert_eq!(cloud, expected);
    }

    #[test]
    fn malformed_fields_fall_back_like_engine_and_web() {
        let list = parse_endpoints(
            r#"[{"id":"a","enabled":"yes","events":["task.failed",3],"headers":{"K":"v","N":1}},
                 42,
                 {"id":"b","enabled":false,"allowHttp":"true"}]"#,
        );
        assert_eq!(list.len(), 2);
        assert!(list[0].enabled);
        assert_eq!(list[0].events, ["task.failed"]);
        assert_eq!(list[0].headers.len(), 1);
        assert!(!list[1].enabled);
        assert!(!list[1].allow_http);
        assert!(parse_endpoints("{ not json").is_empty());
        assert!(parse_endpoints(r#"{"id":"a"}"#).is_empty());
    }
}
