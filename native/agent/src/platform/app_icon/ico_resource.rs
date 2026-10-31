//! `.ico` 文件与 PE 图标资源（`RT_ICON` / `RT_GROUP_ICON`）的纯 Rust 解析 / 构造，以及程序本体补丁
//! 用到的纯判定（临时文件命名、开发构建路径识别）。无任何 OS 依赖，全部边界检查，供 Windows 实现与
//! 跨平台单测共用。
//!
//! 两种结构的关系：`.ico` 文件 = `ICONDIR` + 每帧 16 字节 `ICONDIRENTRY`（带文件内偏移）+ 帧数据；
//! PE 资源 = 每帧一个 `RT_ICON`（内容就是帧数据）+ 一个 `RT_GROUP_ICON`（`GRPICONDIR`，每帧 14 字节
//! `GRPICONDIRENTRY`，用资源 ID 取代文件偏移）。因此一帧在两处的比较只看头部字段与帧数据字节。

use std::path::Path;

const ICONDIR_LEN: usize = 6;
const ICONDIRENTRY_LEN: usize = 16;
const GRPICONDIRENTRY_LEN: usize = 14;
/// `ICONDIR.idType`：1 = 图标（2 = 光标）。
const TYPE_ICON: u16 = 1;
/// 一个图标文件 / 资源组允许的最大帧数（正常图标十帧上下；上限防畸形输入）。
const MAX_FRAMES: usize = 64;

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub(super) enum IcoError {
    #[error("truncated or malformed icon data")]
    Malformed,
    #[error("not an icon (type {0})")]
    NotIcon(u16),
    #[error("icon has no frames or more than {MAX_FRAMES}")]
    FrameCount,
}

/// 一帧图标：头部字段 + 原始帧数据（PNG 或 DIB，原样保留）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct IconFrame {
    /// 0 表示 256。
    pub width: u8,
    /// 0 表示 256。
    pub height: u8,
    pub color_count: u8,
    pub planes: u16,
    pub bit_count: u16,
    pub data: Vec<u8>,
}

/// `GRPICONDIR` 中的一项：头部字段 + 对应 `RT_ICON` 资源 ID。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct GroupEntry {
    pub width: u8,
    pub height: u8,
    pub color_count: u8,
    pub planes: u16,
    pub bit_count: u16,
    pub bytes_in_res: u32,
    pub id: u16,
}

fn u16_at(bytes: &[u8], offset: usize) -> Result<u16, IcoError> {
    let end = offset.checked_add(2).ok_or(IcoError::Malformed)?;
    let raw: [u8; 2] = bytes
        .get(offset..end)
        .and_then(|slice| slice.try_into().ok())
        .ok_or(IcoError::Malformed)?;
    Ok(u16::from_le_bytes(raw))
}

fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, IcoError> {
    let end = offset.checked_add(4).ok_or(IcoError::Malformed)?;
    let raw: [u8; 4] = bytes
        .get(offset..end)
        .and_then(|slice| slice.try_into().ok())
        .ok_or(IcoError::Malformed)?;
    Ok(u32::from_le_bytes(raw))
}

fn u8_at(bytes: &[u8], offset: usize) -> Result<u8, IcoError> {
    bytes.get(offset).copied().ok_or(IcoError::Malformed)
}

/// 校验 `ICONDIR` / `GRPICONDIR` 头（保留字 0、类型 1）并返回帧数。
fn read_dir_header(bytes: &[u8]) -> Result<usize, IcoError> {
    if u16_at(bytes, 0)? != 0 {
        return Err(IcoError::Malformed);
    }
    let kind = u16_at(bytes, 2)?;
    if kind != TYPE_ICON {
        return Err(IcoError::NotIcon(kind));
    }
    let count = usize::from(u16_at(bytes, 4)?);
    if count == 0 || count > MAX_FRAMES {
        return Err(IcoError::FrameCount);
    }
    Ok(count)
}

/// 解析 `.ico` 文件。
pub(super) fn parse_ico(bytes: &[u8]) -> Result<Vec<IconFrame>, IcoError> {
    let count = read_dir_header(bytes)?;
    (0..count)
        .map(|index| {
            let base = ICONDIR_LEN + index * ICONDIRENTRY_LEN;
            let size =
                usize::try_from(u32_at(bytes, base + 8)?).map_err(|_| IcoError::Malformed)?;
            let offset =
                usize::try_from(u32_at(bytes, base + 12)?).map_err(|_| IcoError::Malformed)?;
            let end = offset.checked_add(size).ok_or(IcoError::Malformed)?;
            let data = bytes.get(offset..end).ok_or(IcoError::Malformed)?;
            if data.is_empty() {
                return Err(IcoError::Malformed);
            }
            Ok(IconFrame {
                width: u8_at(bytes, base)?,
                height: u8_at(bytes, base + 1)?,
                color_count: u8_at(bytes, base + 2)?,
                planes: u16_at(bytes, base + 4)?,
                bit_count: u16_at(bytes, base + 6)?,
                data: data.to_vec(),
            })
        })
        .collect()
}

/// 解析 `RT_GROUP_ICON` 资源数据。
pub(super) fn parse_group(bytes: &[u8]) -> Result<Vec<GroupEntry>, IcoError> {
    let count = read_dir_header(bytes)?;
    (0..count)
        .map(|index| {
            let base = ICONDIR_LEN + index * GRPICONDIRENTRY_LEN;
            Ok(GroupEntry {
                width: u8_at(bytes, base)?,
                height: u8_at(bytes, base + 1)?,
                color_count: u8_at(bytes, base + 2)?,
                planes: u16_at(bytes, base + 4)?,
                bit_count: u16_at(bytes, base + 6)?,
                bytes_in_res: u32_at(bytes, base + 8)?,
                id: u16_at(bytes, base + 12)?,
            })
        })
        .collect()
}

impl IconFrame {
    /// 由组条目与它引用的 `RT_ICON` 数据组成一帧；数据长度与条目声明不符返回 `None`。
    pub(super) fn from_group_entry(entry: &GroupEntry, data: &[u8]) -> Option<Self> {
        (usize::try_from(entry.bytes_in_res).ok()? == data.len()).then(|| Self {
            width: entry.width,
            height: entry.height,
            color_count: entry.color_count,
            planes: entry.planes,
            bit_count: entry.bit_count,
            data: data.to_vec(),
        })
    }
}

/// 两组帧是否显示同一套图像：逐帧比较尺寸与帧数据。位深 / 调色板数等头部元数据由生成工具决定，
/// 不影响显示，不参与比较（否则资源编译器规整过头部的原装 exe 会被误判为不同）。
pub(super) fn frames_match(left: &[IconFrame], right: &[IconFrame]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(a, b)| (a.width, a.height) == (b.width, b.height) && a.data == b.data)
}

/// 构造 `RT_GROUP_ICON` 资源数据：第 `i` 帧引用 `ids[i]`。`ids` 与 `frames` 长度必须一致。
pub(super) fn build_group(frames: &[IconFrame], ids: &[u16]) -> Result<Vec<u8>, IcoError> {
    if frames.is_empty() || frames.len() > MAX_FRAMES || frames.len() != ids.len() {
        return Err(IcoError::FrameCount);
    }
    let count = u16::try_from(frames.len()).map_err(|_| IcoError::FrameCount)?;
    let mut out = Vec::with_capacity(ICONDIR_LEN + frames.len() * GRPICONDIRENTRY_LEN);
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&TYPE_ICON.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    for (frame, &id) in frames.iter().zip(ids) {
        let size = u32::try_from(frame.data.len()).map_err(|_| IcoError::Malformed)?;
        out.extend_from_slice(&[frame.width, frame.height, frame.color_count, 0]);
        out.extend_from_slice(&frame.planes.to_le_bytes());
        out.extend_from_slice(&frame.bit_count.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&id.to_le_bytes());
    }
    Ok(out)
}

/// 为新帧分配 `RT_ICON` 资源 ID：优先沿用旧组的 ID（原地覆盖，不留孤儿），不够再从旧 ID 最大值
/// 之后顺延。
pub(super) fn allocate_ids(old_ids: &[u16], needed: usize) -> Option<Vec<u16>> {
    let mut ids: Vec<u16> = old_ids.iter().copied().take(needed).collect();
    let mut next = old_ids.iter().copied().max().unwrap_or(0);
    while ids.len() < needed {
        next = next.checked_add(1)?;
        ids.push(next);
    }
    Some(ids)
}

/// 补丁临时文件名：`.<exe 文件名>.icon-<tag>.tmp`，与程序同目录（保证最后一步改名在同一卷）。
pub(super) fn tmp_name(exe_file_name: &str, tag: u32) -> String {
    format!(".{exe_file_name}.icon-{tag}.tmp")
}

/// 独占读（`share_mode(0)`）试探的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ExclusiveRead {
    /// 打开成功：没有别的句柄持有它。
    Opened,
    /// `ERROR_SHARING_VIOLATION`：别的句柄（如运行中的映像）持有它。
    SharingViolation,
    Other,
}

/// 对 exe 做写方式试开失败的归类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WriteFailure {
    /// 被运行中的进程占用：稍后重试有意义。
    InUse,
    /// 权限 / 只读属性：重试没有意义，按真实错误上报。
    Denied,
    Other,
}

const CODE_ACCESS_DENIED: u32 = 5;
const CODE_SHARING_VIOLATION: u32 = 32;
const CODE_USER_MAPPED_FILE: u32 = 1224;

/// 把写方式试开的失败归类。`ERROR_ACCESS_DENIED` 二义（运行中的映像、只读属性、ACL 都会给它），
/// 所以：只读属性 → 权限问题；否则用 `probe`（独占读试探，仅此时才调用）判定，只有共享冲突才算
/// 被占用，能打开则说明是 ACL 等真实权限问题。
pub(super) fn classify_write_failure(
    code: Option<u32>,
    read_only: bool,
    probe: impl FnOnce() -> ExclusiveRead,
) -> WriteFailure {
    match code {
        Some(CODE_SHARING_VIOLATION | CODE_USER_MAPPED_FILE) => WriteFailure::InUse,
        Some(CODE_ACCESS_DENIED) if read_only => WriteFailure::Denied,
        Some(CODE_ACCESS_DENIED) => match probe() {
            ExclusiveRead::SharingViolation => WriteFailure::InUse,
            ExclusiveRead::Opened | ExclusiveRead::Other => WriteFailure::Denied,
        },
        _ => WriteFailure::Other,
    }
}

/// `name` 是否是 [`tmp_name`] 生成的临时文件（任意 tag）。启动时据此清理上次崩溃的残留。
pub(super) fn is_tmp_name(name: &str, exe_file_name: &str) -> bool {
    let prefix = format!(".{exe_file_name}.icon-");
    name.strip_prefix(&prefix)
        .and_then(|rest| rest.strip_suffix(".tmp"))
        .is_some_and(|tag| !tag.is_empty() && tag.bytes().all(|byte| byte.is_ascii_digit()))
}

/// 路径是否位于 cargo 构建目录（`target\debug` / `target\release`，含交叉编译的 `target\<triple>\…`）。
/// 开发构建的程序由 cargo 管理，补丁它只会与增量构建互相打架。
pub(super) fn is_cargo_target_path(path: &Path) -> bool {
    let mut after_target = false;
    for component in path.components() {
        let name = component.as_os_str().to_string_lossy();
        if after_target
            && (name.eq_ignore_ascii_case("debug") || name.eq_ignore_ascii_case("release"))
        {
            return true;
        }
        if name.eq_ignore_ascii_case("target") {
            after_target = true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use image::{Rgba, RgbaImage};

    use super::*;
    use crate::app_icon::image::{ICO_SIZES, build_ico};

    fn sample_ico() -> Vec<u8> {
        build_ico(&RgbaImage::from_pixel(256, 256, Rgba([20, 120, 220, 255]))).expect("ico")
    }

    #[test]
    fn ico_parses_every_frame_with_matching_sizes() {
        let frames = parse_ico(&sample_ico()).expect("parse");
        assert_eq!(frames.len(), ICO_SIZES.len());
        for (frame, &size) in frames.iter().zip(&ICO_SIZES) {
            // 宽高字节 0 表示 256。
            let expected = u8::try_from(size).unwrap_or(0);
            assert_eq!((frame.width, frame.height), (expected, expected));
            assert!(!frame.data.is_empty());
        }
    }

    #[test]
    fn group_round_trips_through_resource_bytes() {
        let frames = parse_ico(&sample_ico()).expect("parse");
        let ids: Vec<u16> = (1..=frames.len() as u16).collect();
        let group = build_group(&frames, &ids).expect("build");
        let entries = parse_group(&group).expect("parse group");
        assert_eq!(entries.len(), frames.len());
        for ((entry, frame), &id) in entries.iter().zip(&frames).zip(&ids) {
            assert_eq!(entry.id, id);
            assert_eq!((entry.width, entry.height), (frame.width, frame.height));
            assert_eq!(
                (entry.planes, entry.bit_count),
                (frame.planes, frame.bit_count)
            );
            assert_eq!(entry.bytes_in_res as usize, frame.data.len());
        }
    }

    #[test]
    fn frame_comparison_looks_at_images_not_header_metadata() {
        let blue = parse_ico(&sample_ico()).expect("parse");
        let again = parse_ico(&sample_ico()).expect("parse");
        let red = parse_ico(
            &build_ico(&RgbaImage::from_pixel(256, 256, Rgba([220, 30, 30, 255]))).expect("ico"),
        )
        .expect("parse");
        assert!(frames_match(&blue, &again));
        assert!(!frames_match(&blue, &red));
        assert!(!frames_match(&blue, &blue[..blue.len() - 1]));

        let mut renormalised = blue.clone();
        for frame in &mut renormalised {
            frame.bit_count = 24;
            frame.color_count = 7;
        }
        assert!(frames_match(&blue, &renormalised));
    }

    #[test]
    fn frames_rebuilt_from_group_entries_match_the_ico() {
        let frames = parse_ico(&sample_ico()).expect("parse");
        let ids: Vec<u16> = (1..=frames.len() as u16).collect();
        let group = parse_group(&build_group(&frames, &ids).expect("build")).expect("parse");
        let rebuilt: Option<Vec<IconFrame>> = group
            .iter()
            .zip(&frames)
            .map(|(entry, frame)| IconFrame::from_group_entry(entry, &frame.data))
            .collect();
        assert_eq!(rebuilt, Some(frames.clone()));
        // 数据长度与条目声明不符 → 拒绝。
        assert_eq!(
            IconFrame::from_group_entry(&group[0], &frames[0].data[1..]),
            None
        );
    }

    #[test]
    fn malformed_input_is_rejected_not_panicked_on() {
        let ico = sample_ico();
        assert_eq!(parse_ico(&[]), Err(IcoError::Malformed));
        assert_eq!(parse_ico(&ico[..10]), Err(IcoError::Malformed));
        // 帧数据越界：截掉尾部。
        assert_eq!(parse_ico(&ico[..ico.len() - 1]), Err(IcoError::Malformed));
        // 类型 2 = 光标。
        let mut cursor = ico.clone();
        cursor[2] = 2;
        assert_eq!(parse_ico(&cursor), Err(IcoError::NotIcon(2)));
        // 零帧。
        let mut empty = ico;
        empty[4] = 0;
        empty[5] = 0;
        assert_eq!(parse_ico(&empty), Err(IcoError::FrameCount));
        assert_eq!(parse_group(&[0, 0, 1, 0, 1]), Err(IcoError::Malformed));
    }

    #[test]
    fn ids_reuse_the_old_group_then_extend() {
        assert_eq!(allocate_ids(&[1, 2, 3], 2), Some(vec![1, 2]));
        assert_eq!(allocate_ids(&[1, 2], 4), Some(vec![1, 2, 3, 4]));
        assert_eq!(allocate_ids(&[5, 9], 3), Some(vec![5, 9, 10]));
        assert_eq!(allocate_ids(&[], 2), Some(vec![1, 2]));
        assert_eq!(allocate_ids(&[u16::MAX], 2), None);
    }

    #[test]
    fn tmp_names_round_trip_and_reject_lookalikes() {
        let name = tmp_name("fluxdown-desktop.exe", 4242);
        assert_eq!(name, ".fluxdown-desktop.exe.icon-4242.tmp");
        assert!(is_tmp_name(&name, "fluxdown-desktop.exe"));
        assert!(!is_tmp_name("fluxdown-desktop.exe", "fluxdown-desktop.exe"));
        assert!(!is_tmp_name(
            ".fluxdown-desktop.exe.icon-.tmp",
            "fluxdown-desktop.exe"
        ));
        assert!(!is_tmp_name(
            ".fluxdown-desktop.exe.icon-1a.tmp",
            "fluxdown-desktop.exe"
        ));
        assert!(!is_tmp_name(
            ".other.exe.icon-1.tmp",
            "fluxdown-desktop.exe"
        ));
        assert!(!is_tmp_name(
            ".fluxdown-desktop.exe.icon-1.tmp.bak",
            "fluxdown-desktop.exe"
        ));
    }

    #[test]
    fn access_denied_is_in_use_only_when_an_exclusive_read_conflicts() {
        use ExclusiveRead::{Opened, Other, SharingViolation};
        let verdict = |code, read_only, probe: ExclusiveRead| {
            classify_write_failure(code, read_only, || probe)
        };
        // 无二义的错误码不需要试探。
        assert_eq!(verdict(Some(32), false, Opened), WriteFailure::InUse);
        assert_eq!(verdict(Some(1224), true, Opened), WriteFailure::InUse);
        // 拒绝访问：只读属性 → 权限问题，且不做试探。
        let mut probed = false;
        assert_eq!(
            classify_write_failure(Some(5), true, || {
                probed = true;
                SharingViolation
            }),
            WriteFailure::Denied
        );
        assert!(!probed);
        // 非只读：共享冲突 = 被占用；能独占打开或别的错误 = 真实权限问题。
        assert_eq!(
            verdict(Some(5), false, SharingViolation),
            WriteFailure::InUse
        );
        assert_eq!(verdict(Some(5), false, Opened), WriteFailure::Denied);
        assert_eq!(verdict(Some(5), false, Other), WriteFailure::Denied);
        // 其余错误 / 无错误码一律不是「占用」。
        assert_eq!(
            verdict(Some(2), false, SharingViolation),
            WriteFailure::Other
        );
        assert_eq!(verdict(None, false, SharingViolation), WriteFailure::Other);
    }

    #[test]
    fn cargo_target_directories_are_recognised() {
        assert!(is_cargo_target_path(Path::new(
            "C:/src/FluxDown/target/debug/fluxdown-desktop.exe"
        )));
        assert!(is_cargo_target_path(Path::new(
            "C:/src/FluxDown/Target/RELEASE/fluxdown-desktop.exe"
        )));
        assert!(is_cargo_target_path(Path::new(
            "C:/src/FluxDown/target/x86_64-pc-windows-msvc/release/fluxdown-desktop.exe"
        )));
        assert!(!is_cargo_target_path(Path::new(
            "C:/Users/me/AppData/Local/Programs/FluxDown/fluxdown-desktop.exe"
        )));
        // `release` 但不在 target 之下（如用户把便携版放进名为 release 的文件夹）不算。
        assert!(!is_cargo_target_path(Path::new(
            "D:/release/FluxDown/fluxdown-desktop.exe"
        )));
    }
}
