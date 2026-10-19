//! X11 后端：根窗口 `XGrabKey` 承载组合键，XInput2 原始键盘事件承载双击修饰键。
//!
//! 线程模型：事件线程阻塞在 `wait_for_event`（按键、键盘映射变化、XI2 原始键盘事件）；
//! 应用线程串行处理 [`HotkeyPort::apply`]。两者共享 [`Inner`]（`RustConnection` 本身线程安全）
//! 与互斥保护的 [`State`]。每次 `apply` / 键盘映射变化都整表重建：先注销旧抓取，再逐条重新注册。
//! 抓取随连接存活；事件线程随进程存活（agent 只在进程退出时才会放弃后端）。

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use fluxdown_protocol::shortcuts::{
    GlobalShortcutBackend, GlobalShortcutState, ModifierKey, ShortcutChord, ShortcutTrigger,
};
use x11rb::connection::{Connection, RequestConnection};
use x11rb::errors::ReplyError;
use x11rb::protocol::xinput::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{ConnectionExt as _, GrabMode, Keycode, Mapping, Window};
use x11rb::protocol::{ErrorKind, Event};
use x11rb::rust_connection::RustConnection;

use super::keys;
use crate::hotkey::double_tap::{DoubleTapDetector, KeyInput};
use crate::hotkey::{HotkeyBinding, HotkeyOutcome, HotkeyPort, HotkeySink};

const MASK_SHIFT: u16 = 1 << 0;
const MASK_LOCK: u16 = 1 << 1;
const MASK_CONTROL: u16 = 1 << 2;
/// Mod1..Mod5 对应修饰键下标 3..=7（Shift、Lock、Control 之后）。
const FIRST_MOD_INDEX: usize = 3;
const MODIFIER_COUNT: usize = 8;
/// XInput2 的 `XIAllMasterDevices`。
const ALL_MASTER_DEVICES: u16 = 1;

pub(super) fn start(sink: Arc<dyn HotkeySink>) -> Result<Box<dyn HotkeyPort>, String> {
    let (conn, screen_num) = RustConnection::connect(None)
        .map_err(|e| format!("cannot connect to the X server: {e}"))?;
    let root = conn
        .setup()
        .roots
        .get(screen_num)
        .map(|screen| screen.root)
        .ok_or_else(|| format!("X server has no screen {screen_num}"))?;
    let layout = Layout::load(&conn)?;
    let xi2 = detect_xi2(&conn);
    if !xi2 {
        tracing::info!("XInput2 unavailable; double-tap modifier shortcuts are unsupported");
    }

    let inner = Arc::new(Inner {
        conn,
        root,
        sink,
        xi2,
        state: Mutex::new(State {
            layout,
            bindings: Vec::new(),
            grabs: Vec::new(),
            double_taps: Vec::new(),
            detector: DoubleTapDetector::new(),
            raw_selected: false,
            dead: None,
        }),
    });

    let events = Arc::clone(&inner);
    std::thread::Builder::new()
        .name("hotkey-x11-events".to_owned())
        .spawn(move || event_loop(&events))
        .map_err(|e| format!("cannot start the X11 event thread: {e}"))?;

    let (tx, rx) = mpsc::channel();
    let worker = Arc::clone(&inner);
    std::thread::Builder::new()
        .name("hotkey-x11-apply".to_owned())
        .spawn(move || apply_loop(&worker, &rx))
        .map_err(|e| format!("cannot start the X11 apply thread: {e}"))?;

    Ok(Box::new(X11Port { tx }))
}

struct X11Port {
    tx: Sender<Vec<HotkeyBinding>>,
}

impl HotkeyPort for X11Port {
    fn apply(&self, bindings: Vec<HotkeyBinding>) {
        if let Err(e) = self.tx.send(bindings) {
            tracing::error!("X11 hotkey apply thread is gone: {e}");
        }
    }
}

struct Inner {
    conn: RustConnection,
    root: Window,
    sink: Arc<dyn HotkeySink>,
    /// 服务器支持 XInput2 ≥ 2.0（双击修饰键的前提）。
    xi2: bool,
    state: Mutex<State>,
}

struct State {
    layout: Layout,
    /// 最近一次 `apply` 的整表，键盘映射变化时据此重建。
    bindings: Vec<HotkeyBinding>,
    grabs: Vec<ActiveGrab>,
    double_taps: Vec<(u32, ModifierKey)>,
    detector: DoubleTapDetector,
    /// 根窗口上是否已选择 XI2 原始键盘事件（仅存在双击绑定时选择）。
    raw_selected: bool,
    /// 连接中断后的诊断；此后所有绑定都报告 `Failed`。
    dead: Option<String>,
}

#[derive(Clone, Copy)]
struct ActiveGrab {
    id: u32,
    keycode: Keycode,
    /// 不含锁定键位的修饰键掩码。
    mods: u16,
}

enum GrabFailure {
    InUse,
    Other(String),
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, State> {
        // 状态在每次整表重建时都会被完整重写，中毒后继续使用是安全的。
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// 整表重建并返回逐条结果；调用方在释放锁后上报。
    fn rebuild(&self, state: &mut State) -> Vec<HotkeyOutcome> {
        let bindings = std::mem::take(&mut state.bindings);
        let outcomes = self.rebuild_with(state, &bindings);
        state.bindings = bindings;
        outcomes
    }

    fn rebuild_with(&self, state: &mut State, bindings: &[HotkeyBinding]) -> Vec<HotkeyOutcome> {
        if let Some(detail) = &state.dead {
            return bindings
                .iter()
                .map(|binding| outcome(binding.id, GlobalShortcutState::Failed, Some(detail)))
                .collect();
        }

        self.release_grabs(state);
        state.double_taps.clear();
        state.detector = DoubleTapDetector::new();

        let mut outcomes = Vec::with_capacity(bindings.len());
        for binding in bindings {
            let result = match &binding.trigger {
                ShortcutTrigger::Chord(chord) => self.grab_chord(state, binding.id, chord),
                ShortcutTrigger::DoubleTap(modifier) if self.xi2 => {
                    state.double_taps.push((binding.id, *modifier));
                    outcome(binding.id, GlobalShortcutState::Active, None)
                }
                ShortcutTrigger::DoubleTap(_) => {
                    outcome(binding.id, GlobalShortcutState::Unsupported, None)
                }
            };
            outcomes.push(result);
        }

        let want_raw = !state.double_taps.is_empty();
        if let Err(detail) = self.select_raw(state, want_raw) {
            tracing::warn!("cannot select XInput2 raw key events: {detail}");
            for (id, _) in &state.double_taps {
                if let Some(entry) = outcomes.iter_mut().find(|entry| entry.id == *id) {
                    *entry = outcome(*id, GlobalShortcutState::Failed, Some(&detail));
                }
            }
            state.double_taps.clear();
        }
        outcomes
    }

    fn grab_chord(&self, state: &mut State, id: u32, chord: &ShortcutChord) -> HotkeyOutcome {
        let Some(keysym) = keys::keysym(chord.key()) else {
            return outcome(
                id,
                GlobalShortcutState::Failed,
                Some(&format!("no X11 keysym for key {}", chord.key())),
            );
        };
        let Some(keycode) = state.layout.keycode_for(keysym) else {
            return outcome(
                id,
                GlobalShortcutState::Failed,
                Some(&format!(
                    "key {} is not on the current keyboard layout",
                    chord.key()
                )),
            );
        };
        let mods = state.layout.masks.chord_mask(chord);
        let locks = state.layout.masks.lock_combinations();
        match self.grab_key_all_locks(keycode, mods, &locks) {
            Ok(()) => {
                state.grabs.push(ActiveGrab { id, keycode, mods });
                outcome(id, GlobalShortcutState::Active, None)
            }
            Err(GrabFailure::InUse) => outcome(id, GlobalShortcutState::InUse, None),
            Err(GrabFailure::Other(detail)) => {
                outcome(id, GlobalShortcutState::Failed, Some(&detail))
            }
        }
    }

    /// 对每个忽略锁定键组合发出 `GrabKey` 并逐个检查；任何一个失败都回滚该键的全部抓取。
    fn grab_key_all_locks(
        &self,
        keycode: Keycode,
        mods: u16,
        locks: &[u16],
    ) -> Result<(), GrabFailure> {
        let mut failure = None;
        let mut cookies = Vec::with_capacity(locks.len());
        for lock in locks {
            match self.conn.grab_key(
                false,
                self.root,
                (mods | lock).into(),
                keycode,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
            ) {
                Ok(cookie) => cookies.push(cookie),
                Err(e) => {
                    failure = Some(GrabFailure::Other(format!("GrabKey request failed: {e}")));
                    break;
                }
            }
        }
        for cookie in cookies {
            if let Err(e) = cookie.check()
                && failure.is_none()
            {
                failure = Some(classify_grab_error(e));
            }
        }
        match failure {
            None => Ok(()),
            Some(failure) => {
                self.ungrab_key_all_locks(keycode, mods, locks);
                Err(failure)
            }
        }
    }

    fn ungrab_key_all_locks(&self, keycode: Keycode, mods: u16, locks: &[u16]) {
        for lock in locks {
            let result = match self
                .conn
                .ungrab_key(keycode, self.root, (mods | lock).into())
            {
                Ok(cookie) => cookie.check().map_err(|e| e.to_string()),
                Err(e) => Err(e.to_string()),
            };
            if let Err(e) = result {
                tracing::warn!(keycode, "UngrabKey failed: {e}");
            }
        }
    }

    fn release_grabs(&self, state: &mut State) {
        let locks = state.layout.masks.lock_combinations();
        for grab in std::mem::take(&mut state.grabs) {
            self.ungrab_key_all_locks(grab.keycode, grab.mods, &locks);
        }
    }

    /// 仅在需要双击检测时订阅 XI2 原始键盘事件，无双击绑定时立即取消订阅。
    fn select_raw(&self, state: &mut State, enable: bool) -> Result<(), String> {
        if state.raw_selected == enable {
            return Ok(());
        }
        let mask = if enable {
            xinput::XIEventMask::RAW_KEY_PRESS | xinput::XIEventMask::RAW_KEY_RELEASE
        } else {
            xinput::XIEventMask::from(0u32)
        };
        let masks = [xinput::EventMask {
            deviceid: ALL_MASTER_DEVICES,
            mask: vec![mask],
        }];
        self.conn
            .xinput_xi_select_events(self.root, &masks)
            .map_err(|e| format!("XISelectEvents request failed: {e}"))?
            .check()
            .map_err(|e| format!("XISelectEvents failed: {e}"))?;
        state.raw_selected = enable;
        Ok(())
    }

    fn handle_event(&self, event: Event) {
        match event {
            Event::KeyPress(press) => self.on_grab_press(press.detail, u16::from(press.state)),
            Event::XinputRawKeyPress(raw) => self.on_raw_key(raw.detail, true),
            Event::XinputRawKeyRelease(raw) => self.on_raw_key(raw.detail, false),
            Event::MappingNotify(change)
                if change.request == Mapping::KEYBOARD || change.request == Mapping::MODIFIER =>
            {
                self.on_mapping_changed();
            }
            Event::Error(e) => tracing::debug!("asynchronous X11 error: {e:?}"),
            _ => {}
        }
    }

    fn on_grab_press(&self, keycode: Keycode, raw_state: u16) {
        let fired: Vec<u32> = {
            let state = self.lock();
            let clean = state.layout.masks.clean(raw_state);
            state
                .grabs
                .iter()
                .filter(|grab| grab.keycode == keycode && grab.mods == clean)
                .map(|grab| grab.id)
                .collect()
        };
        for id in fired {
            self.sink.fired(id);
        }
    }

    fn on_raw_key(&self, detail: u32, pressed: bool) {
        let fired: Vec<u32> = {
            let mut state = self.lock();
            if state.double_taps.is_empty() {
                return;
            }
            let modifier = u8::try_from(detail)
                .ok()
                .and_then(|keycode| state.layout.modifier_keys.get(&keycode).copied());
            let input = match (modifier, pressed) {
                (Some(modifier), true) => KeyInput::ModifierDown(modifier),
                (Some(modifier), false) => KeyInput::ModifierUp(modifier),
                (None, true) => KeyInput::OtherDown,
                (None, false) => return,
            };
            match state.detector.feed(input, Instant::now()) {
                Some(tapped) => state
                    .double_taps
                    .iter()
                    .filter(|(_, modifier)| *modifier == tapped)
                    .map(|(id, _)| *id)
                    .collect(),
                None => return,
            }
        };
        for id in fired {
            self.sink.fired(id);
        }
    }

    fn on_mapping_changed(&self) {
        let layout = match Layout::load(&self.conn) {
            Ok(layout) => layout,
            Err(e) => {
                tracing::warn!("cannot reload the X11 keyboard mapping: {e}");
                return;
            }
        };
        let outcomes = {
            let mut state = self.lock();
            state.layout = layout;
            self.rebuild(&mut state)
        };
        self.sink.report(GlobalShortcutBackend::X11, outcomes);
    }

    /// 连接中断：抓取已随连接消失，之后所有绑定报告 `Failed`。
    fn mark_dead(&self, detail: String) {
        let outcomes = {
            let mut state = self.lock();
            state.grabs.clear();
            state.double_taps.clear();
            state.dead = Some(detail.clone());
            state
                .bindings
                .iter()
                .map(|binding| outcome(binding.id, GlobalShortcutState::Failed, Some(&detail)))
                .collect()
        };
        self.sink.report(GlobalShortcutBackend::X11, outcomes);
    }
}

fn event_loop(inner: &Inner) {
    loop {
        match inner.conn.wait_for_event() {
            Ok(event) => inner.handle_event(event),
            Err(e) => {
                tracing::error!("X11 connection lost: {e}");
                inner.mark_dead(format!("X11 connection lost: {e}"));
                return;
            }
        }
    }
}

fn apply_loop(inner: &Inner, rx: &Receiver<Vec<HotkeyBinding>>) {
    while let Ok(mut table) = rx.recv() {
        // 只关心最新的一张表。
        while let Ok(newer) = rx.try_recv() {
            table = newer;
        }
        let outcomes = {
            let mut state = inner.lock();
            state.bindings = table;
            inner.rebuild(&mut state)
        };
        inner.sink.report(GlobalShortcutBackend::X11, outcomes);
    }
}

fn outcome(id: u32, state: GlobalShortcutState, detail: Option<&str>) -> HotkeyOutcome {
    HotkeyOutcome {
        id,
        state,
        detail: detail.map(str::to_owned),
    }
}

fn classify_grab_error(error: ReplyError) -> GrabFailure {
    match error {
        ReplyError::X11Error(x11) if x11.error_kind == ErrorKind::Access => GrabFailure::InUse,
        other => GrabFailure::Other(format!("GrabKey failed: {other}")),
    }
}

fn detect_xi2(conn: &RustConnection) -> bool {
    match conn.extension_information(xinput::X11_EXTENSION_NAME) {
        Ok(Some(_)) => {}
        Ok(None) => return false,
        Err(e) => {
            tracing::warn!("cannot query the XInput extension: {e}");
            return false;
        }
    }
    let reply = conn
        .xinput_xi_query_version(2, 0)
        .map_err(|e| e.to_string())
        .and_then(|cookie| cookie.reply().map_err(|e| e.to_string()));
    match reply {
        Ok(version) => version.major_version >= 2,
        Err(e) => {
            tracing::warn!("XIQueryVersion failed: {e}");
            false
        }
    }
}

/// 键盘映射快照：keycode ↔ keysym、修饰键 keycode、修饰位掩码。
struct Layout {
    min_keycode: Keycode,
    keysyms_per_keycode: usize,
    keysyms: Vec<u32>,
    /// 修饰键 keycode（左右两侧）→ 修饰键。
    modifier_keys: HashMap<Keycode, ModifierKey>,
    masks: Masks,
}

impl Layout {
    fn load(conn: &RustConnection) -> Result<Self, String> {
        let setup = conn.setup();
        let min_keycode = setup.min_keycode;
        let count = setup
            .max_keycode
            .saturating_sub(min_keycode)
            .saturating_add(1);
        let mapping = conn
            .get_keyboard_mapping(min_keycode, count)
            .map_err(|e| format!("GetKeyboardMapping request failed: {e}"))?
            .reply()
            .map_err(|e| format!("GetKeyboardMapping failed: {e}"))?;
        let keysyms_per_keycode = usize::from(mapping.keysyms_per_keycode);
        if keysyms_per_keycode == 0 {
            return Err("the X server returned an empty keyboard mapping".to_owned());
        }
        let modifier_mapping = conn
            .get_modifier_mapping()
            .map_err(|e| format!("GetModifierMapping request failed: {e}"))?
            .reply()
            .map_err(|e| format!("GetModifierMapping failed: {e}"))?;

        let mut layout = Self {
            min_keycode,
            keysyms_per_keycode,
            keysyms: mapping.keysyms,
            modifier_keys: HashMap::new(),
            masks: Masks::default(),
        };
        layout.modifier_keys = layout.collect_modifier_keys();
        layout.masks = Masks::resolve(&layout, &modifier_mapping.keycodes);
        Ok(layout)
    }

    fn rows(&self) -> impl Iterator<Item = (Keycode, &[u32])> {
        self.keysyms
            .chunks(self.keysyms_per_keycode)
            .enumerate()
            .filter_map(|(index, row)| {
                let keycode = usize::from(self.min_keycode).checked_add(index)?;
                Some((Keycode::try_from(keycode).ok()?, row))
            })
    }

    /// 优先选取主层（列号最小）命中的 keycode。
    fn keycode_for(&self, keysym: u32) -> Option<Keycode> {
        (0..self.keysyms_per_keycode).find_map(|column| {
            self.rows()
                .find(|(_, row)| row.get(column) == Some(&keysym))
                .map(|(keycode, _)| keycode)
        })
    }

    fn keycodes_where(&self, wanted: impl Fn(u32) -> bool) -> Vec<Keycode> {
        self.rows()
            .filter(|(_, row)| row.iter().any(|keysym| wanted(*keysym)))
            .map(|(keycode, _)| keycode)
            .collect()
    }

    fn collect_modifier_keys(&self) -> HashMap<Keycode, ModifierKey> {
        self.rows()
            .filter_map(|(keycode, row)| {
                let modifier = keys::modifier_of_keysym(*row.first()?)?;
                Some((keycode, modifier))
            })
            .collect()
    }
}

/// 修饰位掩码（Alt / Super / NumLock 在不同键盘映射下落在不同的 Mod1..Mod5）。
struct Masks {
    alt: u16,
    super_key: u16,
    /// NumLock 的修饰位；键盘上没有 NumLock 时为 `None`。
    num_lock: Option<u16>,
}

impl Default for Masks {
    fn default() -> Self {
        Self {
            alt: 1 << 3,
            super_key: 1 << 6,
            num_lock: Some(1 << 4),
        }
    }
}

impl Masks {
    fn resolve(layout: &Layout, modifier_mapping: &[Keycode]) -> Self {
        let per_modifier = modifier_mapping.len() / MODIFIER_COUNT;
        // 在 Mod1..Mod5 里查找包含给定 keycode 之一的修饰位，优先 `preferred`。
        let find = |preferred: usize, codes: &[Keycode]| -> Option<u16> {
            if per_modifier == 0 || codes.is_empty() {
                return None;
            }
            std::iter::once(preferred)
                .chain(FIRST_MOD_INDEX..MODIFIER_COUNT)
                .find(|index| {
                    modifier_mapping
                        .get(index * per_modifier..(index + 1) * per_modifier)
                        .is_some_and(|row| {
                            row.iter().any(|code| *code != 0 && codes.contains(code))
                        })
                })
                .map(|index| 1u16 << index)
        };

        let alt_codes =
            layout.keycodes_where(|sym| keys::modifier_of_keysym(sym) == Some(ModifierKey::Alt));
        let super_codes =
            layout.keycodes_where(|sym| keys::modifier_of_keysym(sym) == Some(ModifierKey::Super));
        let num_codes = layout.keycodes_where(|sym| sym == keys::KEYSYM_NUM_LOCK);
        let defaults = Self::default();
        Self {
            alt: find(3, &alt_codes).unwrap_or(defaults.alt),
            super_key: find(6, &super_codes).unwrap_or(defaults.super_key),
            num_lock: find(4, &num_codes),
        }
    }

    /// 组合键对应的修饰位掩码。
    fn chord_mask(&self, chord: &ShortcutChord) -> u16 {
        let mut mask = 0;
        if chord.ctrl {
            mask |= MASK_CONTROL;
        }
        if chord.shift {
            mask |= MASK_SHIFT;
        }
        if chord.alt {
            mask |= self.alt;
        }
        if chord.super_key {
            mask |= self.super_key;
        }
        mask
    }

    /// 去掉 CapsLock / NumLock 等无关位后的修饰位。
    fn clean(&self, state: u16) -> u16 {
        state & (MASK_CONTROL | MASK_SHIFT | self.alt | self.super_key)
    }

    /// 每个组合键都要在这些锁定键状态下各抓取一次：无锁定、CapsLock、NumLock、两者。
    fn lock_combinations(&self) -> Vec<u16> {
        let mut combos = vec![0, MASK_LOCK];
        if let Some(num) = self.num_lock {
            combos.push(num);
            combos.push(num | MASK_LOCK);
        }
        combos
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(rows: &[&[u32]], modifier_mapping: &[Keycode]) -> Layout {
        let per = rows.iter().map(|row| row.len()).max().unwrap_or(1);
        let keysyms: Vec<u32> = rows
            .iter()
            .flat_map(|row| {
                let mut padded = row.to_vec();
                padded.resize(per, 0);
                padded
            })
            .collect();
        let mut layout = Layout {
            min_keycode: 8,
            keysyms_per_keycode: per,
            keysyms,
            modifier_keys: HashMap::new(),
            masks: Masks::default(),
        };
        layout.modifier_keys = layout.collect_modifier_keys();
        layout.masks = Masks::resolve(&layout, modifier_mapping);
        layout
    }

    #[test]
    fn keycode_lookup_prefers_the_primary_layer() {
        // keycode 8: 西里尔字母主层、拉丁字母次层；keycode 9: 拉丁字母主层。
        let layout = layout(&[&[0x6d0, 0x6d0, 0x64, 0x44], &[0x64, 0x44, 0, 0]], &[]);
        assert_eq!(layout.keycode_for(0x64), Some(9));
        assert_eq!(layout.keycode_for(0x7a), None);
    }

    #[test]
    fn modifier_masks_follow_the_server_mapping() {
        // keycode 8 = Alt_L（挂在 Mod3 = 下标 5），9 = Super_L（Mod5 = 下标 7），10 = Num_Lock（Mod4 = 下标 6）。
        let rows: &[&[u32]] = &[&[0xffe9], &[0xffeb], &[0xff7f]];
        let mut mapping = vec![0u8; MODIFIER_COUNT * 2];
        mapping[5 * 2] = 8;
        mapping[7 * 2] = 9;
        mapping[6 * 2] = 10;
        let layout = layout(rows, &mapping);
        assert_eq!(layout.masks.alt, 1 << 5);
        assert_eq!(layout.masks.super_key, 1 << 7);
        assert_eq!(layout.masks.num_lock, Some(1 << 6));
        assert_eq!(layout.modifier_keys.get(&8), Some(&ModifierKey::Alt));
        assert_eq!(layout.modifier_keys.get(&9), Some(&ModifierKey::Super));
        assert_eq!(layout.masks.lock_combinations().len(), 4);
        assert_eq!(
            layout.masks.clean(MASK_LOCK | (1 << 6) | MASK_CONTROL),
            MASK_CONTROL
        );
    }

    #[test]
    fn missing_mapping_falls_back_to_conventional_masks() {
        let layout = layout(&[&[0x61]], &[]);
        assert_eq!(layout.masks.alt, 1 << 3);
        assert_eq!(layout.masks.super_key, 1 << 6);
        assert_eq!(layout.masks.num_lock, None);
        assert_eq!(layout.masks.lock_combinations(), vec![0, MASK_LOCK]);
    }

    #[test]
    fn chord_mask_combines_modifiers() {
        let masks = Masks::default();
        let chord = ShortcutChord::new(true, true, true, true, "d");
        let chord = chord.map(|chord| masks.chord_mask(&chord));
        assert_eq!(chord, Some(MASK_CONTROL | MASK_SHIFT | (1 << 3) | (1 << 6)));
    }
}
