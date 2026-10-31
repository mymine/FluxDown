//! 分享面板窗口内容：创建（含哈希进度）→ 链接 / 取件码 / 二维码 → 就地改设置 → 本机网络与接收方 → 停止。
//!
//! 内容一律由 [`ShareHub`] 里的分享渲染（hub 变化经 `cx.observe` 重绘）；面板自己只持有流程阶段、
//! 在途标记与最近一次操作错误。窗口由 app 创建并外包标题栏，关闭走 `window.remove_window()`。

use std::collections::HashSet;

use fluxdown_protocol::{
    RpcErrorData, ShareCreateParams, ShareDto, ShareMode, ShareReceiverDto, ShareStatus,
    ShareUpdateParams,
};
use fluxdown_ui_components::{
    BusyExt as _, ControlExt as _, DialogIntent, FluxIcon, card, dialog_footer, dialog_title,
    field_error, field_label, tabular_numbers, tooltip_view,
};
use fluxdown_ui_i18n::Translator;
use fluxdown_ui_theme::active_theme;
use gpui::{
    Anchor, App, ClipboardItem, Context, Div, Entity, FontWeight, Hsla, InteractiveElement as _,
    IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement as _, Styled,
    Window, div, prelude::FluentBuilder as _,
};
use gpui_component::{
    Disableable as _, Icon, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::{DropdownMenu as _, PopupMenuItem},
    notification::Notification,
    progress::Progress,
    scroll::ScrollableElement as _,
    spinner::Spinner,
    switch::Switch,
    v_flex,
};

use crate::{
    actions::convert_to_cloud,
    cloud_ui::{confirm_delete_cloud_copy, open_convert_dialog, tone_color, upload_texts},
    errors::error_text,
    hub::{CloudOp, ShareHub},
    manage_ui::chip,
    model::{
        NetHealth, NetSegment, PanelTarget, claimed_summary, claims_label,
        cloud::{CloudPhase, UploadLine, cloud_actions, expiry_tiers, retry_tier, upload_line},
        format_bytes, format_datetime, format_pickup_code, format_rate, nat_key, net_health,
        net_segments,
        panel::{
            HashingProgress, PanelPhase, PanelWait, claim_options, current_expiry_label,
            expiry_options, hashing_progress, panel_phase, progress_percent, receiver_name,
        },
        path_kind_key,
        receive::single_line,
        share_headline, share_status_key,
    },
    qr::open_share_qr,
    window_ui::{self, CENTER_MAX_WIDTH, count_text, ellipsized, text},
};

/// 下拉菜单宽 / 高的下限与上限（基准 px，随字号缩放）。
const MENU_MIN_WIDTH: f32 = 120.;
const MENU_MAX_WIDTH: f32 = 260.;
const MENU_MAX_HEIGHT: f32 = 280.;

/// 面板持有的分享；`seen` 表示 hub 已经出现过它（之后 hub 里消失 = 已结束，而不是还没同步）。
struct Held {
    id: String,
    seen: bool,
    /// `agent.share.create` 的返回值：hub 事件还没到之前用它渲染。
    initial: Option<ShareDto>,
}

pub struct SharePanelView {
    hub: Entity<ShareHub>,
    target: PanelTarget,
    wait: PanelWait,
    held: Option<Held>,
    create_error: Option<RpcErrorData>,
    /// 最近一次操作（改设置 / 停止 / 中断连接）失败的文案。
    action_error: Option<SharedString>,
    updating: bool,
    stopping: bool,
    killing: HashSet<String>,
}

impl SharePanelView {
    pub fn new(
        hub: Entity<ShareHub>,
        target: PanelTarget,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&hub, |this, hub, cx| {
            if let Some(held) = this.held.as_mut()
                && !held.seen
                && hub.read(cx).share(&held.id).is_some()
            {
                held.seen = true;
            }
            cx.notify();
        })
        .detach();
        let translator = hub.read(cx).translator().clone();
        cx.observe(&translator, |_, _, cx| cx.notify()).detach();

        let mut view = Self {
            hub,
            target: target.clone(),
            wait: PanelWait::Holding,
            held: None,
            create_error: None,
            action_error: None,
            updating: false,
            stopping: false,
            killing: HashSet::new(),
        };
        match target {
            PanelTarget::Tasks(_) => view.create(cx),
            PanelTarget::Share(id) => {
                view.held = Some(Held {
                    id,
                    seen: true,
                    initial: None,
                });
            }
        }
        view
    }

    /// 凭任务集创建（或取回）规范分享。
    fn create(&mut self, cx: &mut Context<Self>) {
        let PanelTarget::Tasks(task_ids) = &self.target else {
            return;
        };
        let params = ShareCreateParams {
            task_ids: task_ids.clone(),
            ..ShareCreateParams::default()
        };
        self.wait = PanelWait::Creating;
        self.create_error = None;
        let future = self.hub.read(cx).api().create(&params);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = future.await;
            let Ok(()) = this.update(cx, |this, cx| {
                match result {
                    Ok(share) => {
                        let seen = this.hub.read(cx).share(&share.id).is_some();
                        this.held = Some(Held {
                            id: share.id.clone(),
                            seen,
                            initial: Some(share),
                        });
                        this.wait = PanelWait::Holding;
                    }
                    Err(error) => {
                        this.create_error = Some(error);
                        this.wait = PanelWait::Failed;
                    }
                }
                cx.notify();
            }) else {
                // 面板已关闭，不再回写异步结果。
                return;
            };
        })
        .detach();
    }

    fn share_of<'a>(&'a self, hub: &'a ShareHub) -> Option<&'a ShareDto> {
        let held = self.held.as_ref()?;
        hub.share(&held.id).or(if held.seen {
            None
        } else {
            held.initial.as_ref()
        })
    }

    fn set_action_error(&mut self, error: &RpcErrorData, cx: &App) {
        let text = error_text(self.hub.read(cx).translator().read(cx), error);
        self.action_error = Some(SharedString::from(text));
    }

    fn update_share(&mut self, params: ShareUpdateParams, cx: &mut Context<Self>) {
        if self.updating || self.hub.read(cx).is_stale() {
            return;
        }
        self.updating = true;
        self.action_error = None;
        let future = self.hub.read(cx).api().update(&params);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = future.await;
            let Ok(()) = this.update(cx, |this, cx| {
                this.updating = false;
                if let Err(error) = result {
                    this.set_action_error(&error, cx);
                }
                cx.notify();
            }) else {
                // 面板已关闭，不再回写异步结果。
                return;
            };
        })
        .detach();
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.held.as_ref().map(|held| held.id.clone()) else {
            return;
        };
        if self.stopping || self.hub.read(cx).is_stale() {
            return;
        }
        self.stopping = true;
        self.action_error = None;
        let future = self.hub.read(cx).api().stop(&id);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = future.await;
            let Ok(()) = this.update(cx, |this, cx| {
                this.stopping = false;
                match result {
                    Ok(_) => this.wait = PanelWait::Stopped,
                    Err(error) => this.set_action_error(&error, cx),
                }
                cx.notify();
            }) else {
                // 面板已关闭，不再回写异步结果。
                return;
            };
        })
        .detach();
    }

    fn confirm_stop(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.stopping {
            return;
        }
        let translator = self.hub.read(cx).translator().read(cx).clone();
        let title = SharedString::from(translator.text("shareStop").to_owned());
        let description = SharedString::from(translator.text("shareStopConfirm").to_owned());
        let cancel = SharedString::from(translator.text("cancel").to_owned());
        let view = cx.entity();
        window.open_alert_dialog(cx, move |alert, _, cx| {
            let view = view.clone();
            alert
                .title(dialog_title(title.clone(), cx))
                .description(description.clone())
                .footer(dialog_footer(
                    Some(cancel.clone()),
                    title.clone(),
                    DialogIntent::Destructive,
                    cx,
                ))
                .on_ok(move |_, _, cx| {
                    view.update(cx, |this, cx| this.stop(cx));
                    true
                })
        });
    }

    fn kill_session(&mut self, session_id: String, cx: &mut Context<Self>) {
        if self.killing.contains(&session_id) || self.hub.read(cx).is_stale() {
            return;
        }
        self.killing.insert(session_id.clone());
        self.action_error = None;
        let future = self.hub.read(cx).api().session_kill(&session_id);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = future.await;
            let Ok(()) = this.update(cx, |this, cx| {
                this.killing.remove(&session_id);
                if let Err(error) = result {
                    this.set_action_error(&error, cx);
                }
                cx.notify();
            }) else {
                // 面板已关闭，不再回写异步结果。
                return;
            };
        })
        .detach();
    }

    /// 把文本写入剪贴板并弹出成功提示。
    fn copy_text(
        &self,
        text: String,
        message_key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        let message = SharedString::from(
            self.hub
                .read(cx)
                .translator()
                .read(cx)
                .text(message_key)
                .to_owned(),
        );
        window.push_notification(Notification::success(message), cx);
    }

    /// 切到分享页。
    fn open_manage(&self, window: &mut Window, cx: &mut App) {
        let open = self.hub.read(cx).host().open_manage.clone();
        if let Some(open) = open {
            open(window, cx);
        }
    }

    /// 打开设置 › FluxMesh › 网络诊断并聚焦某个会话。
    fn open_diagnostics(&self, session_id: Option<String>, window: &mut Window, cx: &mut App) {
        let open = self.hub.read(cx).host().open_diagnostics.clone();
        if let Some(open) = open {
            open(session_id, window, cx);
        }
    }
}

fn health_color(health: NetHealth, cx: &App) -> Hsla {
    let theme = active_theme(cx);
    let colors = theme.extended().colors;
    match health {
        NetHealth::Direct => colors.success,
        NetHealth::RelayOnly => colors.warning,
        NetHealth::Offline => theme.tokens().colors.destructive,
        NetHealth::Idle => theme.tokens().colors.muted_foreground,
    }
}

fn status_color(status: ShareStatus, cx: &App) -> Hsla {
    let theme = active_theme(cx);
    match status {
        ShareStatus::Active => theme.extended().colors.success,
        ShareStatus::Expired => theme.extended().colors.warning,
        ShareStatus::Takedown => theme.tokens().colors.destructive,
        ShareStatus::Stopped | ShareStatus::Unknown => theme.tokens().colors.muted_foreground,
    }
}

/// 本机网络摘要行文案：`NAT · 运营商 · IPv6 · 中继延迟`。
fn net_line(translator: &Translator, segments: &[NetSegment]) -> String {
    segments
        .iter()
        .map(|segment| match segment {
            NetSegment::Nat(nat) => translator.text(nat_key(*nat)).to_owned(),
            NetSegment::Carrier(text) => single_line(text),
            NetSegment::Ipv6(true) => translator.text("shareIpv6On").to_owned(),
            NetSegment::Ipv6(false) => translator.text("shareIpv6Off").to_owned(),
            NetSegment::RelayLatency(ms) => {
                translator.text_with("shareRelayLatency", &[("ms", &ms.to_string())])
            }
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

impl SharePanelView {
    fn render_creating(&self, hub: &ShareHub, translator: &Translator, cx: &Context<Self>) -> Div {
        let theme = active_theme(cx);
        let tokens = theme.tokens();
        let task_ids: &[String] = match &self.target {
            PanelTarget::Tasks(ids) => ids,
            PanelTarget::Share(_) => &[],
        };
        let progress = hashing_progress(hub.hashing(), task_ids);
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap(tokens.spacing.md)
            .p(tokens.spacing.xl)
            .child(Spinner::new())
            .child(
                div()
                    .text_size(tokens.typography.sm.size)
                    .line_height(tokens.typography.sm.line_height)
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(tokens.colors.foreground)
                    .child(text(translator, "shareCreating")),
            )
            .child(
                div()
                    .max_w(theme.text_extent(CENTER_MAX_WIDTH))
                    .text_center()
                    .text_size(tokens.typography.xs.size)
                    .line_height(tokens.typography.xs.line_height)
                    .text_color(tokens.colors.muted_foreground)
                    .child(text(translator, "shareHashing")),
            )
            .when_some(progress, |this, progress| {
                this.child(self.render_hashing(&progress, cx))
            })
    }

    fn render_hashing(&self, progress: &HashingProgress, cx: &Context<Self>) -> Div {
        let theme = active_theme(cx);
        let tokens = theme.tokens();
        let percent = progress.percent();
        let amount = match percent {
            Some(percent) => format!(
                "{} / {} · {}",
                format_bytes(progress.done_bytes),
                format_bytes(progress.total_bytes),
                crate::model::format_percent(f64::from(percent) / 100.)
            ),
            None => format_bytes(progress.done_bytes),
        };
        v_flex()
            .w_full()
            .max_w(theme.text_extent(CENTER_MAX_WIDTH))
            .gap(tokens.spacing.xs)
            .child(
                Progress::new("share-hashing-progress")
                    .w_full()
                    .color(tokens.colors.primary)
                    .loading(percent.is_none())
                    .value(percent.unwrap_or(0.)),
            )
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .justify_between()
                    .gap(tokens.spacing.md)
                    .text_size(tokens.typography.xs.size)
                    .line_height(tokens.typography.xs.line_height)
                    .text_color(tokens.colors.muted_foreground)
                    .child(
                        ellipsized(
                            progress
                                .title
                                .as_deref()
                                .map(single_line)
                                .unwrap_or_default(),
                        )
                        .flex_1(),
                    )
                    .child(
                        div()
                            .flex_none()
                            .font_features(tabular_numbers())
                            .child(amount),
                    ),
            )
    }

    fn render_live(
        &self,
        share: &ShareDto,
        hub: &ShareHub,
        translator: &Translator,
        cx: &Context<Self>,
    ) -> Div {
        let theme = active_theme(cx);
        let tokens = theme.tokens();
        let extended = theme.extended();
        let stale = hub.is_stale();
        let can_edit = !stale && !self.updating;
        let state = hub.state();
        let net = &state.net_status;
        let health = net_health(net);
        let view = cx.entity();

        let link = SharedString::from(share.url.clone());
        let link_tip = link.clone();
        let copy_link = share.url.clone();
        let copy_code = share.pickup_code.clone();
        let qr_title = share_headline(share).to_owned();
        let qr_url = share.url.clone();
        let translator_entity = hub.translator().clone();

        let link_field = div()
            .id("share-link-field")
            .flex_1()
            .min_w_0()
            .h(theme.density().control)
            .flex()
            .items_center()
            .px(tokens.spacing.sm + tokens.spacing.xxs)
            .rounded(theme.components().button_radius)
            .border(extended.stroke.thin)
            .border_color(tokens.colors.border)
            .bg(tokens.colors.muted)
            .text_size(tokens.typography.sm.size)
            .line_height(tokens.typography.sm.line_height)
            .text_color(tokens.colors.foreground)
            .tooltip(move |window, cx| tooltip_view(link_tip.clone(), window, cx))
            .child(
                div()
                    .min_w_0()
                    .w_full()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis_middle()
                    .child(link),
            );
        let link_row = v_flex()
            .w_full()
            .gap(tokens.spacing.xs + tokens.spacing.xxs)
            .child(field_label(text(translator, "shareLinkLabel"), cx))
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(tokens.spacing.sm)
                    .child(link_field)
                    .child(
                        Button::new("share-copy-link")
                            .primary()
                            .icon(FluxIcon::Copy)
                            .label(text(translator, "shareCopyLink"))
                            .control(cx)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.copy_text(copy_link.clone(), "shareLinkCopied", window, cx);
                            })),
                    ),
            );

        let code_row = v_flex()
            .w_full()
            .gap(tokens.spacing.xs + tokens.spacing.xxs)
            .child(field_label(text(translator, "sharePickupCode"), cx))
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(tokens.spacing.md)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .font_family(tokens.typography.mono.clone())
                            .text_size(tokens.typography.xl.size)
                            .line_height(tokens.typography.xl.line_height)
                            .font_weight(FontWeight::SEMIBOLD)
                            .font_features(tabular_numbers())
                            .text_color(tokens.colors.foreground)
                            .child(format_pickup_code(&share.pickup_code)),
                    )
                    .child(
                        Button::new("share-copy-code")
                            .outline()
                            .icon(FluxIcon::Copy)
                            .label(text(translator, "shareCopyCode"))
                            .control(cx)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.copy_text(copy_code.clone(), "shareCodeCopied", window, cx);
                            })),
                    )
                    .child(
                        Button::new("share-show-qr")
                            .outline()
                            .icon(FluxIcon::QrCode)
                            .label(text(translator, "shareShowQr"))
                            .control(cx)
                            .on_click(move |_, window, cx| {
                                open_share_qr(&translator_entity, &qr_title, &qr_url, window, cx);
                            }),
                    ),
            );

        let chips = self.render_chips(share, hub, translator, can_edit, &view, cx);
        let status = self.render_status(share, hub, translator, health, cx);

        let net_text = net_line(translator, &net_segments(net));
        let health_text = text(translator, health.key());
        let net_block = v_flex()
            .w_full()
            .gap(tokens.spacing.xxs)
            .text_size(tokens.typography.xs.size)
            .line_height(tokens.typography.xs.line_height)
            .child(
                h_flex()
                    .items_center()
                    .gap(tokens.spacing.xs)
                    .text_color(tokens.colors.muted_foreground)
                    .child(
                        Icon::new(FluxIcon::Network)
                            .size(extended.icon.sm)
                            .text_color(tokens.colors.muted_foreground),
                    )
                    .child(
                        ellipsized(text(translator, "shareNetTitle"))
                            .font_weight(FontWeight::MEDIUM),
                    ),
            )
            .when(!net_text.is_empty(), |this| {
                this.child(ellipsized(net_text).text_color(tokens.colors.muted_foreground))
            })
            .child(ellipsized(health_text).text_color(health_color(health, cx)));

        let receivers = self.render_receivers(share, translator, stale, cx);
        let policy = &state.policy_summary;
        let cloud = cloud_actions(share, policy, hub.signed_in(), hub.can_send());
        let upload = upload_line(share)
            .map(|line| self.render_upload(share, &line, cloud.phase, translator, cx));
        let cloud_op = hub.cloud_op(&share.id);
        let convert_button = cloud
            .convert
            .filter(|_| cloud.phase != CloudPhase::Failed)
            .map(|block| {
                let tip = block.map(|block| text(translator, block.key()));
                let id = share.id.clone();
                let hub_entity = self.hub.clone();
                div()
                    .id("share-stage-convert-wrap")
                    .when_some(tip, |this, tip| {
                        this.tooltip(move |window, cx| tooltip_view(tip.clone(), window, cx))
                    })
                    .child(
                        Button::new("share-stage-convert")
                            .outline()
                            .icon(FluxIcon::CloudUpload)
                            .label(text(translator, "shareStageConvert"))
                            .control(cx)
                            .busy(cloud_op == Some(CloudOp::Convert))
                            .disabled(stale || block.is_some() || cloud_op.is_some())
                            .on_click(move |_, window, cx| {
                                open_convert_dialog(&hub_entity, id.clone(), window, cx);
                            }),
                    )
            });
        let delete_button = (cloud.delete && cloud.phase == CloudPhase::Ready).then(|| {
            let id = share.id.clone();
            let hub_entity = self.hub.clone();
            Button::new("share-stage-delete")
                .outline()
                .icon(FluxIcon::Trash2)
                .label(text(translator, "shareStageDelete"))
                .control(cx)
                .busy(cloud_op == Some(CloudOp::Delete))
                .disabled(stale || cloud_op.is_some())
                .on_click(move |_, window, cx| {
                    confirm_delete_cloud_copy(&hub_entity, id.clone(), false, window, cx);
                })
        });

        let body = v_flex()
            .w_full()
            .gap(tokens.spacing.lg)
            .p(tokens.spacing.lg)
            .when(stale, |this| {
                this.child(
                    h_flex()
                        .items_center()
                        .gap(tokens.spacing.xs)
                        .text_size(tokens.typography.xs.size)
                        .line_height(tokens.typography.xs.line_height)
                        .text_color(tokens.colors.muted_foreground)
                        .child(
                            Icon::new(FluxIcon::CircleAlert)
                                .size(extended.icon.sm)
                                .text_color(extended.colors.warning),
                        )
                        .child(ellipsized(text(translator, "localServiceDisconnected")).flex_1()),
                )
            })
            .child(link_row)
            .child(code_row)
            .child(chips)
            .child(status)
            .children(upload)
            .child(net_block)
            .child(receivers)
            .when_some(self.action_error.clone(), |this, message| {
                this.child(field_error(message, cx))
            });

        let stop_busy = self.stopping;
        let footer = window_ui::footer_frame(cx)
            .child(
                Button::new("share-manage-all")
                    .ghost()
                    .label(text(translator, "shareManageAll"))
                    .control(cx)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_manage(window, cx);
                        window.remove_window();
                    })),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap(tokens.spacing.sm)
                    .child(
                        Button::new("share-system-share")
                            .outline()
                            .icon(FluxIcon::Share2)
                            .label(text(translator, "shareSystemShare"))
                            .control(cx)
                            .on_click(cx.listener(|this, _, window, cx| {
                                let url = this
                                    .share_of(this.hub.read(cx))
                                    .map(|share| share.url.clone());
                                if let Some(url) = url {
                                    this.copy_text(url, "shareSystemShareCopied", window, cx);
                                }
                            })),
                    )
                    .children(convert_button)
                    .children(delete_button)
                    .child(
                        Button::new("share-stop")
                            .outline()
                            .label(text(translator, "shareStop"))
                            .control(cx)
                            .busy(stop_busy)
                            .disabled(stale || stop_busy)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.confirm_stop(window, cx);
                            })),
                    ),
            );

        v_flex()
            .size_full()
            .min_h_0()
            .child(window_ui::header(
                if share.files.len() > 1 {
                    FluxIcon::Folders
                } else {
                    FluxIcon::File
                },
                headline_text(share, translator),
                Some(subtitle_text(share, translator)),
                Some((
                    text(translator, share_status_key(share.status)),
                    status_color(share.status, cx),
                )),
                text(translator, "close"),
                cx,
            ))
            .child(
                div().flex_1().min_h_0().w_full().child(
                    div()
                        .id("share-panel-body")
                        .size_full()
                        .overflow_y_scrollbar()
                        .child(body),
                ),
            )
            .child(footer)
    }

    /// 方式 / 有效期 / 次数 / 领取通知。
    fn render_chips(
        &self,
        share: &ShareDto,
        hub: &ShareHub,
        translator: &Translator,
        can_edit: bool,
        view: &Entity<Self>,
        cx: &Context<Self>,
    ) -> Div {
        let theme = active_theme(cx);
        let tokens = theme.tokens();
        let extended = theme.extended();
        let tiers = expiry_tiers(share, &hub.state().policy_summary);
        let menu_min = theme.text_extent(MENU_MIN_WIDTH);
        let menu_max_w = theme.text_extent(MENU_MAX_WIDTH);
        let menu_max_h = theme.text_extent(MENU_MAX_HEIGHT);

        let labelled = |label: SharedString, control: gpui::AnyElement| {
            h_flex()
                .items_center()
                .gap(tokens.spacing.xs + tokens.spacing.xxs)
                .child(
                    div()
                        .flex_none()
                        .text_size(tokens.typography.xs.size)
                        .line_height(tokens.typography.xs.line_height)
                        .text_color(tokens.colors.muted_foreground)
                        .child(label),
                )
                .child(control)
        };

        // 方式：实时 / 云端暂存；信息芯片，悬停说明。
        let (mode_key, mode_hint_key) = if share.mode == ShareMode::Cloud {
            ("shareModeCloud", "shareModeCloudHint")
        } else {
            ("shareModeLive", "shareModeLiveHint")
        };
        let hint = text(translator, mode_hint_key);
        let mode = div()
            .id("share-chip-mode")
            .h(theme.density().control)
            .flex()
            .items_center()
            .gap(tokens.spacing.xs)
            .px(tokens.spacing.sm + tokens.spacing.xxs)
            .rounded(theme.components().button_radius)
            .border(extended.stroke.thin)
            .border_color(extended.colors.hairline)
            .text_size(tokens.typography.sm.size)
            .line_height(tokens.typography.sm.line_height)
            .text_color(tokens.colors.muted_foreground)
            .tooltip(move |window, cx| tooltip_view(hint.clone(), window, cx))
            .child(text(translator, mode_key))
            .child(Icon::new(FluxIcon::Info).size(extended.icon.sm))
            .into_any_element();
        let phase = crate::model::cloud::cloud_phase(share);
        let cloud_state = phase.chip_key().map(|key| {
            chip(
                "share-chip-cloud-state",
                text(translator, key),
                Some(tone_color(phase.tone(), cx)),
                cx,
            )
        });

        // 有效期。
        let expiry_value = current_expiry_label(tiers, &share.tier_id).map_or_else(
            || SharedString::from(format_datetime(&share.expires_at)),
            |label| count_text(translator, label),
        );
        let expiry_items: Vec<(String, SharedString, bool)> = expiry_options(tiers, &share.tier_id)
            .into_iter()
            .map(|option| {
                (
                    option.tier_id,
                    count_text(translator, option.label),
                    option.selected,
                )
            })
            .collect();
        let expiry = {
            let view = view.clone();
            let id = share.id.clone();
            Button::new("share-chip-expiry")
                .outline()
                .control(cx)
                .label(expiry_value)
                .dropdown_caret(true)
                .disabled(!can_edit || expiry_items.is_empty())
                .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, _| {
                    expiry_items.iter().fold(
                        menu.min_w(menu_min)
                            .max_w(menu_max_w)
                            .max_h(menu_max_h)
                            .scrollable(true),
                        |menu, (tier_id, label, selected)| {
                            let view = view.clone();
                            let id = id.clone();
                            let tier_id = tier_id.clone();
                            menu.item(
                                PopupMenuItem::new(label.clone())
                                    .checked(*selected)
                                    .on_click(move |_, _, cx| {
                                        let params = ShareUpdateParams {
                                            id: id.clone(),
                                            tier_id: Some(tier_id.clone()),
                                            ..ShareUpdateParams::default()
                                        };
                                        view.update(cx, |this, cx| this.update_share(params, cx));
                                    }),
                            )
                        },
                    )
                })
                .into_any_element()
        };

        // 次数。
        let claims_items: Vec<(u32, SharedString)> = claim_options(share.max_claims)
            .into_iter()
            .map(|max| (max, count_text(translator, claims_label(max))))
            .collect();
        let claims_value = count_text(translator, claims_label(share.max_claims));
        let current_claims = share.max_claims;
        let claims = {
            let view = view.clone();
            let id = share.id.clone();
            Button::new("share-chip-claims")
                .outline()
                .control(cx)
                .label(claims_value)
                .dropdown_caret(true)
                .disabled(!can_edit)
                .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, _| {
                    claims_items.iter().fold(
                        menu.min_w(menu_min)
                            .max_w(menu_max_w)
                            .max_h(menu_max_h)
                            .scrollable(true),
                        |menu, (max, label)| {
                            let view = view.clone();
                            let id = id.clone();
                            let max = *max;
                            menu.item(
                                PopupMenuItem::new(label.clone())
                                    .checked(max == current_claims)
                                    .on_click(move |_, _, cx| {
                                        let params = ShareUpdateParams {
                                            id: id.clone(),
                                            max_claims: Some(max),
                                            ..ShareUpdateParams::default()
                                        };
                                        view.update(cx, |this, cx| this.update_share(params, cx));
                                    }),
                            )
                        },
                    )
                })
                .into_any_element()
        };

        // 领取时通知。
        let notify = {
            let view = view.clone();
            let id = share.id.clone();
            h_flex()
                .items_center()
                .gap(tokens.spacing.xs + tokens.spacing.xxs)
                .child(
                    Switch::new("share-chip-notify")
                        .checked(share.notify_on_claim)
                        .disabled(!can_edit)
                        .accessibility_label(text(translator, "shareChipNotify"))
                        .on_click(move |checked: &bool, _, cx| {
                            let params = ShareUpdateParams {
                                id: id.clone(),
                                notify_on_claim: Some(*checked),
                                ..ShareUpdateParams::default()
                            };
                            view.update(cx, |this, cx| this.update_share(params, cx));
                        }),
                )
                .child(
                    div()
                        .text_size(tokens.typography.sm.size)
                        .line_height(tokens.typography.sm.line_height)
                        .text_color(tokens.colors.foreground)
                        .child(text(
                            translator,
                            if share.notify_on_claim {
                                "shareNotifyOn"
                            } else {
                                "shareNotifyOff"
                            },
                        )),
                )
                .into_any_element()
        };

        // 共享：其他持有同一文件的设备能否从本机取块。
        let swarm = {
            let view = view.clone();
            let id = share.id.clone();
            h_flex()
                .items_center()
                .gap(tokens.spacing.xs + tokens.spacing.xxs)
                .child(
                    Switch::new("share-chip-swarm")
                        .checked(share.swarm && !share.swarm_locked)
                        // 管理员锁定后不能再打开（云端会以 `shareSwarmLocked` 拒绝），开关直接置灰。
                        .disabled(!can_edit || share.swarm_locked)
                        .accessibility_label(text(translator, "shareChipSwarm"))
                        .on_click(move |checked: &bool, _, cx| {
                            let params = ShareUpdateParams {
                                id: id.clone(),
                                swarm: Some(*checked),
                                ..ShareUpdateParams::default()
                            };
                            view.update(cx, |this, cx| this.update_share(params, cx));
                        }),
                )
                .child(
                    div()
                        .text_size(tokens.typography.sm.size)
                        .line_height(tokens.typography.sm.line_height)
                        .text_color(if share.swarm_locked {
                            tokens.colors.muted_foreground
                        } else {
                            tokens.colors.foreground
                        })
                        .child(text(
                            translator,
                            if share.swarm_locked {
                                "shareSwarmLockedHint"
                            } else if share.swarm {
                                "shareSwarmOn"
                            } else {
                                "shareSwarmOff"
                            },
                        )),
                )
                .into_any_element()
        };

        h_flex()
            .w_full()
            .flex_wrap()
            .items_center()
            .gap_x(tokens.spacing.lg)
            .gap_y(tokens.spacing.sm)
            .child(labelled(text(translator, "shareChipMode"), mode))
            .children(cloud_state)
            .child(labelled(text(translator, "shareChipExpiry"), expiry))
            .child(labelled(text(translator, "shareChipClaims"), claims))
            .child(labelled(text(translator, "shareChipNotify"), notify))
            .child(labelled(text(translator, "shareChipSwarm"), swarm))
            .when(self.updating, |this| {
                this.child(Spinner::new().with_size(gpui_component::Size::Size(extended.icon.sm)))
            })
    }

    /// 状态行：网络健康点 + 已领取次数 + 正在接收人数 + 过期时间。
    fn render_status(
        &self,
        share: &ShareDto,
        _hub: &ShareHub,
        translator: &Translator,
        health: NetHealth,
        cx: &Context<Self>,
    ) -> Div {
        let theme = active_theme(cx);
        let tokens = theme.tokens();
        let (key, claimed, max) = claimed_summary(share.claim_count, share.max_claims);
        let mut parts = vec![translator.text_with(
            key,
            &[("count", &claimed.to_string()), ("max", &max.to_string())],
        )];
        if !share.active_receivers.is_empty() {
            parts.push(translator.text_with(
                "shareActiveReceivers",
                &[("count", &share.active_receivers.len().to_string())],
            ));
        }
        parts.push(translator.text_with(
            "shareExpiresAt",
            &[("time", &format_datetime(&share.expires_at))],
        ));
        h_flex()
            .w_full()
            .items_center()
            .gap(tokens.spacing.sm)
            .child(
                div()
                    .flex_none()
                    .size(tokens.spacing.sm)
                    .rounded(tokens.radius.full)
                    .bg(health_color(health, cx)),
            )
            .child(
                ellipsized(parts.join(" · "))
                    .flex_1()
                    .font_features(tabular_numbers())
                    .text_size(tokens.typography.sm.size)
                    .line_height(tokens.typography.sm.line_height)
                    .text_color(tokens.colors.foreground),
            )
    }

    /// 云端暂存上传进度行：状态（+ 原因）、进度条、已传 / 总量 / 速率，以及取消 / 重试。
    fn render_upload(
        &self,
        share: &ShareDto,
        line: &UploadLine,
        phase: CloudPhase,
        translator: &Translator,
        cx: &Context<Self>,
    ) -> Div {
        let (stale, retry_tier, cloud_op) = {
            let hub = self.hub.read(cx);
            (
                hub.is_stale(),
                retry_tier(share, &hub.state().policy_summary).map(str::to_owned),
                hub.cloud_op(&share.id),
            )
        };
        let theme = active_theme(cx);
        let tokens = theme.tokens();
        let (status, detail) = upload_texts(translator, line);
        let tone = tone_color(phase.tone(), cx);
        let cancel = {
            let id = share.id.clone();
            let hub = self.hub.clone();
            Button::new("share-stage-cancel")
                .ghost()
                .label(text(translator, "shareStageCancelUpload"))
                .control(cx)
                .busy(cloud_op == Some(CloudOp::Delete))
                .disabled(stale || cloud_op.is_some())
                .on_click(move |_, window, cx| {
                    confirm_delete_cloud_copy(&hub, id.clone(), true, window, cx);
                })
        };
        let retry = (phase == CloudPhase::Failed).then(|| {
            let id = share.id.clone();
            let tier = retry_tier.clone();
            let hub = self.hub.clone();
            Button::new("share-stage-retry")
                .outline()
                .icon(FluxIcon::RotateCw)
                .label(text(translator, "shareStageRetry"))
                .control(cx)
                .busy(cloud_op == Some(CloudOp::Convert))
                .disabled(stale || cloud_op.is_some())
                .on_click(move |_, window, cx| {
                    convert_to_cloud(&hub, &id, tier.as_deref(), window, cx);
                })
        });
        card(cx).child(
            v_flex()
                .w_full()
                .gap(tokens.spacing.sm)
                .p(tokens.spacing.md)
                .child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .gap(tokens.spacing.sm)
                        .child(
                            ellipsized(status)
                                .flex_1()
                                .text_size(tokens.typography.sm.size)
                                .line_height(tokens.typography.sm.line_height)
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(tone),
                        )
                        .children(retry)
                        .child(cancel),
                )
                .child(
                    Progress::new("share-upload-progress")
                        .w_full()
                        .color(tone)
                        .value(line.percent),
                )
                .child(
                    ellipsized(detail)
                        .font_features(tabular_numbers())
                        .text_size(tokens.typography.xs.size)
                        .line_height(tokens.typography.xs.line_height)
                        .text_color(tokens.colors.muted_foreground),
                ),
        )
    }

    fn render_receivers(
        &self,
        share: &ShareDto,
        translator: &Translator,
        stale: bool,
        cx: &Context<Self>,
    ) -> Div {
        let theme = active_theme(cx);
        let tokens = theme.tokens();
        let extended = theme.extended();
        let rows: Vec<Div> = share
            .active_receivers
            .iter()
            .enumerate()
            .map(|(index, receiver)| self.render_receiver(index, receiver, translator, stale, cx))
            .collect();
        v_flex()
            .w_full()
            .gap(tokens.spacing.sm)
            .child(
                h_flex()
                    .items_center()
                    .gap(tokens.spacing.xs)
                    .child(field_label(text(translator, "shareReceiversTitle"), cx))
                    .child(
                        Icon::new(FluxIcon::Smartphone)
                            .size(extended.icon.sm)
                            .text_color(tokens.colors.muted_foreground),
                    ),
            )
            .when(rows.is_empty(), |this| {
                this.child(
                    div()
                        .text_size(tokens.typography.xs.size)
                        .line_height(tokens.typography.xs.line_height)
                        .text_color(tokens.colors.muted_foreground)
                        .child(text(translator, "shareReceiversEmpty")),
                )
            })
            .children(rows)
    }

    fn render_receiver(
        &self,
        index: usize,
        receiver: &ShareReceiverDto,
        translator: &Translator,
        stale: bool,
        cx: &Context<Self>,
    ) -> Div {
        let theme = active_theme(cx);
        let tokens = theme.tokens();
        let name = receiver_name(receiver).map_or_else(
            || text(translator, "shareReceiverGuest"),
            |name| SharedString::from(single_line(name)),
        );
        let detail = format!(
            "{} · {} · {}",
            translator.text(path_kind_key(receiver.path_kind)),
            format_rate(receiver.rate_bps),
            crate::model::format_percent(receiver.progress),
        );
        let session = receiver.session_id.clone();
        let diagnose_session = receiver.session_id.clone();
        let killing = self.killing.contains(&receiver.session_id);
        card(cx).child(
            v_flex()
                .w_full()
                .gap(tokens.spacing.sm)
                .p(tokens.spacing.md)
                .child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .gap(tokens.spacing.sm)
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap(tokens.spacing.xxs)
                                .child(
                                    ellipsized(name)
                                        .text_size(tokens.typography.sm.size)
                                        .line_height(tokens.typography.sm.line_height)
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(tokens.colors.foreground),
                                )
                                .child(
                                    ellipsized(detail)
                                        .font_features(tabular_numbers())
                                        .text_size(tokens.typography.xs.size)
                                        .line_height(tokens.typography.xs.line_height)
                                        .text_color(tokens.colors.muted_foreground),
                                ),
                        )
                        .child(
                            Button::new(SharedString::from(format!("share-diagnose-{session}")))
                                .ghost()
                                .label(text(translator, "shareDiagnose"))
                                .control(cx)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_diagnostics(
                                        Some(diagnose_session.clone()),
                                        window,
                                        cx,
                                    );
                                })),
                        )
                        .child(
                            Button::new(SharedString::from(format!("share-kill-{session}")))
                                .ghost()
                                .label(text(translator, "shareDisconnectReceiver"))
                                .control(cx)
                                .busy(killing)
                                .disabled(stale || killing)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.kill_session(session.clone(), cx);
                                })),
                        ),
                )
                .child(
                    Progress::new(("share-receiver-progress", index))
                        .w_full()
                        .color(tokens.colors.primary)
                        .value(progress_percent(receiver.progress)),
                ),
        )
    }
}

/// 标题行主文案；没有标题也没有文件名时给「未命名」。
fn headline_text(share: &ShareDto, translator: &Translator) -> SharedString {
    let headline = share_headline(share).trim();
    if headline.is_empty() {
        text(translator, "shareUntitled")
    } else {
        SharedString::from(single_line(headline))
    }
}

/// 副标题：`N 个文件 · 总大小`。
fn subtitle_text(share: &ShareDto, translator: &Translator) -> SharedString {
    SharedString::from(format!(
        "{} · {}",
        translator.text_with(
            "shareFilesCount",
            &[("count", &share.files.len().to_string())]
        ),
        format_bytes(share.total_bytes),
    ))
}

impl Render for SharePanelView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = active_theme(cx);
        let tokens = theme.tokens().clone();
        let extended_colors = theme.extended().colors;
        let hub_entity = self.hub.clone();
        let hub = hub_entity.read(cx);
        let translator = hub.translator().read(cx);
        let share = self.share_of(hub);
        let phase = panel_phase(self.wait, share);

        let root = v_flex()
            .size_full()
            .min_h_0()
            .bg(tokens.colors.surface)
            .text_color(tokens.colors.foreground);

        match phase {
            PanelPhase::Live => match share {
                Some(share) => root.child(self.render_live(share, hub, translator, cx)),
                None => root,
            },
            PanelPhase::Creating => root
                .child(window_ui::header(
                    FluxIcon::Share2,
                    text(translator, "sharePanelTitle"),
                    None,
                    None,
                    text(translator, "close"),
                    cx,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .child(self.render_creating(hub, translator, cx)),
                ),
            PanelPhase::Failed => {
                let message = self.create_error.as_ref().map_or_else(
                    || text(translator, "localServiceActionFailed"),
                    |error| SharedString::from(error_text(translator, error)),
                );
                let retry = Button::new("share-create-retry")
                    .primary()
                    .icon(FluxIcon::RotateCw)
                    .label(text(translator, "shareRetry"))
                    .control(cx)
                    .disabled(hub.is_stale())
                    .on_click(cx.listener(|this, _, _, cx| this.create(cx)));
                root.child(window_ui::header(
                    FluxIcon::Share2,
                    text(translator, "sharePanelTitle"),
                    None,
                    None,
                    text(translator, "close"),
                    cx,
                ))
                .child(div().flex_1().min_h_0().child(window_ui::notice(
                    FluxIcon::CircleAlert,
                    tokens.colors.destructive,
                    message,
                    cx,
                )))
                .child(window_ui::footer_frame(cx).child(div()).child(retry))
            }
            PanelPhase::Gone | PanelPhase::Closed => {
                let (icon, title, subtitle, chip) = match share {
                    Some(share) => (
                        if share.files.len() > 1 {
                            FluxIcon::Folders
                        } else {
                            FluxIcon::File
                        },
                        headline_text(share, translator),
                        Some(subtitle_text(share, translator)),
                        Some((
                            text(translator, share_status_key(share.status)),
                            status_color(share.status, cx),
                        )),
                    ),
                    None => (
                        FluxIcon::Share2,
                        text(translator, "sharePanelTitle"),
                        None,
                        None,
                    ),
                };
                let chip = if self.wait == PanelWait::Stopped {
                    Some((
                        text(translator, share_status_key(ShareStatus::Stopped)),
                        status_color(ShareStatus::Stopped, cx),
                    ))
                } else {
                    chip
                };
                root.child(window_ui::header(
                    icon,
                    title,
                    subtitle,
                    chip,
                    text(translator, "close"),
                    cx,
                ))
                .child(div().flex_1().min_h_0().child(window_ui::notice(
                    FluxIcon::CircleAlert,
                    extended_colors.warning,
                    text(translator, "shareEnded"),
                    cx,
                )))
                .child(
                    window_ui::footer_frame(cx)
                        .child(
                            Button::new("share-ended-manage")
                                .ghost()
                                .label(text(translator, "shareManageAll"))
                                .control(cx)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_manage(window, cx);
                                    window.remove_window();
                                })),
                        )
                        .child(
                            Button::new("share-ended-close")
                                .outline()
                                .label(text(translator, "close"))
                                .control(cx)
                                .on_click(cx.listener(|_, _, window, _| window.remove_window())),
                        ),
                )
            }
        }
    }
}
