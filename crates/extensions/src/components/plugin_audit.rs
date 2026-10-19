//! 插件审计对话框：按插件筛选、`beforeId` 翻页、复制 JSON、二次确认清空。
//!
//! 动作码（`install`、`consent.granted`、`desktop.load` 等）直接显示原文，不翻译。

use std::{sync::Arc, time::Duration};

use fluxdown_protocol::{PluginAuditEntryDto, PluginAuditListDto};
use fluxdown_ui_components::{
    BusyExt as _, ControlExt as _, FluxIcon, dialog_scroll_body, tabular_numbers,
};
use fluxdown_ui_i18n::Translator;
use fluxdown_ui_theme::active_theme;
use gpui::{
    Anchor, App, AppContext as _, ClipboardItem, Context, Entity, FontWeight, IntoElement,
    ParentElement, Render, SharedString, Styled, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    Disableable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::{DropdownMenu as _, PopupMenuItem},
    v_flex,
};

use crate::{
    ExtensionsPort,
    controller::{clear_plugin_audit, list_plugin_audit},
    error_text,
    pages::Frame,
    ui,
};

/// 每次翻页条数。
const PAGE_SIZE: u32 = 100;

/// 本地时间 `YYYY-MM-DD HH:MM:SS`（unix 毫秒）；超出可表示范围时原样显示毫秒数。
pub(crate) fn format_local(at_ms: i64) -> String {
    use chrono::{Local, TimeZone as _};
    Local.timestamp_millis_opt(at_ms).single().map_or_else(
        || at_ms.to_string(),
        |at| at.format("%Y-%m-%d %H:%M:%S").to_string(),
    )
}

struct PluginAuditDialog {
    translator: Entity<Translator>,
    port: Arc<dyn ExtensionsPort>,
    /// （identity, 展示名）；筛选下拉的候选。
    plugins: Vec<(String, String)>,
    filter: Option<String>,
    entries: Vec<PluginAuditEntryDto>,
    next_before_id: Option<i64>,
    loading: bool,
    error: Option<SharedString>,
    clear_armed: bool,
    copied: bool,
    /// 筛选 / 清空会让在途翻页作废。
    generation: u64,
}

impl PluginAuditDialog {
    fn load(&mut self, reset: bool, cx: &mut Context<Self>) {
        if self.loading && !reset {
            return;
        }
        let before = if reset { None } else { self.next_before_id };
        if !reset && before.is_none() {
            return;
        }
        if reset {
            self.generation = self.generation.wrapping_add(1);
        }
        let generation = self.generation;
        self.loading = true;
        self.error = None;
        cx.notify();
        let future = list_plugin_audit(&self.port, self.filter.as_deref(), before, PAGE_SIZE);
        cx.spawn(async move |this, cx| {
            let result = future.await.and_then(|value| {
                serde_json::from_value::<PluginAuditListDto>(value).map_err(|_| {
                    fluxdown_protocol::RpcErrorData::new(
                        fluxdown_protocol::ApplicationErrorCode::Internal,
                        false,
                    )
                })
            });

            let Ok(()) = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.loading = false;
                match result {
                    Ok(page) => {
                        if reset {
                            this.entries = page.entries;
                        } else {
                            this.entries.extend(page.entries);
                        }
                        this.next_before_id = page.next_before_id;
                    }
                    Err(error) => {
                        let message = error_text(this.translator.read(cx), &error);
                        this.error = Some(message.into());
                    }
                }
                cx.notify();
            }) else {
                // 对话框已释放，停止回写异步结果。
                return;
            };
        })
        .detach();
    }

    fn set_filter(&mut self, filter: Option<String>, cx: &mut Context<Self>) {
        if self.filter == filter {
            return;
        }
        self.filter = filter;
        self.entries.clear();
        self.next_before_id = None;
        self.clear_armed = false;
        self.load(true, cx);
    }

    /// 第一次点击只进入确认态，第二次才真正清空。
    fn clear(&mut self, cx: &mut Context<Self>) {
        if !self.clear_armed {
            self.clear_armed = true;
            cx.notify();
            return;
        }
        self.clear_armed = false;
        let future = clear_plugin_audit(&self.port, self.filter.as_deref());
        cx.spawn(async move |this, cx| {
            let result = future.await;

            let Ok(()) = this.update(cx, |this, cx| {
                match result {
                    // 清空本身会记一条 `audit.clear`，重新拉取以呈现它。
                    Ok(_) => this.load(true, cx),
                    Err(error) => {
                        let message = error_text(this.translator.read(cx), &error);
                        this.error = Some(message.into());
                        cx.notify();
                    }
                }
            }) else {
                // 对话框已释放，停止回写异步结果。
                return;
            };
        })
        .detach();
        cx.notify();
    }

    fn copy_json(&mut self, cx: &mut Context<Self>) {
        let json = match serde_json::to_string_pretty(&self.entries) {
            Ok(json) => json,
            Err(error) => {
                self.error = Some(error.to_string().into());
                cx.notify();
                return;
            }
        };
        cx.write_to_clipboard(ClipboardItem::new_string(json));
        self.copied = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1500))
                .await;

            let Ok(()) = this.update(cx, |this, cx| {
                this.copied = false;
                cx.notify();
            }) else {
                // 对话框已释放，结束回调。
                return;
            };
        })
        .detach();
    }

    fn filter_label(&self, cx: &Context<Self>) -> String {
        let translator = self.translator.read(cx);
        match &self.filter {
            None => translator.text("pluginAuditFilterAll").to_owned(),
            Some(identity) => self
                .plugins
                .iter()
                .find(|(id, _)| id == identity)
                .map_or_else(|| identity.clone(), |(_, name)| name.clone()),
        }
    }
}

/// 列宽（px）：时间 / 来源 / 动作 / 结果固定，目标与详情弹性。
const COL_TIME: f32 = 150.;
const COL_ORIGIN: f32 = 64.;
const COL_ACTION: f32 = 140.;
const COL_OUTCOME: f32 = 64.;

impl Render for PluginAuditDialog {
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
        let text = |key: &str| SharedString::from(translator.text(key).to_owned());
        let show_identity = self.filter.is_none();

        let header_cell = |label: SharedString| {
            div()
                .text_size(frame.extended.caption.size)
                .line_height(frame.extended.caption.line_height)
                .font_weight(FontWeight::MEDIUM)
                .text_color(frame.extended.colors.text_tertiary)
                .child(label)
        };
        let header = h_flex()
            .w_full()
            .gap(tokens.spacing.sm)
            .child(header_cell(text("pluginAuditColTime")).w(theme.text_extent(COL_TIME)))
            .child(header_cell(text("pluginAuditColOrigin")).w(theme.text_extent(COL_ORIGIN)))
            .child(header_cell(text("pluginAuditColAction")).w(theme.text_extent(COL_ACTION)))
            .child(header_cell(text("pluginAuditColTarget")).flex_1().min_w_0())
            .child(header_cell(text("pluginAuditColOutcome")).w(theme.text_extent(COL_OUTCOME)))
            .child(header_cell(text("pluginAuditColDetail")).flex_1().min_w_0());

        let rows = self
            .entries
            .iter()
            .map(|entry| {
                let outcome_color = match entry.outcome.as_str() {
                    "denied" => frame.extended.colors.warning,
                    "error" => tokens.colors.destructive,
                    _ => tokens.colors.muted_foreground,
                };
                h_flex()
                    .w_full()
                    .gap(tokens.spacing.sm)
                    .items_start()
                    .child(
                        ui::meta_text(format_local(entry.at_ms), frame)
                            .font_features(tabular_numbers())
                            .w(theme.text_extent(COL_TIME))
                            .flex_none(),
                    )
                    .child(
                        ui::meta_text(entry.origin.clone(), frame)
                            .w(theme.text_extent(COL_ORIGIN))
                            .flex_none(),
                    )
                    .child(
                        v_flex()
                            .w(theme.text_extent(COL_ACTION))
                            .flex_none()
                            .child(ui::body_text(entry.action.clone(), frame))
                            .when(show_identity, |this| {
                                this.child(ui::meta_text(entry.identity.clone(), frame))
                            }),
                    )
                    .child(
                        ui::meta_text(entry.target.clone(), frame)
                            .flex_1()
                            .min_w_0()
                            .overflow_x_hidden(),
                    )
                    .child(
                        ui::meta_text(entry.outcome.clone(), frame)
                            .text_color(outcome_color)
                            .w(theme.text_extent(COL_OUTCOME))
                            .flex_none(),
                    )
                    .child(
                        ui::meta_text(entry.detail.clone(), frame)
                            .flex_1()
                            .min_w_0()
                            .overflow_x_hidden(),
                    )
            })
            .collect::<Vec<_>>();

        let options = self.plugins.clone();
        let current = self.filter.clone();
        let all_label = text("pluginAuditFilterAll");
        let view = cx.entity();
        let toolbar = h_flex()
            .w_full()
            .gap(tokens.spacing.sm)
            .items_center()
            .child(
                Button::new("plugin-audit-filter")
                    .outline()
                    .label(self.filter_label(cx))
                    .dropdown_caret(true)
                    .control(cx)
                    .dropdown_menu_with_anchor(Anchor::TopLeft, {
                        let view = view.clone();
                        move |menu, _, _| {
                            let all = {
                                let view = view.clone();
                                PopupMenuItem::new(all_label.clone())
                                    .checked(current.is_none())
                                    .on_click(move |_, _, cx| {
                                        view.update(cx, |this, cx| this.set_filter(None, cx));
                                    })
                            };
                            options
                                .iter()
                                .fold(menu.item(all), |menu, (identity, name)| {
                                    let view = view.clone();
                                    let target = identity.clone();
                                    menu.item(
                                        PopupMenuItem::new(name.clone())
                                            .checked(current.as_deref() == Some(identity.as_str()))
                                            .on_click(move |_, _, cx| {
                                                let target = target.clone();
                                                view.update(cx, |this, cx| {
                                                    this.set_filter(Some(target), cx);
                                                });
                                            }),
                                    )
                                })
                                .scrollable(true)
                                .max_h(px(320.))
                        }
                    }),
            )
            .child(div().flex_1())
            .child(
                Button::new("plugin-audit-copy")
                    .ghost()
                    .control(cx)
                    .icon(FluxIcon::Copy)
                    .label(if self.copied {
                        text("pluginAuditCopied")
                    } else {
                        text("pluginAuditCopyJson")
                    })
                    .disabled(self.entries.is_empty())
                    .on_click(cx.listener(|this, _, _, cx| this.copy_json(cx))),
            )
            .child({
                let armed = self.clear_armed;
                let button = Button::new("plugin-audit-clear")
                    .control(cx)
                    .label(if armed {
                        text("pluginAuditClearConfirm")
                    } else {
                        text("pluginAuditClear")
                    })
                    .disabled(self.entries.is_empty() && !armed)
                    .on_click(cx.listener(|this, _, _, cx| this.clear(cx)));
                if armed {
                    button.danger()
                } else {
                    button.ghost()
                }
            });

        v_flex()
            .w_full()
            .gap(tokens.spacing.sm)
            .child(toolbar)
            .when_some(self.error.clone(), |this, error| {
                this.child(ui::status_line(
                    FluxIcon::CircleAlert,
                    tokens.colors.destructive,
                    error,
                    frame,
                ))
            })
            .child(header)
            .child(ui::divider(frame))
            .when(rows.is_empty() && !self.loading, |this| {
                this.child(ui::meta_text(text("pluginAuditEmpty"), frame))
            })
            .when(rows.is_empty() && self.loading, |this| {
                this.child(ui::meta_text(text("pluginCommonLoading"), frame))
            })
            .children(rows)
            .when(self.next_before_id.is_some(), |this| {
                this.child(
                    h_flex().justify_center().child(
                        Button::new("plugin-audit-more")
                            .ghost()
                            .control(cx)
                            .label(text("pluginAuditLoadMore"))
                            .busy(self.loading)
                            .disabled(self.loading)
                            .on_click(cx.listener(|this, _, _, cx| this.load(false, cx))),
                    ),
                )
            })
    }
}

/// 打开审计对话框；`filter` = 预选插件（`None` = 全部插件）。
pub fn open_plugin_audit(
    plugins: Vec<(String, String)>,
    filter: Option<String>,
    translator: Entity<Translator>,
    port: Arc<dyn ExtensionsPort>,
    window: &mut Window,
    cx: &mut App,
) {
    let title = SharedString::from(translator.read(cx).text("pluginAuditTitle").to_owned());
    let state = cx.new(|_| PluginAuditDialog {
        translator,
        port,
        plugins,
        filter,
        entries: Vec::new(),
        next_before_id: None,
        loading: false,
        error: None,
        clear_armed: false,
        copied: false,
        generation: 0,
    });
    state.update(cx, |this, cx| this.load(true, cx));
    window.open_dialog(cx, move |dialog, _, cx| {
        let content = state.clone();
        dialog
            .title(fluxdown_ui_components::dialog_title(title.clone(), cx))
            .w(active_theme(cx).text_extent(960.))
            .content(move |root, _, cx| {
                root.min_h_0().child(dialog_scroll_body(
                    "plugin-audit-body",
                    Some(px(520.)),
                    content.clone(),
                    cx,
                ))
            })
    });
}

#[cfg(test)]
mod tests {
    use chrono::{Local, TimeZone as _};

    use super::format_local;

    #[test]
    fn formats_in_local_time_and_falls_back_out_of_range() {
        let at = Local
            .with_ymd_and_hms(2024, 2, 29, 12, 34, 56)
            .single()
            .expect("valid local time");
        assert_eq!(format_local(at.timestamp_millis()), "2024-02-29 12:34:56");
        assert_eq!(format_local(i64::MAX), i64::MAX.to_string());
    }
}
