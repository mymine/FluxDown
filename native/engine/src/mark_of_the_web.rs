//! 下载来源标记（Mark-of-the-Web）。
//!
//! 网络下载完成的文件需要带上「来自互联网」的系统标记，否则 SmartScreen、
//! Office 受保护视图、Gatekeeper 都把它当作本地可信文件：
//! - Windows：NTFS 备用数据流 `<file>:Zone.Identifier`（`ZoneId=3`）；
//! - macOS：`com.apple.quarantine` 扩展属性；
//! - 其它平台没有对应机制，空操作。
//!
//! Windows 遵循附件管理器策略「不保留附件中的区域信息」
//! （`Policies\Attachments\SaveZoneInformation=1`，HKLM 优先于 HKCU）：命中时
//! 不写 `Zone.Identifier`，与浏览器 / 资源管理器行为一致。策略在每次标记时读取，
//! 改组策略后无需重启。
//!
//! 标记不改变文件内容，对 BT 做种 / 续传无影响。所有失败（FAT/exFAT 不支持
//! ADS、只读介质、`xattr` 缺失）只记日志，绝不影响任务完成状态。
//!
//! NTFS 时间戳属于文件而非流：写 `Zone.Identifier` 会把主文件修改时间刷成当前
//! 时间。开启「使用服务器文件时间」的任务由调用方传 `keep_mtime`，写流后在同一
//! 句柄上回写原修改时间（显式设置后句柄关闭也不会再刷新）；未开启时不多做任何
//! 系统调用，大量小文件 / BT 目录不受影响。macOS 扩展属性只改 ctime，无需处理。

use std::path::{Path, PathBuf};

use crate::logger::log_info;

/// 目录产物（BT 多文件）最多标记的文件数，防止超大种子拖住后台任务。
#[cfg(any(windows, target_os = "macos"))]
const MAX_MARKED_FILES: usize = 50_000;

/// 后台为 `path`（文件或目录）写入来源标记；立即返回，不阻塞调用方。
///
/// `host_url` 为下载源地址、`referrer` 为来源页（均可为空）。目录会被递归展开，
/// 符号链接不跟随。`keep_mtime` 为 `true` 时标记不得改变文件修改时间（已设为
/// 服务器时间的产物）；仅 Windows 需要额外回写。
pub fn spawn_mark_downloaded(path: PathBuf, host_url: String, referrer: String, keep_mtime: bool) {
    if !cfg!(any(windows, target_os = "macos")) {
        return;
    }
    tokio::spawn(async move {
        let collected = tokio::task::spawn_blocking(move || {
            if !platform::policy_allows_marking() {
                return Vec::new();
            }
            collect_files(&path)
        })
        .await;
        let files = match collected {
            Ok(files) => files,
            Err(e) => {
                log_info!("[motw] collect files failed: {e}");
                return;
            }
        };
        if files.is_empty() {
            return;
        }
        platform::apply(files, host_url, referrer, keep_mtime).await;
    });
}

/// 展开目标：文件 → 自身；目录 → 其下全部普通文件（不跟随符号链接）。
#[cfg(any(windows, target_os = "macos", test))]
fn collect_files(path: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return out;
    };
    if meta.is_file() {
        out.push(path.to_path_buf());
        return out;
    }
    if !meta.is_dir() {
        return out;
    }
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                stack.push(entry.path());
            } else if file_type.is_file() {
                out.push(entry.path());
                #[cfg(any(windows, target_os = "macos"))]
                if out.len() >= MAX_MARKED_FILES {
                    return out;
                }
            }
        }
    }
    out
}

#[cfg(not(any(windows, target_os = "macos", test)))]
fn collect_files(_path: &Path) -> Vec<PathBuf> {
    Vec::new()
}

/// 只接受 http(s)/ftp URL 写入标记，剥掉内嵌凭据与控制字符（ADS 内容按行解析，
/// 换行会注入额外键）。其它 scheme（`magnet:`、`torrent-file://local` 等）返回空。
#[cfg(any(windows, test))]
fn sanitize_zone_url(url: &str) -> String {
    let url = url.trim();
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("ftp://"))
    {
        return String::new();
    }
    if url.chars().any(char::is_control) {
        return String::new();
    }
    let Some(scheme_end) = url.find("://") else {
        return String::new();
    };
    let (scheme, rest) = url.split_at(scheme_end + 3);
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(authority_end);
    let host = authority.rsplit('@').next().unwrap_or(authority);
    format!("{scheme}{host}{tail}")
}

/// `SaveZoneInformation` 取值：1 = 不保留区域信息（组策略「已启用」）。
#[cfg(any(windows, test))]
const SAVE_ZONE_INFORMATION_DISABLED: u32 = 1;

/// 附件管理器策略判定：机器级（HKLM）存在即生效，否则看用户级（HKCU）；
/// 只有显式 1 才关闭标记，未配置 / 2 / 其它值保持 Windows 默认（写入）。
#[cfg(any(windows, test))]
fn zone_information_allowed(machine: Option<u32>, user: Option<u32>) -> bool {
    machine.or(user) != Some(SAVE_ZONE_INFORMATION_DISABLED)
}

/// `Zone.Identifier` 流内容：`ZoneId=3`（Internet），可带 ReferrerUrl / HostUrl。
#[cfg(any(windows, test))]
fn zone_identifier_content(host_url: &str, referrer: &str) -> String {
    let mut content = String::from("[ZoneTransfer]\r\nZoneId=3\r\n");
    let referrer = sanitize_zone_url(referrer);
    if !referrer.is_empty() {
        content.push_str("ReferrerUrl=");
        content.push_str(&referrer);
        content.push_str("\r\n");
    }
    let host = sanitize_zone_url(host_url);
    if !host.is_empty() {
        content.push_str("HostUrl=");
        content.push_str(&host);
        content.push_str("\r\n");
    }
    content
}

/// `com.apple.quarantine` 值：`<flags>;<十六进制秒>;<应用名>;`（UUID 段可省）。
#[cfg(any(target_os = "macos", test))]
fn quarantine_value(now_secs: u64) -> String {
    format!("0081;{now_secs:x};FluxDown;")
}

#[cfg(windows)]
mod platform {
    use super::{Path, PathBuf, log_info, zone_identifier_content, zone_information_allowed};
    use std::io::Write;
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

    const ATTACHMENTS_POLICY_KEY: &str =
        r"Software\Microsoft\Windows\CurrentVersion\Policies\Attachments";

    /// 附件管理器策略是否允许写 `Zone.Identifier`（阻塞调用，读注册表）。
    pub(super) fn policy_allows_marking() -> bool {
        let machine = read_save_zone_information(HKEY_LOCAL_MACHINE, "HKLM");
        let user = read_save_zone_information(HKEY_CURRENT_USER, "HKCU");
        let allowed = zone_information_allowed(machine, user);
        if !allowed {
            log_info!("[motw] SaveZoneInformation policy disables Zone.Identifier, skipped");
        }
        allowed
    }

    /// 读取 `SaveZoneInformation`；键 / 值不存在为未配置，其它读取错误记日志后按未配置处理。
    fn read_save_zone_information(root: winreg::HKEY, label: &str) -> Option<u32> {
        let key = match RegKey::predef(root).open_subkey(ATTACHMENTS_POLICY_KEY) {
            Ok(key) => key,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
            Err(e) => {
                log_info!("[motw] open {label} attachments policy failed: {e}");
                return None;
            }
        };
        match key.get_value::<u32, _>("SaveZoneInformation") {
            Ok(value) => Some(value),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                log_info!("[motw] read {label} SaveZoneInformation failed: {e}");
                None
            }
        }
    }

    pub(super) async fn apply(
        files: Vec<PathBuf>,
        host_url: String,
        referrer: String,
        keep_mtime: bool,
    ) {
        let result = tokio::task::spawn_blocking(move || {
            let content = zone_identifier_content(&host_url, &referrer);
            for file in &files {
                if let Err(e) = write_zone_identifier(file, &content, keep_mtime) {
                    log_info!("[motw] Zone.Identifier failed for {}: {e}", file.display());
                }
            }
        })
        .await;
        if let Err(e) = result {
            log_info!("[motw] mark task failed: {e}");
        }
    }

    /// 写入 `<file>:Zone.Identifier`；`keep_mtime` 时写前取主文件修改时间，写后在
    /// 同一流句柄上回写。修改时间读取 / 回写失败只记日志：来源标记优先于时间戳。
    pub(super) fn write_zone_identifier(
        file: &Path,
        content: &str,
        keep_mtime: bool,
    ) -> std::io::Result<()> {
        let original = if keep_mtime {
            match std::fs::metadata(file).and_then(|meta| meta.modified()) {
                Ok(modified) => Some(modified),
                Err(e) => {
                    log_info!(
                        "[motw] read mtime of {} failed, server time may be lost: {e}",
                        file.display()
                    );
                    None
                }
            }
        } else {
            None
        };
        let mut stream = file.as_os_str().to_owned();
        stream.push(":Zone.Identifier");
        let mut ads = std::fs::File::create(PathBuf::from(stream))?;
        ads.write_all(content.as_bytes())?;
        if let Some(modified) = original
            && let Err(e) = ads.set_times(std::fs::FileTimes::new().set_modified(modified))
        {
            log_info!(
                "[motw] restore mtime of {} failed, server time lost: {e}",
                file.display()
            );
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::{PathBuf, log_info, quarantine_value};

    /// macOS 没有关闭隔离标记的系统策略。
    pub(super) fn policy_allows_marking() -> bool {
        true
    }

    /// 单次 `xattr` 调用处理的文件数（避免超长命令行，又不为每个文件起一个进程）。
    const BATCH: usize = 200;

    pub(super) async fn apply(
        files: Vec<PathBuf>,
        _host_url: String,
        _referrer: String,
        _keep_mtime: bool,
    ) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let value = quarantine_value(now);
        for chunk in files.chunks(BATCH) {
            let mut cmd = tokio::process::Command::new("/usr/bin/xattr");
            crate::proc::no_console_window(&mut cmd);
            cmd.arg("-w").arg("com.apple.quarantine").arg(&value);
            for file in chunk {
                cmd.arg(file);
            }
            cmd.stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true);
            match cmd.status().await {
                Ok(status) if status.success() => {}
                Ok(status) => log_info!("[motw] xattr exited with {status}"),
                Err(e) => log_info!("[motw] xattr spawn failed: {e}"),
            }
        }
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod platform {
    use super::PathBuf;

    pub(super) fn policy_allows_marking() -> bool {
        false
    }

    pub(super) async fn apply(
        _files: Vec<PathBuf>,
        _host_url: String,
        _referrer: String,
        _keep_mtime: bool,
    ) {
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zone_content_carries_sanitized_urls_and_no_injected_keys() {
        let content = zone_identifier_content(
            "https://user:secret@cdn.example.com/a/b.bin?x=1",
            "https://example.com/page",
        );
        assert_eq!(
            content,
            "[ZoneTransfer]\r\nZoneId=3\r\nReferrerUrl=https://example.com/page\r\n\
             HostUrl=https://cdn.example.com/a/b.bin?x=1\r\n"
        );

        // 控制字符（换行）整条丢弃，不能让调用方注入额外键。
        let injected = zone_identifier_content("https://a.test/x\r\nZoneId=0", "");
        assert_eq!(injected, "[ZoneTransfer]\r\nZoneId=3\r\n");
    }

    #[test]
    fn non_network_urls_are_not_written() {
        assert_eq!(sanitize_zone_url("magnet:?xt=urn:btih:abc"), "");
        assert_eq!(sanitize_zone_url("torrent-file://local"), "");
        assert_eq!(sanitize_zone_url(""), "");
    }

    #[test]
    fn save_zone_information_policy_precedence() {
        // 未配置 / 显式保留：写入（Windows 默认行为）。
        assert!(zone_information_allowed(None, None));
        assert!(zone_information_allowed(None, Some(2)));
        // 用户级「不保留」生效。
        assert!(!zone_information_allowed(None, Some(1)));
        // 机器级优先于用户级，双向覆盖。
        assert!(!zone_information_allowed(Some(1), Some(2)));
        assert!(zone_information_allowed(Some(2), Some(1)));
    }

    #[test]
    fn quarantine_value_is_hex_timestamped() {
        assert_eq!(quarantine_value(255), "0081;ff;FluxDown;");
    }

    /// NTFS 写 ADS 会刷新主文件修改时间：开启服务器时间（keep_mtime）必须原样保留
    /// 已设好的服务器时间；未开启时不回写，修改时间为标记时刻。
    #[cfg(windows)]
    #[test]
    fn zone_identifier_keeps_mtime_only_when_requested() -> std::io::Result<()> {
        let root = std::env::temp_dir().join(format!(
            "fluxdown_motw_mtime_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&root)?;
        // 2020-01-01T00:00:00Z，模拟已落地的服务器 Last-Modified。
        let server_time = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_577_836_800);
        for (keep_mtime, name) in [(true, "keep.bin"), (false, "plain.bin")] {
            let file = root.join(name);
            std::fs::write(&file, b"payload")?;
            std::fs::File::options()
                .write(true)
                .open(&file)?
                .set_times(std::fs::FileTimes::new().set_modified(server_time))?;

            platform::write_zone_identifier(&file, "[ZoneTransfer]\r\nZoneId=3\r\n", keep_mtime)?;

            let mtime = std::fs::metadata(&file)?.modified()?;
            if keep_mtime {
                assert_eq!(mtime, server_time, "keep_mtime 必须保留服务器时间");
            } else {
                assert!(mtime > server_time, "未开启时不得回写服务器时间");
            }
            let mut stream = file.as_os_str().to_owned();
            stream.push(":Zone.Identifier");
            assert!(std::fs::read_to_string(PathBuf::from(stream))?.contains("ZoneId=3"));
        }
        if let Err(error) = std::fs::remove_dir_all(&root)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!("best-effort test directory cleanup: {error}");
        }
        Ok(())
    }

    #[test]
    fn collect_files_expands_directories_and_skips_symlinks_free_paths() {
        let root = std::env::temp_dir().join(format!(
            "fluxdown_motw_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let nested = root.join("sub");
        assert!(std::fs::create_dir_all(&nested).is_ok());
        assert!(std::fs::write(root.join("a.bin"), b"a").is_ok());
        assert!(std::fs::write(nested.join("b.bin"), b"b").is_ok());

        let mut files = collect_files(&root);
        files.sort();
        assert_eq!(files, vec![root.join("a.bin"), nested.join("b.bin")]);
        assert_eq!(collect_files(&root.join("a.bin")), vec![root.join("a.bin")]);
        assert!(collect_files(&root.join("missing")).is_empty());

        if let Err(error) = std::fs::remove_dir_all(&root)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!("best-effort test directory cleanup: {error}");
        }
    }
}
