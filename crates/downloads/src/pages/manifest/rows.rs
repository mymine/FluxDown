use fluxdown_ui_components::{
    CheckState, ControlExt as _, FluxIcon, card, check_mark, check_row, field_hint, tabular_numbers,
};
use fluxdown_ui_theme::active_theme;
use gpui::{
    Anchor, AnyElement, ClickEvent, Context, Div, InteractiveElement as _, IntoElement,
    ParentElement, SharedString, StatefulInteractiveElement as _, Styled, div,
};
use gpui_component::{
    Icon,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::{DropdownMenu as _, PopupMenuItem},
    v_flex,
};

use super::ManifestView;
use crate::model::{
    format_bytes,
    manifest::{ManifestRow, ManifestStat},
};

impl ManifestView {
    pub(super) fn render_rows(&self, cx: &mut Context<Self>) -> Div {
        let tokens = active_theme(cx).tokens().clone();
        let mut rows = v_flex().w_full().gap(tokens.spacing.xxs);
        if self.selection.rows().is_empty() {
            rows = rows.child(
                div()
                    .px(tokens.spacing.md)
                    .py(tokens.spacing.lg)
                    .child(field_hint(self.t("manifestTreeEmpty", cx), cx)),
            );
        } else {
            for row in self.selection.rows() {
                rows = rows.child(match row {
                    ManifestRow::Directory { node, label, stat } => {
                        self.directory_row(*node, label, *stat, cx)
                    }
                    ManifestRow::File { index, show_path } => self.file_row(*index, *show_path, cx),
                });
            }
        }
        card(cx).w_full().p(tokens.spacing.xs).child(rows)
    }
    fn directory_row(
        &self,
        node: usize,
        label: &str,
        stat: ManifestStat,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tokens = active_theme(cx).tokens().clone();
        let state = if stat.selected == 0 {
            CheckState::Unchecked
        } else if stat.selected == stat.count {
            CheckState::Checked
        } else {
            CheckState::Indeterminate
        };
        let size = if stat.unknown > 0 && stat.size == 0 {
            self.t("manifestDirSizeUnknown", cx)
        } else {
            SharedString::from(format_bytes(stat.size))
        };
        let count = self.text_with(
            "manifestItemsCount",
            &[("count", &stat.count.to_string())],
            cx,
        );
        let caption = SharedString::from(format!("{count} · {size}"));
        h_flex()
            .w_full()
            .gap(tokens.spacing.xs)
            .child(
                // 与文件行 `check_row` 同几何（px xs + control 行高），复选框不被按钮内边距挤压。
                div()
                    .id(("manifest-dir-check", node))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .min_h(active_theme(cx).density().control)
                    .px(tokens.spacing.xs)
                    .rounded(tokens.radius.md)
                    .cursor_pointer()
                    .hover({
                        let hover = active_theme(cx).extended().colors.row_hover;
                        move |style| style.bg(hover)
                    })
                    .tooltip({
                        let label = SharedString::from(label.to_owned());
                        move |window, cx| {
                            fluxdown_ui_components::tooltip_view(label.clone(), window, cx)
                        }
                    })
                    .child(check_mark(state, cx))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.selection.toggle_directory(node);
                        cx.notify();
                    })),
            )
            .child(
                Button::new(("manifest-dir-open", node))
                    .ghost()
                    .control(cx)
                    .flex_1()
                    .min_w_0()
                    .child(
                        h_flex()
                            .w_full()
                            .gap(tokens.spacing.sm)
                            .child(
                                Icon::new(FluxIcon::FolderOpen)
                                    .text_color(tokens.colors.muted_foreground),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .child(SharedString::from(label.to_owned())),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(tokens.typography.xs.size)
                                    .font_features(tabular_numbers())
                                    .text_color(tokens.colors.muted_foreground)
                                    .child(caption),
                            )
                            .child(
                                Icon::new(FluxIcon::ChevronRight)
                                    .text_color(tokens.colors.muted_foreground),
                            ),
                    )
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.selection.navigate(node);
                        cx.notify();
                    })),
            )
            .into_any_element()
    }
    fn file_row(&self, index: usize, show_path: bool, cx: &mut Context<Self>) -> AnyElement {
        let tokens = active_theme(cx).tokens().clone();
        let item = &self.selection.items()[index];
        let size = self.selection.size(index);
        let size = if size <= 0 {
            self.t("manifestFileSizeUnknown", cx)
        } else {
            SharedString::from(format_bytes(size as u64))
        };
        let name = if show_path && !item.path.is_empty() {
            format!("{}/{}", item.path, item.name)
        } else {
            item.name.clone()
        };
        let label = h_flex()
            .flex_1()
            .min_w_0()
            .gap(tokens.spacing.sm)
            .child(Icon::new(FluxIcon::File).text_color(tokens.colors.muted_foreground))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(SharedString::from(name)),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(tokens.typography.xs.size)
                    .font_features(tabular_numbers())
                    .text_color(tokens.colors.muted_foreground)
                    .child(size),
            );
        let this = cx.weak_entity();
        let row = check_row(
            ("manifest-file", index),
            self.selection.is_selected(index),
            label,
            move |_, _, cx| {
                let Ok(()) = this.update(cx, |this, cx| {
                    this.selection.toggle_file(index);
                    cx.notify();
                }) else {
                    // The containing form has been released.
                    return;
                };
            },
            cx,
        )
        .flex_1()
        .min_w_0();
        let mut container = h_flex()
            .w_full()
            .flex_wrap()
            .gap(tokens.spacing.xs)
            .child(row);
        if !item.variants.is_empty() {
            container = container.child(self.variant_menu(index, cx));
        }
        container.into_any_element()
    }
    fn variant_menu(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let item = &self.selection.items()[index];
        let current = self.selection.variant_id(index).map(str::to_owned);
        let default = self.t("manifestVariantDefault", cx);
        let label = current
            .as_ref()
            .and_then(|id| item.variants.iter().find(|variant| &variant.id == id))
            .map_or_else(
                || default.clone(),
                |variant| {
                    SharedString::from(if variant.label.is_empty() {
                        variant.id.clone()
                    } else {
                        variant.label.clone()
                    })
                },
            );
        let mut options = Vec::with_capacity(item.variants.len() + 1);
        options.push((None, default));
        options.extend(item.variants.iter().map(|variant| {
            let size = if variant.size <= 0 {
                self.t("manifestFileSizeUnknown", cx)
            } else {
                SharedString::from(format_bytes(variant.size as u64))
            };
            let label = if variant.label.is_empty() {
                &variant.id
            } else {
                &variant.label
            };
            (
                Some(variant.id.clone()),
                SharedString::from(format!("{label} · {size}")),
            )
        }));
        let this = cx.weak_entity();
        Button::new(("manifest-variant", index))
            .outline()
            .control(cx)
            .max_w(active_theme(cx).text_extent(200.))
            .label(label)
            .tooltip(self.t("manifestVariantLabel", cx))
            .dropdown_caret(true)
            .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, _| {
                options.iter().fold(menu, |menu, (variant, label)| {
                    let this = this.clone();
                    let variant = variant.clone();
                    let checked = variant == current;
                    menu.item(PopupMenuItem::new(label.clone()).checked(checked).on_click(
                        move |_, _, cx| {
                            let Ok(()) = this.update(cx, |this, cx| {
                                if !this.selection.set_variant(index, variant.as_deref()) {
                                    return;
                                }
                                cx.notify();
                            }) else {
                                // The containing form has been released.
                                return;
                            };
                        },
                    ))
                })
            })
            .into_any_element()
    }
}
