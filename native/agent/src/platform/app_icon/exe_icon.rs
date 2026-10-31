//! Windows：让桌面程序 `fluxdown-desktop.exe` 自身的图标资源跟随应用图标设置（资源管理器 / 任务管理器 /
//! 开始菜单搜索 / 进程列表显示的都是 exe 资源）。GPUI 的窗口类图标也取自同一份资源（组 ID 1）。
//!
//! # 流程（比较后再写，空闲静默）
//!
//! 1. 只在存在同级 `fluxdown-desktop.exe`、且不在 cargo 构建目录（`target\debug|release`）时才动手；
//!    清理上次崩溃遗留的 `.fluxdown-desktop.exe.icon-*.tmp`。
//! 2. 默认图标 = 安装包原样：只有 `binary-icon.json` 里记录过「我们改过这个 exe」才需要还原。
//! 3. 读 exe 当前的 `RT_GROUP_ICON`(1) 与它引用的 `RT_ICON` 帧（[`PeResources`]，纯 Rust 读文件），
//!    与期望的 `.ico` 逐帧比较；一致则无事可做（安装包升级换回默认 exe 后，下次对账发现不一致并重新补丁）。
//! 4. 不一致：先试探 exe 能否以写方式打开（桌面进程在跑时系统拒绝）——被占用就标记**待办**，由调用方
//!    在官方 UI 断开后重试，不做任何复制。
//! 5. 可写：同目录复制成临时文件，`BeginUpdateResourceW(FALSE)` 删旧 `RT_ICON`、写新 `RT_ICON` 与新
//!    `RT_GROUP_ICON`，`EndUpdateResourceW(FALSE)` 提交；用 [`PeResources`] 重新读临时文件，帧完全一致
//!    才 `MoveFileExW(REPLACE_EXISTING | WRITE_THROUGH)` 换入。换入时仍被占用同样记为待办。永不把运行中
//!    的 exe 改名挪开。
//! 6. 换入成功后通知外壳刷新图标缓存。
//!
//! 资源 ID 沿用旧组的 `RT_ICON` ID（原地覆盖），不够再顺延；语言沿用旧组的语言。

use std::ffi::c_void;
use std::os::windows::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION, ERROR_USER_MAPPED_FILE, HANDLE, WIN32_ERROR,
};
use windows::Win32::Storage::FileSystem::{
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
};
use windows::Win32::System::LibraryLoader::{
    BeginUpdateResourceW, EndUpdateResourceW, UpdateResourceW,
};
use windows::core::PCWSTR;

use super::binary_record::BinaryRecord;
use super::ico_resource::{
    ExclusiveRead, IconFrame, WriteFailure, allocate_ids, build_group, classify_write_failure,
    frames_match, is_cargo_target_path, is_tmp_name, parse_group, parse_ico, tmp_name,
};
use super::pe_resources::PeResources;
use super::windows::{notify_assoc_changed, notify_item_updated, to_wide};

/// 安装包自带的默认图标（与 `crates/app/build.rs` 嵌入 exe 的是同一个文件）。
const DEFAULT_ICO: &[u8] = include_bytes!("../../../../../assets/logo/windows/app_icon.ico");
/// 资源类型 `RT_ICON` / `RT_GROUP_ICON`。
const RT_ICON_ID: u16 = 3;
const RT_GROUP_ICON_ID: u16 = 14;
/// GPUI 窗口类与 Explorer 读取的图标组 ID（见 `crates/app/build.rs`）。
const GROUP_ICON_NAME: u16 = 1;

/// 一次补丁的结果。
enum Patched {
    Swapped,
    /// exe 正被运行中的进程占用。
    InUse,
}

/// 刷新 exe 图标；返回 `Ok(true)` = 待办（exe 被占用，稍后重试）。
pub(super) fn sync(
    preference: &str,
    desired_ico: Option<&[u8]>,
    icons_dir: &Path,
) -> Result<bool, String> {
    let Some(exe) = crate::platform::desktop_executable() else {
        tracing::debug!("desktop executable not found; skipping exe icon sync");
        return Ok(false);
    };
    if is_cargo_target_path(&exe) {
        tracing::debug!(exe = %exe.display(), "desktop executable is a cargo build output; skipping exe icon sync");
        return Ok(false);
    }
    sweep_stale_temp_files(&exe);

    let key = exe.to_string_lossy().into_owned();
    let mut record = BinaryRecord::load(icons_dir);
    if desired_ico.is_none() && record.get(&key).is_none() {
        // 默认 = 安装包原样，且我们从没改过：不读不写。
        return Ok(false);
    }
    let wanted = parse_ico(desired_ico.unwrap_or(DEFAULT_ICO))
        .map_err(|error| format!("invalid icon for the desktop executable: {error}"))?;
    let current = read_exe_icons(&exe)?;
    if !frames_match(&current.frames, &wanted) {
        match patch(&exe, &current, &wanted)? {
            Patched::Swapped => {
                tracing::info!(exe = %exe.display(), preference, "desktop executable icon updated");
                notify_item_updated(&exe);
                notify_assoc_changed();
            }
            Patched::InUse => {
                tracing::debug!(exe = %exe.display(), "desktop executable is in use; icon update pending");
                return Ok(true);
            }
        }
    }
    let changed = if desired_ico.is_some() {
        record.set(&key, preference)
    } else {
        record.remove(&key)
    };
    if changed && let Err(error) = record.save(icons_dir) {
        tracing::warn!(%error, "could not save the executable icon record");
    }
    Ok(false)
}

// ---------------------------------------------------------------------------
// 读取
// ---------------------------------------------------------------------------

/// exe 当前的图标资源。
struct ExeIcons {
    /// 图标组使用的语言（组 ID 1 的首个语言）。
    language: u16,
    /// 图标组 ID 1 的全部语言（多于一个说明有历史残留，补丁时一并删除多余的）。
    group_languages: Vec<u16>,
    /// 组引用的 `RT_ICON` ID 及各自存在的语言，与 `frames` 同序。
    icons: Vec<(u16, Vec<u16>)>,
    frames: Vec<IconFrame>,
}

fn read_exe_icons(path: &Path) -> Result<ExeIcons, String> {
    let context = |what: &str, error: &dyn std::fmt::Display| {
        format!("{what} in {}: {error}", path.display())
    };
    let resources =
        PeResources::open(path).map_err(|error| context("could not read resources", &error))?;
    let group_languages = resources
        .languages(RT_GROUP_ICON_ID, GROUP_ICON_NAME)
        .map_err(|error| context("could not list icon group languages", &error))?;
    let &language = group_languages
        .first()
        .ok_or_else(|| format!("{} has no icon group {GROUP_ICON_NAME}", path.display()))?;
    let group = resources
        .data(RT_GROUP_ICON_ID, GROUP_ICON_NAME, language)
        .map_err(|error| context("could not read the icon group", &error))?
        .ok_or_else(|| format!("{} has no icon group {GROUP_ICON_NAME}", path.display()))?;
    let entries = parse_group(group).map_err(|error| context("invalid icon group", &error))?;

    let mut icons = Vec::with_capacity(entries.len());
    let mut frames = Vec::with_capacity(entries.len());
    for entry in &entries {
        let languages = resources
            .languages(RT_ICON_ID, entry.id)
            .map_err(|error| context("could not list icon languages", &error))?;
        // 优先与组相同的语言；没有则取该图标唯一存在的语言。
        let icon_language = if languages.contains(&language) {
            language
        } else {
            *languages.first().ok_or_else(|| {
                format!("{} is missing icon resource {}", path.display(), entry.id)
            })?
        };
        let data = resources
            .data(RT_ICON_ID, entry.id, icon_language)
            .map_err(|error| context("could not read an icon", &error))?
            .ok_or_else(|| format!("{} is missing icon resource {}", path.display(), entry.id))?;
        let frame = IconFrame::from_group_entry(entry, data).ok_or_else(|| {
            format!(
                "icon resource {} in {} has an unexpected size",
                entry.id,
                path.display()
            )
        })?;
        icons.push((entry.id, languages));
        frames.push(frame);
    }
    Ok(ExeIcons {
        language,
        group_languages,
        icons,
        frames,
    })
}

// ---------------------------------------------------------------------------
// 补丁
// ---------------------------------------------------------------------------

/// 换入（`MoveFileExW`）被运行中的映像挡住时系统给出的错误；`ERROR_ACCESS_DENIED` 另需
/// [`probe_write_access`] 区分「被占用」与真正的权限问题。
const SWAP_BLOCKED_CODES: [WIN32_ERROR; 3] = [
    ERROR_ACCESS_DENIED,
    ERROR_SHARING_VIOLATION,
    ERROR_USER_MAPPED_FILE,
];

fn is_swap_blocked(error: &windows::core::Error) -> bool {
    SWAP_BLOCKED_CODES
        .iter()
        .any(|code| error.code() == code.to_hresult())
}

/// 目标 exe 当前能否被写入。
enum WriteAccess {
    Writable,
    /// 正被运行中的进程占用。
    InUse,
    /// 真正的权限 / 属性问题（附系统原因）。
    Denied(String),
}

/// 以写方式试开 exe。运行中的映像不能被写开；但 `ERROR_ACCESS_DENIED` 也可能是只读属性或 ACL，
/// 此时用独占读试探区分：只有「独占读也因共享冲突失败」才算被占用。
fn probe_write_access(exe: &Path) -> WriteAccess {
    let error = match std::fs::OpenOptions::new().write(true).open(exe) {
        Ok(_) => return WriteAccess::Writable,
        Err(error) => error,
    };
    let read_only = std::fs::metadata(exe).is_ok_and(|metadata| metadata.permissions().readonly());
    let code = error
        .raw_os_error()
        .and_then(|code| u32::try_from(code).ok());
    match classify_write_failure(code, read_only, || probe_exclusive_read(exe)) {
        WriteFailure::InUse => WriteAccess::InUse,
        WriteFailure::Denied | WriteFailure::Other => WriteAccess::Denied(error.to_string()),
    }
}

/// 以 `share_mode(0)`（不与任何人共享）试开读：被运行中的映像持有时报共享冲突。
fn probe_exclusive_read(exe: &Path) -> ExclusiveRead {
    match std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(exe)
    {
        Ok(_) => ExclusiveRead::Opened,
        Err(error)
            if error
                .raw_os_error()
                .and_then(|code| u32::try_from(code).ok())
                == Some(ERROR_SHARING_VIOLATION.0) =>
        {
            ExclusiveRead::SharingViolation
        }
        Err(_) => ExclusiveRead::Other,
    }
}

/// 同目录临时文件；析构时删除（已被改名换入则文件不存在，视为成功）。
struct TempFile {
    path: PathBuf,
}

impl TempFile {
    fn beside(exe: &Path) -> Result<Self, String> {
        let name = exe
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("{} has no usable file name", exe.display()))?;
        Ok(Self {
            path: exe.with_file_name(tmp_name(name, std::process::id())),
        })
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        match std::fs::remove_file(&self.path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                tracing::debug!(path = %self.path.display(), %error, "could not remove temporary exe copy");
            }
        }
    }
}

/// 删除上次崩溃 / 断电遗留的临时副本（任意 pid 标签）。
fn sweep_stale_temp_files(exe: &Path) {
    let (Some(dir), Some(name)) = (exe.parent(), exe.file_name().and_then(|name| name.to_str()))
    else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        if !file_name
            .to_str()
            .is_some_and(|candidate| is_tmp_name(candidate, name))
        {
            continue;
        }
        match std::fs::remove_file(entry.path()) {
            Ok(()) => {
                tracing::debug!(path = %entry.path().display(), "removed stale exe icon temp file")
            }
            Err(error) => {
                tracing::debug!(path = %entry.path().display(), %error, "could not remove stale exe icon temp file");
            }
        }
    }
}

fn patch(exe: &Path, current: &ExeIcons, wanted: &[IconFrame]) -> Result<Patched, String> {
    // 先试探写访问：运行中的映像不能以写方式打开，又快又不产生任何副本。
    match probe_write_access(exe) {
        WriteAccess::Writable => {}
        WriteAccess::InUse => return Ok(Patched::InUse),
        WriteAccess::Denied(reason) => {
            return Err(format!("{} is not writable: {reason}", exe.display()));
        }
    }
    let old_ids: Vec<u16> = current.icons.iter().map(|(id, _)| *id).collect();
    let ids = allocate_ids(&old_ids, wanted.len())
        .ok_or_else(|| "no free icon resource ids".to_owned())?;
    let group = build_group(wanted, &ids)
        .map_err(|error| format!("could not build the icon group: {error}"))?;

    let temp = TempFile::beside(exe)?;
    // 上次同 pid 的残留（pid 复用）先清掉，避免 copy 沿用旧文件属性。
    match std::fs::remove_file(&temp.path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("could not clear {}: {error}", temp.path.display())),
    }
    std::fs::copy(exe, &temp.path)
        .map_err(|error| format!("could not copy {} for patching: {error}", exe.display()))?;
    write_icon_resources(&temp.path, current, wanted, &ids, &group)?;

    let patched = read_exe_icons(&temp.path)?;
    if !frames_match(&patched.frames, wanted) {
        return Err(format!(
            "icon verification failed for the patched copy of {}",
            exe.display()
        ));
    }
    match replace_file(&temp.path, exe) {
        Ok(()) => Ok(Patched::Swapped),
        // 换入被挡：再确认一次是进程占用（而非目录权限等），否则按真实错误上报。
        Err(error) if is_swap_blocked(&error) => match probe_write_access(exe) {
            WriteAccess::InUse => Ok(Patched::InUse),
            WriteAccess::Writable | WriteAccess::Denied(_) => {
                Err(format!("could not replace {}: {error}", exe.display()))
            }
        },
        Err(error) => Err(format!("could not replace {}: {error}", exe.display())),
    }
}

fn write_icon_resources(
    target: &Path,
    current: &ExeIcons,
    wanted: &[IconFrame],
    ids: &[u16],
    group: &[u8],
) -> Result<(), String> {
    let session = UpdateSession::begin(target)?;
    // 旧图标：没被新帧复用、或复用但语言与组不同的条目一律删掉（语言不同不删会变成重复项）。
    for (id, languages) in &current.icons {
        for &language in languages {
            let reused = ids.contains(id) && language == current.language;
            if !reused {
                session.update(RT_ICON_ID, *id, language, None)?;
            }
        }
    }
    for &language in &current.group_languages {
        if language != current.language {
            session.update(RT_GROUP_ICON_ID, GROUP_ICON_NAME, language, None)?;
        }
    }
    for (frame, &id) in wanted.iter().zip(ids) {
        session.update(RT_ICON_ID, id, current.language, Some(&frame.data))?;
    }
    session.update(
        RT_GROUP_ICON_ID,
        GROUP_ICON_NAME,
        current.language,
        Some(group),
    )?;
    session.commit()
}

/// `MAKEINTRESOURCE`：整数资源 ID 以「低 16 位有值的指针」传入。
fn int_resource(id: u16) -> PCWSTR {
    PCWSTR(usize::from(id) as *const u16)
}

/// `BeginUpdateResourceW` 打开的更新会话。未提交就析构 = 丢弃全部修改。
struct UpdateSession {
    handle: Option<HANDLE>,
}

impl UpdateSession {
    fn begin(path: &Path) -> Result<Self, String> {
        let wide = to_wide(path);
        // SAFETY: `wide` 以 NUL 结尾且活过本次调用；bDeleteExistingResources = FALSE 保留其余资源。
        // 成功返回的句柄由本结构独占，提交或丢弃时各结束一次。
        let handle = unsafe { BeginUpdateResourceW(PCWSTR(wide.as_ptr()), false) }
            .map_err(|error| format!("BeginUpdateResourceW({}) failed: {error}", path.display()))?;
        Ok(Self {
            handle: Some(handle),
        })
    }

    /// `data = None` 删除该 `(类型, ID, 语言)` 资源，否则写入（覆盖）。
    fn update(&self, kind: u16, id: u16, language: u16, data: Option<&[u8]>) -> Result<(), String> {
        let handle = self
            .handle
            .ok_or_else(|| "resource update session already finished".to_owned())?;
        let (pointer, length) = match data {
            Some(bytes) => (
                Some(bytes.as_ptr().cast::<c_void>()),
                u32::try_from(bytes.len()).map_err(|_| "icon resource too large".to_owned())?,
            ),
            None => (None, 0),
        };
        // SAFETY: `handle` 是仍未结束的有效更新会话；类型 / 名称是整数资源 ID；`pointer` 要么为空（删除，
        // 长度 0），要么指向 `bytes` 的 `length` 字节，调用返回前不被释放，API 在调用内复制数据。
        unsafe {
            UpdateResourceW(
                handle,
                int_resource(kind),
                int_resource(id),
                language,
                pointer,
                length,
            )
        }
        .map_err(|error| format!("UpdateResourceW({kind}/{id}/{language}) failed: {error}"))
    }

    /// 提交修改（fDiscard = FALSE）。
    fn commit(mut self) -> Result<(), String> {
        let handle = self
            .handle
            .take()
            .ok_or_else(|| "resource update session already finished".to_owned())?;
        // SAFETY: `handle` 来自 `BeginUpdateResourceW` 且刚从会话里取出，只在这里结束一次。
        unsafe { EndUpdateResourceW(handle, false) }
            .map_err(|error| format!("EndUpdateResourceW failed: {error}"))
    }
}

impl Drop for UpdateSession {
    fn drop(&mut self) {
        let Some(handle) = self.handle.take() else {
            return;
        };
        // SAFETY: `handle` 来自 `BeginUpdateResourceW`，未提交的会话在这里结束一次；fDiscard = TRUE 丢弃修改。
        let discarded = unsafe { EndUpdateResourceW(handle, true) };
        if let Err(error) = discarded {
            tracing::debug!(%error, "could not discard the resource update session");
        }
    }
}

fn replace_file(from: &Path, to: &Path) -> Result<(), windows::core::Error> {
    let from_wide = to_wide(from);
    let to_path = to_wide(to);
    // SAFETY: 两个路径都以 NUL 结尾且活过本次调用；REPLACE_EXISTING 原子替换目标，WRITE_THROUGH 等待落盘。
    unsafe {
        MoveFileExW(
            PCWSTR(from_wide.as_ptr()),
            PCWSTR(to_path.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    }
}
