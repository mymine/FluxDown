//! 下拉 / 开关 / 端口输入控件与选项值映射。

use fluxdown_protocol::{ShareConnectionMode, ShareExitBehavior, ShareLogLevel};
use fluxdown_ui_components::ControlExt as _;
use fluxdown_ui_theme::active_theme;
use gpui::{
    Anchor, AnyElement, App, Context, Entity, IntoElement as _, SharedString, Styled as _, Window,
    prelude::FluentBuilder as _,
};
use gpui_component::{
    Disableable as _,
    button::Button,
    input::{Input, InputState},
    menu::{DropdownMenu as _, PopupMenuItem},
    switch::Switch,
};

use super::{DROPDOWN_MAX_WIDTH, DROPDOWN_MIN_WIDTH, NUMBER_WIDTH, ShareSettingsView};

pub(super) const fn portmapper_value(value: Option<bool>) -> &'static str {
    match value {
        None => "auto",
        Some(true) => "on",
        Some(false) => "off",
    }
}

pub(super) const fn mode_value(mode: ShareConnectionMode) -> &'static str {
    match mode {
        ShareConnectionMode::Auto => "auto",
        ShareConnectionMode::RelayOnly => "relayOnly",
        ShareConnectionMode::DirectOnly => "directOnly",
    }
}

pub(super) const fn exit_value(behavior: ShareExitBehavior) -> &'static str {
    match behavior {
        ShareExitBehavior::Prompt => "prompt",
        ShareExitBehavior::ConvertToCloud => "convertToCloud",
        ShareExitBehavior::Quit => "quit",
    }
}

pub(super) const fn log_level_value(level: ShareLogLevel) -> &'static str {
    match level {
        ShareLogLevel::Error => "error",
        ShareLogLevel::Warn => "warn",
        ShareLogLevel::Info => "info",
        ShareLogLevel::Debug => "debug",
        ShareLogLevel::Trace => "trace",
    }
}

pub(super) fn option(
    value: impl Into<SharedString>,
    label: impl Into<SharedString>,
) -> (SharedString, SharedString) {
    (value.into(), label.into())
}

pub(super) type Select =
    fn(&mut ShareSettingsView, &str, &mut Window, &mut Context<ShareSettingsView>);

impl ShareSettingsView {
    /// 下拉按钮：触发器显示当前值的标签，菜单项带勾选；选中经弱引用回到本视图。
    pub(super) fn dropdown(
        &self,
        id: &'static str,
        options: Vec<(SharedString, SharedString)>,
        current: SharedString,
        full_width: bool,
        select: Select,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let disabled = !self.editable(cx);
        let view = cx.entity().downgrade();
        let label = options
            .iter()
            .find(|(value, _)| *value == current)
            .map_or_else(|| current.clone(), |(_, label)| label.clone());
        let theme = active_theme(cx);
        Button::new(id)
            .outline()
            .control(cx)
            .label(label)
            .dropdown_caret(true)
            .disabled(disabled)
            .map(|this| {
                if full_width {
                    this.w_full()
                } else {
                    this.min_w(theme.text_extent(DROPDOWN_MIN_WIDTH))
                        .max_w(theme.text_extent(DROPDOWN_MAX_WIDTH))
                }
            })
            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                options.iter().fold(menu, |menu, (value, label)| {
                    let checked = *value == current;
                    let value = value.clone();
                    let view = view.clone();
                    menu.item(PopupMenuItem::new(label.clone()).checked(checked).on_click(
                        move |_, window, cx| {
                            let Ok(()) =
                                view.update(cx, |this, cx| select(this, &value, window, cx))
                            else {
                                // 设置页已关闭。
                                return;
                            };
                        },
                    ))
                })
            })
            .into_any_element()
    }

    pub(super) fn switch(
        &self,
        id: &'static str,
        checked: bool,
        change: fn(&mut Self, bool, &mut Window, &mut Context<Self>),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let disabled = !self.editable(cx);
        self.switch_with(id, checked, disabled, change, cx)
    }

    pub(super) fn switch_with(
        &self,
        id: &'static str,
        checked: bool,
        disabled: bool,
        change: fn(&mut Self, bool, &mut Window, &mut Context<Self>),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        Switch::new(id)
            .checked(checked)
            .disabled(disabled)
            .on_click(cx.listener(move |this, checked: &bool, window, cx| {
                change(this, *checked, window, cx);
            }))
            .into_any_element()
    }

    pub(super) fn port_input(&self, input: &Entity<InputState>, cx: &App) -> AnyElement {
        Input::new(input)
            .control(cx)
            .disabled(!self.editable(cx))
            .w(active_theme(cx).text_extent(NUMBER_WIDTH))
            .into_any_element()
    }
}
