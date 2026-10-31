//! 分享二维码对话框。

use std::sync::Arc;

use fluxdown_ui_components::{QrPalette, dialog_title, qr_image};
use fluxdown_ui_i18n::Translator;
use fluxdown_ui_theme::active_theme;
use gpui::{
    App, Entity, FontWeight, Image, InteractiveElement as _, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, img,
};
use gpui_component::{WindowExt as _, v_flex};

use crate::model::receive::single_line;

/// 二维码对话框内容宽度（基准 px，随字号缩放）。
const DIALOG_WIDTH: f32 = 340.;
/// 二维码显示边长（基准 px，随字号缩放）。
const QR_SIZE: f32 = 208.;

/// 打开分享链接的二维码对话框：打开时只编码一次，渲染读取已编码的图；
/// 链接无法编码（为空或超出二维码容量）时只显示链接文本。
pub(crate) fn open_share_qr(
    translator: &Entity<Translator>,
    share_title: &str,
    url: &str,
    window: &mut Window,
    cx: &mut App,
) {
    let heading = SharedString::from(translator.read(cx).text("shareQrTitle").to_owned());
    let untitled = translator.read(cx).text("shareUntitled").to_owned();
    let title = SharedString::from(if share_title.trim().is_empty() {
        untitled
    } else {
        single_line(share_title)
    });
    let link = SharedString::from(url.to_owned());
    let qr: Option<Arc<Image>> = qr_image(url, QrPalette::themed(cx)).map(Arc::new);

    window.open_dialog(cx, move |dialog, _, cx| {
        let heading = heading.clone();
        let title = title.clone();
        let link = link.clone();
        let qr = qr.clone();
        dialog
            .title(dialog_title(heading, cx))
            .w(active_theme(cx).text_extent(DIALOG_WIDTH))
            .content(move |content, _, cx| {
                let theme = active_theme(cx);
                let tokens = theme.tokens();
                let extended = theme.extended();
                let qr_size = theme.text_extent(QR_SIZE);
                let title = title.clone();
                let link = link.clone();
                let tooltip_title = title.clone();
                let tooltip_link = link.clone();
                content.min_h_0().child(
                    v_flex()
                        .w_full()
                        .min_w_0()
                        .items_center()
                        .gap(tokens.spacing.md)
                        .children(qr.clone().map(|image| {
                            // 二维码自带不透明静区；外框只加描边与圆角裁切。
                            div()
                                .flex_none()
                                .rounded(tokens.radius.md)
                                .overflow_hidden()
                                .border(extended.stroke.thin)
                                .border_color(extended.colors.hairline)
                                .child(img(image).size(qr_size))
                        }))
                        .child(
                            div()
                                .id("share-qr-title")
                                .w_full()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_center()
                                .text_size(tokens.typography.sm.size)
                                .line_height(tokens.typography.sm.line_height)
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(tokens.colors.foreground)
                                .tooltip(move |window, cx| {
                                    fluxdown_ui_components::tooltip_view(
                                        tooltip_title.clone(),
                                        window,
                                        cx,
                                    )
                                })
                                .child(title),
                        )
                        .child(
                            div()
                                .id("share-qr-link")
                                .w_full()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis_middle()
                                .text_center()
                                .text_size(tokens.typography.xs.size)
                                .line_height(tokens.typography.xs.line_height)
                                .text_color(tokens.colors.muted_foreground)
                                .tooltip(move |window, cx| {
                                    fluxdown_ui_components::tooltip_view(
                                        tooltip_link.clone(),
                                        window,
                                        cx,
                                    )
                                })
                                .child(link),
                        ),
                )
            })
    });
}
