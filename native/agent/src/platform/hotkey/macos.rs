//! macOS 全局热键后端（只能在主线程使用，对象留在宿主 tao 事件循环里）。
//!
//! - 组合键：Carbon `RegisterEventHotKey`，经应用事件目标上的 `kEventHotKeyPressed` 处理器回调。
//!   Carbon 热键不需要任何系统授权；已被其它程序占用时返回 `eventHotKeyExistsErr`。
//! - 双击修饰键：`NSEvent` 全局监听（FlagsChanged | KeyDown）喂给 [`DoubleTapDetector`]。
//!   全局键盘监听需要「辅助功能」授权：未授权时双击条目报告 `PermissionRequired`，并由宿主事件
//!   循环按 [`RECHECK_INTERVAL`] 的节拍复查（仅在存在待授权条目时才有定时器），授权后安装监听
//!   并重新回报。首次下发含双击的绑定表时弹一次系统授权提示。
//!
//! unsafe 边界：Carbon / ApplicationServices 的 C 函数、`NSEvent` 监听令牌的移除，以及监听回调里
//! 对 AppKit 传入事件的短期借用；全部在本文件内封闭，拥有的句柄立即进入 RAII（`!Send`）。

use std::cell::RefCell;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use block2::RcBlock;
use fluxdown_protocol::shortcuts::{
    GlobalShortcutBackend, GlobalShortcutState, ModifierKey, ShortcutChord, ShortcutTrigger,
};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool};
use objc2::{MainThreadMarker, class, msg_send};
use objc2_app_kit::{NSEvent, NSEventMask, NSEventModifierFlags, NSEventType};
use objc2_foundation::{NSDictionary, NSString};

use crate::hotkey::double_tap::{DoubleTapDetector, KeyInput};
use crate::hotkey::{HotkeyBinding, HotkeyOutcome, HotkeySink};

/// 授权未就绪时的复查间隔。
const RECHECK_INTERVAL: Duration = Duration::from_millis(2500);

type OsStatus = i32;
type EventTargetRef = *mut c_void;
type EventHandlerRef = *mut c_void;
type EventHotKeyRef = *mut c_void;
type EventRef = *mut c_void;
/// `EventHandlerUPP`：`(EventHandlerCallRef, EventRef, void *userData) -> OSStatus`。
type EventHandlerProc = extern "C" fn(*mut c_void, EventRef, *mut c_void) -> OsStatus;

const NO_ERR: OsStatus = 0;
/// `eventHotKeyExistsErr`：同一热键已被本进程或其它程序注册。
const EVENT_HOT_KEY_EXISTS_ERR: OsStatus = -9878;
/// `eventNotHandledErr`。
const EVENT_NOT_HANDLED_ERR: OsStatus = -9874;
/// `kEventClassKeyboard` = `'keyb'`。
const EVENT_CLASS_KEYBOARD: u32 = 0x6B65_7962;
/// `kEventHotKeyPressed`。
const EVENT_HOT_KEY_PRESSED: u32 = 5;
/// `kEventParamDirectObject` = `'----'`。
const EVENT_PARAM_DIRECT_OBJECT: u32 = 0x2D2D_2D2D;
/// `typeEventHotKeyID` = `'hkid'`。
const TYPE_EVENT_HOT_KEY_ID: u32 = 0x686B_6964;
/// 热键签名 `'FLXD'`；回调只处理本签名的热键。
const HOT_KEY_SIGNATURE: u32 = 0x464C_5844;

const CMD_KEY: u32 = 0x0100;
const SHIFT_KEY: u32 = 0x0200;
const OPTION_KEY: u32 = 0x0800;
const CONTROL_KEY: u32 = 0x1000;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct EventHotKeyId {
    signature: u32,
    id: u32,
}

#[repr(C)]
struct EventTypeSpec {
    event_class: u32,
    event_kind: u32,
}

// SAFETY: Signatures follow Carbon HIToolbox `CarbonEvents.h`: OSStatus is i32, ItemCount /
// OptionBits / EventParamName / EventParamType are u32, ByteCount is pointer-sized unsigned,
// EventHotKeyID is passed by value as two u32 fields (`#[repr(C)]`), and every *Ref type is an
// opaque pointer.
#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn GetApplicationEventTarget() -> EventTargetRef;
    fn InstallEventHandler(
        target: EventTargetRef,
        handler: EventHandlerProc,
        type_count: u32,
        types: *const EventTypeSpec,
        user_data: *mut c_void,
        out_ref: *mut EventHandlerRef,
    ) -> OsStatus;
    fn RemoveEventHandler(handler: EventHandlerRef) -> OsStatus;
    fn RegisterEventHotKey(
        key_code: u32,
        modifiers: u32,
        id: EventHotKeyId,
        target: EventTargetRef,
        options: u32,
        out_ref: *mut EventHotKeyRef,
    ) -> OsStatus;
    fn UnregisterEventHotKey(hot_key: EventHotKeyRef) -> OsStatus;
    fn GetEventParameter(
        event: EventRef,
        name: u32,
        desired_type: u32,
        actual_type: *mut u32,
        buffer_size: usize,
        actual_size: *mut usize,
        data: *mut EventHotKeyId,
    ) -> OsStatus;
}

// SAFETY: Signatures follow ApplicationServices `AXUIElement.h`: Boolean is `unsigned char`, and
// the options argument is a (toll-free bridged) CFDictionaryRef or NULL.
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> u8;
    fn AXIsProcessTrustedWithOptions(options: *const c_void) -> u8;
}

thread_local! {
    /// Carbon 处理器回调（主线程）把触发转交给的 sink。
    static CHORD_SINK: RefCell<Option<Arc<dyn HotkeySink>>> = const { RefCell::new(None) };
}

/// Carbon 热键按下回调。只在主线程由应用事件目标派发；不得 panic（跨 FFI 边界）。
extern "C" fn hot_key_pressed(
    _call: *mut c_void,
    event: EventRef,
    _user_data: *mut c_void,
) -> OsStatus {
    let mut hot_key = EventHotKeyId::default();
    // SAFETY: `event` is the live EventRef Carbon passes for the duration of this callback;
    // `hot_key` is a valid `EventHotKeyId` out buffer whose size matches `buffer_size`, and the
    // optional actual-type / actual-size outputs are NULL.
    let status = unsafe {
        GetEventParameter(
            event,
            EVENT_PARAM_DIRECT_OBJECT,
            TYPE_EVENT_HOT_KEY_ID,
            std::ptr::null_mut(),
            size_of::<EventHotKeyId>(),
            std::ptr::null_mut(),
            &mut hot_key,
        )
    };
    if status != NO_ERR || hot_key.signature != HOT_KEY_SIGNATURE {
        return EVENT_NOT_HANDLED_ERR;
    }
    let delivered = CHORD_SINK.try_with(|slot| match slot.try_borrow() {
        Ok(slot) => slot.as_ref().map(|sink| sink.fired(hot_key.id)).is_some(),
        Err(_) => false,
    });
    if delivered == Ok(true) {
        NO_ERR
    } else {
        EVENT_NOT_HANDLED_ERR
    }
}

/// 已安装的应用级 Carbon 事件处理器；`Drop` 移除并清空 sink。
struct CarbonHandler(EventHandlerRef);

impl CarbonHandler {
    fn install(sink: Arc<dyn HotkeySink>) -> Result<Self, String> {
        let types = [EventTypeSpec {
            event_class: EVENT_CLASS_KEYBOARD,
            event_kind: EVENT_HOT_KEY_PRESSED,
        }];
        // SAFETY: no preconditions; returns the process-wide application event target (or NULL).
        let target = unsafe { GetApplicationEventTarget() };
        if target.is_null() {
            return Err("GetApplicationEventTarget returned NULL".to_owned());
        }
        let mut handler: EventHandlerRef = std::ptr::null_mut();
        // SAFETY: `target` is the live application target; `hot_key_pressed` matches
        // EventHandlerUPP and uses no user data; `types` holds exactly one valid spec that
        // outlives the call; `handler` is a valid out pointer.
        let status = unsafe {
            InstallEventHandler(
                target,
                hot_key_pressed,
                1,
                types.as_ptr(),
                std::ptr::null_mut(),
                &mut handler,
            )
        };
        if status != NO_ERR || handler.is_null() {
            return Err(format!("InstallEventHandler failed: OSStatus {status}"));
        }
        let installed = Self(handler);
        CHORD_SINK.with(|slot| *slot.borrow_mut() = Some(sink));
        Ok(installed)
    }
}

impl Drop for CarbonHandler {
    fn drop(&mut self) {
        // SAFETY: `self.0` is the handler returned by InstallEventHandler and is removed exactly
        // once, here.
        let status = unsafe { RemoveEventHandler(self.0) };
        if status != NO_ERR {
            tracing::warn!(status, "RemoveEventHandler failed");
        }
        CHORD_SINK.with(|slot| *slot.borrow_mut() = None);
    }
}

/// 已注册的 Carbon 热键；`Drop` 注销。
struct ChordRegistration(EventHotKeyRef);

impl Drop for ChordRegistration {
    fn drop(&mut self) {
        // SAFETY: `self.0` is the reference returned by a successful RegisterEventHotKey and is
        // unregistered exactly once, here.
        let status = unsafe { UnregisterEventHotKey(self.0) };
        if status != NO_ERR {
            tracing::warn!(status, "UnregisterEventHotKey failed");
        }
    }
}

/// `SHORTCUT_KEYS` 主键 → `kVK_*` 虚拟键码（ANSI 键位；码值与键盘布局无关的物理位置）。
fn key_code(key: &str) -> Option<u32> {
    Some(match key {
        "a" => 0x00,
        "s" => 0x01,
        "d" => 0x02,
        "f" => 0x03,
        "h" => 0x04,
        "g" => 0x05,
        "z" => 0x06,
        "x" => 0x07,
        "c" => 0x08,
        "v" => 0x09,
        "b" => 0x0B,
        "q" => 0x0C,
        "w" => 0x0D,
        "e" => 0x0E,
        "r" => 0x0F,
        "y" => 0x10,
        "t" => 0x11,
        "1" => 0x12,
        "2" => 0x13,
        "3" => 0x14,
        "4" => 0x15,
        "6" => 0x16,
        "5" => 0x17,
        "=" => 0x18,
        "9" => 0x19,
        "7" => 0x1A,
        "-" => 0x1B,
        "8" => 0x1C,
        "0" => 0x1D,
        "]" => 0x1E,
        "o" => 0x1F,
        "u" => 0x20,
        "[" => 0x21,
        "i" => 0x22,
        "p" => 0x23,
        "enter" => 0x24,
        "l" => 0x25,
        "j" => 0x26,
        "'" => 0x27,
        "k" => 0x28,
        ";" => 0x29,
        "\\" => 0x2A,
        "," => 0x2B,
        "/" => 0x2C,
        "n" => 0x2D,
        "m" => 0x2E,
        "." => 0x2F,
        "tab" => 0x30,
        "space" => 0x31,
        "`" => 0x32,
        "backspace" => 0x33,
        "escape" => 0x35,
        "f17" => 0x40,
        "f18" => 0x4F,
        "f19" => 0x50,
        "f20" => 0x5A,
        "f5" => 0x60,
        "f6" => 0x61,
        "f7" => 0x62,
        "f3" => 0x63,
        "f8" => 0x64,
        "f9" => 0x65,
        "f11" => 0x67,
        "f13" => 0x69,
        "f16" => 0x6A,
        "f14" => 0x6B,
        "f10" => 0x6D,
        "f12" => 0x6F,
        "f15" => 0x71,
        "insert" => 0x72,
        "home" => 0x73,
        "pageup" => 0x74,
        "delete" => 0x75,
        "f4" => 0x76,
        "end" => 0x77,
        "f2" => 0x78,
        "pagedown" => 0x79,
        "f1" => 0x7A,
        "left" => 0x7B,
        "right" => 0x7C,
        "down" => 0x7D,
        "up" => 0x7E,
        _ => return None,
    })
}

fn carbon_modifiers(chord: &ShortcutChord) -> u32 {
    let mut modifiers = 0;
    if chord.super_key {
        modifiers |= CMD_KEY;
    }
    if chord.shift {
        modifiers |= SHIFT_KEY;
    }
    if chord.alt {
        modifiers |= OPTION_KEY;
    }
    if chord.ctrl {
        modifiers |= CONTROL_KEY;
    }
    modifiers
}

fn outcome(id: u32, state: GlobalShortcutState, detail: Option<String>) -> HotkeyOutcome {
    HotkeyOutcome { id, state, detail }
}

/// 注册一个组合键；成功时返回守卫（调用方持有至注销）。
fn register_chord(
    binding_id: u32,
    chord: &ShortcutChord,
) -> (HotkeyOutcome, Option<ChordRegistration>) {
    let Some(code) = key_code(chord.key()) else {
        return (
            outcome(
                binding_id,
                GlobalShortcutState::Unsupported,
                Some(format!("no macOS key code for `{}`", chord.key())),
            ),
            None,
        );
    };
    let hot_key_id = EventHotKeyId {
        signature: HOT_KEY_SIGNATURE,
        id: binding_id,
    };
    let mut hot_key: EventHotKeyRef = std::ptr::null_mut();
    // SAFETY: `GetApplicationEventTarget` has no preconditions; the result (possibly NULL) is
    // only passed back to Carbon below.
    let target = unsafe { GetApplicationEventTarget() };
    // SAFETY: arguments are plain values; `hot_key` is a valid out pointer. A NULL target makes
    // Carbon return an error status, which is handled below.
    let status = unsafe {
        RegisterEventHotKey(
            code,
            carbon_modifiers(chord),
            hot_key_id,
            target,
            0,
            &mut hot_key,
        )
    };
    match status {
        NO_ERR if !hot_key.is_null() => (
            outcome(binding_id, GlobalShortcutState::Active, None),
            Some(ChordRegistration(hot_key)),
        ),
        EVENT_HOT_KEY_EXISTS_ERR => (outcome(binding_id, GlobalShortcutState::InUse, None), None),
        status => (
            outcome(
                binding_id,
                GlobalShortcutState::Failed,
                Some(format!("RegisterEventHotKey failed: OSStatus {status}")),
            ),
            None,
        ),
    }
}

/// 进程是否已获「辅助功能」授权；`prompt` 为真时未授权会弹出系统授权提示。
fn accessibility_trusted(prompt: bool) -> bool {
    if !prompt {
        // SAFETY: no preconditions.
        return unsafe { AXIsProcessTrusted() } != 0;
    }
    // `kAXTrustedCheckOptionPrompt` 的字符串值；选项字典以 CFBoolean（NSNumber 桥接）为值。
    let key = NSString::from_str("AXTrustedCheckOptionPrompt");
    // SAFETY: `numberWithBool:` is a class method taking one BOOL and returning an autoreleased
    // NSNumber (or nil), matching the declared `Option<Retained<AnyObject>>` result.
    let value: Option<Retained<AnyObject>> =
        unsafe { msg_send![class!(NSNumber), numberWithBool: Bool::YES] };
    let Some(value) = value else {
        tracing::warn!("NSNumber numberWithBool: returned nil; accessibility prompt skipped");
        // SAFETY: no preconditions.
        return unsafe { AXIsProcessTrusted() } != 0;
    };
    let options: Retained<NSDictionary<NSString, AnyObject>> =
        NSDictionary::from_retained_objects(&[&*key], &[value]);
    // SAFETY: `options` is a live NSDictionary, toll-free bridged to the CFDictionaryRef the API
    // expects, and stays alive until the call returns.
    unsafe { AXIsProcessTrustedWithOptions(Retained::as_ptr(&options).cast()) != 0 }
}

/// 修饰键当前按下状态，下标同 `ModifierKey::ALL`（Ctrl, Alt, Shift, Super）。
type Held = [bool; 4];

fn decode_flags(flags: NSEventModifierFlags) -> Held {
    [
        flags.contains(NSEventModifierFlags::Control),
        flags.contains(NSEventModifierFlags::Option),
        flags.contains(NSEventModifierFlags::Shift),
        flags.contains(NSEventModifierFlags::Command),
    ]
}

/// 两次修饰键状态之间的按下 / 松开事件（先松后按）。
fn flag_changes(previous: Held, next: Held) -> Vec<KeyInput> {
    let mut inputs = Vec::new();
    for (slot, modifier) in ModifierKey::ALL.into_iter().enumerate() {
        if previous[slot] && !next[slot] {
            inputs.push(KeyInput::ModifierUp(modifier));
        }
    }
    for (slot, modifier) in ModifierKey::ALL.into_iter().enumerate() {
        if !previous[slot] && next[slot] {
            inputs.push(KeyInput::ModifierDown(modifier));
        }
    }
    inputs
}

/// 全局监听回调与后端共享的双击判定状态（主线程内使用）。
struct DoubleTapState {
    detector: DoubleTapDetector,
    held: Held,
    /// 监听中的双击绑定（绑定 id、修饰键）。
    targets: Vec<(u32, ModifierKey)>,
    sink: Arc<dyn HotkeySink>,
}

impl DoubleTapState {
    fn feed(&mut self, input: KeyInput, now: Instant) {
        if let Some(modifier) = self.detector.feed(input, now) {
            for (id, target) in &self.targets {
                if *target == modifier {
                    self.sink.fired(*id);
                }
            }
        }
    }

    fn on_event(&mut self, kind: NSEventType, flags: NSEventModifierFlags, now: Instant) {
        if kind == NSEventType::KeyDown {
            self.feed(KeyInput::OtherDown, now);
        } else if kind == NSEventType::FlagsChanged {
            let next = decode_flags(flags);
            for input in flag_changes(self.held, next) {
                self.feed(input, now);
            }
            self.held = next;
        }
    }
}

/// 已安装的 `NSEvent` 全局监听；`Drop` 移除。
struct Monitor(Retained<AnyObject>);

impl Drop for Monitor {
    fn drop(&mut self) {
        // SAFETY: `self.0` is the token returned by `addGlobalMonitorForEventsMatchingMask:handler:`
        // and is removed exactly once, here.
        unsafe { NSEvent::removeMonitor(&self.0) };
    }
}

fn install_monitor(state: &Rc<RefCell<DoubleTapState>>) -> Option<Monitor> {
    let shared = Rc::clone(state);
    let handler = RcBlock::new(move |event: NonNull<NSEvent>| {
        // SAFETY: AppKit passes a valid NSEvent that outlives this callback invocation; the
        // reference is only used inside it.
        let event = unsafe { event.as_ref() };
        let (kind, flags) = (event.r#type(), event.modifierFlags());
        match shared.try_borrow_mut() {
            Ok(mut state) => state.on_event(kind, flags, Instant::now()),
            Err(_) => tracing::warn!("double-tap monitor re-entered; event dropped"),
        }
    });
    NSEvent::addGlobalMonitorForEventsMatchingMask_handler(
        NSEventMask::FlagsChanged | NSEventMask::KeyDown,
        &handler,
    )
    .map(Monitor)
}

/// 主线程上的全局热键后端。
pub(crate) struct Backend {
    sink: Arc<dyn HotkeySink>,
    /// Carbon 处理器安装失败时组合键全部报告失败，双击仍可用。
    carbon: Result<CarbonHandler, String>,
    chords: Vec<ChordRegistration>,
    double_tap: Rc<RefCell<DoubleTapState>>,
    monitor: Option<Monitor>,
    /// 已因双击绑定弹过系统授权提示。
    prompted: bool,
    /// 当前绑定表的最近一次结果（含等待授权的双击条目）。
    outcomes: Vec<HotkeyOutcome>,
    /// 等待「辅助功能」授权的双击绑定。
    awaiting_permission: Vec<(u32, ModifierKey)>,
    recheck_at: Option<Instant>,
}

impl Backend {
    /// `_main_thread` 证明调用线程是主线程（后端含 `!Send` 的 AppKit / Carbon 资源）。
    pub(crate) fn new(_main_thread: MainThreadMarker, sink: Arc<dyn HotkeySink>) -> Self {
        let carbon = CarbonHandler::install(Arc::clone(&sink));
        if let Err(detail) = &carbon {
            tracing::warn!(detail, "Carbon hotkey handler unavailable");
        }
        let double_tap = Rc::new(RefCell::new(DoubleTapState {
            detector: DoubleTapDetector::new(),
            held: [false; 4],
            targets: Vec::new(),
            sink: Arc::clone(&sink),
        }));
        Self {
            sink,
            carbon,
            chords: Vec::new(),
            double_tap,
            monitor: None,
            prompted: false,
            outcomes: Vec::new(),
            awaiting_permission: Vec::new(),
            recheck_at: None,
        }
    }

    /// 整表替换并回报。
    pub(crate) fn apply(&mut self, bindings: Vec<HotkeyBinding>) {
        self.chords.clear();
        self.outcomes.clear();
        let mut double_taps = Vec::new();
        for binding in bindings {
            match &binding.trigger {
                ShortcutTrigger::Chord(chord) => {
                    if let Err(detail) = &self.carbon {
                        self.outcomes.push(outcome(
                            binding.id,
                            GlobalShortcutState::Failed,
                            Some(detail.clone()),
                        ));
                        continue;
                    }
                    let (result, registration) = register_chord(binding.id, chord);
                    self.outcomes.push(result);
                    self.chords.extend(registration);
                }
                ShortcutTrigger::DoubleTap(modifier) => double_taps.push((binding.id, *modifier)),
            }
        }
        self.apply_double_taps(double_taps);
        self.report();
    }

    fn apply_double_taps(&mut self, targets: Vec<(u32, ModifierKey)>) {
        self.recheck_at = None;
        self.awaiting_permission.clear();
        if targets.is_empty() {
            self.monitor = None;
            self.set_targets(&targets);
            return;
        }
        let trusted = accessibility_trusted(!self.prompted);
        self.prompted = true;
        if trusted {
            self.activate(&targets);
            return;
        }
        self.monitor = None;
        self.set_targets(&[]);
        self.outcomes.extend(
            targets
                .iter()
                .map(|(id, _)| outcome(*id, GlobalShortcutState::PermissionRequired, None)),
        );
        self.awaiting_permission = targets;
        self.recheck_at = Some(Instant::now() + RECHECK_INTERVAL);
    }

    /// 已获授权：监听这些双击绑定（必要时安装全局监听）并记录结果。
    fn activate(&mut self, targets: &[(u32, ModifierKey)]) {
        self.set_targets(targets);
        if self.monitor.is_none() {
            self.reset_double_tap();
            self.monitor = install_monitor(&self.double_tap);
        }
        let (state, detail) = if self.monitor.is_some() {
            (GlobalShortcutState::Active, None)
        } else {
            (
                GlobalShortcutState::Failed,
                Some("NSEvent global monitor could not be installed".to_owned()),
            )
        };
        self.outcomes.extend(
            targets
                .iter()
                .map(|(id, _)| outcome(*id, state, detail.clone())),
        );
    }

    fn set_targets(&self, targets: &[(u32, ModifierKey)]) {
        match self.double_tap.try_borrow_mut() {
            Ok(mut state) => state.targets = targets.to_vec(),
            Err(_) => tracing::warn!("double-tap state busy; target update dropped"),
        }
    }

    /// 新装监听前以当前修饰键状态重置判定，避免沿用上次监听残留的按下记录。
    fn reset_double_tap(&self) {
        match self.double_tap.try_borrow_mut() {
            Ok(mut state) => {
                state.detector = DoubleTapDetector::new();
                state.held = decode_flags(NSEvent::modifierFlags_class());
            }
            Err(_) => tracing::warn!("double-tap state busy; reset dropped"),
        }
    }

    fn report(&self) {
        self.sink
            .report(GlobalShortcutBackend::MacOs, self.outcomes.clone());
    }

    /// 授权复查的下一个到期时间；没有待授权条目时为 `None`（宿主循环不设定时器）。
    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        self.recheck_at
    }

    /// 宿主定时器到期：授权就绪后安装双击监听并重新回报，否则继续等待。
    pub(crate) fn on_timer(&mut self, now: Instant) {
        let Some(deadline) = self.recheck_at else {
            return;
        };
        if now < deadline {
            return;
        }
        if !accessibility_trusted(false) {
            self.recheck_at = Some(now + RECHECK_INTERVAL);
            return;
        }
        self.recheck_at = None;
        let targets = std::mem::take(&mut self.awaiting_permission);
        self.outcomes
            .retain(|entry| !targets.iter().any(|(id, _)| *id == entry.id));
        self.activate(&targets);
        self.report();
    }
}

#[cfg(test)]
mod tests {
    use fluxdown_protocol::shortcuts::SHORTCUT_KEYS;

    use super::*;

    #[test]
    fn every_shortcut_key_has_a_unique_virtual_key_code() {
        let mut codes: Vec<u32> = SHORTCUT_KEYS
            .iter()
            .map(|key| key_code(key).unwrap_or_else(|| panic!("no key code for {key}")))
            .collect();
        codes.sort_unstable();
        let total = codes.len();
        codes.dedup();
        assert_eq!(codes.len(), total);
    }

    #[test]
    fn chord_modifiers_map_to_carbon_masks() {
        let chord = ShortcutChord::new(true, true, true, true, "d");
        let chord = chord.unwrap_or_else(|| panic!("valid chord"));
        assert_eq!(
            carbon_modifiers(&chord),
            CMD_KEY | SHIFT_KEY | OPTION_KEY | CONTROL_KEY
        );
        let plain = ShortcutChord::new(false, true, false, false, "d")
            .unwrap_or_else(|| panic!("valid chord"));
        assert_eq!(carbon_modifiers(&plain), OPTION_KEY);
    }

    #[test]
    fn flag_changes_emit_releases_before_presses() {
        assert_eq!(
            flag_changes([false; 4], [true, false, false, false]),
            [KeyInput::ModifierDown(ModifierKey::Ctrl)]
        );
        assert_eq!(
            flag_changes([true, false, false, false], [false; 4]),
            [KeyInput::ModifierUp(ModifierKey::Ctrl)]
        );
        assert_eq!(
            flag_changes([true, false, false, false], [false, false, false, true]),
            [
                KeyInput::ModifierUp(ModifierKey::Ctrl),
                KeyInput::ModifierDown(ModifierKey::Super)
            ]
        );
        assert!(flag_changes([true; 4], [true; 4]).is_empty());
    }

    #[test]
    fn decoded_flags_follow_modifier_order() {
        assert_eq!(
            decode_flags(NSEventModifierFlags::Control | NSEventModifierFlags::Command),
            [true, false, false, true]
        );
        assert_eq!(
            decode_flags(NSEventModifierFlags::Option | NSEventModifierFlags::Shift),
            [false, true, true, false]
        );
    }
}
