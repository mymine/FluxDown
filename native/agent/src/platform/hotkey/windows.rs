//! Windows 全局快捷键后端。
//!
//! 一条专用线程 `fluxdown-hotkeys` 跑 `GetMessageW` 循环：
//! - 组合键用 `RegisterHotKey(NULL, ..)`，`WM_HOTKEY` 投递到本线程队列；
//! - 双击修饰键用 `WH_KEYBOARD_LL` 低级键盘钩子，仅在存在双击绑定时安装；钩子回调永不吞键，
//!   状态放线程局部（钩子在安装线程的消息循环里回调），判定交给 [`DoubleTapDetector`]。
//!
//! [`HotkeyPort::apply`] 只把新表放进共享槽并 `PostThreadMessageW` 唤醒线程；注册 / 注销 /
//! 钩子增删与结果回报都在该线程内完成，所有 OS 资源由线程绑定的 RAII 守卫持有。

use std::cell::RefCell;
use std::marker::PhantomData;
use std::ptr;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use fluxdown_protocol::shortcuts::{
    GlobalShortcutBackend, GlobalShortcutState, ModifierKey, ShortcutChord, ShortcutTrigger,
};
use windows_sys::Win32::Foundation::{
    ERROR_HOTKEY_ALREADY_REGISTERED, GetLastError, LPARAM, LRESULT, POINT, WPARAM,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, RegisterHotKey, UnregisterHotKey,
    VK_BACK, VK_CONTROL, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_F1, VK_HOME, VK_INSERT,
    VK_LCONTROL, VK_LEFT, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_MENU, VK_NEXT, VK_OEM_1, VK_OEM_2,
    VK_OEM_3, VK_OEM_4, VK_OEM_5, VK_OEM_6, VK_OEM_7, VK_OEM_COMMA, VK_OEM_MINUS, VK_OEM_PERIOD,
    VK_OEM_PLUS, VK_PRIOR, VK_RCONTROL, VK_RETURN, VK_RIGHT, VK_RMENU, VK_RSHIFT, VK_RWIN,
    VK_SHIFT, VK_SPACE, VK_TAB, VK_UP,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, HC_ACTION, HHOOK, KBDLLHOOKSTRUCT, MSG, PM_NOREMOVE, PeekMessageW,
    PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_APP, WM_HOTKEY,
    WM_KEYDOWN, WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_USER,
};

use crate::hotkey::double_tap::{DoubleTapDetector, KeyInput};
use crate::hotkey::{HotkeyBinding, HotkeyOutcome, HotkeyPort, HotkeySink};

/// 唤醒热键线程应用新绑定表的线程消息。
const WM_APPLY: u32 = WM_APP + 1;
/// `RegisterHotKey` 对线程注册（hwnd = NULL）允许的 id 上限（0x0000..=0xBFFF）。
const MAX_REGISTRATION_ID: usize = 0xBFFF;
/// 等待热键线程建好消息队列的上限；线程启动只需微秒级。
const READY_TIMEOUT: Duration = Duration::from_secs(5);

const BACKEND: GlobalShortcutBackend = GlobalShortcutBackend::Windows;

/// 启动热键线程并返回端口；线程消息队列就绪后才返回。
pub(crate) fn start(sink: Arc<dyn HotkeySink>) -> Result<Box<dyn HotkeyPort>, String> {
    let pending = Arc::new(Mutex::new(None));
    let (ready_tx, ready_rx) = mpsc::sync_channel::<u32>(1);
    let thread_sink = Arc::clone(&sink);
    let thread_pending = Arc::clone(&pending);
    std::thread::Builder::new()
        .name("fluxdown-hotkeys".to_owned())
        .spawn(move || thread_main(thread_sink, thread_pending, ready_tx))
        .map_err(|error| format!("could not spawn the hotkey thread: {error}"))?;
    let thread_id = ready_rx
        .recv_timeout(READY_TIMEOUT)
        .map_err(|error| format!("the hotkey thread did not become ready: {error}"))?;
    Ok(Box::new(WindowsPort {
        thread_id,
        pending,
        sink,
    }))
}

type PendingTable = Arc<Mutex<Option<Vec<HotkeyBinding>>>>;

struct WindowsPort {
    thread_id: u32,
    pending: PendingTable,
    sink: Arc<dyn HotkeySink>,
}

impl HotkeyPort for WindowsPort {
    fn apply(&self, bindings: Vec<HotkeyBinding>) {
        // 只保留最新一版：线程醒来时取走最后写入的表。
        *self.pending.lock().unwrap_or_else(PoisonError::into_inner) = Some(bindings);
        if let Err(detail) = post_thread_message(self.thread_id, WM_APPLY) {
            tracing::error!(%detail, "could not hand the hotkey table to its thread");
            let stranded = self
                .pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take()
                .unwrap_or_default();
            let outcomes = stranded
                .iter()
                .map(|binding| failed(binding.id, detail.clone()))
                .collect();
            self.sink.report(BACKEND, outcomes);
        }
    }
}

impl Drop for WindowsPort {
    fn drop(&mut self) {
        if let Err(detail) = post_thread_message(self.thread_id, WM_QUIT) {
            tracing::debug!(%detail, "hotkey thread already stopped");
        }
    }
}

fn post_thread_message(thread_id: u32, message: u32) -> Result<(), String> {
    // SAFETY: 参数全是标量，不含指针；目标线程不存在或没有消息队列时仅返回 0。
    let posted = unsafe { PostThreadMessageW(thread_id, message, 0, 0) };
    if posted == 0 {
        Err(format!(
            "PostThreadMessageW failed (Win32 error {})",
            last_error()
        ))
    } else {
        Ok(())
    }
}

fn last_error() -> u32 {
    // SAFETY: GetLastError 无前置条件，只读调用线程的最近错误码。
    unsafe { GetLastError() }
}

fn failed(id: u32, detail: String) -> HotkeyOutcome {
    HotkeyOutcome {
        id,
        state: GlobalShortcutState::Failed,
        detail: Some(detail),
    }
}

fn outcome(id: u32, state: GlobalShortcutState) -> HotkeyOutcome {
    HotkeyOutcome {
        id,
        state,
        detail: None,
    }
}

// ───────────────────────── 热键线程 ─────────────────────────

fn thread_main(sink: Arc<dyn HotkeySink>, pending: PendingTable, ready: mpsc::SyncSender<u32>) {
    let mut msg = MSG {
        hwnd: ptr::null_mut(),
        message: 0,
        wParam: 0,
        lParam: 0,
        time: 0,
        pt: POINT { x: 0, y: 0 },
    };
    // SAFETY: msg 是有效的可写输出槽；hwnd = NULL 表示本线程队列，过滤范围取一个空闲的
    // WM_USER，PM_NOREMOVE 不取走任何消息。该调用的目的是为本线程创建消息队列，
    // 使 PostThreadMessageW 可达。
    let has_message =
        unsafe { PeekMessageW(&mut msg, ptr::null_mut(), WM_USER, WM_USER, PM_NOREMOVE) };
    if has_message != 0 {
        tracing::trace!("hotkey thread queue already holds a message");
    }
    // SAFETY: GetCurrentThreadId 无前置条件。
    let thread_id = unsafe { GetCurrentThreadId() };
    if ready.send(thread_id).is_err() {
        tracing::warn!("hotkey backend start was abandoned before the thread became ready");
        return;
    }

    let mut engine = Engine::new(sink);
    loop {
        // SAFETY: msg 是有效的可写输出槽；hwnd = NULL 取本线程所有消息。
        let status = unsafe { GetMessageW(&mut msg, ptr::null_mut(), 0, 0) };
        match status {
            0 => break,
            -1 => {
                let code = last_error();
                tracing::error!(code, "hotkey message loop failed");
                engine.fail_all(format!("hotkey message loop failed (Win32 error {code})"));
                break;
            }
            _ => match msg.message {
                WM_HOTKEY => engine.fire(msg.wParam),
                WM_APPLY => {
                    let next = pending
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .take();
                    if let Some(bindings) = next {
                        engine.apply(bindings);
                    }
                }
                _ => {}
            },
        }
    }
    tracing::debug!("hotkey thread stopped");
}

/// 线程内的注册状态；字段声明顺序即释放顺序（先注销热键，再卸钩子）。
struct Engine {
    sink: Arc<dyn HotkeySink>,
    hotkeys: Vec<RegisteredHotkey>,
    hook: Option<HookGuard>,
    /// 当前表的绑定 id，供循环异常退出时统一回报失败。
    ids: Vec<u32>,
}

impl Engine {
    fn new(sink: Arc<dyn HotkeySink>) -> Self {
        Self {
            sink,
            hotkeys: Vec::new(),
            hook: None,
            ids: Vec::new(),
        }
    }

    fn apply(&mut self, bindings: Vec<HotkeyBinding>) {
        // 整表替换：旧热键先全部注销（守卫 Drop）。
        self.hotkeys.clear();
        self.ids = bindings.iter().map(|binding| binding.id).collect();
        let mut outcomes = Vec::with_capacity(bindings.len());
        let mut taps = Vec::new();
        for (index, binding) in bindings.iter().enumerate() {
            match &binding.trigger {
                ShortcutTrigger::Chord(chord) => {
                    outcomes.push(self.register_chord(index, binding.id, chord));
                }
                ShortcutTrigger::DoubleTap(modifier) => taps.push((*modifier, binding.id)),
            }
        }
        let tap_ids: Vec<u32> = taps.iter().map(|(_, id)| *id).collect();
        match self.sync_hook(taps) {
            Ok(()) => outcomes.extend(
                tap_ids
                    .into_iter()
                    .map(|id| outcome(id, GlobalShortcutState::Active)),
            ),
            Err(detail) => {
                tracing::warn!(%detail, "double-tap keyboard hook unavailable");
                outcomes.extend(tap_ids.into_iter().map(|id| failed(id, detail.clone())));
            }
        }
        self.sink.report(BACKEND, outcomes);
    }

    fn register_chord(&mut self, index: usize, id: u32, chord: &ShortcutChord) -> HotkeyOutcome {
        let Some(vk) = vk_for_key(chord.key()) else {
            return outcome(id, GlobalShortcutState::Unsupported);
        };
        let registration = (index < MAX_REGISTRATION_ID)
            .then(|| i32::try_from(index + 1).ok())
            .flatten();
        let Some(registration) = registration else {
            return failed(id, "too many global shortcuts".to_owned());
        };
        let modifiers = chord_modifiers(chord) | MOD_NOREPEAT;
        // SAFETY: hwnd = NULL 把热键注册到调用线程（消息循环所在线程），参数全是标量；
        // 成功后立即交给线程绑定的 RegisteredHotkey 守卫，在本线程注销。
        let registered =
            unsafe { RegisterHotKey(ptr::null_mut(), registration, modifiers, u32::from(vk)) };
        if registered == 0 {
            let code = last_error();
            return if code == ERROR_HOTKEY_ALREADY_REGISTERED {
                outcome(id, GlobalShortcutState::InUse)
            } else {
                failed(id, format!("RegisterHotKey failed (Win32 error {code})"))
            };
        }
        self.hotkeys.push(RegisteredHotkey {
            registration,
            binding: id,
            thread: PhantomData,
        });
        outcome(id, GlobalShortcutState::Active)
    }

    /// 存在双击绑定时保证钩子已装并更新目标表；否则卸钩子。
    fn sync_hook(&mut self, taps: Vec<(ModifierKey, u32)>) -> Result<(), String> {
        if taps.is_empty() {
            self.hook = None;
            return Ok(());
        }
        match &self.hook {
            Some(hook) => hook.retarget(taps),
            None => self.hook = Some(HookGuard::install(Arc::clone(&self.sink), taps)?),
        }
        Ok(())
    }

    fn fire(&self, registration: WPARAM) {
        let hit = self
            .hotkeys
            .iter()
            .find(|hotkey| usize::try_from(hotkey.registration).is_ok_and(|id| id == registration));
        match hit {
            Some(hotkey) => self.sink.fired(hotkey.binding),
            None => tracing::debug!(registration, "WM_HOTKEY for an unknown registration"),
        }
    }

    /// 消息循环异常退出：整表标记失败，资源随 Engine 释放。
    fn fail_all(&self, detail: String) {
        let outcomes = self
            .ids
            .iter()
            .map(|id| failed(*id, detail.clone()))
            .collect();
        self.sink.report(BACKEND, outcomes);
    }
}

/// 已注册的线程热键；Drop 时在注册线程注销。`PhantomData<*const ()>` 使其 `!Send + !Sync`。
struct RegisteredHotkey {
    registration: i32,
    binding: u32,
    thread: PhantomData<*const ()>,
}

impl Drop for RegisteredHotkey {
    fn drop(&mut self) {
        // SAFETY: 该 id 由同一线程的 RegisterHotKey(NULL, id, ..) 成功注册且只注销一次；
        // 守卫 !Send，Drop 必然发生在注册线程。
        let removed = unsafe { UnregisterHotKey(ptr::null_mut(), self.registration) };
        if removed == 0 {
            tracing::warn!(
                registration = self.registration,
                code = last_error(),
                "UnregisterHotKey failed"
            );
        }
    }
}

fn chord_modifiers(chord: &ShortcutChord) -> u32 {
    let mut modifiers = 0;
    if chord.ctrl {
        modifiers |= MOD_CONTROL;
    }
    if chord.alt {
        modifiers |= MOD_ALT;
    }
    if chord.shift {
        modifiers |= MOD_SHIFT;
    }
    if chord.super_key {
        modifiers |= MOD_WIN;
    }
    modifiers
}

/// 规范主键名 → 虚拟键码；覆盖 `SHORTCUT_KEYS` 全部条目，未知返回 `None`。
fn vk_for_key(key: &str) -> Option<u16> {
    let bytes = key.as_bytes();
    if let [byte @ b'a'..=b'z'] = bytes {
        return Some(u16::from(byte.to_ascii_uppercase()));
    }
    if let [byte @ b'0'..=b'9'] = bytes {
        return Some(u16::from(*byte));
    }
    if let Some(number) = key.strip_prefix('f').and_then(|n| n.parse::<u16>().ok())
        && (1..=24).contains(&number)
    {
        return Some(VK_F1 + number - 1);
    }
    Some(match key {
        "space" => VK_SPACE,
        "enter" => VK_RETURN,
        "tab" => VK_TAB,
        "escape" => VK_ESCAPE,
        "backspace" => VK_BACK,
        "delete" => VK_DELETE,
        "insert" => VK_INSERT,
        "home" => VK_HOME,
        "end" => VK_END,
        "pageup" => VK_PRIOR,
        "pagedown" => VK_NEXT,
        "up" => VK_UP,
        "down" => VK_DOWN,
        "left" => VK_LEFT,
        "right" => VK_RIGHT,
        "-" => VK_OEM_MINUS,
        "=" => VK_OEM_PLUS,
        "[" => VK_OEM_4,
        "]" => VK_OEM_6,
        "\\" => VK_OEM_5,
        ";" => VK_OEM_1,
        "'" => VK_OEM_7,
        "," => VK_OEM_COMMA,
        "." => VK_OEM_PERIOD,
        "/" => VK_OEM_2,
        "`" => VK_OEM_3,
        _ => return None,
    })
}

// ───────────────────────── 双击修饰键：低级键盘钩子 ─────────────────────────

/// 钩子回调里的判定状态（纯数据，便于测试）。
struct TapState {
    detector: DoubleTapDetector,
    /// 每个修饰键当前按住的物理键位（左 / 右 / 通用各一位），全部松开才送出 `ModifierUp`。
    pressed: [u8; 4],
    /// 双击目标：修饰键 → 绑定 id。
    targets: Vec<(ModifierKey, u32)>,
}

impl TapState {
    fn new(targets: Vec<(ModifierKey, u32)>) -> Self {
        Self {
            detector: DoubleTapDetector::new(),
            pressed: [0; 4],
            targets,
        }
    }

    /// 喂入一次按键事件，返回应触发的绑定 id（通常为空，不分配）。
    fn feed(&mut self, vk: u32, down: bool, now: Instant) -> Vec<u32> {
        let input = match modifier_of(vk) {
            Some((modifier, bit)) => {
                let slot = &mut self.pressed[slot_index(modifier)];
                if down {
                    *slot |= bit;
                    KeyInput::ModifierDown(modifier)
                } else {
                    *slot &= !bit;
                    if *slot != 0 {
                        return Vec::new();
                    }
                    KeyInput::ModifierUp(modifier)
                }
            }
            None if down => KeyInput::OtherDown,
            None => return Vec::new(),
        };
        match self.detector.feed(input, now) {
            Some(modifier) => self
                .targets
                .iter()
                .filter(|(target, _)| *target == modifier)
                .map(|(_, id)| *id)
                .collect(),
            None => Vec::new(),
        }
    }
}

fn slot_index(modifier: ModifierKey) -> usize {
    match modifier {
        ModifierKey::Ctrl => 0,
        ModifierKey::Alt => 1,
        ModifierKey::Shift => 2,
        ModifierKey::Super => 3,
    }
}

/// 虚拟键码 → (修饰键, 物理键位)；左右两侧与通用码映射到同一修饰键。
fn modifier_of(vk: u32) -> Option<(ModifierKey, u8)> {
    Some(match u16::try_from(vk).ok()? {
        VK_LCONTROL => (ModifierKey::Ctrl, 0b001),
        VK_RCONTROL => (ModifierKey::Ctrl, 0b010),
        VK_CONTROL => (ModifierKey::Ctrl, 0b100),
        VK_LMENU => (ModifierKey::Alt, 0b001),
        VK_RMENU => (ModifierKey::Alt, 0b010),
        VK_MENU => (ModifierKey::Alt, 0b100),
        VK_LSHIFT => (ModifierKey::Shift, 0b001),
        VK_RSHIFT => (ModifierKey::Shift, 0b010),
        VK_SHIFT => (ModifierKey::Shift, 0b100),
        VK_LWIN => (ModifierKey::Super, 0b001),
        VK_RWIN => (ModifierKey::Super, 0b010),
        _ => return None,
    })
}

thread_local! {
    /// 钩子回调没有用户数据：状态放在安装钩子的线程上（回调就在该线程的消息循环里执行）。
    static TAP: RefCell<Option<(Arc<dyn HotkeySink>, TapState)>> = const { RefCell::new(None) };
}

/// 已安装的 `WH_KEYBOARD_LL` 钩子；Drop 时卸载并清空线程局部状态。
/// `PhantomData<*const ()>` 使其 `!Send + !Sync`：钩子与 [`TAP`] 都绑定安装线程。
struct HookGuard {
    handle: HHOOK,
    thread: PhantomData<*const ()>,
}

impl HookGuard {
    fn install(
        sink: Arc<dyn HotkeySink>,
        targets: Vec<(ModifierKey, u32)>,
    ) -> Result<Self, String> {
        // SAFETY: lpModuleName = NULL 取当前进程可执行文件的模块句柄，无前置条件；
        // 返回的句柄不增加引用计数，也不需要释放。
        let module = unsafe { GetModuleHandleW(ptr::null()) };
        if module.is_null() {
            return Err(format!(
                "GetModuleHandleW failed (Win32 error {})",
                last_error()
            ));
        }
        // 先放好状态，避免钩子生效后到状态就位前丢事件。
        TAP.replace(Some((sink, TapState::new(targets))));
        // SAFETY: keyboard_hook 与 HOOKPROC 签名一致且为 'static；WH_KEYBOARD_LL 钩子的
        // hMod 取本进程模块、dwThreadId = 0（全局低级钩子，回调在本线程消息循环中执行）。
        let handle = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), module, 0) };
        if handle.is_null() {
            let code = last_error();
            TAP.replace(None);
            return Err(format!("SetWindowsHookExW failed (Win32 error {code})"));
        }
        Ok(Self {
            handle,
            thread: PhantomData,
        })
    }

    fn retarget(&self, targets: Vec<(ModifierKey, u32)>) {
        TAP.with_borrow_mut(|slot| {
            if let Some((_, state)) = slot {
                state.targets = targets;
            }
        });
    }
}

impl Drop for HookGuard {
    fn drop(&mut self) {
        // SAFETY: handle 来自本线程成功的 SetWindowsHookExW，仅此守卫拥有且只卸载一次。
        let removed = unsafe { UnhookWindowsHookEx(self.handle) };
        if removed == 0 {
            tracing::warn!(code = last_error(), "UnhookWindowsHookEx failed");
        }
        TAP.replace(None);
    }
}

/// `WH_KEYBOARD_LL` 回调：只观察，永远交给下一个钩子，不吞键。
///
/// # Safety
///
/// 仅由系统作为 `HOOKPROC` 调用：`code >= 0` 且为 `HC_ACTION` 时 `lparam` 指向在本次回调期间
/// 有效的 `KBDLLHOOKSTRUCT`。
unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if u32::try_from(code).is_ok_and(|code| code == HC_ACTION) {
        let event = lparam as *const KBDLLHOOKSTRUCT;
        // SAFETY: HC_ACTION 时系统保证 lparam 指向回调期间有效的 KBDLLHOOKSTRUCT；
        // 引用只在本语句内借用，随即复制出 vkCode。
        let vk = unsafe { event.as_ref() }.map(|event| event.vkCode);
        if let (Some(vk), Ok(message)) = (vk, u32::try_from(wparam)) {
            on_key_event(message, vk);
        }
    }
    // SAFETY: 原样转发系统传入的参数；hhk 参数在现代 Windows 上被忽略，可传 NULL。
    unsafe { CallNextHookEx(ptr::null_mut(), code, wparam, lparam) }
}

fn on_key_event(message: u32, vk: u32) {
    let down = match message {
        WM_KEYDOWN | WM_SYSKEYDOWN => true,
        WM_KEYUP | WM_SYSKEYUP => false,
        _ => return,
    };
    let fired = TAP.with(|cell| {
        let Ok(mut slot) = cell.try_borrow_mut() else {
            tracing::warn!("double-tap state is already borrowed");
            return None;
        };
        let (sink, state) = slot.as_mut()?;
        let ids = state.feed(vk, down, Instant::now());
        (!ids.is_empty()).then(|| (Arc::clone(sink), ids))
    });
    // 在释放借用之后再通知，避免回调重入线程局部状态。
    if let Some((sink, ids)) = fired {
        for id in ids {
            sink.fired(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use fluxdown_protocol::shortcuts::SHORTCUT_KEYS;

    use super::*;

    #[test]
    fn every_shortcut_key_has_a_virtual_key() {
        let mut seen = HashSet::new();
        for key in SHORTCUT_KEYS {
            let vk = vk_for_key(key).unwrap_or_else(|| panic!("no virtual key for {key}"));
            assert!(seen.insert(vk), "duplicate virtual key for {key}");
        }
        assert_eq!(vk_for_key("a"), Some(0x41));
        assert_eq!(vk_for_key("0"), Some(0x30));
        assert_eq!(vk_for_key("f1"), Some(0x70));
        assert_eq!(vk_for_key("f20"), Some(0x83));
        assert_eq!(vk_for_key("pageup"), Some(0x21));
        assert_eq!(vk_for_key("`"), Some(0xC0));
        assert_eq!(vk_for_key("unknown"), None);
        assert_eq!(vk_for_key("f25"), None);
    }

    #[test]
    fn chord_modifiers_map_to_mod_flags() {
        let chord = ShortcutChord::new(true, false, true, true, "k").expect("valid chord");
        assert_eq!(chord_modifiers(&chord), MOD_CONTROL | MOD_SHIFT | MOD_WIN);
    }

    fn tap(state: &mut TapState, vk: u16, down: bool, at: Instant) -> Vec<u32> {
        state.feed(u32::from(vk), down, at)
    }

    #[test]
    fn double_tap_fires_every_target_of_the_modifier() {
        let mut state = TapState::new(vec![
            (ModifierKey::Ctrl, 1),
            (ModifierKey::Alt, 2),
            (ModifierKey::Ctrl, 3),
        ]);
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        assert!(tap(&mut state, VK_LCONTROL, true, t0).is_empty());
        assert!(tap(&mut state, VK_LCONTROL, false, t0 + ms(40)).is_empty());
        // 第二次用右 Ctrl：左右视为同一修饰键。
        assert!(tap(&mut state, VK_RCONTROL, true, t0 + ms(120)).is_empty());
        assert_eq!(tap(&mut state, VK_RCONTROL, false, t0 + ms(160)), [1, 3]);
    }

    #[test]
    fn other_key_between_taps_cancels() {
        let mut state = TapState::new(vec![(ModifierKey::Ctrl, 1)]);
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        tap(&mut state, VK_LCONTROL, true, t0);
        tap(&mut state, VK_LCONTROL, false, t0 + ms(30));
        tap(&mut state, 0x41, true, t0 + ms(60));
        tap(&mut state, 0x41, false, t0 + ms(80));
        tap(&mut state, VK_LCONTROL, true, t0 + ms(100));
        assert!(tap(&mut state, VK_LCONTROL, false, t0 + ms(130)).is_empty());
    }

    #[test]
    fn both_sides_must_be_released_before_modifier_up() {
        let mut state = TapState::new(vec![(ModifierKey::Shift, 7)]);
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        tap(&mut state, VK_LSHIFT, true, t0);
        tap(&mut state, VK_RSHIFT, true, t0 + ms(10));
        tap(&mut state, VK_LSHIFT, false, t0 + ms(20));
        // 右 Shift 仍按住：不应被当成松开。
        assert_eq!(state.pressed[slot_index(ModifierKey::Shift)], 0b010);
        tap(&mut state, VK_RSHIFT, false, t0 + ms(30));
        assert_eq!(state.pressed[slot_index(ModifierKey::Shift)], 0);
    }
}
