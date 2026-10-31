//! 举报对话框：5 个固定原因单选 + 补充说明 + 联系方式，经 `agent.share.report` 提交。
//!
//! 入口刻意弱化（接收确认页脚的文字按钮、任务详情「传输诊断」接收信息行的文字按钮），不占主按钮区。

use fluxdown_protocol::{ShareReportParams, ShareReportReason};
use fluxdown_ui_components::{
    ControlExt as _, DialogIntent, check_row, dialog_footer, dialog_title, field_label,
};
use fluxdown_ui_theme::active_theme;
use gpui::{
    App, AppContext as _, Context, ElementId, Entity, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Window,
};
use gpui_component::{
    WindowExt as _,
    input::{Input, InputState, Textarea, TextareaState},
    v_flex,
};

use crate::{
    actions::{notify_error, notify_success},
    errors::error_text,
    hub::ShareHub,
    window_ui::text,
};

/// 对话框内容宽度（基准 px，随字号缩放）。
const DIALOG_WIDTH: f32 = 400.;
/// 补充说明上限（字符，与云端校验一致）。
const DETAIL_MAX_CHARS: usize = 2000;
/// 联系方式上限（字符，与云端校验一致）。
const CONTACT_MAX_CHARS: usize = 200;

/// 原因的文案键。
#[must_use]
pub(crate) const fn reason_label_key(reason: ShareReportReason) -> &'static str {
    match reason {
        ShareReportReason::Copyright => "shareReportReasonCopyright",
        ShareReportReason::Porn => "shareReportReasonPorn",
        ShareReportReason::Illegal => "shareReportReasonIllegal",
        ShareReportReason::Malware => "shareReportReasonMalware",
        ShareReportReason::Other => "shareReportReasonOther",
    }
}

/// 空白输入视为未填；其余按字符边界截断到上限。
fn normalized(value: &str, max_chars: usize) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.chars().take(max_chars).collect())
}

/// 表单状态：当前原因 + 两个输入框。
struct ReportForm {
    reasons: Vec<(ShareReportReason, SharedString)>,
    selected: ShareReportReason,
    detail_label: SharedString,
    contact_label: SharedString,
    detail: Entity<TextareaState>,
    contact: Entity<InputState>,
}

impl ReportForm {
    fn params(&self, share_id: &str, cx: &App) -> ShareReportParams {
        ShareReportParams {
            share_id: share_id.to_owned(),
            reason: self.selected,
            detail: normalized(&self.detail.read(cx).value(), DETAIL_MAX_CHARS),
            contact: normalized(&self.contact.read(cx).value(), CONTACT_MAX_CHARS),
        }
    }
}

impl Render for ReportForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let tokens = active_theme(cx).tokens().clone();
        v_flex()
            .w_full()
            .min_w_0()
            .gap(tokens.spacing.md)
            .child(
                v_flex().w_full().gap(tokens.spacing.xxs).children(
                    self.reasons
                        .iter()
                        .enumerate()
                        .map(|(index, (reason, label))| {
                            let view = view.clone();
                            let reason = *reason;
                            check_row(
                                ElementId::NamedInteger(
                                    SharedString::from("share-report-reason"),
                                    index as u64,
                                ),
                                reason == self.selected,
                                label.clone(),
                                move |_, _, cx| {
                                    view.update(cx, |this, cx| {
                                        this.selected = reason;
                                        cx.notify();
                                    });
                                },
                                cx,
                            )
                        }),
                ),
            )
            .child(
                v_flex()
                    .w_full()
                    .gap(tokens.spacing.xs)
                    .child(field_label(self.detail_label.clone(), cx))
                    .child(Textarea::new(&self.detail).w_full()),
            )
            .child(
                v_flex()
                    .w_full()
                    .gap(tokens.spacing.xs)
                    .child(field_label(self.contact_label.clone(), cx))
                    .child(Input::new(&self.contact).control(cx).w_full()),
            )
    }
}

/// 打开举报对话框；`share_id` 是云端分享 ID。提交后在触发窗口给出成功 / 错误通知。
pub(crate) fn open_report_dialog(
    hub: &Entity<ShareHub>,
    share_id: String,
    window: &mut Window,
    cx: &mut App,
) {
    let translator = hub.read(cx).translator().read(cx).clone();
    let reasons: Vec<(ShareReportReason, SharedString)> = ShareReportReason::ALL
        .iter()
        .map(|reason| (*reason, text(&translator, reason_label_key(*reason))))
        .collect();
    let form = cx.new(|cx| ReportForm {
        reasons,
        selected: ShareReportReason::Copyright,
        detail_label: text(&translator, "shareReportDetail"),
        contact_label: text(&translator, "shareReportContact"),
        detail: cx.new(|cx| TextareaState::new(window, cx).auto_grow(3, 6)),
        contact: cx.new(|cx| InputState::new(window, cx)),
    });
    let title = text(&translator, "shareReportTitle");
    let ok = text(&translator, "shareReportSubmit");
    let cancel = text(&translator, "cancel");
    let sent = text(&translator, "shareReportSent");
    let hub = hub.clone();
    window.open_dialog(cx, move |dialog, _, cx| {
        let form_content = form.clone();
        let form_ok = form.clone();
        let hub = hub.clone();
        let share_id = share_id.clone();
        let translator = translator.clone();
        let sent = sent.clone();
        dialog
            .title(dialog_title(title.clone(), cx))
            .w(active_theme(cx).text_extent(DIALOG_WIDTH))
            .content(move |content, _, _| content.min_h_0().child(form_content.clone()))
            .footer(dialog_footer(
                Some(cancel.clone()),
                ok.clone(),
                DialogIntent::Confirm,
                cx,
            ))
            .on_ok(move |_, window, cx| {
                let params = form_ok.read(cx).params(&share_id, cx);
                let future = hub.read(cx).api().report(&params);
                let handle = window.window_handle();
                let translator = translator.clone();
                let sent = sent.clone();
                cx.spawn(async move |cx| match future.await {
                    Ok(_) => notify_success(handle, cx, sent.to_string()),
                    Err(error) => notify_error(handle, cx, error_text(&translator, &error)),
                })
                .detach();
                true
            })
    });
}

#[cfg(test)]
mod tests {
    use fluxdown_protocol::ShareReportReason;

    use super::{normalized, reason_label_key};

    #[test]
    fn every_reason_has_a_distinct_label_key() {
        let keys: Vec<&str> = ShareReportReason::ALL
            .iter()
            .map(|reason| reason_label_key(*reason))
            .collect();
        assert_eq!(keys.len(), 5);
        for (index, key) in keys.iter().enumerate() {
            assert!(key.starts_with("shareReportReason"));
            assert!(!keys[..index].contains(key));
        }
    }

    #[test]
    fn blank_text_is_omitted_and_long_text_cut_on_char_boundary() {
        assert_eq!(normalized("  \n ", 10), None);
        assert_eq!(normalized(" 举报 ", 10).as_deref(), Some("举报"));
        assert_eq!(normalized("举报内容很长", 3).as_deref(), Some("举报内"));
    }
}
