//! 快捷键展示与录制状态：设置页与 app（命令面板、应用内双击修饰键监听）共用。

use fluxdown_protocol::shortcuts::{ShortcutTrigger, key_label};
use fluxdown_ui_i18n::Translator;
use gpui::{App, Global};

/// 当前平台的触发器展示文本；双击修饰键经 `shortcutDoubleTap` 本地化（如「双击 Ctrl」）。
pub fn shortcut_label(trigger: &ShortcutTrigger, translator: &Translator) -> String {
    match trigger {
        ShortcutTrigger::Chord(chord) => chord.label(),
        ShortcutTrigger::DoubleTap(modifier) => {
            translator.text_with("shortcutDoubleTap", &[("key", modifier.label())])
        }
    }
}

/// 拆成逐个键帽的展示文本（修饰键按平台规范顺序，主键在后）；双击修饰键为同一键帽两次。
pub(crate) fn shortcut_keycaps(trigger: &ShortcutTrigger) -> Vec<&'static str> {
    match trigger {
        ShortcutTrigger::Chord(chord) => chord
            .modifiers()
            .map(|modifier| modifier.label())
            .chain(std::iter::once(key_label(chord.key())))
            .collect(),
        ShortcutTrigger::DoubleTap(modifier) => vec![modifier.label(); 2],
    }
}

#[derive(Default)]
struct ShortcutRecording(bool);

impl Global for ShortcutRecording {}

/// 设置页是否正在录制快捷键；录制期间应用内双击修饰键监听必须暂停，避免录制动作被当成触发。
pub fn shortcut_recording(cx: &App) -> bool {
    cx.try_global::<ShortcutRecording>()
        .is_some_and(|recording| recording.0)
}

/// 设置页录制器进入 / 退出录制态时调用。
pub(crate) fn set_shortcut_recording(recording: bool, cx: &mut App) {
    cx.set_global(ShortcutRecording(recording));
}
