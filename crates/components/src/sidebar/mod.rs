mod motion;

use std::{ops::Range, time::Instant};

use fluxdown_ui_theme::active_theme;
use gpui::{
    AnyElement, App, AppContext as _, Context, ElementId, Entity, EventEmitter,
    InteractiveElement as _, IntoElement, MouseButton, ParentElement as _, Pixels, RenderOnce,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, percentage, px,
};
use gpui_component::{Icon, ResizableState, h_flex, h_resizable, resizable_panel};

use crate::{FluxIcon, toolbar_action_button, tooltip_view};

use motion::SidebarMotion;

/// 用户修改的侧栏布局；宿主可据此保存设备本地偏好。
#[derive(Clone, Copy, Debug)]
pub struct SidebarChange {
    /// 展开时的宽度，收起不会将它改成零。
    pub width: Pixels,
    /// 用户手动收起状态，与内容是否可见独立。
    pub collapsed: bool,
}

/// 侧栏收起 / 展开按钮：展开时放在侧栏标题行右端，收起后由 shell 放在顶栏最左。
///
/// `label` 同时作为提示与无障碍名称；点击等价于 [`SidebarState::toggle`]。
pub fn sidebar_toggle_button(
    id: &'static str,
    label: SharedString,
    sidebar: &Entity<SidebarState>,
    cx: &App,
) -> impl IntoElement + use<> {
    let state = sidebar.clone();
    div()
        .id(SharedString::from(format!("{id}-tooltip")))
        .flex_none()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .tooltip({
            let label = label.clone();
            move |window, cx| tooltip_view(label.clone(), window, cx)
        })
        .child(
            toolbar_action_button(
                id,
                label,
                Icon::new(FluxIcon::PanelRight)
                    .size(active_theme(cx).extended().icon.md)
                    .rotate(percentage(0.5)),
                false,
                false,
                cx,
            )
            .on_click(move |_, _, cx| state.update(cx, |state, cx| state.toggle(cx))),
        )
}

/// 页面独立的侧栏状态，供页面布局和 shell 顶栏按钮共享。
///
/// ```no_run
/// use fluxdown_ui_components::SidebarState;
/// use gpui::{Context, px};
/// fn create(cx: &mut Context<SidebarState>) -> SidebarState {
///     SidebarState::new(px(240.), px(176.)..px(400.), cx)
/// }
/// ```
pub struct SidebarState {
    width: Pixels,
    width_range: Range<Pixels>,
    collapsed: bool,
    available: bool,
    motion: Option<SidebarMotion>,
    split: Entity<ResizableState>,
    split_initialized: bool,
}

impl SidebarState {
    /// 创建默认展开的侧栏；宽度范围由页面决定。
    pub fn new(width: Pixels, width_range: Range<Pixels>, cx: &mut Context<Self>) -> Self {
        Self {
            width: width.clamp(width_range.start, width_range.end),
            width_range,
            collapsed: false,
            available: true,
            motion: None,
            split: cx.new(|_| ResizableState::default()),
            split_initialized: false,
        }
    }

    /// 当前记住的展开宽度。
    pub fn width(&self) -> Pixels {
        self.width
    }

    /// 用户是否手动收起侧栏。
    pub fn is_collapsed(&self) -> bool {
        self.collapsed
    }

    /// 是否有可供展示的侧栏内容；为假时 shell 不显示切换按钮。
    pub fn is_available(&self) -> bool {
        self.available
    }

    /// [`Self::set_available`] + [`Self::set_layout`] 投影这组值是否会改变状态。
    ///
    /// 宿主在渲染期投影偏好时先用它判定，未变化就不 `update`，避免其他读者被标记重建。
    pub fn needs_projection(&self, available: bool, width: Pixels, collapsed: bool) -> bool {
        let width = width.clamp(self.width_range.start, self.width_range.end);
        self.available != available || self.width != width || self.collapsed != collapsed
    }

    /// 投影已保存的布局，不发出用户修改事件，避免偏好回流形成写入循环。
    pub fn set_layout(&mut self, width: Pixels, collapsed: bool, cx: &mut Context<Self>) {
        let width = width.clamp(self.width_range.start, self.width_range.end);
        if self.width == width && self.collapsed == collapsed {
            return;
        }
        if self.width != width {
            self.split.update(cx, |split, _| split.clear());
            self.split_initialized = false;
        }
        self.width = width;
        self.collapsed = collapsed;
        cx.notify();
    }

    /// 内容全部隐藏时立即释放占位，但保留用户的展开宽度和手动收起状态。
    pub fn set_available(&mut self, available: bool, cx: &mut Context<Self>) {
        if self.available != available {
            self.available = available;
            cx.notify();
        }
    }

    /// 用户触发收起 / 展开；无可见内容时不改变布局偏好。
    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        if self.available {
            self.collapsed = !self.collapsed;
            self.emit_change(cx);
        }
    }

    fn resize(&mut self, width: Pixels, cx: &mut Context<Self>) {
        let width = width.clamp(self.width_range.start, self.width_range.end);
        if self.width != width {
            self.width = width;
            self.emit_change(cx);
        }
    }

    fn emit_change(&self, cx: &mut Context<Self>) {
        cx.emit(SidebarChange {
            width: self.width,
            collapsed: self.collapsed,
        });
        cx.notify();
    }
}

impl EventEmitter<SidebarChange> for SidebarState {}

/// 可拖拽调宽、可动画收起的侧栏与主内容组合。
///
/// 先创建面板，再根据 [`Self::is_sidebar_visible`] 按需构造侧栏；完全隐藏时不构造导航控件。
#[derive(IntoElement)]
pub struct SidebarPanel {
    id: ElementId,
    state: Entity<SidebarState>,
    split: Entity<ResizableState>,
    width: Pixels,
    width_range: Range<Pixels>,
    amount: f32,
    sidebar: Option<AnyElement>,
    content: AnyElement,
}

impl SidebarPanel {
    /// 采样本帧动画，并按需请求下一帧；侧栏内容以固定展开宽度布局。
    pub fn new(
        id: impl Into<ElementId>,
        state: &Entity<SidebarState>,
        content: impl IntoElement,
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        let now = Instant::now();
        // 状态无需变化时只读不写：gpui-fast 把渲染期的 entity 写入记为外部修改，
        // 每帧 `update` 会让 shell 顶栏等其他读者每帧重建。
        let sampled = Self::sample(state, now, window, cx);
        let (split, width, width_range, amount) = sampled.unwrap_or_else(|| {
            state.update(cx, |state, cx| {
                let open = state.available && !state.collapsed;
                let motion = state
                    .motion
                    .get_or_insert_with(|| SidebarMotion::settled(open));
                motion.retarget(open, now, state.available && !cx.reduce_motion());
                let amount = motion.amount(now);
                if motion.is_animating(now) {
                    window.request_animation_frame();
                }
                if amount == 1. && !state.split_initialized && Self::split_has_content(state, cx) {
                    state.split.update(cx, |split, cx| split.reset_panel(1, cx));
                    state.split_initialized = true;
                }
                (
                    state.split.clone(),
                    state.width,
                    state.width_range.clone(),
                    amount,
                )
            })
        });
        Self {
            id: id.into(),
            state: state.clone(),
            split,
            width,
            width_range,
            amount,
            sidebar: None,
            content: content.into_any_element(),
        }
    }

    /// 只读采样；需要改写动画 / 分栏状态时返回 `None`，由调用方走 `update`。
    fn sample(
        state: &Entity<SidebarState>,
        now: Instant,
        window: &mut Window,
        cx: &App,
    ) -> Option<(Entity<ResizableState>, Pixels, Range<Pixels>, f32)> {
        let state = state.read(cx);
        let open = state.available && !state.collapsed;
        let motion = state.motion.as_ref()?;
        if motion.needs_retarget(open, state.available && !cx.reduce_motion()) {
            return None;
        }
        let amount = motion.amount(now);
        if amount == 1. && !state.split_initialized && Self::split_has_content(state, cx) {
            return None;
        }
        if motion.is_animating(now) {
            window.request_animation_frame();
        }
        Some((
            state.split.clone(),
            state.width,
            state.width_range.clone(),
            amount,
        ))
    }

    fn split_has_content(state: &SidebarState, cx: &App) -> bool {
        state
            .split
            .read(cx)
            .sizes()
            .get(1)
            .is_some_and(|size| *size > px(1.))
    }

    /// 动画尚未完全收起时仍需要侧栏内容，以完成淡出。
    pub fn is_sidebar_visible(&self) -> bool {
        self.amount > 0.
    }

    /// 添加本帧侧栏内容；主内容始终保持挂载。
    pub fn sidebar(mut self, sidebar: impl IntoElement) -> Self {
        self.sidebar = Some(sidebar.into_any_element());
        self
    }
}

impl RenderOnce for SidebarPanel {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        if self.amount == 1. {
            let state = self.state;
            return h_resizable(self.id)
                .with_state(&self.split)
                .on_resize(move |split, _, cx| {
                    split.update(cx, |split, cx| split.reset_panel(1, cx));
                    if let Some(width) = split.read(cx).sizes().first().copied()
                        && width > px(0.)
                    {
                        state.update(cx, |state, cx| state.resize(width, cx));
                    }
                })
                .child(
                    resizable_panel()
                        .size(self.width)
                        .flex_none()
                        .size_range(self.width_range)
                        .children(self.sidebar),
                )
                .child(resizable_panel().child(self.content))
                .into_any_element();
        }
        let extended = active_theme(cx).extended();
        let mut row = h_flex().size_full().min_w_0().min_h_0();
        if self.amount > 0. {
            row = row.child(
                div()
                    .w(self.width * self.amount)
                    .h_full()
                    .flex_none()
                    .overflow_hidden()
                    .child(
                        div()
                            .w(self.width)
                            .h_full()
                            .opacity(self.amount)
                            .border_r(extended.stroke.thin)
                            .border_color(extended.colors.hairline)
                            .children(self.sidebar),
                    ),
            );
        }
        row.child(
            div()
                .flex_1()
                .h_full()
                .min_w_0()
                .min_h_0()
                .child(self.content),
        )
        .into_any_element()
    }
}
