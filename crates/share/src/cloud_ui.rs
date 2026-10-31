//! 云端暂存的共用对话框：转换时选保留期、删除云端副本（上传中为取消上传）的确认。
//!
//! 分享面板与「我的分享」页共用；对话框确认后经 [`crate::actions`] 调 agent，错误以通知展示。

use fluxdown_protocol::{ShareTierDto, ShareUploadStatus};
use fluxdown_ui_components::{DialogIntent, check_row, dialog_footer, dialog_title, field_label};
use fluxdown_ui_i18n::Translator;
use fluxdown_ui_theme::active_theme;
use gpui::{
    App, AppContext as _, Context, ElementId, Entity, Hsla, IntoElement, ParentElement as _,
    Render, SharedString, Styled as _, Window, div,
};
use gpui_component::{WindowExt as _, v_flex};

use crate::{
    actions::{convert_to_cloud, delete_cloud_copy},
    errors::reason_key,
    hub::ShareHub,
    model::{
        cloud::{CloudTone, UploadLine, default_tier_index},
        format_bytes, format_percent, format_rate, tier_label,
    },
    window_ui::{count_text, text},
};

/// 转换对话框内容宽度（基准 px，随字号缩放）。
const DIALOG_WIDTH: f32 = 380.;

/// 保留期选择：档位列表 + 当前选中的下标。
struct TierPicker {
    /// `(档位 ID, 展示文案)`，顺序同云端策略。
    tiers: Vec<(String, SharedString)>,
    selected: usize,
}

impl TierPicker {
    fn selected_id(&self) -> Option<String> {
        self.tiers.get(self.selected).map(|(id, _)| id.clone())
    }
}

impl Render for TierPicker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let tokens = active_theme(cx).tokens().clone();
        v_flex()
            .w_full()
            .gap(tokens.spacing.xxs)
            .children(self.tiers.iter().enumerate().map(|(index, (_, label))| {
                let view = view.clone();
                check_row(
                    ElementId::NamedInteger(SharedString::from("share-stage-tier"), index as u64),
                    index == self.selected,
                    label.clone(),
                    move |_, _, cx| {
                        view.update(cx, |this, cx| {
                            this.selected = index;
                            cx.notify();
                        });
                    },
                    cx,
                )
            }))
    }
}

/// 云端策略给出保留期档位 → 选择项。
fn tier_items(
    translator: &fluxdown_ui_i18n::Translator,
    tiers: &[ShareTierDto],
) -> Vec<(String, SharedString)> {
    tiers
        .iter()
        .map(|tier| {
            (
                tier.id.clone(),
                count_text(translator, tier_label(tier.hours)),
            )
        })
        .collect()
}

/// 「转为云端暂存…」：先让用户选保留期再确认；云端没有给出保留期档位时直接用云端默认档开始。
pub(crate) fn open_convert_dialog(
    hub: &Entity<ShareHub>,
    share_id: String,
    window: &mut Window,
    cx: &mut App,
) {
    let (tiers, translator) = {
        let hub_ref = hub.read(cx);
        (
            hub_ref.state().policy_summary.retention_tiers.clone(),
            hub_ref.translator().read(cx).clone(),
        )
    };
    let Some(default_index) = default_tier_index(&tiers) else {
        convert_to_cloud(hub, &share_id, None, window, cx);
        return;
    };
    let picker = cx.new(|_| TierPicker {
        tiers: tier_items(&translator, &tiers),
        selected: default_index,
    });
    let title = text(&translator, "shareStageConvertTitle");
    let hint = text(&translator, "shareStageConvertHint");
    let tier_label_text = text(&translator, "shareStageTier");
    let ok = text(&translator, "shareStageConvertConfirm");
    let cancel = text(&translator, "cancel");
    let hub = hub.clone();
    window.open_dialog(cx, move |dialog, _, cx| {
        let picker_content = picker.clone();
        let picker_ok = picker.clone();
        let hub = hub.clone();
        let share_id = share_id.clone();
        let hint = hint.clone();
        let tier_label_text = tier_label_text.clone();
        dialog
            .title(dialog_title(title.clone(), cx))
            .w(active_theme(cx).text_extent(DIALOG_WIDTH))
            .content(move |content, _, cx| {
                let tokens = active_theme(cx).tokens().clone();
                content.min_h_0().child(
                    v_flex()
                        .w_full()
                        .min_w_0()
                        .gap(tokens.spacing.md)
                        .child(
                            div()
                                .text_size(tokens.typography.sm.size)
                                .line_height(tokens.typography.sm.line_height)
                                .text_color(tokens.colors.muted_foreground)
                                .child(hint.clone()),
                        )
                        .child(
                            v_flex()
                                .w_full()
                                .gap(tokens.spacing.xs)
                                .child(field_label(tier_label_text.clone(), cx))
                                .child(picker_content.clone()),
                        ),
                )
            })
            .footer(dialog_footer(
                Some(cancel.clone()),
                ok.clone(),
                DialogIntent::Confirm,
                cx,
            ))
            .on_ok(move |_, window, cx| {
                let tier = picker_ok.read(cx).selected_id();
                convert_to_cloud(&hub, &share_id, tier.as_deref(), window, cx);
                true
            })
    });
}

/// 确认后删除云端副本；`uploading` 为 `true` 时文案是「取消上传」。
pub(crate) fn confirm_delete_cloud_copy(
    hub: &Entity<ShareHub>,
    share_id: String,
    uploading: bool,
    window: &mut Window,
    cx: &mut App,
) {
    let translator = hub.read(cx).translator().read(cx).clone();
    let (title_key, description_key) = if uploading {
        ("shareStageCancelUpload", "shareStageCancelConfirm")
    } else {
        ("shareStageDelete", "shareStageDeleteConfirm")
    };
    let title = text(&translator, title_key);
    let description = text(&translator, description_key);
    let cancel = text(&translator, "cancel");
    let hub = hub.clone();
    window.open_alert_dialog(cx, move |alert, _, cx| {
        let hub = hub.clone();
        let share_id = share_id.clone();
        alert
            .title(dialog_title(title.clone(), cx))
            .description(description.clone())
            .footer(dialog_footer(
                Some(cancel.clone()),
                title.clone(),
                DialogIntent::Destructive,
                cx,
            ))
            .on_ok(move |_, window, cx| {
                delete_cloud_copy(&hub, &share_id, window, cx);
                true
            })
    });
}

/// 云端状态的着色：就绪 = 成功，上传中 / 暂停 = 警示，失败 = 危险，其余中性。
pub(crate) fn tone_color(tone: CloudTone, cx: &App) -> Hsla {
    let theme = active_theme(cx);
    match tone {
        CloudTone::Good => theme.extended().colors.success,
        CloudTone::Warn => theme.extended().colors.warning,
        CloudTone::Bad => theme.tokens().colors.destructive,
        CloudTone::Muted => theme.tokens().colors.muted_foreground,
    }
}

/// 上传进度行的两段文案：`(状态 [· 原因], 已传 / 总量 · 百分比 [· 速率])`。
pub(crate) fn upload_texts(translator: &Translator, line: &UploadLine) -> (String, String) {
    let mut status = translator
        .text(match line.status {
            ShareUploadStatus::Uploading => "shareStageUploading",
            ShareUploadStatus::Paused => "shareStagePaused",
            ShareUploadStatus::Failed => "shareStageFailed",
        })
        .to_owned();
    if line.status != ShareUploadStatus::Uploading
        && let Some(reason) = line.reason
    {
        status.push_str(" · ");
        status.push_str(translator.text(reason_key(Some(reason))));
    }
    let percent = format_percent(f64::from(line.percent) / 100.);
    let done = format_bytes(line.done_bytes);
    let total = format_bytes(line.total_bytes);
    let detail = if line.status == ShareUploadStatus::Uploading {
        translator.text_with(
            "shareStageProgress",
            &[
                ("done", &done),
                ("total", &total),
                ("percent", &percent),
                ("rate", &format_rate(line.rate_bps)),
            ],
        )
    } else {
        translator.text_with(
            "shareStageProgressIdle",
            &[("done", &done), ("total", &total), ("percent", &percent)],
        )
    };
    (status, detail)
}
