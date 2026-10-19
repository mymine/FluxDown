//! 双击修饰键判定（平台无关状态机）。各平台后端把底层键盘事件归一成 [`KeyInput`] 喂进来。
//!
//! 一次「单击」= 只按下一个修饰键（左右任一侧）、期间没有其它按键、按住不超过
//! [`MAX_HOLD`] 后松开；同一修饰键两次单击的松开间隔不超过
//! [`DOUBLE_TAP_INTERVAL_MS`] 即触发。

use std::time::{Duration, Instant};

use fluxdown_protocol::shortcuts::{DOUBLE_TAP_INTERVAL_MS, ModifierKey};

/// 单击允许的最长按住时间：更久视为按住修饰键在做别的事。
pub(crate) const MAX_HOLD: Duration = Duration::from_millis(DOUBLE_TAP_INTERVAL_MS);
const INTERVAL: Duration = Duration::from_millis(DOUBLE_TAP_INTERVAL_MS);

/// 归一化后的键盘输入。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum KeyInput {
    ModifierDown(ModifierKey),
    ModifierUp(ModifierKey),
    /// 任何非修饰键按下。
    OtherDown,
}

#[derive(Debug, Default)]
pub(crate) struct DoubleTapDetector {
    /// 当前按住的修饰键（按 `ModifierKey::ALL` 下标）。
    held: [bool; 4],
    /// 本轮按键（从第一个修饰键按下到全部松开）首个按下的修饰键与时间。
    session: Option<(ModifierKey, Instant)>,
    /// 本轮仍是「单独一个修饰键」。
    clean: bool,
    last_tap: Option<(ModifierKey, Instant)>,
}

fn index(modifier: ModifierKey) -> usize {
    match modifier {
        ModifierKey::Ctrl => 0,
        ModifierKey::Alt => 1,
        ModifierKey::Shift => 2,
        ModifierKey::Super => 3,
    }
}

impl DoubleTapDetector {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// 喂入一个输入；构成双击时返回该修饰键。
    pub(crate) fn feed(&mut self, input: KeyInput, now: Instant) -> Option<ModifierKey> {
        match input {
            KeyInput::OtherDown => {
                self.clean = false;
                self.last_tap = None;
                None
            }
            KeyInput::ModifierDown(modifier) => {
                let slot = index(modifier);
                if self.held[slot] {
                    // 自动重复。
                    return None;
                }
                if self.held.iter().any(|held| *held) {
                    self.clean = false;
                    self.last_tap = None;
                } else {
                    self.session = Some((modifier, now));
                    self.clean = true;
                }
                self.held[slot] = true;
                None
            }
            KeyInput::ModifierUp(modifier) => {
                self.held[index(modifier)] = false;
                if self.held.iter().any(|held| *held) {
                    return None;
                }
                let session = self.session.take();
                let clean = std::mem::replace(&mut self.clean, false);
                let tap = match session {
                    Some((first, pressed_at))
                        if clean
                            && first == modifier
                            && now.saturating_duration_since(pressed_at) <= MAX_HOLD =>
                    {
                        modifier
                    }
                    _ => {
                        self.last_tap = None;
                        return None;
                    }
                };
                match self.last_tap.take() {
                    Some((previous, at))
                        if previous == tap && now.saturating_duration_since(at) <= INTERVAL =>
                    {
                        Some(tap)
                    }
                    _ => {
                        self.last_tap = Some((tap, now));
                        None
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use KeyInput::{ModifierDown, ModifierUp, OtherDown};

    fn run(inputs: &[(u64, KeyInput)]) -> Vec<ModifierKey> {
        let start = Instant::now();
        let mut detector = DoubleTapDetector::new();
        inputs
            .iter()
            .filter_map(|(ms, input)| detector.feed(*input, start + Duration::from_millis(*ms)))
            .collect()
    }

    const CTRL: ModifierKey = ModifierKey::Ctrl;
    const SHIFT: ModifierKey = ModifierKey::Shift;

    #[test]
    fn two_quick_taps_fire_once() {
        let fired = run(&[
            (0, ModifierDown(CTRL)),
            (60, ModifierUp(CTRL)),
            (150, ModifierDown(CTRL)),
            (200, ModifierUp(CTRL)),
            (260, ModifierDown(CTRL)),
            (300, ModifierUp(CTRL)),
        ]);
        // 第三击开始新一轮，不与第二击再组成双击。
        assert_eq!(fired, [CTRL]);
    }

    #[test]
    fn auto_repeat_does_not_count_as_new_press() {
        let fired = run(&[
            (0, ModifierDown(CTRL)),
            (30, ModifierDown(CTRL)),
            (60, ModifierUp(CTRL)),
            (120, ModifierDown(CTRL)),
            (180, ModifierUp(CTRL)),
        ]);
        assert_eq!(fired, [CTRL]);
    }

    #[test]
    fn slow_second_tap_or_long_hold_does_not_fire() {
        assert!(
            run(&[
                (0, ModifierDown(CTRL)),
                (50, ModifierUp(CTRL)),
                (600, ModifierDown(CTRL)),
                (650, ModifierUp(CTRL)),
            ])
            .is_empty()
        );
        assert!(
            run(&[
                (0, ModifierDown(CTRL)),
                (900, ModifierUp(CTRL)),
                (950, ModifierDown(CTRL)),
                (1000, ModifierUp(CTRL)),
            ])
            .is_empty()
        );
    }

    #[test]
    fn chords_and_other_keys_break_the_sequence() {
        // Ctrl+C 之后再单击 Ctrl 不算双击。
        assert!(
            run(&[
                (0, ModifierDown(CTRL)),
                (20, OtherDown),
                (60, ModifierUp(CTRL)),
                (120, ModifierDown(CTRL)),
                (160, ModifierUp(CTRL)),
            ])
            .is_empty()
        );
        // Ctrl+Shift 同按不算单击。
        assert!(
            run(&[
                (0, ModifierDown(CTRL)),
                (10, ModifierDown(SHIFT)),
                (40, ModifierUp(SHIFT)),
                (60, ModifierUp(CTRL)),
                (120, ModifierDown(CTRL)),
                (160, ModifierUp(CTRL)),
            ])
            .is_empty()
        );
        // 两击之间按了普通键。
        assert!(
            run(&[
                (0, ModifierDown(CTRL)),
                (40, ModifierUp(CTRL)),
                (80, OtherDown),
                (120, ModifierDown(CTRL)),
                (160, ModifierUp(CTRL)),
            ])
            .is_empty()
        );
    }

    #[test]
    fn different_modifiers_do_not_pair() {
        assert!(
            run(&[
                (0, ModifierDown(CTRL)),
                (40, ModifierUp(CTRL)),
                (100, ModifierDown(SHIFT)),
                (140, ModifierUp(SHIFT)),
            ])
            .is_empty()
        );
        assert_eq!(
            run(&[
                (0, ModifierDown(CTRL)),
                (40, ModifierUp(CTRL)),
                (100, ModifierDown(SHIFT)),
                (140, ModifierUp(SHIFT)),
                (200, ModifierDown(SHIFT)),
                (240, ModifierUp(SHIFT)),
            ]),
            [SHIFT]
        );
    }
}
