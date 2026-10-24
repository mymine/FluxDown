//! 插件市场收藏：收藏状态、排序 / 「我的收藏」工具栏、行内星标按钮。

use std::collections::{BTreeMap, HashSet};

use fluxdown_protocol::{
    ApplicationErrorCode, PluginFavoriteResultDto, PluginFavoritesDto, RpcErrorData,
};
use fluxdown_ui_components::{BusyExt as _, ControlExt as _, FluxIcon, IconControlExt as _};
use gpui::{
    Anchor, Context, Entity, IntoElement, ParentElement, SharedString, Styled, Window,
    prelude::FluentBuilder as _, px,
};
use gpui_component::{
    Disableable as _, Icon,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputState},
    menu::{DropdownMenu as _, PopupMenuItem},
    switch::Switch,
};

use super::{Frame, plugins::MARKET_PAGE_SIZE};
use crate::{ExtensionsView, error_text, market::MarketSort, ui};

/// 收藏数据与在途状态（仅内存）。
#[derive(Default)]
pub(crate) struct FavoritesUi {
    /// 每个插件的收藏总数（只含 > 0）。
    pub counts: BTreeMap<String, u64>,
    /// 当前账号收藏的插件 id。
    pub mine: HashSet<String>,
    /// 最近一次列表结果里的登录态；决定星标 / 「我的收藏」是否可用。
    pub signed_in: bool,
    /// agent 快照里的登录态；变化时触发重新加载。
    pub session: bool,
    pub loading: bool,
    pub load_failed: bool,
    /// 登录态变化后需要重新加载。
    pub reload: bool,
    /// 收藏 / 取消收藏在途的插件 id。
    pub pending: HashSet<String>,
}

impl FavoritesUi {
    /// 记录 agent 的登录态；变化时标记重新加载（下次渲染触发），登出立即清掉本人收藏，
    /// 不等网络往返。返回是否变化（调用方据此重绘）。
    pub fn observe_session(&mut self, signed_in: bool) -> bool {
        if self.session == signed_in {
            return false;
        }
        self.session = signed_in;
        self.reload = true;
        if !signed_in {
            self.signed_in = false;
            self.mine.clear();
        }
        true
    }

    fn usable(&self) -> bool {
        self.signed_in && !self.load_failed
    }
}

fn incompatible() -> RpcErrorData {
    RpcErrorData::new(ApplicationErrorCode::ProtocolIncompatible, false)
}

impl ExtensionsView {
    /// 登录态变化后（市场已请求过）重新加载收藏。
    pub(crate) fn ensure_favorites_loaded(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let favorites = &self.plugins.market.favorites;
        if self.controller.is_stale()
            || favorites.loading
            || !favorites.reload
            || !self.plugins.market.requested
        {
            return;
        }
        self.load_favorites(window, cx);
    }

    pub(crate) fn load_favorites(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let favorites = &mut self.plugins.market.favorites;
        favorites.loading = true;
        favorites.reload = false;
        let future = self.controller.plugin_favorites();
        cx.spawn_in(window, async move |this, cx| {
            let result = future.await.and_then(|value| {
                serde_json::from_value::<PluginFavoritesDto>(value).map_err(|_| incompatible())
            });
            let Ok(()) = this.update(cx, |this, cx| {
                let favorites = &mut this.plugins.market.favorites;
                favorites.loading = false;
                match result {
                    Ok(dto) => {
                        favorites.counts = dto.counts;
                        favorites.mine = dto.mine.into_iter().collect();
                        favorites.signed_in = dto.signed_in;
                        favorites.load_failed = false;
                    }
                    Err(_) => {
                        // 收藏失败不影响市场列表：清空数据，星标禁用。
                        favorites.counts.clear();
                        favorites.mine.clear();
                        favorites.signed_in = false;
                        favorites.load_failed = true;
                    }
                }
                cx.notify();
            }) else {
                // 扩展视图已释放，结束回调，不再更新状态。
                return;
            };
        })
        .detach();
    }

    fn toggle_favorite(&mut self, plugin_id: String, window: &mut Window, cx: &mut Context<Self>) {
        let favorites = &mut self.plugins.market.favorites;
        if !favorites.pending.insert(plugin_id.clone()) {
            return;
        }
        let target = !favorites.mine.contains(&plugin_id);
        let future = self
            .controller
            .set_plugin_favorite(plugin_id.clone(), target);
        cx.spawn_in(window, async move |this, cx| {
            let result = future.await.and_then(|value| {
                serde_json::from_value::<PluginFavoriteResultDto>(value).map_err(|_| incompatible())
            });
            let Ok(()) = this.update_in(cx, |this, window, cx| {
                this.finish_favorite(&plugin_id, result, window, cx);
            }) else {
                // 扩展视图已释放，结束回调，不再更新状态。
                return;
            };
        })
        .detach();
        cx.notify();
    }

    fn finish_favorite(
        &mut self,
        plugin_id: &str,
        result: Result<PluginFavoriteResultDto, RpcErrorData>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let favorites = &mut self.plugins.market.favorites;
        favorites.pending.remove(plugin_id);
        match result {
            Ok(dto) => {
                if dto.favorited {
                    favorites.mine.insert(dto.plugin_id.clone());
                } else {
                    favorites.mine.remove(&dto.plugin_id);
                }
                if dto.count > 0 {
                    favorites.counts.insert(dto.plugin_id, dto.count);
                } else {
                    favorites.counts.remove(&dto.plugin_id);
                }
            }
            Err(error) => {
                let translator = self.translator.read(cx);
                let message = format!(
                    "{}: {}",
                    translator.text("marketFavoriteFailed"),
                    error_text(translator, &error)
                );
                self.toast_error(message, window, cx);
            }
        }
        cx.notify();
    }

    /// 搜索框 + 排序下拉 + 「我的收藏」开关。
    pub(crate) fn render_market_toolbar(
        &self,
        search: &Entity<InputState>,
        frame: Frame<'_>,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let Frame {
            translator, tokens, ..
        } = frame;
        let market = &self.plugins.market;
        let sort = market.sort;
        let view = cx.entity();
        let labels = MarketSort::ALL.map(|option| {
            (
                option,
                SharedString::from(translator.text(option.label_key()).to_owned()),
            )
        });
        let picker = Button::new("market-sort")
            .outline()
            .label(format!(
                "{}: {}",
                translator.text("marketSortLabel"),
                translator.text(sort.label_key())
            ))
            .dropdown_caret(true)
            .control(cx)
            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                labels.iter().fold(menu, |menu, (option, label)| {
                    let view = view.clone();
                    let option = *option;
                    menu.item(
                        PopupMenuItem::new(label.clone())
                            .checked(option == sort)
                            .on_click(move |_, _, cx| {
                                view.update(cx, |this, cx| {
                                    this.plugins.market.sort = option;
                                    this.plugins.market.limit = MARKET_PAGE_SIZE;
                                    cx.notify();
                                });
                            }),
                    )
                })
            });
        let can_filter = market.favorites.usable();
        h_flex()
            .w_full()
            .gap(tokens.spacing.sm)
            .items_center()
            .child(
                Input::new(search)
                    .control(cx)
                    .flex_1()
                    .prefix(
                        Icon::new(FluxIcon::Search)
                            .size(frame.extended.icon.md)
                            .text_color(tokens.colors.muted_foreground),
                    )
                    .cleanable(true),
            )
            .child(picker)
            .child(ui::meta_text(
                translator.text("marketFavoritesOnly").to_owned(),
                frame,
            ))
            .child(
                Switch::new("market-favorites-only")
                    .checked(market.favorites_only && can_filter)
                    .disabled(!can_filter)
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.plugins.market.favorites_only = *checked;
                        this.plugins.market.limit = MARKET_PAGE_SIZE;
                        cx.notify();
                    })),
            )
    }

    /// 行内星标按钮 + 收藏数（> 0 才显示）。
    pub(crate) fn render_favorite_button(
        &self,
        index: usize,
        plugin_id: &str,
        frame: Frame<'_>,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let Frame {
            translator,
            tokens,
            stale,
            ..
        } = frame;
        let favorites = &self.plugins.market.favorites;
        let favorited = favorites.mine.contains(plugin_id);
        let pending = favorites.pending.contains(plugin_id);
        let count = favorites.counts.get(plugin_id).copied().unwrap_or(0);
        let tooltip = if favorites.load_failed {
            "marketFavoritesLoadFailed"
        } else if !favorites.signed_in {
            "marketFavoriteLoginRequired"
        } else if favorited {
            "marketFavoriteRemove"
        } else {
            "marketFavoriteAdd"
        };
        let icon_color = if favorited {
            tokens.colors.primary
        } else {
            tokens.colors.muted_foreground
        };
        let target = plugin_id.to_owned();
        h_flex()
            .gap(tokens.spacing.xxs)
            .items_center()
            .when(count > 0, |this| {
                this.child(ui::meta_text(count.to_string(), frame).min_w(px(16.)))
            })
            .child(
                Button::new(("market-favorite", index))
                    .ghost()
                    .control_icon(cx)
                    .icon(Icon::new(FluxIcon::Star).text_color(icon_color))
                    .tooltip(translator.text(tooltip).to_owned())
                    .busy(pending)
                    .disabled(stale || pending || !favorites.usable())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.toggle_favorite(target.clone(), window, cx);
                    })),
            )
    }

    /// 收藏加载失败时的淡色提示行。
    pub(crate) fn favorites_load_failed_hint(&self, frame: Frame<'_>) -> Option<impl IntoElement> {
        self.plugins.market.favorites.load_failed.then(|| {
            ui::meta_text(
                frame
                    .translator
                    .text("marketFavoritesLoadFailed")
                    .to_owned(),
                frame,
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::FavoritesUi;

    #[test]
    fn session_change_marks_reload_and_sign_out_drops_mine() {
        let mut favorites = FavoritesUi::default();
        assert!(!favorites.observe_session(false));
        assert!(!favorites.reload);

        assert!(favorites.observe_session(true));
        assert!(favorites.reload);
        favorites.reload = false;
        favorites.signed_in = true;
        favorites.mine.insert("a@a".to_owned());
        favorites.counts.insert("a@a".to_owned(), 3);

        // 资料刷新等仍为已登录的会话事件不触发重载。
        assert!(!favorites.observe_session(true));
        assert!(!favorites.reload);

        assert!(favorites.observe_session(false));
        assert!(favorites.reload);
        assert!(!favorites.signed_in);
        assert!(favorites.mine.is_empty());
        // 公开计数与登录无关，保留到重新加载。
        assert_eq!(favorites.counts.get("a@a"), Some(&3));
    }
}
