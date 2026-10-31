//! 页内定高列表（网络诊断的指标表 / 日志 / 原始快照）与外层页面之间的滚轮链式传递。
//!
//! `gpui_base::ScrollBounce` 按平台惯例让指针下最内层滚动区独占整个手势、到边也不交还页面，于是列表滚到
//! 顶后指针停在列表上就再也滚不动页面。这里改为浏览器式「手势锁定 + 链式传递」：一次手势开始时（触控板
//! `Started`，或距上一帧滚轮超过 [`GESTURE_GAP`]），列表还能朝该方向滚就整次手势归列表（滚到边也不会
//! 半途带动页面）；已在该方向的边上（或内容不溢出）就整次手势交给页面。停顿后再滚即重新判定。
//!
//! 只在捕获阶段工作：整页 `overflow_y_scrollbar` 在子树之前的捕获监听里认领、在子树之后的捕获监听里
//! 滚动并截停；本区的监听注册在两者之间，归列表时自己滚动并截停，页面那一步就不会执行；归页面时不动作。

use std::{
    cell::Cell,
    rc::Rc,
    time::{Duration, Instant},
};

use gpui::{
    DispatchPhase, ElementId, HitboxBehavior, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, ScrollEvent, Styled as _, TouchPhase, UniformList,
    UniformListScrollHandle, canvas, div, px,
};
use gpui_base::ScrollbarHandle as _;
use gpui_component::scroll::ScrollableElement as _;

/// 两帧滚轮间隔超过它即视为新手势（无相位的鼠标滚轮 / 平滑滚动驱动靠它分段）。
const GESTURE_GAP: Duration = Duration::from_millis(250);
/// 判定「已在边上」的容差（px），吸收浮点舍入。
const EDGE_EPSILON: f32 = 0.5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Owner {
    Inner,
    Outer,
}

/// 当前手势归属。
#[derive(Clone, Copy, Debug, Default)]
struct Latch {
    owner: Option<Owner>,
    last: Option<Instant>,
}

impl Latch {
    /// 这一帧是否由列表处理。`offset_y ∈ [-max, 0]`，`delta_y > 0` 朝内容顶部。
    /// 零位移帧（触控板 `Ended` 等）不决定归属，沿用已锁定的。
    fn route(
        &mut self,
        now: Instant,
        started: bool,
        delta_y: f32,
        offset_y: f32,
        max: f32,
    ) -> bool {
        let fresh = started
            || self
                .last
                .is_none_or(|at| now.saturating_duration_since(at) >= GESTURE_GAP);
        self.last = Some(now);
        if fresh {
            self.owner = None;
        }
        if self.owner.is_none() && delta_y != 0. {
            self.owner = Some(if can_scroll(offset_y, max, delta_y) {
                Owner::Inner
            } else {
                Owner::Outer
            });
        }
        self.owner == Some(Owner::Inner)
    }
}

/// 列表能否朝 `delta_y` 方向继续滚动。
fn can_scroll(offset_y: f32, max: f32, delta_y: f32) -> bool {
    if max <= EDGE_EPSILON {
        return false;
    }
    if delta_y > 0. {
        offset_y < -EDGE_EPSILON
    } else if delta_y < 0. {
        offset_y > -max + EDGE_EPSILON
    } else {
        false
    }
}

/// 一个页内定高列表的滚动位置与手势归属。
#[derive(Clone, Default)]
pub(crate) struct NestedScroll {
    handle: UniformListScrollHandle,
    latch: Rc<Cell<Latch>>,
}

impl NestedScroll {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// 定高 `height` 的滚动区：列表 + 竖向滚动条 + 链式滚轮处理。
    pub(crate) fn area(
        &self,
        id: impl Into<ElementId>,
        list: UniformList,
        height: Pixels,
    ) -> impl IntoElement {
        let handle = self.handle.clone();
        let latch = self.latch.clone();
        div()
            .id(id)
            .relative()
            .w_full()
            .h(height)
            .child(
                canvas(
                    |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
                    move |_, hitbox, window, _| {
                        let view = window.current_view();
                        window.on_mouse_event(move |event: &ScrollEvent, phase, window, cx| {
                            if phase != DispatchPhase::Capture
                                || !hitbox.should_handle_scroll(window)
                            {
                                return;
                            }
                            let delta = event.delta.pixel_delta(window.line_height());
                            if delta.x.abs() > delta.y.abs() {
                                return;
                            }
                            let max = (handle.content_size().height
                                - handle.viewport_bounds().size.height)
                                .max(px(0.));
                            let mut offset = handle.offset();
                            let mut state = latch.get();
                            let mine = state.route(
                                cx.background_executor().now(),
                                event.touch_phase == TouchPhase::Started,
                                delta.y.as_f32(),
                                offset.y.as_f32(),
                                max.as_f32(),
                            );
                            latch.set(state);
                            if !mine {
                                return;
                            }
                            offset.y = (offset.y + delta.y).clamp(-max, px(0.));
                            handle.set_offset(offset);
                            cx.stop_propagation();
                            cx.notify(view);
                        });
                    },
                )
                .absolute()
                .size_full(),
            )
            .child(list.track_scroll(&self.handle).size_full())
            .vertical_scrollbar(&self.handle)
    }
}

#[cfg(test)]
mod tests {
    use super::{GESTURE_GAP, Latch};
    use std::time::{Duration, Instant};

    const MAX: f32 = 200.;

    #[test]
    fn gesture_starting_at_top_goes_to_page_for_the_whole_gesture() {
        let t0 = Instant::now();
        let mut latch = Latch::default();
        // 已在顶部继续上滚 → 页面。
        assert!(!latch.route(t0, true, 10., 0., MAX));
        // 同一手势里改向下滚也仍归页面，不会半途切回列表。
        assert!(!latch.route(t0 + Duration::from_millis(16), false, -10., 0., MAX));
    }

    #[test]
    fn gesture_starting_inside_stays_with_list_past_the_edge() {
        let t0 = Instant::now();
        let mut latch = Latch::default();
        assert!(latch.route(t0, true, 10., -50., MAX));
        // 列表已滚到顶，同一手势（含惯性）的后续帧仍归列表，页面不跟着动。
        assert!(latch.route(t0 + Duration::from_millis(16), false, 10., 0., MAX));
    }

    #[test]
    fn a_pause_starts_a_new_gesture_that_hands_off_at_the_edge() {
        let t0 = Instant::now();
        let mut latch = Latch::default();
        assert!(latch.route(t0, false, 10., -5., MAX));
        let later = t0 + GESTURE_GAP;
        assert!(!latch.route(later, false, 10., 0., MAX));
    }

    #[test]
    fn bottom_edge_and_non_overflowing_lists_hand_off() {
        let t0 = Instant::now();
        assert!(!Latch::default().route(t0, true, -10., -MAX, MAX));
        assert!(Latch::default().route(t0, true, 10., -MAX, MAX));
        assert!(!Latch::default().route(t0, true, 10., 0., 0.));
    }

    #[test]
    fn zero_delta_frames_keep_the_current_owner() {
        let t0 = Instant::now();
        let mut latch = Latch::default();
        assert!(!latch.route(t0, true, 0., -50., MAX));
        assert!(latch.route(t0 + Duration::from_millis(16), false, 10., -50., MAX));
        assert!(latch.route(t0 + Duration::from_millis(32), false, 0., -40., MAX));
    }
}
