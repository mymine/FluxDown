//! 停靠详情面板的「任务组概览」：组行聚焦时取代任务详情，聚合全组（不受列表筛选影响）。

use fluxdown_ui_components::{ControlExt as _, FluxIcon, tabular_numbers};
use fluxdown_ui_theme::active_theme;
use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement, SharedString, StatefulInteractiveElement as _, Styled, div,
    prelude::FluentBuilder as _, px,
};
use gpui_component::{button::Button, h_flex, progress::Progress, v_flex};

use super::DownloadView;
use crate::{
    components::{
        group_visuals::{group_counts_line, group_folder_icon},
        task_table::status_color,
    },
    model::{TaskState, format_bytes},
    pages::task_detail::detail_row,
    strings::DownloadStrings,
};

/// 概览头部文件夹图标边长。
const HEADER_ICON_SIZE: f32 = 32.;

impl DownloadView {
    /// 组概览；组已不在快照里（被删除）时返回 `None`，面板回到空态。
    pub(super) fn render_group_overview(
        &self,
        group_id: &str,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let summary = self.controller.group_summary(group_id)?;
        let aggregate = summary.aggregate;
        let counts = aggregate.counts;
        let state = aggregate.state();
        let strings = &self.strings;
        let queue = {
            let local = self.controller.store().local();
            let mut queues = local
                .iter()
                .filter(|task| task.group_id == group_id)
                .map(|task| task.queue_id.as_str());
            let first = queues.next().map(str::to_owned);
            first.filter(|first| queues.all(|queue| queue == first))
        };
        let queue_label = queue.map(|queue_id| {
            let name = self.controller.queue_name(&queue_id).unwrap_or_default();
            strings.queue_name(&queue_id, name)
        });

        let theme = active_theme(cx);
        let tokens = theme.tokens().clone();
        let extended = theme.extended().clone();
        let progress_line = if aggregate.speed > 0 {
            let speed = format!("{}/s", format_bytes(aggregate.speed));
            match aggregate.eta_seconds.filter(|seconds| *seconds <= 86_400) {
                Some(eta) => format!("{speed} · {}", strings.format_eta(eta)),
                None => speed,
            }
        } else {
            strings.state_label(state).to_string()
        };
        let bytes_line = if aggregate.size_bytes > 0 {
            format!(
                "{} / {}",
                format_bytes(aggregate.downloaded_bytes),
                format_bytes(aggregate.size_bytes)
            )
        } else {
            format_bytes(aggregate.downloaded_bytes)
        };

        let mut actions = h_flex().gap(tokens.spacing.xs).flex_wrap();
        if counts.any_active() {
            actions = actions.child(self.group_panel_button(
                "group-panel-pause",
                FluxIcon::Pause,
                strings.group_pause_all.clone(),
                group_id,
                |view, id, cx| view.group_pause_all(id, cx),
                cx,
            ));
        } else if counts.any_resumable() {
            actions = actions.child(self.group_panel_button(
                "group-panel-resume",
                FluxIcon::Play,
                strings.group_resume_all.clone(),
                group_id,
                |view, id, cx| view.group_resume_all(id, cx),
                cx,
            ));
        }
        if counts.failed > 0 {
            actions = actions.child(self.group_panel_button(
                "group-panel-retry",
                FluxIcon::RotateCw,
                strings.group_retry_failed.clone(),
                group_id,
                |view, id, cx| view.group_retry_failed(id, cx),
                cx,
            ));
        }
        actions = actions.child(self.group_panel_button(
            "group-panel-reveal",
            FluxIcon::FolderOpen,
            strings.group_open_folder.clone(),
            group_id,
            |view, id, cx| view.group_open_folder(id, cx),
            cx,
        ));

        let created = if summary.meta.created_at_secs > 0 {
            DownloadStrings::format_datetime(summary.meta.created_at_secs)
        } else {
            SharedString::from("—")
        };
        let info = v_flex()
            .when(!summary.meta.source_url.is_empty(), |this| {
                this.child(detail_row(
                    strings.group_detail_source.clone(),
                    div()
                        .truncate()
                        .child(SharedString::from(summary.meta.source_url.clone())),
                    cx,
                ))
            })
            .child(detail_row(
                strings.group_detail_save_dir.clone(),
                div()
                    .truncate()
                    .child(SharedString::from(summary.meta.save_dir.clone())),
                cx,
            ))
            .child(detail_row(
                strings.group_detail_created_at.clone(),
                div().child(created),
                cx,
            ))
            .when_some(queue_label, |this, queue| {
                this.child(detail_row(
                    strings.group_detail_queue.clone(),
                    div().child(queue),
                    cx,
                ))
            });

        let percent = (aggregate.progress * 100.).clamp(0., 100.);
        Some(
            v_flex()
                .id("group-overview")
                .size_full()
                .min_w_0()
                .overflow_y_scroll()
                .p(tokens.spacing.md)
                .gap(tokens.spacing.md)
                .child(
                    h_flex()
                        .min_w_0()
                        .gap(tokens.spacing.md)
                        .child(group_folder_icon(
                            false,
                            counts.total,
                            px(HEADER_ICON_SIZE),
                            cx,
                        ))
                        .child(
                            v_flex()
                                .min_w_0()
                                .child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(tokens.typography.sm.size)
                                        .line_height(tokens.typography.sm.line_height)
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(tokens.colors.foreground)
                                        .child(SharedString::from(summary.meta.name.clone())),
                                )
                                .child(
                                    div()
                                        .text_size(tokens.typography.xs.size)
                                        .line_height(tokens.typography.xs.line_height)
                                        .text_color(extended.colors.text_tertiary)
                                        .child(
                                            strings
                                                .format_group_subtitle(&strings.state_label(state)),
                                        ),
                                ),
                        ),
                )
                .child(
                    v_flex()
                        .gap(tokens.spacing.xs)
                        .font_features(tabular_numbers())
                        .child(
                            h_flex()
                                .items_end()
                                .justify_between()
                                .gap(tokens.spacing.sm)
                                .child(
                                    div()
                                        .text_size(extended.title.size)
                                        .line_height(extended.title.line_height)
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(tokens.colors.foreground)
                                        .child(format!("{percent:.1}%")),
                                )
                                .child(
                                    div()
                                        .text_size(tokens.typography.xs.size)
                                        .line_height(tokens.typography.xs.line_height)
                                        .text_color(tokens.colors.muted_foreground)
                                        .child(bytes_line),
                                ),
                        )
                        .child(
                            Progress::new("group-overview-progress")
                                .w_full()
                                .color(if state == TaskState::Completed {
                                    extended.colors.status_completed
                                } else {
                                    extended.colors.progress_fill
                                })
                                .value(percent),
                        )
                        .child(
                            div()
                                .text_size(tokens.typography.xs.size)
                                .line_height(tokens.typography.xs.line_height)
                                .text_color(status_color(state, cx))
                                .child(progress_line),
                        )
                        .child(
                            group_counts_line(strings.group_count_segments(&counts, None), cx)
                                .text_size(tokens.typography.xs.size)
                                .line_height(tokens.typography.xs.line_height),
                        ),
                )
                .child(actions)
                .child(info)
                .into_any_element(),
        )
    }

    fn group_panel_button(
        &self,
        id: &'static str,
        icon: FluxIcon,
        label: SharedString,
        group_id: &str,
        action: fn(&mut DownloadView, String, &mut Context<DownloadView>),
        cx: &mut Context<Self>,
    ) -> Button {
        let group_id = group_id.to_owned();
        Button::new(id)
            .outline()
            .control(cx)
            .icon(icon)
            .label(label)
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                action(this, group_id.clone(), cx);
            }))
    }
}
