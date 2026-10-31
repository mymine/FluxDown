//! 申诉对话框：对已下架的分享说明理由，经 `agent.share.appeal` 提交（每个分享一次）。

use fluxdown_protocol::ShareAppealParams;
use fluxdown_ui_components::{DialogIntent, dialog_footer, dialog_title, field_label};
use fluxdown_ui_theme::active_theme;
use gpui::{App, AppContext as _, Entity, ParentElement as _, Styled as _, Window};
use gpui_component::{
    WindowExt as _,
    input::{Textarea, TextareaState},
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
/// 申诉说明上限（字符，与云端校验一致）。
const STATEMENT_MAX_CHARS: usize = 1000;

/// 去掉首尾空白后按字符边界截断；空白返回 `None`（不提交）。
fn normalized_statement(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.chars().take(STATEMENT_MAX_CHARS).collect())
}

/// 打开申诉对话框；说明为空时点击提交不关闭对话框。
pub(crate) fn open_appeal_dialog(
    hub: &Entity<ShareHub>,
    share_id: String,
    window: &mut Window,
    cx: &mut App,
) {
    let translator = hub.read(cx).translator().read(cx).clone();
    let statement = cx.new(|cx| TextareaState::new(window, cx).auto_grow(4, 8));
    let title = text(&translator, "shareAppealTitle");
    let label = text(&translator, "shareAppealStatement");
    let ok = text(&translator, "shareReportSubmit");
    let cancel = text(&translator, "cancel");
    let sent = text(&translator, "shareAppealSent");
    let hub = hub.clone();
    window.open_dialog(cx, move |dialog, _, cx| {
        let statement_content = statement.clone();
        let statement_ok = statement.clone();
        let hub = hub.clone();
        let share_id = share_id.clone();
        let translator = translator.clone();
        let sent = sent.clone();
        let label = label.clone();
        dialog
            .title(dialog_title(title.clone(), cx))
            .w(active_theme(cx).text_extent(DIALOG_WIDTH))
            .content(move |content, _, cx| {
                let tokens = active_theme(cx).tokens().clone();
                content.min_h_0().child(
                    v_flex()
                        .w_full()
                        .min_w_0()
                        .gap(tokens.spacing.xs)
                        .child(field_label(label.clone(), cx))
                        .child(Textarea::new(&statement_content).w_full()),
                )
            })
            .footer(dialog_footer(
                Some(cancel.clone()),
                ok.clone(),
                DialogIntent::Confirm,
                cx,
            ))
            .on_ok(move |_, window, cx| {
                let Some(statement) = normalized_statement(&statement_ok.read(cx).value()) else {
                    return false;
                };
                let future = hub.read(cx).api().appeal(&ShareAppealParams {
                    share_id: share_id.clone(),
                    statement,
                });
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
    use super::{STATEMENT_MAX_CHARS, normalized_statement};

    #[test]
    fn blank_statement_is_rejected_and_long_one_is_capped() {
        assert_eq!(normalized_statement("   \n"), None);
        assert_eq!(normalized_statement(" 请复核 ").as_deref(), Some("请复核"));
        let long = "诉".repeat(STATEMENT_MAX_CHARS + 50);
        assert_eq!(
            normalized_statement(&long).map(|value| value.chars().count()),
            Some(STATEMENT_MAX_CHARS)
        );
    }
}
