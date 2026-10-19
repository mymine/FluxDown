//! 多插件选择：同一链接被 2+ 个已启用插件的 resolver 匹配时，让用户为本次任务选一个，
//! 可选「记为该站点默认插件」。候选卡片单选（整卡可点、可聚焦），默认项带徽标并预选。

use fluxdown_protocol::{PluginCandidateDto, SelectionOutcome};
use fluxdown_ui_components::{FluxIcon, check_row, field_hint};
use fluxdown_ui_theme::active_theme;
use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, HighlightStyle, InteractiveElement as _,
    IntoElement, ParentElement, SharedString, StatefulInteractiveElement as _, Styled, StyledText,
    div, prelude::FluentBuilder as _,
};
use gpui_component::{Icon, h_flex, scroll::ScrollableElement as _, v_flex};

use super::{SelectionState, SelectionView};

/// 默认预选：`defaultChoice` 指向的候选；不在候选里（或不是插件答复）时取第一个。
pub(super) fn initial_selection(
    candidates: &[PluginCandidateDto],
    default_choice: &SelectionOutcome,
) -> String {
    if let SelectionOutcome::Plugin { identity, .. } = default_choice
        && candidates
            .iter()
            .any(|candidate| &candidate.identity == identity)
    {
        return identity.clone();
    }
    candidates
        .first()
        .map(|candidate| candidate.identity.clone())
        .unwrap_or_default()
}

/// 用户确认时的答复；未选中任何候选时不可确认。
pub(super) fn confirm_outcome(selected: &str, remember: bool) -> Option<SelectionOutcome> {
    (!selected.is_empty()).then(|| SelectionOutcome::Plugin {
        identity: selected.to_owned(),
        remember,
    })
}

/// 把 `{count}` 替换后的描述按 `{site}` 切开，返回完整文本与站点名所在字节区间（用于加粗）。
fn description_with_site(
    template: &str,
    count: usize,
    site: &str,
) -> (String, Option<std::ops::Range<usize>>) {
    let template = template.replace("{count}", &count.to_string());
    match template.find("{site}") {
        Some(start) => {
            let text = template.replacen("{site}", site, 1);
            (text, Some(start..start + site.len()))
        }
        None => (template, None),
    }
}

impl SelectionView {
    pub(super) fn plugin_description(&self, cx: &Context<Self>) -> AnyElement {
        let SelectionState::Plugin {
            site, candidates, ..
        } = &self.state
        else {
            return div().into_any_element();
        };
        let theme = active_theme(cx);
        let tokens = theme.tokens();
        let (text, site_range) =
            description_with_site(&self.strings.plugin_desc, candidates.len(), site);
        let highlights = site_range
            .map(|range| {
                (
                    range,
                    HighlightStyle {
                        font_weight: Some(FontWeight::SEMIBOLD),
                        color: Some(tokens.colors.foreground),
                        ..HighlightStyle::default()
                    },
                )
            })
            .into_iter()
            .collect::<Vec<_>>();
        StyledText::new(SharedString::from(text))
            .with_highlights(highlights)
            .into_any_element()
    }

    pub(super) fn render_plugin(&self, cx: &mut Context<Self>) -> AnyElement {
        let SelectionState::Plugin {
            site,
            url,
            candidates,
            default_identity,
            selected,
            remember,
        } = &self.state
        else {
            return div().into_any_element();
        };
        let theme = active_theme(cx);
        let tokens = theme.tokens().clone();
        let extended = theme.extended().clone();
        let caption = extended.caption;
        let radio_size = theme.density().check_mark;
        let badge_radius = theme.components().badge_radius;

        let cards = candidates.iter().map(|candidate| {
            let checked = &candidate.identity == selected;
            let is_default = &candidate.identity == default_identity;
            let identity = candidate.identity.clone();
            let hover = extended.colors.row_hover;
            let indicator = div()
                .size(radio_size)
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .border(extended.stroke.thin)
                .map(|dot| {
                    if checked {
                        dot.border_color(tokens.colors.primary)
                            .bg(tokens.colors.primary)
                            .child(
                                Icon::new(FluxIcon::CheckboxCheck)
                                    .size(gpui::px(11.))
                                    .text_color(tokens.colors.primary_foreground),
                            )
                    } else {
                        dot.border_color(tokens.colors.muted_foreground.opacity(0.55))
                            .bg(tokens.colors.surface)
                    }
                });
            let title_row = h_flex()
                .items_center()
                .gap(tokens.spacing.sm)
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(tokens.typography.sm.size)
                        .line_height(tokens.typography.sm.line_height)
                        .font_weight(FontWeight::MEDIUM)
                        .child(SharedString::from(candidate.name.clone())),
                )
                .when(is_default, |row| {
                    row.child(
                        div()
                            .flex_none()
                            .px(tokens.spacing.sm)
                            .py(tokens.spacing.xxs)
                            .rounded(badge_radius)
                            .bg(tokens.colors.primary.opacity(0.12))
                            .text_size(caption.size)
                            .line_height(caption.line_height)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(tokens.colors.primary)
                            .child(self.strings.plugin_default_badge.clone()),
                    )
                });
            let meta = div()
                .truncate()
                .font_family(tokens.typography.mono.clone())
                .text_size(tokens.typography.xs.size)
                .line_height(tokens.typography.xs.line_height)
                .text_color(tokens.colors.muted_foreground)
                .child(SharedString::from(format!(
                    "v{} · {}",
                    candidate.version, candidate.identity
                )));
            div()
                .id(SharedString::from(format!("plugin-choice-{identity}")))
                .w_full()
                .cursor_pointer()
                .flex()
                .items_start()
                .gap(tokens.spacing.sm)
                .px(tokens.spacing.md)
                .py(tokens.spacing.sm)
                .rounded(tokens.radius.md)
                .border(extended.stroke.thin)
                .map(|card| {
                    if checked {
                        card.border_color(tokens.colors.primary)
                            .bg(tokens.colors.accent)
                    } else {
                        card.border_color(extended.colors.hairline)
                            .hover(move |style| style.bg(hover))
                    }
                })
                // gpui 把获得焦点元素上的 Enter / Space 转为点击，这里只需可聚焦并给焦点环。
                .focusable()
                .tab_stop(true)
                .focus_visible(|style| style.border_color(tokens.colors.ring))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if let SelectionState::Plugin { selected, .. } = &mut this.state {
                        selected.clone_from(&identity);
                    }
                    cx.notify();
                }))
                .child(div().pt(tokens.spacing.xxs).child(indicator))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap(tokens.spacing.xxs)
                        .child(title_row)
                        .child(meta)
                        .when(!candidate.description.is_empty(), |column| {
                            column.child(
                                div()
                                    .line_clamp(2)
                                    .text_size(tokens.typography.xs.size)
                                    .line_height(tokens.typography.xs.line_height)
                                    .text_color(tokens.colors.muted_foreground)
                                    .child(SharedString::from(candidate.description.clone())),
                            )
                        }),
                )
        });

        let this = cx.weak_entity();
        let remember_label =
            SharedString::from(self.strings.plugin_remember.replace("{site}", site));
        let remember_row = v_flex()
            .gap(tokens.spacing.xxs)
            .px(tokens.spacing.md)
            .pb(tokens.spacing.md)
            .child(check_row(
                "plugin-choice-remember",
                *remember,
                remember_label,
                move |check, _, cx| {
                    let Ok(()) = this.update(cx, |this, cx| {
                        if let SelectionState::Plugin { remember, .. } = &mut this.state {
                            *remember = check;
                        }
                        cx.notify();
                    }) else {
                        // 视图已释放，结束回调而不再更新状态。
                        return;
                    };
                },
                cx,
            ))
            .child(
                div()
                    .pl(tokens.spacing.xs + radio_size + tokens.spacing.sm)
                    .child(field_hint(self.strings.plugin_remember_hint.clone(), cx)),
            );

        v_flex()
            .size_full()
            .child(
                div()
                    .px(tokens.spacing.md)
                    .pb(tokens.spacing.sm)
                    .truncate()
                    .text_size(tokens.typography.xs.size)
                    .line_height(tokens.typography.xs.line_height)
                    .text_color(extended.colors.text_tertiary)
                    .child(SharedString::from(url.clone())),
            )
            .child(
                div().flex_1().min_h_0().child(
                    v_flex()
                        .id("selection-plugin-scroll")
                        .size_full()
                        .gap(tokens.spacing.sm)
                        .px(tokens.spacing.md)
                        .pb(tokens.spacing.md)
                        .children(cards)
                        .overflow_y_scrollbar(),
                ),
            )
            .child(remember_row)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(identity: &str) -> PluginCandidateDto {
        PluginCandidateDto {
            identity: identity.to_owned(),
            name: identity.to_owned(),
            version: "1.0.0".to_owned(),
            description: String::new(),
        }
    }

    fn plugin_default(identity: &str) -> SelectionOutcome {
        SelectionOutcome::Plugin {
            identity: identity.to_owned(),
            remember: false,
        }
    }

    #[test]
    fn initial_selection_follows_default_choice() {
        let candidates = [candidate("a"), candidate("b")];
        assert_eq!(initial_selection(&candidates, &plugin_default("b")), "b");
    }

    #[test]
    fn initial_selection_falls_back_to_first_candidate() {
        let candidates = [candidate("a"), candidate("b")];
        assert_eq!(initial_selection(&candidates, &plugin_default("zzz")), "a");
        assert_eq!(
            initial_selection(&candidates, &SelectionOutcome::Cancelled),
            "a"
        );
        assert_eq!(initial_selection(&[], &SelectionOutcome::Cancelled), "");
    }

    #[test]
    fn confirm_outcome_carries_remember_and_rejects_empty() {
        assert_eq!(
            confirm_outcome("a", true),
            Some(SelectionOutcome::Plugin {
                identity: "a".to_owned(),
                remember: true
            })
        );
        assert_eq!(confirm_outcome("", false), None);
    }

    #[test]
    fn description_marks_site_range() {
        let (text, range) =
            description_with_site("{count} plugins handle {site} links", 3, "bilibili.com");
        assert_eq!(text, "3 plugins handle bilibili.com links");
        assert_eq!(&text[range.expect("site range")], "bilibili.com");
    }
}
