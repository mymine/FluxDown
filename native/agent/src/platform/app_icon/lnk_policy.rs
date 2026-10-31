//! `.lnk` 图标改写的判定逻辑（纯函数，供 Windows 实现与跨平台单测共用）。
//!
//! 任务栏固定项目录里放着所有应用的快捷方式，且系统自带固定项（资源管理器、UWP）只有 PIDL、
//! 没有文件系统目标。因此判定 **fail-closed**：目标无法解析成本应用桌面程序的一律不动。

use std::path::Path;

/// 对一个快捷方式要做的修改。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct IconEdit {
    /// 新的 `IconLocation` 路径；空串 = 清空（外壳回落到目标 exe 的 0 号图标）。
    pub icon_path: String,
}

/// 目标路径比较：忽略 ASCII 之外的大小写差异之外的一切格式差异——分隔符、`\\?\` 前缀、
/// 大小写（Windows 文件系统大小写不敏感）。
fn normalize(path: &str) -> String {
    let path = path.trim();
    let path = path.strip_prefix(r"\\?\").unwrap_or(path);
    path.replace('/', "\\").to_lowercase()
}

pub(super) fn same_path(left: &str, right: &str) -> bool {
    let (left, right) = (normalize(left), normalize(right));
    !left.is_empty() && left == right
}

/// 决定是否改写快捷方式的图标。
///
/// - `target`：`IShellLinkW::GetPath` 解析出的目标（PIDL-only 固定项为空串）；
/// - `desktop_exe`：本应用桌面程序；
/// - `current_icon`：当前 `IconLocation` 路径；
/// - `desired`：非默认时的 `.ico`；`None` = 默认（清空）；
/// - `owned_icons_dir`：本应用图标目录。默认时只清空指向本目录文件的图标，用户自己给快捷方式
///   设的图标不动。
///
/// 返回 `None` = 不需要改（外来 / 无法解析 / 已是目标状态）。
pub(super) fn plan_edit(
    target: &str,
    desktop_exe: &Path,
    current_icon: &str,
    desired: Option<&Path>,
    owned_icons_dir: &Path,
) -> Option<IconEdit> {
    if !same_path(target, &desktop_exe.to_string_lossy()) {
        return None;
    }
    match desired {
        Some(icon) => {
            let icon = icon.to_string_lossy();
            (!same_path(current_icon, &icon)).then(|| IconEdit {
                icon_path: icon.into_owned(),
            })
        }
        None => {
            // 按字符串取父目录：判定与宿主平台的路径分隔符无关（快捷方式里恒为 Windows 路径）。
            let ours = current_icon
                .rsplit_once(['\\', '/'])
                .is_some_and(|(parent, _)| same_path(parent, &owned_icons_dir.to_string_lossy()));
            (!current_icon.is_empty() && ours).then(|| IconEdit {
                icon_path: String::new(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = r"C:\Users\me\AppData\Local\Programs\FluxDown\fluxdown-desktop.exe";
    const ICONS: &str = r"C:\Users\me\AppData\Roaming\FluxDown\agent\icons";

    fn plan(target: &str, current: &str, desired: Option<&str>) -> Option<IconEdit> {
        plan_edit(
            target,
            Path::new(EXE),
            current,
            desired.map(Path::new),
            Path::new(ICONS),
        )
    }

    fn set(path: &str) -> Option<IconEdit> {
        Some(IconEdit {
            icon_path: path.to_owned(),
        })
    }

    #[test]
    fn foreign_target_is_never_touched() {
        let bolt = format!(r"{ICONS}\bolt.ico");
        assert_eq!(plan(r"C:\Windows\explorer.exe", "", Some(&bolt)), None);
        assert_eq!(plan(r"C:\Windows\explorer.exe", &bolt, None), None);
    }

    #[test]
    fn unresolvable_target_is_never_touched() {
        // PIDL-only 固定项：GetPath 返回空串。
        let bolt = format!(r"{ICONS}\bolt.ico");
        assert_eq!(plan("", "", Some(&bolt)), None);
        assert_eq!(plan("", &bolt, None), None);
    }

    #[test]
    fn own_shortcut_gets_the_new_icon() {
        let custom = format!(r"{ICONS}\custom-0a1b2c3d4e5f.ico");

        assert_eq!(plan(EXE, "", Some(&custom)), set(&custom));
        let bolt = format!(r"{ICONS}\bolt.ico");
        assert_eq!(plan(EXE, &bolt, Some(&custom)), set(&custom));
    }

    #[test]
    fn own_shortcut_already_showing_the_icon_is_left_alone() {
        let bolt = format!(r"{ICONS}\bolt.ico");

        assert_eq!(plan(EXE, &bolt, Some(&bolt)), None);
        // 大小写 / 分隔符差异不算变化。
        assert_eq!(plan(EXE, &bolt.to_uppercase(), Some(&bolt)), None);
        assert_eq!(plan(EXE, &bolt.replace('\\', "/"), Some(&bolt)), None);
    }

    #[test]
    fn target_comparison_ignores_case_prefix_and_separators() {
        let bolt = format!(r"{ICONS}\bolt.ico");
        let verbatim = format!(r"\\?\{EXE}");

        assert_eq!(plan(&verbatim, "", Some(&bolt)), set(&bolt));
        assert_eq!(plan(&EXE.to_uppercase(), "", Some(&bolt)), set(&bolt));
        assert_eq!(plan(&EXE.replace('\\', "/"), "", Some(&bolt)), set(&bolt));
    }

    #[test]
    fn default_clears_only_icons_we_wrote_on_our_own_shortcut() {
        let bolt = format!(r"{ICONS}\bolt.ico");

        assert_eq!(plan(EXE, &bolt, None), set(""));
        // 已经是默认：不写。
        assert_eq!(plan(EXE, "", None), None);
        // 用户给自己快捷方式设的别的图标：保留。
        assert_eq!(plan(EXE, r"D:\themes\mine.ico", None), None);
    }
}
