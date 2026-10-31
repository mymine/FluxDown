//! 一个传输会话的诊断详情渲染：会话摘要、逐路径统计、路径事件时间线。
//! 设置 › FluxMesh › 网络诊断里展开的会话与任务详情「传输诊断」页共用同一份。

use fluxdown_protocol::{
    SharePathKind, SharePathSummaryDto, ShareSessionBytesDto, ShareSessionDetailDto,
};
use fluxdown_ui_components::tabular_numbers;
use fluxdown_ui_i18n::Translator;
use fluxdown_ui_theme::active_theme;
use gpui::{
    App, Div, FontWeight, Hsla, ParentElement as _, SharedString, Styled as _, div,
    prelude::FluentBuilder as _,
};
use gpui_component::{h_flex, v_flex};

use crate::{
    manage_ui::{body_text, chip, clipped, fact, kv_row, meta_text, mono_text, text, text_with},
    model::{
        diagnostics::{
            PLACEHOLDER, bytes_split, event_kind_key, format_bandwidth, format_clock, format_loss,
            format_millis, format_rtt, selected_path_id, served_file_label, short_id,
        },
        format_bytes, format_datetime, nat_key, path_kind_key,
    },
};

/// 时间线最多显示最近多少条事件（agent 的环形缓冲本身有界，这里再兜一层）。
const MAX_EVENTS: usize = 40;
/// 时间线时间列宽（默认字号下的 px，`HH:MM:SS.mmm` 等宽）。
const EVENT_TIME_WIDTH: f32 = 84.;
/// 时间线事件名列宽（默认字号下的 px）。
const EVENT_KIND_WIDTH: f32 = 150.;

/// 路径类别徽标的色调：直连与云端副本用成功色，中继与桥接用警示色，未选定不着色。
pub(crate) fn path_tone(kind: SharePathKind, cx: &App) -> Option<Hsla> {
    let colors = active_theme(cx).extended().colors;
    match kind {
        SharePathKind::DirectLan
        | SharePathKind::DirectWan
        | SharePathKind::Cdn
        | SharePathKind::Swarm => Some(colors.success),
        SharePathKind::Relay | SharePathKind::Bridge => Some(colors.warning),
        SharePathKind::Unknown => None,
    }
}

/// 来源字节摘要：`直连 X (p%) · 中继 Y`；云端暂存 / 桥接有字节时依次追加 ` · 云端 Z` / ` · 桥接 W`。
pub(crate) fn bytes_summary(translator: &Translator, bytes: &ShareSessionBytesDto) -> String {
    let split = bytes_split(bytes);
    let direct = format_bytes(split.direct);
    let relay = format_bytes(split.relay);
    let mut summary = match split.direct_percent {
        Some(percent) => text_with(
            translator,
            "shareDiagBytesSplitPct",
            &[
                ("direct", &direct),
                ("percent", &format!("{percent}%")),
                ("relay", &relay),
            ],
        ),
        None => text_with(
            translator,
            "shareDiagBytesSplit",
            &[("direct", &direct), ("relay", &relay)],
        ),
    }
    .to_string();
    for (key, amount) in split.extras() {
        summary.push_str(" · ");
        summary.push_str(&text_with(
            translator,
            key,
            &[("size", &format_bytes(amount))],
        ));
    }
    summary
}

fn subheading(label: SharedString, cx: &App) -> Div {
    let theme = active_theme(cx);
    let tokens = theme.tokens();
    div()
        .pt(tokens.spacing.xs)
        .text_size(tokens.typography.xs.size)
        .line_height(tokens.typography.xs.line_height)
        .font_weight(FontWeight::MEDIUM)
        .text_color(tokens.colors.muted_foreground)
        .child(label)
}

/// 会话详情。`scope` 作元素 ID 前缀，同一视图里必须唯一（带会话 ID）。
pub(crate) fn session_detail(
    detail: &ShareSessionDetailDto,
    translator: &Translator,
    scope: &str,
    cx: &App,
) -> Div {
    let theme = active_theme(cx);
    let tokens = theme.tokens();
    let colors = tokens.colors;
    let summary = &detail.summary;

    let peer = detail
        .peer_device_name
        .as_deref()
        .filter(|name| !name.trim().is_empty())
        .map_or_else(
            || short_id(&detail.peer_endpoint_id),
            |name| format!("{name} ({})", short_id(&detail.peer_endpoint_id)),
        );
    let bytes_text = SharedString::from(bytes_summary(translator, &summary.bytes));
    let first_direct = summary.first_direct_ms.map_or_else(
        || text(translator, "shareDiagNotDirect"),
        |ms| SharedString::from(format_millis(ms)),
    );

    let mut rows = v_flex().w_full().gap(tokens.spacing.xs);
    rows = rows.child(kv_row(
        text(translator, "shareDiagPeer"),
        clipped(format!("{scope}-peer"), peer),
        cx,
    ));
    rows = rows.child(kv_row(
        text(translator, "shareDiagPeerNat"),
        text(translator, nat_key(detail.peer_nat_type)),
        cx,
    ));
    if let Some(isp) = detail.peer_isp.as_deref().filter(|isp| !isp.is_empty()) {
        rows = rows.child(kv_row(
            text(translator, "shareDiagPeerCarrier"),
            clipped(format!("{scope}-isp"), isp.to_owned()),
            cx,
        ));
    }
    if !summary.started_at.is_empty() {
        rows = rows.child(kv_row(
            text(translator, "shareDiagStarted"),
            SharedString::from(format_datetime(&summary.started_at)),
            cx,
        ));
    }
    rows = rows
        .child(kv_row(
            text(translator, "shareDiagConnectMs"),
            SharedString::from(format_millis(summary.connect_ms)),
            cx,
        ))
        .child(kv_row(
            text(translator, "shareDiagFirstDirect"),
            first_direct,
            cx,
        ))
        .child(kv_row(text(translator, "shareDiagBytes"), bytes_text, cx))
        .child(kv_row(
            text(translator, "shareDiagVerifyFailures"),
            div()
                .when(summary.verify_failures > 0, |this| {
                    this.text_color(colors.destructive)
                })
                .child(SharedString::from(summary.verify_failures.to_string())),
            cx,
        ));
    if let Some(error) = summary.error.as_deref().filter(|error| !error.is_empty()) {
        rows = rows.child(kv_row(
            text(translator, "shareDiagError"),
            clipped(format!("{scope}-error"), error.to_owned()).text_color(colors.destructive),
            cx,
        ));
    }

    let current = selected_path_id(&summary.events);
    let paths = if summary.paths.is_empty() {
        vec![meta_text(cx).child(text(translator, "shareDiagNoPaths"))]
    } else {
        summary
            .paths
            .iter()
            .enumerate()
            .map(|(index, path)| {
                path_row(
                    path,
                    current == Some(path.id.as_str()),
                    translator,
                    &format!("{scope}-path-{index}"),
                    cx,
                )
            })
            .collect()
    };

    let skipped = summary.events.len().saturating_sub(MAX_EVENTS);
    let events: Vec<Div> = summary
        .events
        .iter()
        .skip(skipped)
        .enumerate()
        .map(|(index, event)| {
            let kind = event_kind_key(&event.kind).map_or_else(
                || SharedString::from(event.kind.clone()),
                |key| text(translator, key),
            );
            h_flex()
                .w_full()
                .items_baseline()
                .gap(tokens.spacing.md)
                .child(
                    mono_text(cx)
                        .w(theme.text_extent(EVENT_TIME_WIDTH))
                        .flex_none()
                        .text_color(colors.muted_foreground)
                        .child(SharedString::from(format_clock(&event.at))),
                )
                .child(
                    body_text(cx)
                        .w(theme.text_extent(EVENT_KIND_WIDTH))
                        .flex_none()
                        .child(clipped(format!("{scope}-event-kind-{index}"), kind)),
                )
                .child(
                    mono_text(cx)
                        .flex_1()
                        .min_w_0()
                        .text_color(colors.muted_foreground)
                        .child(clipped(
                            format!("{scope}-event-path-{index}"),
                            event.path.clone(),
                        )),
                )
        })
        .collect();

    // 共享提供会话：按内容累计的服务量（文件名本机未知时显示短根哈希）。
    let served: Vec<Div> = detail
        .swarm_files
        .iter()
        .enumerate()
        .map(|(index, file)| {
            h_flex()
                .w_full()
                .items_baseline()
                .gap(tokens.spacing.md)
                .child(body_text(cx).flex_1().min_w_0().child(clipped(
                    format!("{scope}-served-{index}"),
                    served_file_label(file),
                )))
                .child(
                    meta_text(cx)
                        .flex_none()
                        .font_features(tabular_numbers())
                        .child(SharedString::from(format!(
                            "{} · {}",
                            format_bytes(file.bytes),
                            text_with(
                                translator,
                                "shareDiagSwarmChunks",
                                &[("count", &file.chunks.to_string())],
                            ),
                        ))),
                )
        })
        .collect();

    v_flex()
        .w_full()
        .gap(tokens.spacing.sm)
        .child(subheading(text(translator, "shareDiagSummary"), cx))
        .child(rows)
        .when(!served.is_empty(), |this| {
            this.child(subheading(text(translator, "shareDiagSwarmFiles"), cx))
                .child(v_flex().w_full().gap(tokens.spacing.xxs).children(served))
        })
        .child(subheading(text(translator, "shareDiagPaths"), cx))
        .child(v_flex().w_full().gap(tokens.spacing.sm).children(paths))
        .child(subheading(text(translator, "shareDiagTimeline"), cx))
        .child(if events.is_empty() {
            v_flex().child(meta_text(cx).child(text(translator, "shareDiagNoEvents")))
        } else {
            v_flex().w_full().gap(tokens.spacing.xxs).children(events)
        })
}

/// 一条路径：首行类别徽标 + 对端地址，次行带标签的 RTT / 丢包 / 拥塞窗口 / 带宽估计 / 收发包数。
fn path_row(
    path: &SharePathSummaryDto,
    in_use: bool,
    translator: &Translator,
    scope: &str,
    cx: &App,
) -> Div {
    let theme = active_theme(cx);
    let tokens = theme.tokens();
    let remote = if path.remote.is_empty() {
        PLACEHOLDER.to_owned()
    } else {
        path.remote.clone()
    };
    v_flex()
        .w_full()
        .gap(tokens.spacing.xxs)
        .child(
            h_flex()
                .w_full()
                .items_center()
                .gap(tokens.spacing.sm)
                .child(chip(
                    format!("{scope}-kind"),
                    text(translator, path_kind_key(path.kind)),
                    path_tone(path.kind, cx),
                    cx,
                ))
                .when(in_use, |row| {
                    row.child(chip(
                        format!("{scope}-use"),
                        text(translator, "shareDiagCurrentPath"),
                        Some(theme.extended().colors.accent_text),
                        cx,
                    ))
                })
                .child(
                    mono_text(cx)
                        .flex_1()
                        .min_w_0()
                        .child(clipped(format!("{scope}-remote"), remote)),
                ),
        )
        .child(
            h_flex()
                .w_full()
                .flex_wrap()
                .gap_x(tokens.spacing.lg)
                .gap_y(tokens.spacing.xxs)
                .child(fact(
                    text(translator, "shareDiagColRtt"),
                    format_rtt(path.rtt_ms),
                    cx,
                ))
                .child(fact(
                    text(translator, "shareDiagColLoss"),
                    format_loss(path),
                    cx,
                ))
                .child(fact(
                    text(translator, "shareDiagColCwnd"),
                    path.cwnd.to_string(),
                    cx,
                ))
                .child(fact(
                    text(translator, "shareDiagColBandwidth"),
                    format_bandwidth(path.bw_estimate),
                    cx,
                ))
                .child(fact(
                    text(translator, "shareDiagColTraffic"),
                    format!("{} / {}", path.sent, path.recv),
                    cx,
                )),
        )
}
