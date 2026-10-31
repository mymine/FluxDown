//! Windows：桌面进程已有窗口的 `WM_SETICON`（跨进程）与 `.lnk` 的 `IconLocation` 改写。
//!
//! # 线程与所有权
//!
//! 所有 FFI 都在一条常驻工作线程上串行执行（惰性创建，空闲时阻塞在通道上，没有定时器）：
//! - 线程内持有 STA COM 守卫（[`Com`]，`!Send`），`.lnk` 的 COM 对象都在其生命周期内创建 / 释放；
//! - `HICON` 由 [`OwnedIcon`]（`!Send`）独占，随线程存在。窗口只引用句柄、不拥有它，因此旧图标只在
//!   新图标已成功设到全部窗口之后才销毁；有窗口没更新成功时旧图标保留到下一次完整成功。
//!
//! # 识别
//!
//! 窗口：类名恰为 `GpuiFast::Window` 且所属进程映像（`QueryFullProcessImageNameW`）等于同目录桌面程序。
//! 快捷方式：解析出的目标等于桌面程序，PIDL-only 固定项（`GetPath` 为空）一律跳过。不接受调用方
//! 传入句柄 / PID / 路径。

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::mpsc;

use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree, IPersistFile, STGM_READ,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows::Win32::UI::Shell::{
    FOLDERID_Desktop, FOLDERID_Programs, FOLDERID_QuickLaunch, IShellLinkW, KF_FLAG_DEFAULT,
    SHCNE_ASSOCCHANGED, SHCNE_UPDATEITEM, SHCNF_IDLIST, SHCNF_PATHW, SHChangeNotify,
    SHGetKnownFolderPath, ShellLink,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyIcon, EnumWindows, GetClassNameW, GetWindowThreadProcessId, HICON, ICON_BIG, ICON_SMALL,
    IMAGE_ICON, LR_LOADFROMFILE, LoadImageW, SM_CXSMICON, SMTO_ABORTIFHUNG, SendMessageTimeoutW,
    WM_SETICON,
};
use windows::core::{BOOL, GUID, IUnknown, Interface, PCWSTR, PWSTR};

use super::lnk_policy;
use crate::platform::windows_com::Com;

/// GPUI（gpui-fast）注册的顶层窗口类名。
const WINDOW_CLASS: &str = "GpuiFast::Window";
/// 单个窗口处理 `WM_SETICON` 的等待上限；挂起的窗口跳过而不是拖住 agent。
const SEND_TIMEOUT_MS: u32 = 1000;
/// `ICON_BIG` 取 256px：16 / 32px 在 DPI 缩放下会被任务栏忽略。
const BIG_ICON_PX: i32 = 256;
const FALLBACK_SMALL_ICON_PX: i32 = 16;
const BASE_DPI: u32 = 96;
const MAX_PATH_CHARS: usize = 1024;
/// `GetWindowsClassName` 缓冲：类名远短于此，超长的必然不是目标类。
const CLASS_NAME_CHARS: usize = 64;

enum Job {
    Windows {
        ico: Option<PathBuf>,
        reply: mpsc::SyncSender<Result<(), String>>,
    },
    Shortcuts {
        ico: Option<PathBuf>,
        icons_dir: PathBuf,
        reply: mpsc::SyncSender<Result<(), String>>,
    },
}

static WORKER: OnceLock<Result<mpsc::Sender<Job>, String>> = OnceLock::new();

fn worker() -> Result<&'static mpsc::Sender<Job>, String> {
    WORKER
        .get_or_init(|| {
            let (sender, jobs) = mpsc::channel();
            std::thread::Builder::new()
                .name("fluxdown-app-icon".to_owned())
                .spawn(move || run_worker(&jobs))
                .map(|_| sender)
                .map_err(|error| format!("could not start app icon worker: {error}"))
        })
        .as_ref()
        .map_err(Clone::clone)
}

fn request(make: impl FnOnce(mpsc::SyncSender<Result<(), String>>) -> Job) -> Result<(), String> {
    let sender = worker()?;
    let (reply, answer) = mpsc::sync_channel(1);
    sender
        .send(make(reply))
        .map_err(|_| "app icon worker stopped".to_owned())?;
    answer
        .recv()
        .map_err(|_| "app icon worker stopped".to_owned())?
}

/// 给桌面进程的所有窗口设置（`Some`）或还原（`None`）图标。
pub(super) fn sync_windows(ico: Option<&Path>) -> Result<(), String> {
    request(|reply| Job::Windows {
        ico: ico.map(Path::to_path_buf),
        reply,
    })
}

/// 改写目标为桌面程序的 `.lnk` 的 `IconLocation`：`Some` = 该 `.ico`，`None` = 清除本应用写入的图标。
pub(super) fn sync_shortcuts(ico: Option<&Path>, icons_dir: &Path) -> Result<(), String> {
    request(|reply| Job::Shortcuts {
        ico: ico.map(Path::to_path_buf),
        icons_dir: icons_dir.to_path_buf(),
        reply,
    })
}

fn run_worker(jobs: &mpsc::Receiver<Job>) {
    let mut icons = WindowIcons::default();
    let com = Com::init();
    while let Ok(job) = jobs.recv() {
        let (result, reply) = match job {
            Job::Windows { ico, reply } => (icons.sync(ico.as_deref()), reply),
            Job::Shortcuts {
                ico,
                icons_dir,
                reply,
            } => {
                let result = match &com {
                    Ok(_) => sweep_shortcuts(ico.as_deref(), &icons_dir),
                    Err(error) => Err(error.clone()),
                };
                (result, reply)
            }
        };
        if reply.send(result).is_err() {
            tracing::debug!("app icon requester went away before the result was ready");
        }
    }
}

// ---------------------------------------------------------------------------
// 窗口图标
// ---------------------------------------------------------------------------

/// 独占的 `HICON`（`LoadImageW` 成功返回的句柄），析构时只销毁这一次。
struct OwnedIcon(HICON);

impl OwnedIcon {
    fn load(path: &Path, px: i32) -> Result<Self, String> {
        let wide = to_wide(path);
        // SAFETY: `wide` 以 NUL 结尾且活过本次调用；hinst 为 None + LR_LOADFROMFILE 表示从文件加载，
        // 返回的新图标句柄归调用方所有。
        let loaded = unsafe {
            LoadImageW(
                None,
                PCWSTR(wide.as_ptr()),
                IMAGE_ICON,
                px,
                px,
                LR_LOADFROMFILE,
            )
        };
        loaded
            .map(|handle| Self(HICON(handle.0)))
            .map_err(|error| format!("LoadImageW({}, {px}px) failed: {error}", path.display()))
    }
}

impl Drop for OwnedIcon {
    fn drop(&mut self) {
        // SAFETY: `self.0` 是 `LoadImageW` 返回、由本结构独占所有的图标句柄，只在这里销毁一次。
        let destroyed = unsafe { DestroyIcon(self.0) };
        if let Err(error) = destroyed {
            tracing::debug!(%error, "could not destroy app icon handle");
        }
    }
}

/// 已设到窗口上的图标集合（按边长缓存）及因窗口更新失败而暂不能销毁的旧图标。
#[derive(Default)]
struct WindowIcons {
    ico: Option<PathBuf>,
    loaded: HashMap<i32, OwnedIcon>,
    retired: Vec<OwnedIcon>,
}

impl WindowIcons {
    fn sync(&mut self, ico: Option<&Path>) -> Result<(), String> {
        let Some(desktop_exe) = desktop_exe_string() else {
            tracing::debug!("desktop executable not found; skipping window icon sync");
            return Ok(());
        };
        let windows = desktop_windows(&desktop_exe)?;
        if windows.is_empty() {
            // 没有任何窗口引用图标句柄：旧图标可以全部销毁。
            self.ico = ico.map(Path::to_path_buf);
            self.loaded.clear();
            self.retired.clear();
            return Ok(());
        }
        let mut failures = Vec::new();
        match ico {
            None => {
                for &window in &windows {
                    for which in [ICON_SMALL, ICON_BIG] {
                        if let Err(error) = set_window_icon(window, which, None) {
                            failures.push(error);
                        }
                    }
                }
                self.release(failures.is_empty());
                self.ico = None;
            }
            Some(path) => {
                if self.ico.as_deref() != Some(path) {
                    self.retired
                        .extend(self.loaded.drain().map(|(_, icon)| icon));
                    self.ico = Some(path.to_path_buf());
                }
                for &window in &windows {
                    let small = self.icon(path, small_icon_px(window))?;
                    let big = self.icon(path, BIG_ICON_PX)?;
                    for (which, icon) in [(ICON_SMALL, small), (ICON_BIG, big)] {
                        if let Err(error) = set_window_icon(window, which, Some(icon)) {
                            failures.push(error);
                        }
                    }
                }
                if failures.is_empty() {
                    self.retired.clear();
                }
            }
        }
        match failures.into_iter().next() {
            None => Ok(()),
            Some(first) => Err(first),
        }
    }

    /// 还原默认后释放：全部窗口都已清除才销毁，否则保留给下一次完整成功。
    fn release(&mut self, all_updated: bool) {
        if all_updated {
            self.loaded.clear();
            self.retired.clear();
        } else {
            self.retired
                .extend(self.loaded.drain().map(|(_, icon)| icon));
        }
    }

    fn icon(&mut self, path: &Path, px: i32) -> Result<HICON, String> {
        let icon = match self.loaded.entry(px) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => entry.insert(OwnedIcon::load(path, px)?),
        };
        Ok(icon.0)
    }
}

/// `which` = `ICON_SMALL` / `ICON_BIG`；`icon = None` 让窗口回落到窗口类图标（exe 资源）。
fn set_window_icon(window: HWND, which: u32, icon: Option<HICON>) -> Result<(), String> {
    let wparam = WPARAM(which as usize);
    let lparam = LPARAM(icon.map_or(0, |icon| icon.0 as isize));
    let mut previous = 0usize;
    // SAFETY: `window` 来自刚做的 EnumWindows（句柄失效只会让调用失败）；WM_SETICON 的 lparam 要么为 0，
    // 要么是仍由调用方 `OwnedIcon` 持有的有效 HICON；SMTO_ABORTIFHUNG + 超时保证不会无限阻塞；
    // `previous` 是有效的输出缓冲。
    let sent = unsafe {
        SendMessageTimeoutW(
            window,
            WM_SETICON,
            wparam,
            lparam,
            SMTO_ABORTIFHUNG,
            SEND_TIMEOUT_MS,
            Some(&mut previous),
        )
    };
    // 函数返回值为 0 才是失败 / 超时；消息自身的结果（旧图标）可以合法地为 0，在 `previous` 里。
    if sent.0 == 0 {
        return Err(format!(
            "WM_SETICON({which}) failed or timed out: {}",
            windows::core::Error::from_win32()
        ));
    }
    Ok(())
}

/// 窗口当前 DPI 下小图标的边长。
fn small_icon_px(window: HWND) -> i32 {
    // SAFETY: 无效窗口句柄只会得到 0。
    let dpi = unsafe { GetDpiForWindow(window) };
    let dpi = if dpi == 0 { BASE_DPI } else { dpi };
    // SAFETY: 纯查询，无指针参数。
    let px = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, dpi) };
    if px > 0 { px } else { FALLBACK_SMALL_ICON_PX }
}

/// 桌面进程拥有的顶层窗口：类名精确匹配，且进程映像等于桌面程序。
fn desktop_windows(desktop_exe: &str) -> Result<Vec<HWND>, String> {
    let mut verdicts: HashMap<u32, bool> = HashMap::new();
    let mut owned = Vec::new();
    for (window, pid) in enumerate_class_windows()? {
        let is_desktop = *verdicts.entry(pid).or_insert_with(|| {
            process_image(pid).is_some_and(|image| {
                lnk_policy::same_path(&canonical_string(Path::new(&image)), desktop_exe)
            })
        });
        if is_desktop {
            owned.push(window);
        }
    }
    Ok(owned)
}

/// 所有类名为 [`WINDOW_CLASS`] 的顶层窗口及其所属进程 id。
fn enumerate_class_windows() -> Result<Vec<(HWND, u32)>, String> {
    let mut found: Vec<(HWND, u32)> = Vec::new();
    let lparam = LPARAM(std::ptr::from_mut(&mut found) as isize);
    // SAFETY: `lparam` 是 `found` 的地址，`found` 在 EnumWindows 返回前一直存活且只经回调独占借用；
    // 回调在本线程同步执行。
    let result = unsafe { EnumWindows(Some(collect_class_window), lparam) };
    result.map_err(|error| format!("EnumWindows failed: {error}"))?;
    Ok(found)
}

unsafe extern "system" fn collect_class_window(window: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: `lparam` 由 `enumerate_class_windows` 传入，是仍存活的 `Vec<(HWND, u32)>` 的唯一借用。
    let found = unsafe { &mut *(lparam.0 as *mut Vec<(HWND, u32)>) };
    let mut class = [0u16; CLASS_NAME_CHARS];
    // SAFETY: `class` 是有效的可写缓冲；窗口句柄失效只会返回 0。
    let len = unsafe { GetClassNameW(window, &mut class) };
    let matches = usize::try_from(len)
        .ok()
        .and_then(|len| class.get(..len))
        .is_some_and(|name| String::from_utf16_lossy(name) == WINDOW_CLASS);
    if matches {
        let mut pid = 0u32;
        // SAFETY: `pid` 是有效的输出变量。
        let thread = unsafe { GetWindowThreadProcessId(window, Some(&mut pid)) };
        if thread != 0 && pid != 0 {
            found.push((window, pid));
        }
    }
    BOOL::from(true)
}

struct ProcessHandle(HANDLE);

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        // SAFETY: `self.0` 是 `OpenProcess` 返回、由本结构独占的进程句柄，只关闭这一次。
        let closed = unsafe { CloseHandle(self.0) };
        if let Err(error) = closed {
            tracing::debug!(%error, "could not close process handle");
        }
    }
}

/// 进程的完整映像路径；无权限或进程已退出返回 `None`。
fn process_image(pid: u32) -> Option<String> {
    // SAFETY: 只请求 PROCESS_QUERY_LIMITED_INFORMATION；成功返回的句柄立刻交给 `ProcessHandle`。
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let handle = ProcessHandle(handle);
    let mut buffer = vec![0u16; 32_768];
    let mut len = u32::try_from(buffer.len()).ok()?;
    // SAFETY: `buffer` 可写且容量等于 `len`；`handle` 在调用期间有效。
    let queried = unsafe {
        QueryFullProcessImageNameW(
            handle.0,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut len,
        )
    };
    queried.ok()?;
    buffer
        .get(..usize::try_from(len).ok()?)
        .map(String::from_utf16_lossy)
}

fn desktop_exe_string() -> Option<String> {
    crate::platform::desktop_executable().map(|path| canonical_string(&path))
}

/// 规范化路径文本（解析符号链接 / 短名）；路径不存在时原样返回。
fn canonical_string(path: &Path) -> String {
    std::fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

pub(super) fn to_wide(path: &Path) -> Vec<u16> {
    OsStr::new(path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn wide_to_string(buffer: &[u16]) -> String {
    let len = buffer
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(buffer.len());
    buffer
        .get(..len)
        .map(String::from_utf16_lossy)
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// `.lnk`
// ---------------------------------------------------------------------------

/// 扫描候选快捷方式并改写目标为桌面程序的那些；有改动时通知外壳刷新图标缓存。
fn sweep_shortcuts(ico: Option<&Path>, icons_dir: &Path) -> Result<(), String> {
    let Some(desktop_exe) = desktop_exe_string() else {
        tracing::debug!("desktop executable not found; skipping shortcut sweep");
        return Ok(());
    };
    let mut changed = false;
    let mut first_error = None;
    for lnk in candidate_shortcuts() {
        match update_link(&lnk, &desktop_exe, ico, icons_dir) {
            Ok(false) => {}
            Ok(true) => {
                changed = true;
                notify_item_updated(&lnk);
                tracing::info!(path = %lnk.display(), "updated shortcut icon");
            }
            // 任务栏固定项目录里多是其他应用的快捷方式：读不了只说明它不归我们管，不算失败。
            Err(LinkFailure::Unreadable(error)) => {
                tracing::debug!(path = %lnk.display(), %error, "skipped unreadable shortcut");
            }
            Err(LinkFailure::Write(error)) => {
                tracing::debug!(path = %lnk.display(), %error, "could not update shortcut icon");
                first_error.get_or_insert_with(|| format!("{}: {error}", lnk.display()));
            }
        }
    }
    if changed {
        notify_assoc_changed();
    }
    first_error.map_or(Ok(()), Err)
}

/// 安装器创建的桌面 / 开始菜单快捷方式，加任务栏固定项目录里的全部 `.lnk`（名称不由我们决定，
/// 逐个按目标过滤）。
fn candidate_shortcuts() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(desktop) = known_folder(&FOLDERID_Desktop) {
        candidates.push(desktop.join("FluxDown.lnk"));
    }
    if let Some(programs) = known_folder(&FOLDERID_Programs) {
        candidates.push(programs.join("FluxDown").join("FluxDown.lnk"));
    }
    if let Some(quick_launch) = known_folder(&FOLDERID_QuickLaunch) {
        let pinned = quick_launch.join("User Pinned").join("TaskBar");
        if let Ok(entries) = std::fs::read_dir(&pinned) {
            candidates.extend(entries.flatten().map(|entry| entry.path()).filter(|path| {
                path.extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("lnk"))
            }));
        }
    }
    candidates.retain(|path| path.is_file());
    candidates
}

/// 外壳分配的 PWSTR，析构时 `CoTaskMemFree`。
struct ShellString(PWSTR);

impl Drop for ShellString {
    fn drop(&mut self) {
        // SAFETY: `self.0` 由 SHGetKnownFolderPath 用 CoTaskMemAlloc 分配、由本结构独占，只释放这一次。
        unsafe { CoTaskMemFree(Some(self.0.0.cast_const().cast())) };
    }
}

fn known_folder(id: &GUID) -> Option<PathBuf> {
    // SAFETY: `id` 指向有效 GUID；无 token（当前用户），成功返回的字符串交给 `ShellString` 释放。
    let raw = unsafe { SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None) }.ok()?;
    let owned = ShellString(raw);
    // SAFETY: `owned.0` 是外壳返回的、以 NUL 结尾的宽字符串，在 `owned` 析构前有效。
    let text = unsafe { owned.0.to_string() }.ok()?;
    Some(PathBuf::from(text))
}

/// 单个快捷方式的失败：判定归属之前（外来快捷方式也可能读不了）与改写本应用快捷方式时分开。
enum LinkFailure {
    Unreadable(windows::core::Error),
    Write(windows::core::Error),
}

/// 改写单个快捷方式；返回是否有写入。只读加载：目标不是本应用时绝不打开写入。
fn update_link(
    lnk: &Path,
    desktop_exe: &str,
    ico: Option<&Path>,
    icons_dir: &Path,
) -> Result<bool, LinkFailure> {
    // SAFETY: 调用线程已有 STA COM 守卫（见 `run_worker`）；ShellLink 是进程内服务器，无外部指针参数。
    let link: IShellLinkW =
        unsafe { CoCreateInstance(&ShellLink, None::<&IUnknown>, CLSCTX_INPROC_SERVER) }
            .map_err(LinkFailure::Unreadable)?;
    let file: IPersistFile = link.cast().map_err(LinkFailure::Unreadable)?;
    let wide = to_wide(lnk);
    // SAFETY: `wide` 以 NUL 结尾且活过本次调用。
    unsafe { file.Load(PCWSTR(wide.as_ptr()), STGM_READ) }.map_err(LinkFailure::Unreadable)?;

    let mut target = [0u16; MAX_PATH_CHARS];
    // SAFETY: `target` 是有效可写缓冲；允许不取 WIN32_FIND_DATAW（传空指针）。PIDL-only 固定项
    // 返回成功但缓冲为空，由判定函数按外来快捷方式处理。
    unsafe { link.GetPath(&mut target, std::ptr::null_mut(), 0) }
        .map_err(LinkFailure::Unreadable)?;
    let mut current_icon = [0u16; MAX_PATH_CHARS];
    let mut icon_index = 0i32;
    // SAFETY: 两个输出缓冲 / 变量都有效且可写。
    unsafe { link.GetIconLocation(&mut current_icon, &mut icon_index) }
        .map_err(LinkFailure::Unreadable)?;

    let target = wide_to_string(&target);
    let target = if target.is_empty() {
        target
    } else {
        canonical_string(Path::new(&target))
    };
    let current_icon = wide_to_string(&current_icon);
    let Some(edit) = lnk_policy::plan_edit(
        &target,
        Path::new(desktop_exe),
        &current_icon,
        ico,
        icons_dir,
    ) else {
        return Ok(false);
    };

    let icon_wide = to_wide(Path::new(&edit.icon_path));
    // SAFETY: `icon_wide` 以 NUL 结尾且活过本次调用；空串表示清空图标位置。
    unsafe { link.SetIconLocation(PCWSTR(icon_wide.as_ptr()), 0) }.map_err(LinkFailure::Write)?;
    // SAFETY: `wide` 以 NUL 结尾且活过本次调用；显式路径保存会按写入方式重新打开同一个文件。
    unsafe { file.Save(PCWSTR(wide.as_ptr()), true) }.map_err(LinkFailure::Write)?;
    Ok(true)
}

pub(super) fn notify_item_updated(lnk: &Path) {
    let wide = to_wide(lnk);
    // SAFETY: SHCNF_PATHW 要求第一个参数是以 NUL 结尾的宽路径；`wide` 活过本次调用，
    // 外壳在调用内同步复制。
    unsafe {
        SHChangeNotify(
            SHCNE_UPDATEITEM,
            SHCNF_PATHW,
            Some(wide.as_ptr().cast()),
            None,
        )
    };
}

/// 单项 `UPDATEITEM` 不会让任务栏重画已显示的固定项（图标取自外壳图标缓存），关联变更会刷新缓存。
pub(super) fn notify_assoc_changed() {
    // SAFETY: SHCNF_IDLIST 且两个条目都为空，是 SHCNE_ASSOCCHANGED 的标准用法。
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
}
