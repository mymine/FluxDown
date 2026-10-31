//! 「我的分享」列表：行内操作（复制链接 / 二维码 / 停止·删除 / 更多）、批量条与空态。

use std::ops::Range;

use fluxdown_protocol::{ShareDto, ShareStatus};
use fluxdown_ui_components::{
    BusyExt as _, ButtonVariant, ControlExt as _, DialogIntent, FluxIcon, IconControlExt as _,
    icon_button, loading_icon_button, tabular_numbers,
};
use fluxdown_ui_i18n::Translator;
use fluxdown_ui_theme::active_theme;
use gpui::{
    Anchor, App, ClipboardItem, Context, Div, ElementId, Hsla, InteractiveElement as _,
    IntoElement, ParentElement as _, Pixels, SharedString, StatefulInteractiveElement as _,
    Styled as _, Window, div, uniform_list,
};
use gpui_component::{
    Disableable as _, Icon, WindowExt as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    menu::{DropdownMenu as _, PopupMenuItem},
    notification::Notification,
    v_flex,
};

use super::{
    ShareView,
    table::{columns, or_placeholder, row_height},
};
use crate::{
    actions::notify_error,
    appeal::open_appeal_dialog,
    cloud_ui::{confirm_delete_cloud_copy, open_convert_dialog},
    errors::error_text,
    manage_ui::{
        body_text, chip, clipped, empty_state, meta_text, text, text_with, tooltip_builder,
    },
    model::{
        AppealUi, PanelTarget, ShareAction, appeal_state, batch_actions, claimed_summary,
        cloud::{CloudPhase, cloud_actions},
        format_bytes, format_datetime, is_editable, ordered_shares, share_headline,
        share_status_key, source_missing_badge,
    },
    qr::open_share_qr,
};

pub(super) fn share_status_tone(status: ShareStatus, cx: &App) -> Option<Hsla> {
    let colors = active_theme(cx).extended().colors;
    match status {
        ShareStatus::Active => Some(colors.success),
        ShareStatus::Takedown => Some(active_theme(cx).tokens().colors.destructive),
        ShareStatus::Expired | ShareStatus::Stopped | ShareStatus::Unknown => None,
    }
}

impl ShareView {
    pub(super) fn open_share(&self, share_id: &str, window: &mut Window, cx: &mut App) {
        let open = self.hub.read(cx).host().open_panel.clone();
        if let Some(open) = open {
            open(PanelTarget::Share(share_id.to_owned()), window, cx);
        }
    }

    pub(super) fn copy_link(&self, url: String, window: &mut Window, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(url));
        let message = text(self.translator.read(cx), "shareLinkCopied");
        window.push_notification(Notification::success(message), cx);
    }

    /// 确认后停止分享（仍有效的分享才可停止）。
    pub(super) fn confirm_stop(
        &mut self,
        share_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let translator = self.translator.read(cx).clone();
        let title = text(&translator, "shareStop");
        let description = text(&translator, "shareStopConfirm");
        let cancel = text(&translator, "cancel");
        let view = cx.weak_entity();
        window.open_alert_dialog(cx, move |alert, _, cx| {
            let view = view.clone();
            let share_id = share_id.clone();
            alert
                .title(fluxdown_ui_components::dialog_title(title.clone(), cx))
                .description(description.clone())
                .footer(fluxdown_ui_components::dialog_footer(
                    Some(cancel.clone()),
                    title.clone(),
                    fluxdown_ui_components::DialogIntent::Destructive,
                    cx,
                ))
                .on_ok(move |_, window, cx| {
                    view.update(cx, |this, cx| this.stop_share(share_id.clone(), window, cx))
                        .is_ok()
                })
        });
    }

    /// 确认后删除分享记录（单项与批量共用；分享中的由 agent 先停止）。
    pub(super) fn confirm_delete(
        &mut self,
        ids: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ids.is_empty() {
            return;
        }
        let translator = self.translator.read(cx).clone();
        let title = text(&translator, "shareDeleteConfirmTitle");
        let description = text_with(
            &translator,
            "shareDeleteConfirmBody",
            &[("n", &ids.len().to_string())],
        );
        let ok = text(&translator, "shareActionDelete");
        let cancel = text(&translator, "cancel");
        let view = cx.weak_entity();
        window.open_alert_dialog(cx, move |alert, _, cx| {
            let view = view.clone();
            let ids = ids.clone();
            alert
                .title(fluxdown_ui_components::dialog_title(title.clone(), cx))
                .description(description.clone())
                .footer(fluxdown_ui_components::dialog_footer(
                    Some(cancel.clone()),
                    ok.clone(),
                    DialogIntent::Destructive,
                    cx,
                ))
                .on_ok(move |_, window, cx| {
                    view.update(cx, |this, cx| {
                        this.run_action(ShareAction::Delete, ids.clone(), window, cx);
                    })
                    .is_ok()
                })
        });
    }

    pub(super) fn stop_share(&mut self, share_id: String, window: &Window, cx: &mut Context<Self>) {
        self.run_action(ShareAction::Stop, vec![share_id], window, cx);
    }

    /// 对 `ids` 串行执行操作：防重入（`busy`）、逐个调用单项 RPC，失败汇总后提示一次。
    pub(super) fn run_action(
        &mut self,
        action: ShareAction,
        ids: Vec<String>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let ids: Vec<String> = ids
            .into_iter()
            .filter(|id| match self.busy.entry(id.clone()) {
                std::collections::hash_map::Entry::Occupied(_) => false,
                std::collections::hash_map::Entry::Vacant(slot) => {
                    slot.insert(action);
                    true
                }
            })
            .collect();
        if ids.is_empty() {
            return;
        }
        let handle = window.window_handle();
        let translator = self.translator.read(cx).clone();
        let api = self.hub.read(cx).api();
        let single = ids.len() == 1;
        cx.spawn(async move |this, cx| {
            let mut failed = 0_usize;
            let mut reason: Option<String> = None;
            for id in ids {
                let result = match action {
                    ShareAction::Start => api.resume(&id).await.map(drop),
                    ShareAction::Stop => api.stop(&id).await.map(drop),
                    ShareAction::Delete => api.delete(&id).await.map(drop),
                };
                let Ok(()) = this.update(cx, |this, cx| {
                    this.busy.remove(&id);
                    cx.notify();
                }) else {
                    return;
                };
                if let Err(error) = result {
                    failed += 1;
                    reason.get_or_insert_with(|| error_text(&translator, &error));
                }
            }
            let Some(reason) = reason else {
                return;
            };
            let message = if single {
                reason
            } else {
                text_with(
                    &translator,
                    "shareBatchPartialFailed",
                    &[("failed", &failed.to_string()), ("reason", &reason)],
                )
                .to_string()
            };
            notify_error(handle, cx, message);
        })
        .detach();
        cx.notify();
    }

    fn toggle_selected(&mut self, id: &str, checked: bool, cx: &mut Context<Self>) {
        let changed = if checked {
            self.selected.insert(id.to_owned())
        } else {
            self.selected.remove(id)
        };
        if changed {
            cx.notify();
        }
    }

    /// 全选 / 取消全选（已全选则清空）。
    fn toggle_select_all(&mut self, cx: &mut Context<Self>) {
        let all: Vec<String> = self
            .hub
            .read(cx)
            .state()
            .shares
            .iter()
            .map(|share| share.id.clone())
            .collect();
        if !all.is_empty() && self.selected.len() == all.len() {
            self.selected.clear();
        } else {
            self.selected = all.into_iter().collect();
        }
        cx.notify();
    }

    /// 选中项里适用于 `action` 的分享 ID（列表顺序）。
    fn batch_targets(&self, action: ShareAction, cx: &App) -> Vec<String> {
        ordered_shares(&self.hub.read(cx).state().shares)
            .into_iter()
            .filter(|share| self.selected.contains(&share.id) && action.applies(share))
            .map(|share| share.id.clone())
            .collect()
    }

    /// 选中项里是否有正由 `action` 处理的分享：批量按钮据此转圈。
    fn batch_busy(&self, action: ShareAction) -> bool {
        self.selected
            .iter()
            .any(|id| self.busy.get(id) == Some(&action))
    }

    /// 批量操作条：已选数量 + 开始 / 停止 / 删除。
    fn render_batch_bar(&self, translator: &Translator, cx: &mut Context<Self>) -> Div {
        let tokens = active_theme(cx).tokens().clone();
        let stale = self.hub.read(cx).is_stale();
        let actions = batch_actions(
            self.hub
                .read(cx)
                .state()
                .shares
                .iter()
                .filter(|share| self.selected.contains(&share.id)),
        );
        let count = text_with(
            translator,
            "shareSelectedCount",
            &[("n", &self.selected.len().to_string())],
        );
        h_flex()
            .w_full()
            .flex_none()
            .items_center()
            .gap(tokens.spacing.sm)
            .px(tokens.spacing.sm)
            .child(body_text(cx).mr(tokens.spacing.sm).child(count))
            .child(
                Button::new("share-batch-start")
                    .outline()
                    .icon(FluxIcon::Play)
                    .label(text(translator, "shareBatchStart"))
                    .control(cx)
                    .busy(self.batch_busy(ShareAction::Start))
                    .disabled(!actions.start || stale || self.batch_busy(ShareAction::Start))
                    .on_click(cx.listener(|this, _, window, cx| {
                        let ids = this.batch_targets(ShareAction::Start, cx);
                        this.run_action(ShareAction::Start, ids, window, cx);
                    })),
            )
            .child(
                Button::new("share-batch-stop")
                    .outline()
                    .icon(FluxIcon::X)
                    .label(text(translator, "shareBatchStop"))
                    .control(cx)
                    .busy(self.batch_busy(ShareAction::Stop))
                    .disabled(!actions.stop || stale || self.batch_busy(ShareAction::Stop))
                    .on_click(cx.listener(|this, _, window, cx| {
                        let ids = this.batch_targets(ShareAction::Stop, cx);
                        this.run_action(ShareAction::Stop, ids, window, cx);
                    })),
            )
            .child(
                Button::new("share-batch-delete")
                    .outline()
                    .icon(FluxIcon::Trash2)
                    .label(text(translator, "shareBatchDelete"))
                    .control(cx)
                    .busy(self.batch_busy(ShareAction::Delete))
                    .disabled(!actions.delete || stale || self.batch_busy(ShareAction::Delete))
                    .on_click(cx.listener(|this, _, window, cx| {
                        let ids = this.batch_targets(ShareAction::Delete, cx);
                        this.confirm_delete(ids, window, cx);
                    })),
            )
    }

    pub(super) fn render_mine(&self, translator: &Translator, cx: &mut Context<Self>) -> Div {
        let count = self.hub.read(cx).state().shares.len();
        let container = v_flex().flex_1().min_h_0().w_full();
        if count == 0 {
            return container.child(empty_state(
                FluxIcon::Share2,
                None,
                text(translator, "shareMineEmpty"),
                None,
                cx,
            ));
        }
        let cols = columns(cx);
        let all_selected = self.selected.len() == count;
        let select_all = h_flex()
            .w(cols.select)
            .flex_none()
            .justify_center()
            .child(
                Checkbox::new("share-select-all")
                    .checked(all_selected)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_select_all(cx))),
            )
            .into_any_element();
        let batch_bar = (!self.selected.is_empty()).then(|| self.render_batch_bar(translator, cx));
        container
            .children(batch_bar)
            .child(self.header_row(
                vec![
                    (text(translator, "shareColFile"), None),
                    (text(translator, "shareColClaims"), Some(cols.claims)),
                    (text(translator, "shareColStatus"), Some(cols.status)),
                ],
                cols.share_actions,
                Some(select_all),
                cx,
            ))
            .child(
                uniform_list(
                    "share-mine",
                    count,
                    cx.processor(move |this, range: Range<usize>, _, cx| {
                        // 只复制可见行，行渲染需要可变 `cx`（监听器），不能同时借着 hub 状态。
                        let rows: Vec<ShareDto> = {
                            let state = this.hub.read(cx).state();
                            ordered_shares(&state.shares)
                                .into_iter()
                                .skip(range.start)
                                .take(range.len())
                                .cloned()
                                .collect()
                        };
                        let translator = this.translator.read(cx).clone();
                        rows.into_iter()
                            .enumerate()
                            .map(|(offset, share)| {
                                this.render_share_row(range.start + offset, share, &translator, cx)
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .flex_1()
                .min_h_0()
                .w_full(),
            )
    }

    pub(super) fn render_share_row(
        &self,
        index: usize,
        share: ShareDto,
        translator: &Translator,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = active_theme(cx);
        let tokens = theme.tokens().clone();
        let colors = tokens.colors;
        let cols = columns(cx);
        let stale = self.hub.read(cx).is_stale();
        let editable = is_editable(&share);
        let busy = self.busy.contains_key(&share.id);
        let busy_action = self.busy.get(&share.id).copied();
        let selecting = !self.selected.is_empty();
        let checked = self.selected.contains(&share.id);
        let headline = or_placeholder(share_headline(&share));
        let row_id = |part: &str| {
            ElementId::NamedInteger(
                SharedString::from(format!("share-mine-{part}")),
                index as u64,
            )
        };

        // 副行：文件数 · 大小 · 到期时间 [· 活动接收方]。
        let files = text_with(
            translator,
            "shareFilesCount",
            &[("count", &share.files.len().to_string())],
        );
        let mut meta = vec![files.to_string(), format_bytes(share.total_bytes)];
        let expires = format_datetime(&share.expires_at);
        if !expires.trim().is_empty() {
            meta.push(expires);
        }
        if !share.active_receivers.is_empty() {
            meta.push(
                text_with(
                    translator,
                    "shareActiveReceivers",
                    &[("count", &share.active_receivers.len().to_string())],
                )
                .to_string(),
            );
        }
        let (claimed_key, claimed, max) = claimed_summary(share.claim_count, share.max_claims);
        let claims = text_with(
            translator,
            claimed_key,
            &[("count", &claimed.to_string()), ("max", &max.to_string())],
        );
        let (status_label, status_tone) = if source_missing_badge(&share) {
            (
                text(translator, "shareStatusSourceMissing"),
                Some(colors.destructive),
            )
        } else {
            (
                text(translator, share_status_key(share.status)),
                share_status_tone(share.status, cx),
            )
        };
        let mut status = chip(row_id("status"), status_label, status_tone, cx);
        if share.status == ShareStatus::Takedown
            && let Some(reason) = share.takedown_reason.as_deref().filter(|r| !r.is_empty())
        {
            let tip = text_with(translator, "shareTakedownReason", &[("reason", reason)]);
            status = status.tooltip(tooltip_builder(tip));
        }

        let icon_size = theme.extended().icon.md;
        let copy_label = text(translator, "shareCopyLink");
        let qr_label = text(translator, "shareShowQr");
        let stop_label = text(translator, "shareStop");
        let delete_label = text(translator, "shareActionDelete");
        let (copy_url, qr_url, qr_title) = (share.url.clone(), share.url.clone(), headline.clone());
        let (open_id, stop_id, delete_id, select_id) = (
            share.id.clone(),
            share.id.clone(),
            share.id.clone(),
            share.id.clone(),
        );
        let hub_translator = self.translator.clone();

        // 有效分享 = 停止；其余 = 删除。
        let stop_or_delete = if editable {
            loading_icon_button(
                row_id("stop"),
                stop_label.clone(),
                Icon::new(FluxIcon::X)
                    .size(icon_size)
                    .text_color(colors.destructive),
                ButtonVariant::Ghost,
                busy_action == Some(ShareAction::Stop),
                cx,
            )
            .tooltip(tooltip_builder(stop_label))
            .disabled(stale || busy)
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.confirm_stop(stop_id.clone(), window, cx);
            }))
        } else {
            loading_icon_button(
                row_id("delete"),
                delete_label.clone(),
                Icon::new(FluxIcon::Trash2)
                    .size(icon_size)
                    .text_color(colors.destructive),
                ButtonVariant::Ghost,
                busy_action == Some(ShareAction::Delete),
                cx,
            )
            .tooltip(tooltip_builder(delete_label))
            .disabled(stale || busy)
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.confirm_delete(vec![delete_id.clone()], window, cx);
            }))
        };

        let more_items = self.more_items(&share, translator, stale || busy, cx);
        let more_button = if more_items.is_empty() {
            // 没有可用项时留出按钮位，保持图标列对齐。
            div()
                .w(theme.density().control)
                .flex_none()
                .into_any_element()
        } else {
            let label = text(translator, "moreActions");
            let (hub, view, id) = (self.hub.clone(), cx.weak_entity(), share.id.clone());
            // 外包一层吞掉点击冒泡：菜单触发不应同时打开分享面板。
            div()
                .id(row_id("more-wrap"))
                .flex_none()
                .tooltip(tooltip_builder(label))
                .on_click(|_, _, cx| cx.stop_propagation())
                .child(
                    Button::new(row_id("more"))
                        .ghost()
                        .icon(Icon::new(FluxIcon::Ellipsis).size(icon_size))
                        .control_icon(cx)
                        .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                            more_items.iter().fold(menu, |menu, item| {
                                let mut entry = PopupMenuItem::new(item.label.clone())
                                    .disabled(item.disabled || item.action.is_none());
                                if let Some(action) = item.action {
                                    let (hub, view, id) = (hub.clone(), view.clone(), id.clone());
                                    entry = entry.on_click(move |_, window, cx| match action {
                                        MoreAction::ConvertCloud => {
                                            open_convert_dialog(&hub, id.clone(), window, cx);
                                        }
                                        MoreAction::DeleteCloudCopy { uploading } => {
                                            confirm_delete_cloud_copy(
                                                &hub,
                                                id.clone(),
                                                uploading,
                                                window,
                                                cx,
                                            );
                                        }
                                        MoreAction::Appeal => {
                                            open_appeal_dialog(&hub, id.clone(), window, cx);
                                        }
                                        MoreAction::Start => {
                                            let Ok(()) = view.update(cx, |this, cx| {
                                                this.run_action(
                                                    ShareAction::Start,
                                                    vec![id.clone()],
                                                    window,
                                                    cx,
                                                );
                                            }) else {
                                                // 分享页已释放。
                                                return;
                                            };
                                        }
                                    });
                                }
                                menu.item(entry)
                            })
                        }),
                )
                .into_any_element()
        };

        let group = SharedString::from(format!("share-mine-group-{index}"));
        let select_cell = h_flex()
            .w(cols.select)
            .flex_none()
            .justify_center()
            .opacity(if selecting || checked { 1. } else { 0. })
            .group_hover(group.clone(), |style| style.opacity(1.))
            .child(
                Checkbox::new(row_id("select"))
                    .checked(checked)
                    .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                        cx.stop_propagation();
                        this.toggle_selected(&select_id, *checked, cx);
                    })),
            );

        let cell = |width: Pixels, value: SharedString, id: ElementId| {
            body_text(cx)
                .w(width)
                .flex_none()
                .font_features(tabular_numbers())
                .child(clipped(id, value))
        };

        h_flex()
            .id(row_id("row"))
            .w_full()
            .h(row_height(cx))
            .flex_none()
            .items_center()
            .gap(tokens.spacing.md)
            .px(tokens.spacing.sm)
            .rounded(tokens.radius.md)
            .cursor_pointer()
            .group(group)
            .hover(move |style| style.bg(theme.extended().colors.row_hover))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_share(&open_id, window, cx);
            }))
            .child(select_cell)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(body_text(cx).child(clipped(row_id("title"), headline)))
                    .child(meta_text(cx).child(clipped(row_id("meta"), meta.join(" · ")))),
            )
            .child(cell(cols.claims, claims, row_id("claims")))
            .child(h_flex().w(cols.status).flex_none().min_w_0().child(status))
            .child(
                h_flex()
                    .w(cols.share_actions)
                    .flex_none()
                    .justify_end()
                    .gap(tokens.spacing.xxs)
                    .child(
                        icon_button(
                            row_id("copy"),
                            copy_label.clone(),
                            Icon::new(FluxIcon::Copy).size(icon_size),
                            ButtonVariant::Ghost,
                            cx,
                        )
                        .tooltip(tooltip_builder(copy_label))
                        .disabled(!editable)
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.copy_link(copy_url.clone(), window, cx);
                            },
                        )),
                    )
                    .child(
                        icon_button(
                            row_id("qr"),
                            qr_label.clone(),
                            Icon::new(FluxIcon::QrCode).size(icon_size),
                            ButtonVariant::Ghost,
                            cx,
                        )
                        .tooltip(tooltip_builder(qr_label))
                        .disabled(!editable)
                        .on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            open_share_qr(&hub_translator, &qr_title, &qr_url, window, cx);
                        }),
                    )
                    .child(stop_or_delete)
                    .child(more_button),
            )
    }

    /// 行尾「更多」菜单的条目：云端转存 / 删除云端副本、申诉（或申诉状态）、重新开始。
    /// `locked` = 与 agent 断开或该分享正在提交操作。
    fn more_items(
        &self,
        share: &ShareDto,
        translator: &Translator,
        locked: bool,
        cx: &App,
    ) -> Vec<MoreItem> {
        let mut items = Vec::new();
        let cloud = {
            let hub = self.hub.read(cx);
            cloud_actions(
                share,
                &hub.state().policy_summary,
                hub.signed_in(),
                hub.can_send(),
            )
        };
        if let Some(block) = cloud.convert {
            let label = text(
                translator,
                if cloud.phase == CloudPhase::Failed {
                    "shareStageRetry"
                } else {
                    "shareStageConvert"
                },
            );
            let label = block.map_or_else(
                || label.clone(),
                |block| SharedString::from(format!("{label} · {}", translator.text(block.key()))),
            );
            items.push(MoreItem {
                label,
                disabled: block.is_some() || locked,
                action: Some(MoreAction::ConvertCloud),
            });
        } else if cloud.delete {
            let uploading = cloud.phase.has_upload();
            items.push(MoreItem {
                label: text(
                    translator,
                    if uploading {
                        "shareStageCancelUpload"
                    } else {
                        "shareStageDelete"
                    },
                ),
                disabled: locked,
                action: Some(MoreAction::DeleteCloudCopy { uploading }),
            });
        }
        if is_editable(share) {
            return items;
        }
        match appeal_state(share) {
            AppealUi::CanAppeal => items.push(MoreItem {
                label: text(translator, "shareAppeal"),
                disabled: locked,
                action: Some(MoreAction::Appeal),
            }),
            // 处理中 / 未通过：只作状态展示（不可点）。
            state @ (AppealUi::Pending | AppealUi::Rejected) => items.push(MoreItem {
                label: text(
                    translator,
                    if state == AppealUi::Pending {
                        "shareAppealPending"
                    } else {
                        "shareAppealRejected"
                    },
                ),
                disabled: true,
                action: None,
            }),
            AppealUi::None if share.status != ShareStatus::Takedown => items.push(MoreItem {
                label: text(
                    translator,
                    if share.source_missing {
                        "shareStartDisabledMissing"
                    } else {
                        "shareActionStart"
                    },
                ),
                disabled: !ShareAction::Start.applies(share) || locked,
                action: Some(MoreAction::Start),
            }),
            AppealUi::None => {}
        }
        items
    }
}

/// 「更多」菜单里的动作。
#[derive(Clone, Copy)]
enum MoreAction {
    ConvertCloud,
    DeleteCloudCopy { uploading: bool },
    Appeal,
    Start,
}

/// 「更多」菜单的一个条目；`action` 为空 = 只读状态行。
struct MoreItem {
    label: SharedString,
    disabled: bool,
    action: Option<MoreAction>,
}
