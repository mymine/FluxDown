//! Linux 用户级 XDG 图标覆盖：`$XDG_DATA_HOME/icons/hicolor/256x256/apps/com.fluxdown.app.png`。
//!
//! 系统包只装 `hicolor/256x256/apps` 同名图标；用户目录同尺寸子目录优先命中。写入 / 删除先比较，
//! 只有真的变了才通知图标主题（GTK 按主题目录 mtime 重扫），空闲时不产生任何 IO。

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// 桌面入口 / 图标名（`packaging/linux/com.fluxdown.app.desktop` 的 `Icon=`）。
const ICON_NAME: &str = "com.fluxdown.app";

/// 用户数据目录：`XDG_DATA_HOME`（必须是绝对路径，规范要求忽略相对值）否则 `$HOME/.local/share`。
pub(super) fn data_home(
    xdg_data_home: Option<OsString>,
    home: Option<OsString>,
) -> Option<PathBuf> {
    xdg_data_home
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            home.map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .map(|home| home.join(".local/share"))
        })
}

#[cfg(target_os = "linux")]
pub(super) fn data_home_from_env() -> Option<PathBuf> {
    data_home(std::env::var_os("XDG_DATA_HOME"), std::env::var_os("HOME"))
}

pub(super) fn theme_dir(data_home: &Path) -> PathBuf {
    data_home.join("icons/hicolor")
}

pub(super) fn override_path(data_home: &Path) -> PathBuf {
    theme_dir(data_home)
        .join("256x256/apps")
        .join(format!("{ICON_NAME}.png"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Change {
    Written,
    Removed,
    Unchanged,
}

/// `png = Some` 写入覆盖（字节相同则不动），`None` 删除覆盖（不存在则不动）。
pub(super) fn apply_override(data_home: &Path, png: Option<&[u8]>) -> std::io::Result<Change> {
    let path = override_path(data_home);
    match png {
        Some(png) => {
            if std::fs::read(&path).is_ok_and(|existing| existing == png) {
                return Ok(Change::Unchanged);
            }
            crate::app_icon::write_atomic(&path, png)?;
            Ok(Change::Written)
        }
        None => match std::fs::remove_file(&path) {
            Ok(()) => Ok(Change::Removed),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Change::Unchanged),
            Err(error) => Err(error),
        },
    }
}

/// 把主题目录 mtime 推到现在：GTK 图标主题据此重新扫描。
pub(super) fn touch_theme_dir(data_home: &Path) -> std::io::Result<()> {
    let dir = theme_dir(data_home);
    std::fs::create_dir_all(&dir)?;
    std::fs::File::open(&dir)?.set_modified(SystemTime::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "fluxdown-xdg-{name}-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn absolute_xdg_data_home_wins_over_home() {
        assert_eq!(
            data_home(Some("/data".into()), Some("/home/me".into())),
            Some(PathBuf::from("/data"))
        );
    }

    #[test]
    fn missing_or_relative_xdg_data_home_falls_back_to_home() {
        let expected = Some(PathBuf::from("/home/me/.local/share"));
        assert_eq!(data_home(None, Some("/home/me".into())), expected);
        assert_eq!(
            data_home(Some("relative/dir".into()), Some("/home/me".into())),
            expected
        );
        assert_eq!(
            data_home(Some("".into()), Some("/home/me".into())),
            expected
        );
        assert_eq!(data_home(None, None), None);
        assert_eq!(data_home(None, Some("relative".into())), None);
    }

    #[test]
    fn override_lives_in_the_256_hicolor_apps_directory() {
        assert_eq!(
            override_path(Path::new("/home/me/.local/share")),
            PathBuf::from("/home/me/.local/share/icons/hicolor/256x256/apps/com.fluxdown.app.png")
        );
    }

    #[test]
    fn override_is_written_once_and_removed_by_default() {
        let home = temp_dir("override");

        assert_eq!(
            apply_override(&home, Some(b"png-1")).expect("write"),
            Change::Written
        );
        assert_eq!(
            apply_override(&home, Some(b"png-1")).expect("same"),
            Change::Unchanged
        );
        assert_eq!(
            apply_override(&home, Some(b"png-2")).expect("replace"),
            Change::Written
        );
        assert_eq!(std::fs::read(override_path(&home)).expect("read"), b"png-2");
        assert_eq!(
            apply_override(&home, None).expect("remove"),
            Change::Removed
        );
        assert_eq!(
            apply_override(&home, None).expect("again"),
            Change::Unchanged
        );
        assert!(!override_path(&home).exists());
        std::fs::remove_dir_all(&home).expect("cleanup");
    }

    #[test]
    fn touching_the_theme_dir_advances_its_mtime() {
        let home = temp_dir("touch");
        std::fs::create_dir_all(theme_dir(&home)).expect("theme dir");
        let past = SystemTime::now() - std::time::Duration::from_secs(3600);
        std::fs::File::open(theme_dir(&home))
            .and_then(|dir| dir.set_modified(past))
            .expect("age dir");

        touch_theme_dir(&home).expect("touch");

        let modified = std::fs::metadata(theme_dir(&home))
            .and_then(|meta| meta.modified())
            .expect("mtime");
        assert!(modified > past + std::time::Duration::from_secs(1800));
        std::fs::remove_dir_all(&home).expect("cleanup");
    }
}
