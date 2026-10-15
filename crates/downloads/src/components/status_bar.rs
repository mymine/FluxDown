//! 状态栏：左 = 全局速度 + 全部暂停 / 全部开始；右 = 限速、代理、完成后关机、剩余空间。
//!
//! 下载限速走云同步偏好写入；上传限速与代理模式走 `DownloadsCommand::PatchConfig`
//!（`upload_limit_bytes` 为字节/秒，`proxy_mode` 为现有四模式之一）。
//! 完成后关机走宿主注入的 `ShutdownPort`（状态机与真正执行归 agent，本文件只发请求 +
//! 刷新倒计时显示）。

use std::{collections::BTreeMap, rc::Rc, time::Duration};

/// 数字输入弹窗的确认回调。
type NumberConfirm = Rc<dyn Fn(i64, &mut App)>;

use fluxdown_protocol::ApplicationErrorCode;
use fluxdown_ui_components::{
    ControlExt as _, DialogIntent, FluxIcon, dialog_footer, dialog_title, field_hint, form,
    form_field, tabular_numbers, toolbar_action_button,
};
use fluxdown_ui_theme::active_theme;
use gpui::{
    Anchor, App, AppContext as _, ClickEvent, Context, Div, Hsla, InteractiveElement as _,
    IntoElement, ParentElement, SharedString, StatefulInteractiveElement as _, Styled, WeakEntity,
    Window, div, prelude::FluentBuilder as _,
};
use gpui_component::{
    Icon, Sizable as _, Size, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputState},
    menu::{DropdownMenu as _, PopupMenuItem},
    tooltip::Tooltip,
};

use crate::{
    components::task_table::ToolbarCommand,
    controller::DownloadsCommand,
    model::{format_bytes, shutdown::ShutdownRequest},
    pages::downloads::DownloadView,
};

/// 下载 / 上传限速预设（MB/s）。
const SPEED_PRESETS_MB: [i64; 4] = [1, 5, 10, 50];
/// 完成后关机延迟预设（分钟）。
const SHUTDOWN_PRESETS_MIN: [i64; 4] = [1, 5, 10, 30];

/// 状态栏带文字的小按钮外壳：ghost、`density.statusControl` 高、横向 `spacing.xs`。
///
/// gpui-component 按钮会按 `Size` 在内部 label 上覆盖字号与图标尺寸，所以内容一律经
/// [`status_button_content`] 作为子元素传入，确保 caption 字号 + `icon.sm` 生效。
pub(super) fn status_button(id: &'static str, cx: &App) -> Button {
    let theme = active_theme(cx);
    Button::new(id)
        .ghost()
        .with_size(Size::XSmall)
        .h(theme.density().status_control)
        .px(theme.tokens().spacing.xs)
}

/// 状态栏按钮内容：可选图标 + 可选文字，caption 字号、等宽数字；`color` 为空时继承按钮前景色。
pub(super) fn status_button_content(
    icon: Option<FluxIcon>,
    text: Option<SharedString>,
    color: Option<Hsla>,
    cx: &App,
) -> Div {
    let theme = active_theme(cx);
    let extended = theme.extended();
    let icon_size = extended.icon.sm;
    h_flex()
        .items_center()
        .gap(theme.tokens().spacing.xxs)
        .text_size(extended.caption.size)
        .line_height(extended.caption.line_height)
        .font_features(tabular_numbers())
        .when_some(color, |this, color| this.text_color(color))
        .when_some(icon, |this, icon| {
            this.child(
                Icon::new(icon)
                    .size(icon_size)
                    .when_some(color, |icon, color| icon.text_color(color)),
            )
        })
        .when_some(text, |this, text| this.child(text))
}

fn format_countdown(remaining: Duration) -> String {
    let total = remaining.as_secs();
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// 预设 MB/s → `speed_limit_bytes`/`upload_limit_bytes` 配置值（字节/秒）。
fn mb_to_bytes(mb: i64) -> i64 {
    mb * 1_048_576
}

/// 自定义弹窗 KB/s 输入 → 配置值（字节/秒）；负值视为 0。
fn kb_to_bytes(kb: i64) -> i64 {
    kb.max(0) * 1024
}

/// 分钟延迟 → `ShutdownRequest::ArmAfter` 所需 `Duration`；负值视为 0。
fn minutes_to_duration(minutes: i64) -> Duration {
    Duration::from_secs(60 * minutes.max(0) as u64)
}

fn apply_speed_limit(view: WeakEntity<DownloadView>, key: &'static str, value: i64, cx: &mut App) {
    let Ok(()) = view.update(cx, |this, cx| this.execute_config_patch(key, value, cx)) else {
        // 视图已释放，结束这次回调而不再更新状态。
        return;
    };
}

/// 数字输入弹窗文案：自定义限速（KB/s）与自定义关机延迟（分钟）复用。
#[derive(Clone)]
struct NumberPrompt {
    title: SharedString,
    label: SharedString,
    /// 输入框尾部单位（`KB/s` / `分钟`）。
    unit: SharedString,
    hint: Option<SharedString>,
    cancel_label: SharedString,
    confirm_label: SharedString,
}

/// 弹窗输入解析：空白、非数字或小于 `min` 都视为无效，避免误当成 0 触发副作用。
fn parse_number_input(text: &str, min: i64) -> Option<i64> {
    text.trim()
        .parse::<i64>()
        .ok()
        .filter(|value| *value >= min)
}

/// 单个数字输入确认弹窗：标题 → 表单字段（标签 + 带单位输入框 + 可选说明）→ 底栏。
fn open_number_prompt(
    window: &mut Window,
    cx: &mut App,
    prompt: NumberPrompt,
    min: i64,
    on_confirm: NumberConfirm,
) {
    let input = cx.new(|cx| InputState::new(window, cx));
    let dialog_input = input.clone();
    window.open_dialog(cx, move |dialog, _, cx| {
        let content_input = dialog_input.clone();
        let ok_input = dialog_input.clone();
        let on_confirm = on_confirm.clone();
        let prompt = prompt.clone();
        dialog
            .title(dialog_title(prompt.title.clone(), cx))
            .w(active_theme(cx).text_extent(520.))
            .content({
                let prompt = prompt.clone();
                move |content, _, cx| {
                    let unit = field_hint(prompt.unit.clone(), cx).flex_none();
                    content.child(form(cx).child(form_field(
                        prompt.label.clone(),
                        Input::new(&content_input).control(cx).suffix(unit).w_full(),
                        prompt.hint.clone(),
                        cx,
                    )))
                }
            })
            .footer(dialog_footer(
                Some(prompt.cancel_label.clone()),
                prompt.confirm_label.clone(),
                DialogIntent::Confirm,
                cx,
            ))
            .on_ok(move |_, _, cx| {
                let text = ok_input.read(cx).value().to_string();
                let Some(value) = parse_number_input(&text, min) else {
                    return false;
                };
                on_confirm(value, cx);
                true
            })
    });
    input.update(cx, |input, cx| input.focus(window, cx));
}

impl DownloadView {
    /// 下载限速仍经云同步偏好写入；其余数值配置使用 daemon 增量写回。
    fn execute_config_patch(&mut self, key: &'static str, value: i64, cx: &mut Context<Self>) {
        // 下载限速属于云同步目录（download.speed_limit_bytes）：必须走偏好写入链路，
        // 直写 daemon 配置不会被标记为本地改动，之后会被云端旧值覆盖。
        let command = if key == "speed_limit_bytes" {
            DownloadsCommand::SetSyncedPreference {
                key: "download.speed_limit_bytes",
                value: serde_json::Value::from(value),
            }
        } else {
            let mut values = BTreeMap::new();
            values.insert(key.to_owned(), value.to_string());
            DownloadsCommand::PatchConfig {
                values,
                expected_revision: self.controller.config_revision(),
            }
        };
        self.execute_status_config_command(command, cx);
    }

    /// 状态栏配置冲突 / 失败复用现有横幅字段，不新增错误文案或宿主回调。
    pub(super) fn execute_status_config_command(
        &mut self,
        command: DownloadsCommand,
        cx: &mut Context<Self>,
    ) {
        let future = self.controller.execute(command);
        let conflict_message = SharedString::from(
            self.translator
                .read(cx)
                .text("localServiceConflict")
                .to_owned(),
        );
        let failed_message = self.strings.action_failed.clone();
        cx.spawn(async move |this, cx| {
            if let Err(error) = future.await {
                let message = if error.code == ApplicationErrorCode::Conflict {
                    conflict_message
                } else {
                    failed_message
                };

                let Ok(()) = this.update(cx, |this, cx| {
                    this.last_error = Some(message);
                    cx.notify();
                }) else {
                    // 视图已释放，结束这次回调而不再更新状态。
                    return;
                };
            }
        })
        .detach();
    }

    /// 下载 / 上传限速控件：图标 + 当前值的幽灵按钮，点开自动收起的菜单（预设 + 自定义）。
    /// `config_key` 为 `speed_limit_bytes` 或 `upload_limit_bytes`（字节/秒）。
    fn render_speed_limit_control(
        &self,
        element_id: &'static str,
        icon: FluxIcon,
        title_key: &'static str,
        config_key: &'static str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let translator = self.translator.read(cx);
        let value = self
            .controller
            .config_str(config_key)
            .parse::<i64>()
            .unwrap_or(0);
        let off_label = SharedString::from(translator.text("statusSpeedLimitOff").to_owned());
        let trigger_label = if value <= 0 {
            off_label.clone()
        } else {
            SharedString::from(format!("{}/s", format_bytes(value as u64)))
        };
        let title = SharedString::from(translator.text(title_key).to_owned());
        let custom_label = SharedString::from(translator.text("speedLimitCustom").to_owned());
        let desc_key = if config_key == "upload_limit_bytes" {
            "uploadLimitDesc"
        } else {
            "speedLimitDesc"
        };
        let prompt = NumberPrompt {
            title: title.clone(),
            label: custom_label.clone(),
            unit: SharedString::from(translator.text("statusSpeedLimitKbs").to_owned()),
            hint: Some(SharedString::from(translator.text(desc_key).to_owned())),
            cancel_label: SharedString::from(translator.text("cancel").to_owned()),
            confirm_label: SharedString::from(translator.text("confirm").to_owned()),
        };
        let view = cx.weak_entity();

        // 限速生效时触发器用强调色文字，一眼可见当前被限速；未限速保持中性。
        let limited_color = (value > 0).then(|| active_theme(cx).extended().colors.accent_text);

        status_button(element_id, cx)
            .child(status_button_content(
                Some(icon),
                Some(trigger_label),
                limited_color,
                cx,
            ))
            .tooltip(title.clone())
            .dropdown_menu_with_anchor(Anchor::BottomRight, move |menu, _, _| {
                let mut menu = menu.item(
                    PopupMenuItem::new(off_label.clone())
                        .checked(value <= 0)
                        .on_click({
                            let view = view.clone();
                            move |_, _, cx| apply_speed_limit(view.clone(), config_key, 0, cx)
                        }),
                );
                let mut preset_hit = value <= 0;
                for mb in SPEED_PRESETS_MB {
                    let bytes = mb_to_bytes(mb);
                    preset_hit |= bytes == value;
                    menu = menu.item(
                        PopupMenuItem::new(SharedString::from(format!("{mb} MB/s")))
                            .checked(bytes == value)
                            .on_click({
                                let view = view.clone();
                                move |_, _, cx| {
                                    apply_speed_limit(view.clone(), config_key, bytes, cx)
                                }
                            }),
                    );
                }
                menu.separator().item(
                    PopupMenuItem::new(custom_label.clone())
                        .checked(!preset_hit)
                        .on_click({
                            let view = view.clone();
                            let prompt = prompt.clone();
                            move |_, window, cx| {
                                let view = view.clone();
                                open_number_prompt(
                                    window,
                                    cx,
                                    prompt.clone(),
                                    0,
                                    Rc::new(move |kb, cx| {
                                        apply_speed_limit(
                                            view.clone(),
                                            config_key,
                                            kb_to_bytes(kb),
                                            cx,
                                        );
                                    }),
                                );
                            }
                        }),
                )
            })
    }

    /// 完成后关机控件：未启用时只显示电源图标（悬浮提示 + 预设菜单）；
    /// 已启用 / 倒计时中显示 warning 色倒计时文字，点击取消。
    fn render_shutdown_control(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let translator = self.translator.read(cx);
        let Some(shutdown) = self.host.shutdown.clone() else {
            return div().into_any_element();
        };
        let status = self
            .host
            .shutdown_status
            .as_ref()
            .map(|status| status.get())
            .unwrap_or_default();
        let can_arm = self.controller.runtime_stats().active_tasks > 0;
        let cancel_label = SharedString::from(translator.text("shutdownCancelButton").to_owned());
        let warning = active_theme(cx).extended().colors.warning;

        let armed_text = if let Some(remaining) = status.countdown_remaining {
            Some(translator.text_with(
                "shutdownCountdown",
                &[("time", &format_countdown(remaining))],
            ))
        } else {
            status.armed_delay.map(|delay| {
                if delay.is_zero() {
                    translator.text("shutdownArmedHintImmediate").to_owned()
                } else {
                    translator.text_with(
                        "shutdownArmedHint",
                        &[("m", &(delay.as_secs() / 60).to_string())],
                    )
                }
            })
        };
        if let Some(text) = armed_text {
            return status_button("shutdown-cancel", cx)
                .tooltip(cancel_label)
                .child(status_button_content(
                    Some(FluxIcon::Power),
                    Some(SharedString::from(text)),
                    Some(warning),
                    cx,
                ))
                .on_click(move |_, _, cx| shutdown(ShutdownRequest::Disarm, cx))
                .into_any_element();
        }

        let title = SharedString::from(translator.text("shutdownTitle").to_owned());
        let need_active_hint =
            SharedString::from(translator.text("shutdownNeedActiveTask").to_owned());
        let immediate_label = SharedString::from(translator.text("shutdownImmediate").to_owned());
        let custom_label = SharedString::from(translator.text("speedLimitCustom").to_owned());
        let prompt = NumberPrompt {
            title: title.clone(),
            label: SharedString::from(translator.text("shutdownDelayLabel").to_owned()),
            unit: SharedString::from(translator.text("shutdownMinutesUnit").to_owned()),
            hint: None,
            cancel_label: SharedString::from(translator.text("cancel").to_owned()),
            confirm_label: SharedString::from(translator.text("confirm").to_owned()),
        };
        let minute_presets: Vec<(i64, SharedString)> = SHUTDOWN_PRESETS_MIN
            .iter()
            .map(|&minutes| {
                let label =
                    translator.text_with("shutdownDelayMinutes", &[("m", &minutes.to_string())]);
                (minutes, SharedString::from(label))
            })
            .collect();

        status_button("shutdown-trigger", cx)
            .min_w(active_theme(cx).density().status_control)
            .child(status_button_content(Some(FluxIcon::Power), None, None, cx))
            .tooltip(if can_arm {
                title.clone()
            } else {
                need_active_hint
            })
            .dropdown_menu_with_anchor(Anchor::BottomRight, move |menu, _, _| {
                let mut menu = menu.item(
                    PopupMenuItem::new(immediate_label.clone())
                        .disabled(!can_arm)
                        .on_click({
                            let shutdown = shutdown.clone();
                            move |_, _, cx| {
                                shutdown(ShutdownRequest::ArmAfter(Duration::ZERO), cx);
                            }
                        }),
                );
                for (minutes, label) in minute_presets.clone() {
                    menu = menu.item(PopupMenuItem::new(label).disabled(!can_arm).on_click({
                        let shutdown = shutdown.clone();
                        move |_, _, cx| {
                            shutdown(ShutdownRequest::ArmAfter(minutes_to_duration(minutes)), cx);
                        }
                    }));
                }
                menu.separator().item(
                    PopupMenuItem::new(custom_label.clone())
                        .disabled(!can_arm)
                        .on_click({
                            let shutdown = shutdown.clone();
                            let prompt = prompt.clone();
                            move |_, window, cx| {
                                let shutdown = shutdown.clone();
                                open_number_prompt(
                                    window,
                                    cx,
                                    prompt.clone(),
                                    // 0 分钟等同立即关机，「立即」是独立菜单项；自定义必须 ≥ 1。
                                    1,
                                    Rc::new(move |minutes, cx| {
                                        shutdown(
                                            ShutdownRequest::ArmAfter(minutes_to_duration(minutes)),
                                            cx,
                                        );
                                    }),
                                );
                            }
                        }),
                )
            })
            .into_any_element()
    }

    /// 状态栏里的小图标按钮（全部暂停 / 全部开始）；`blocker` 为不可用原因：有值时置灰
    /// 禁用，tooltip 改为说明原因（tooltip 挂在外层，禁用时仍可悬停）。
    fn render_status_action(
        &self,
        id: &'static str,
        label: SharedString,
        icon: FluxIcon,
        command: ToolbarCommand,
        blocker: Option<SharedString>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = active_theme(cx);
        let icon_size = theme.extended().icon.sm;
        let control_size = theme.density().status_control;
        let disabled = blocker.is_some();
        let tooltip_label = blocker.unwrap_or_else(|| label.clone());
        // 禁用的按钮不派发点击（`toolbar_action_button` 经 `Button::disabled`）。
        let button = toolbar_action_button(
            id,
            label,
            Icon::new(icon).size(icon_size),
            false,
            disabled,
            cx,
        )
        .size(control_size)
        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
            this.execute_toolbar(command, cx);
        }));
        div()
            .id(SharedString::from(format!("{id}-tooltip")))
            .flex_none()
            .tooltip(move |window, cx| Tooltip::new(tooltip_label.clone()).build(window, cx))
            .child(button)
    }

    /// 状态栏：左 = 全局速度 + 全部暂停 / 全部开始；右 = 限速、代理、完成后关机、剩余空间。
    pub(crate) fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let stats = self.controller.runtime_stats();
        let download_speed = format!("{}/s", format_bytes(stats.total_download_bps.max(0) as u64));
        let upload_speed = format!("{}/s", format_bytes(stats.total_upload_bps.max(0) as u64));
        let disk_free = stats.disk_free_bytes.map(|bytes| {
            self.translator
                .read(cx)
                .text_with("diskSpaceFreeLabel", &[("size", &format_bytes(bytes))])
        });

        let theme = active_theme(cx);
        let spacing = theme.tokens().spacing;
        let muted = theme.tokens().colors.muted_foreground;
        let extended = theme.extended();
        let chrome = extended.colors.chrome;
        let hairline = extended.colors.hairline;
        let stroke = extended.stroke.thin;
        let bar_height = theme.density().status_bar;
        let caption_size = extended.caption.size;
        let caption_line_height = extended.caption.line_height;
        let icon_size = extended.icon.sm;

        let pause_all = self.render_status_action(
            "status-pause-all",
            self.strings.pause_all.clone(),
            FluxIcon::Pause,
            ToolbarCommand::PauseAll,
            self.pause_all_blocker(),
            cx,
        );
        let resume_all = self.render_status_action(
            "status-resume-all",
            self.strings.resume_all.clone(),
            FluxIcon::Play,
            ToolbarCommand::ResumeAll,
            self.resume_all_blocker(),
            cx,
        );
        let download_limit = self.render_speed_limit_control(
            "status-download-limit",
            FluxIcon::ArrowDown,
            "speedLimitTitle",
            "speed_limit_bytes",
            cx,
        );
        let upload_limit = self.render_speed_limit_control(
            "status-upload-limit",
            FluxIcon::ArrowUp,
            "uploadLimit",
            "upload_limit_bytes",
            cx,
        );
        let proxy_control = self.render_proxy_control(cx);
        let shutdown_control = self.render_shutdown_control(cx);
        let icon_cell = move |icon: FluxIcon, text: String| {
            h_flex()
                .flex_none()
                .items_center()
                .gap(spacing.xxs)
                .child(Icon::new(icon).size(icon_size).text_color(muted))
                .child(text)
        };

        h_flex()
            .w_full()
            .h(bar_height)
            .flex_none()
            .items_center()
            .justify_between()
            .gap(spacing.md)
            .px(spacing.sm)
            .bg(chrome)
            .border_t(stroke)
            .border_color(hairline)
            .text_size(caption_size)
            .line_height(caption_line_height)
            .font_features(tabular_numbers())
            .text_color(muted)
            .child(
                h_flex()
                    .min_w_0()
                    .items_center()
                    .gap(spacing.md)
                    .child(icon_cell(FluxIcon::ArrowDown, download_speed))
                    .child(icon_cell(FluxIcon::ArrowUp, upload_speed))
                    .child(
                        h_flex()
                            .items_center()
                            .gap(spacing.xxs)
                            .child(pause_all)
                            .child(resume_all),
                    ),
            )
            .child(
                h_flex()
                    .min_w_0()
                    .items_center()
                    .gap(spacing.xs)
                    .child(download_limit)
                    .child(upload_limit)
                    .child(proxy_control)
                    .child(shutdown_control)
                    .children(
                        // 与左侧按钮内边距对齐，使右侧控件与磁盘信息的视觉间距一致。
                        disk_free.map(|disk_free| {
                            icon_cell(FluxIcon::HardDrive, disk_free).px(spacing.xs)
                        }),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        format_countdown, kb_to_bytes, mb_to_bytes, minutes_to_duration, parse_number_input,
    };

    #[test]
    fn number_input_rejects_blank_garbage_and_below_minimum() {
        assert_eq!(parse_number_input(" 15 ", 1), Some(15));
        assert_eq!(parse_number_input("", 1), None);
        assert_eq!(parse_number_input("abc", 1), None);
        assert_eq!(parse_number_input("-5", 1), None);
        assert_eq!(parse_number_input("0", 1), None);
        assert_eq!(parse_number_input("0", 0), Some(0));
        assert_eq!(parse_number_input("", 0), None);
    }

    #[test]
    fn mb_preset_converts_to_bytes_per_second() {
        assert_eq!(mb_to_bytes(1), 1_048_576);
        assert_eq!(mb_to_bytes(50), 52_428_800);
    }

    #[test]
    fn kb_custom_input_converts_to_bytes_and_clamps_negative() {
        assert_eq!(kb_to_bytes(512), 524_288);
        assert_eq!(kb_to_bytes(0), 0);
        assert_eq!(kb_to_bytes(-10), 0);
    }

    #[test]
    fn minutes_preset_converts_to_duration_and_clamps_negative() {
        assert_eq!(minutes_to_duration(5), Duration::from_secs(300));
        assert_eq!(minutes_to_duration(0), Duration::ZERO);
        assert_eq!(minutes_to_duration(-1), Duration::ZERO);
    }

    #[test]
    fn countdown_switches_from_mm_ss_to_h_mm_ss_at_one_hour() {
        assert_eq!(format_countdown(Duration::from_secs(59)), "0:59");
        assert_eq!(format_countdown(Duration::from_secs(3599)), "59:59");
        assert_eq!(format_countdown(Duration::from_secs(3600)), "1:00:00");
        assert_eq!(format_countdown(Duration::from_secs(3661)), "1:01:01");
    }
}
