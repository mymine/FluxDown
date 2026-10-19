//! 快捷键录制的纯逻辑：击键 → 触发器映射、双击修饰键识别、冲突改派后的偏好值。
//!
//! 不依赖 GPUI 实体，便于单测；录制器（焦点 / 事件接线）在 `sections/shortcuts.rs`。

use std::time::{Duration, Instant};

use fluxdown_protocol::shortcuts::{
    DOUBLE_TAP_INTERVAL_MS, ModifierKey, ResolvedShortcut, ShortcutChord, ShortcutRejection,
    ShortcutScope, ShortcutSpec, ShortcutTrigger, resolve_shortcuts, shortcut_conflicts,
    with_shortcut_override,
};
use gpui::Modifiers;
use serde_json::Value;

/// 录制被拒绝的原因（对应行内错误文案）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RecordError {
    /// 全局组合键缺少 Ctrl / Alt / Super。
    GlobalNeedsModifier,
    /// 主键不在可绑定集合内。
    UnsupportedKey,
    /// 当前后端不支持双击修饰键（Wayland portal）。
    DoubleTapUnsupported,
}

impl RecordError {
    pub(crate) fn i18n_key(self) -> &'static str {
        match self {
            Self::GlobalNeedsModifier => "shortcutErrorGlobalNeedsModifier",
            Self::UnsupportedKey => "shortcutErrorUnsupportedKey",
            Self::DoubleTapUnsupported => "shortcutErrorDoubleTapUnsupported",
        }
    }
}

/// 一次按键事件的解读。
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum KeyInterpretation {
    /// 仅修饰键本身（交给双击识别，不当作组合键）。
    ModifierOnly,
    /// 无修饰键的 Escape：取消录制。
    Cancel,
    Trigger(ShortcutTrigger),
    Rejected(RecordError),
}

const MODIFIER_KEY_NAMES: &[&str] = &[
    "shift", "control", "ctrl", "alt", "option", "platform", "cmd", "command", "super", "win",
    "meta", "function", "fn", "capslock",
];

/// GPUI 修饰键状态 → 平台无关修饰键集合（规范顺序，忽略 Fn）。
pub(crate) fn pressed_modifiers(modifiers: &Modifiers) -> Vec<ModifierKey> {
    [
        (modifiers.control, ModifierKey::Ctrl),
        (modifiers.alt, ModifierKey::Alt),
        (modifiers.shift, ModifierKey::Shift),
        (modifiers.platform, ModifierKey::Super),
    ]
    .into_iter()
    .filter_map(|(on, key)| on.then_some(key))
    .collect()
}

/// 解读一次按键：返回应录入的组合键，或取消 / 忽略 / 拒绝。
pub(crate) fn interpret_keystroke(
    modifiers: &Modifiers,
    key: &str,
    scope: ShortcutScope,
) -> KeyInterpretation {
    let lower = key.to_ascii_lowercase();
    if MODIFIER_KEY_NAMES.contains(&lower.as_str()) {
        return KeyInterpretation::ModifierOnly;
    }
    let bare = !modifiers.control && !modifiers.alt && !modifiers.shift && !modifiers.platform;
    if bare && lower == "escape" {
        return KeyInterpretation::Cancel;
    }
    let Some(chord) = ShortcutChord::new(
        modifiers.control,
        modifiers.alt,
        modifiers.shift,
        modifiers.platform,
        &lower,
    ) else {
        return KeyInterpretation::Rejected(RecordError::UnsupportedKey);
    };
    let trigger = ShortcutTrigger::Chord(chord);
    match trigger.check_scope(scope) {
        Ok(()) => KeyInterpretation::Trigger(trigger),
        Err(ShortcutRejection::GlobalNeedsModifier) => {
            KeyInterpretation::Rejected(RecordError::GlobalNeedsModifier)
        }
    }
}

/// 双击修饰键识别：单个修饰键「单独按下再单独松开」算一次轻点，
/// 两次轻点间隔不超过 [`DOUBLE_TAP_INTERVAL_MS`] 即为双击；期间夹杂其它按键 / 修饰键则作废。
#[derive(Debug)]
pub(crate) struct DoubleTapRecorder {
    /// 自全部松开后单独按下、尚未松开的修饰键。
    held: Option<ModifierKey>,
    /// 上一次按键事件时没有任何修饰键按下。
    all_released: bool,
    last_tap: Option<(ModifierKey, Instant)>,
}

impl Default for DoubleTapRecorder {
    fn default() -> Self {
        Self {
            held: None,
            all_released: true,
            last_tap: None,
        }
    }
}

impl DoubleTapRecorder {
    /// 修饰键状态变化；第二次轻点完成时返回该修饰键。
    pub(crate) fn modifiers_changed(
        &mut self,
        down: &[ModifierKey],
        now: Instant,
    ) -> Option<ModifierKey> {
        match down {
            [] => {
                self.all_released = true;
                let released = self.held.take()?;
                let interval = Duration::from_millis(DOUBLE_TAP_INTERVAL_MS);
                match self.last_tap {
                    Some((previous, at))
                        if previous == released
                            && now.saturating_duration_since(at) <= interval =>
                    {
                        self.last_tap = None;
                        Some(released)
                    }
                    _ => {
                        self.last_tap = Some((released, now));
                        None
                    }
                }
            }
            [only] if self.all_released => {
                self.all_released = false;
                self.held = Some(*only);
                None
            }
            [only] if self.held == Some(*only) => None,
            _ => {
                self.all_released = false;
                self.held = None;
                self.last_tap = None;
                None
            }
        }
    }

    /// 作废当前轻点序列（有其它按键按下 / 结束录制）。
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }
}

/// 录入一个触发器后的偏好值与被改派的动作。
#[derive(Debug)]
pub(crate) struct Assignment {
    pub(crate) value: Value,
    /// 同作用域内因冲突被摘掉该触发器的其它动作。
    pub(crate) displaced: Vec<&'static ShortcutSpec>,
}

/// 把 `trigger` 加给 `spec`；同作用域冲突的其它动作同时摘除（一次写出新的整表）。
pub(crate) fn assign_trigger(
    scope: ShortcutScope,
    overrides: Option<&Value>,
    spec: &ShortcutSpec,
    trigger: &ShortcutTrigger,
) -> Assignment {
    let resolved = resolve_shortcuts(scope, overrides);
    let conflicting: Vec<&ResolvedShortcut> =
        shortcut_conflicts(&resolved, spec, trigger).collect();
    let mut value = overrides.cloned().unwrap_or(Value::Null);
    let mut displaced = Vec::with_capacity(conflicting.len());
    for other in conflicting {
        let kept: Vec<ShortcutTrigger> = other
            .triggers
            .iter()
            .filter(|existing| *existing != trigger)
            .cloned()
            .collect();
        value = with_shortcut_override(Some(&value), other.spec, &kept);
        displaced.push(other.spec);
    }
    let mut own: Vec<ShortcutTrigger> = resolved
        .iter()
        .find(|entry| entry.spec.id == spec.id)
        .map(|entry| entry.triggers.clone())
        .unwrap_or_default();
    if !own.contains(trigger) {
        own.push(trigger.clone());
    }
    Assignment {
        value: with_shortcut_override(Some(&value), spec, &own),
        displaced,
    }
}

/// 从 `spec` 上移除一个触发器后的偏好值。
pub(crate) fn remove_trigger(
    scope: ShortcutScope,
    overrides: Option<&Value>,
    spec: &ShortcutSpec,
    trigger: &ShortcutTrigger,
) -> Value {
    let kept: Vec<ShortcutTrigger> = resolve_shortcuts(scope, overrides)
        .into_iter()
        .find(|entry| entry.spec.id == spec.id)
        .map(|entry| entry.triggers)
        .unwrap_or_default()
        .into_iter()
        .filter(|existing| existing != trigger)
        .collect();
    with_shortcut_override(overrides, spec, &kept)
}

/// 把 `spec` 恢复为平台默认后的偏好值。
pub(crate) fn reset_trigger(overrides: Option<&Value>, spec: &ShortcutSpec) -> Value {
    with_shortcut_override(overrides, spec, &spec.defaults())
}

/// 另一作用域里同样绑了 `trigger` 的动作（行内「同一按键也是…」警告用）。
pub(crate) fn cross_scope_matches<'a>(
    other_scope: &'a [ResolvedShortcut],
    trigger: &ShortcutTrigger,
) -> Vec<&'a ResolvedShortcut> {
    other_scope
        .iter()
        .filter(|entry| entry.triggers.contains(trigger))
        .collect()
}

#[cfg(test)]
mod tests {
    use fluxdown_protocol::shortcuts::shortcut_spec;
    use serde_json::json;

    use super::*;

    fn trigger(text: &str) -> ShortcutTrigger {
        text.parse().expect("test trigger")
    }

    fn mods(control: bool, alt: bool, shift: bool, platform: bool) -> Modifiers {
        Modifiers {
            control,
            alt,
            shift,
            platform,
            function: false,
        }
    }

    #[test]
    fn keystroke_maps_modifiers_and_key() {
        assert_eq!(
            interpret_keystroke(&mods(true, false, true, false), "D", ShortcutScope::App),
            KeyInterpretation::Trigger(trigger("ctrl-shift-d"))
        );
        assert_eq!(
            interpret_keystroke(&mods(false, false, false, true), ",", ShortcutScope::App),
            KeyInterpretation::Trigger(trigger("super-,"))
        );
    }

    #[test]
    fn modifier_keys_and_escape_are_not_chords() {
        assert_eq!(
            interpret_keystroke(
                &mods(true, false, false, false),
                "control",
                ShortcutScope::App
            ),
            KeyInterpretation::ModifierOnly
        );
        assert_eq!(
            interpret_keystroke(
                &mods(false, false, false, false),
                "escape",
                ShortcutScope::App
            ),
            KeyInterpretation::Cancel
        );
        assert_eq!(
            interpret_keystroke(
                &mods(true, false, false, false),
                "escape",
                ShortcutScope::App
            ),
            KeyInterpretation::Trigger(trigger("ctrl-escape"))
        );
    }

    #[test]
    fn unsupported_keys_and_global_without_modifier_are_rejected() {
        assert_eq!(
            interpret_keystroke(&mods(true, false, false, false), "é", ShortcutScope::App),
            KeyInterpretation::Rejected(RecordError::UnsupportedKey)
        );
        assert_eq!(
            interpret_keystroke(&mods(false, false, true, false), "a", ShortcutScope::Global),
            KeyInterpretation::Rejected(RecordError::GlobalNeedsModifier)
        );
        assert_eq!(
            interpret_keystroke(
                &mods(false, false, false, false),
                "f9",
                ShortcutScope::Global
            ),
            KeyInterpretation::Trigger(trigger("f9"))
        );
    }

    #[test]
    fn double_tap_requires_two_quick_lone_taps() {
        let start = Instant::now();
        let ms = |n: u64| start + Duration::from_millis(n);
        let mut recorder = DoubleTapRecorder::default();
        assert_eq!(
            recorder.modifiers_changed(&[ModifierKey::Ctrl], ms(0)),
            None
        );
        assert_eq!(recorder.modifiers_changed(&[], ms(50)), None);
        assert_eq!(
            recorder.modifiers_changed(&[ModifierKey::Ctrl], ms(120)),
            None
        );
        assert_eq!(
            recorder.modifiers_changed(&[], ms(200)),
            Some(ModifierKey::Ctrl)
        );
    }

    #[test]
    fn double_tap_is_voided_by_slow_taps_other_keys_and_chords() {
        let start = Instant::now();
        let ms = |n: u64| start + Duration::from_millis(n);
        let mut recorder = DoubleTapRecorder::default();
        recorder.modifiers_changed(&[ModifierKey::Alt], ms(0));
        recorder.modifiers_changed(&[], ms(10));
        recorder.modifiers_changed(&[ModifierKey::Alt], ms(20));
        // 第二次松开超出间隔。
        assert_eq!(recorder.modifiers_changed(&[], ms(10 + 401)), None);

        let mut recorder = DoubleTapRecorder::default();
        recorder.modifiers_changed(&[ModifierKey::Shift], ms(0));
        recorder.modifiers_changed(&[], ms(10));
        recorder.reset();
        recorder.modifiers_changed(&[ModifierKey::Shift], ms(20));
        assert_eq!(recorder.modifiers_changed(&[], ms(30)), None);

        let mut recorder = DoubleTapRecorder::default();
        recorder.modifiers_changed(&[ModifierKey::Ctrl], ms(0));
        recorder.modifiers_changed(&[ModifierKey::Ctrl, ModifierKey::Shift], ms(5));
        recorder.modifiers_changed(&[], ms(10));
        recorder.modifiers_changed(&[ModifierKey::Ctrl], ms(20));
        assert_eq!(recorder.modifiers_changed(&[], ms(30)), None);

        // 不同修饰键各点一次不算双击。
        let mut recorder = DoubleTapRecorder::default();
        recorder.modifiers_changed(&[ModifierKey::Ctrl], ms(0));
        recorder.modifiers_changed(&[], ms(10));
        recorder.modifiers_changed(&[ModifierKey::Alt], ms(20));
        assert_eq!(recorder.modifiers_changed(&[], ms(30)), None);
    }

    #[test]
    fn double_tap_ignores_auto_repeat_while_held() {
        let start = Instant::now();
        let ms = |n: u64| start + Duration::from_millis(n);
        let mut recorder = DoubleTapRecorder::default();
        recorder.modifiers_changed(&[ModifierKey::Ctrl], ms(0));
        recorder.modifiers_changed(&[ModifierKey::Ctrl], ms(10));
        recorder.modifiers_changed(&[], ms(20));
        recorder.modifiers_changed(&[ModifierKey::Ctrl], ms(30));
        assert_eq!(
            recorder.modifiers_changed(&[], ms(40)),
            Some(ModifierKey::Ctrl)
        );
    }

    #[test]
    fn assigning_a_taken_trigger_moves_it_in_one_write() {
        let spec = shortcut_spec(ShortcutScope::Global, "pause_all").expect("spec");
        let other = shortcut_spec(ShortcutScope::Global, "resume_all").expect("spec");
        let overrides = json!({ "resume_all": ["ctrl-alt-p"] });
        let assignment = assign_trigger(
            ShortcutScope::Global,
            Some(&overrides),
            spec,
            &trigger("ctrl-alt-p"),
        );
        assert_eq!(assignment.displaced.len(), 1);
        assert_eq!(assignment.displaced[0].id, other.id);
        // 被摘空的动作回到默认（未绑定），不留条目。
        assert_eq!(assignment.value, json!({ "pause_all": ["ctrl-alt-p"] }));
    }

    #[test]
    fn assigning_within_overlapping_domains_displaces_the_other_action() {
        let spec = shortcut_spec(ShortcutScope::App, "pause_all").expect("spec");
        let assignment = assign_trigger(ShortcutScope::App, None, spec, &trigger("g"));
        assert_eq!(assignment.displaced.len(), 1);
        assert_eq!(assignment.displaced[0].id, "cycle_group");
        let resolved = resolve_shortcuts(ShortcutScope::App, Some(&assignment.value));
        let cycle_group = resolved
            .iter()
            .find(|entry| entry.spec.id == "cycle_group")
            .expect("cycle_group");
        assert!(cycle_group.triggers.is_empty());
        let pause_all = resolved
            .iter()
            .find(|entry| entry.spec.id == "pause_all")
            .expect("pause_all");
        assert_eq!(pause_all.triggers, vec![trigger("g")]);
    }

    #[test]
    fn remove_and_reset_round_trip() {
        let spec = shortcut_spec(ShortcutScope::App, "delete_selected").expect("spec");
        let removed = remove_trigger(ShortcutScope::App, None, spec, &trigger("delete"));
        let resolved = resolve_shortcuts(ShortcutScope::App, Some(&removed));
        let entry = resolved
            .iter()
            .find(|entry| entry.spec.id == "delete_selected")
            .expect("entry");
        assert!(!entry.triggers.contains(&trigger("delete")));
        assert!(entry.customized);
        let restored = reset_trigger(Some(&removed), spec);
        assert_eq!(restored, json!({}));
    }

    #[test]
    fn cross_scope_lookup_finds_same_trigger() {
        let global = resolve_shortcuts(
            ShortcutScope::Global,
            Some(&json!({ "show_window": ["ctrl-alt-d"] })),
        );
        let hits = cross_scope_matches(&global, &trigger("ctrl-alt-d"));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].spec.id, "show_window");
        assert!(cross_scope_matches(&global, &trigger("ctrl-alt-x")).is_empty());
    }
}
