//! 受管外部工具（yt-dlp / ffmpeg / …）子进程的 `PATH` 补全。
//!
//! 根因：GUI / 服务管理器拉起的宿主进程继承的 `PATH` 常**缺失用户自己加的目录**，子进程
//! `Command` 默认只继承这份残缺 `PATH` → yt-dlp 找不到 JS 运行时（node / deno / bun）→
//! YouTube 签名挑战求解失败：只剩缩略图，或带登录 Cookie 时报
//! `The page needs to be reloaded`（可用的客户端全部依赖挑战求解）。
//!
//! - Windows：缺用户环境变量段（scoop 的 `~\scoop\shims` 等），从注册表读权威完整 PATH；
//! - 其余平台：macOS 从 Dock / Finder / 登录项（launchd）启动只有
//!   `/usr/bin:/bin:/usr/sbin:/sbin`，Linux 桌面会话与服务也只有系统目录；Homebrew、
//!   fnm / nvm、`~/.bun/bin` 等都写在 shell 配置里。取用户登录 shell 的 PATH（进程内只取一次）。
//!
//! 两边都不硬编码具体安装目录。

use tokio::process::Command;

use crate::logger::log_info;
#[cfg(not(windows))]
use crate::logger::log_warn;

/// 为受管外部工具子进程补全 `PATH`，使其能发现用户/系统安装的 JS 运行时（node/deno）等——
/// 这些常装在**用户级 PATH**（如 scoop 的 `~\scoop\shims`、`~\scoop\apps\nodejs\current`）。
///
/// 从注册表读**权威完整 PATH**（`HKLM\...\Session Manager\Environment\Path` +
/// `HKCU\Environment\Path`），与进程现有 `PATH` 合并去重后设回子进程。
#[cfg(windows)]
pub(super) async fn apply_full_path_env(cmd: &mut Command) {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

    fn read_path(root: winreg::RegKey, sub: &str) -> Option<String> {
        root.open_subkey(sub)
            .ok()
            .and_then(|k| k.get_value::<String, _>("Path").ok())
            .filter(|s| !s.trim().is_empty())
    }

    // 收集：进程现有 PATH（保留其顺序优先）+ 注册表系统/用户 PATH，去重合并。
    let mut seen = std::collections::HashSet::new();
    let mut merged: Vec<String> = Vec::new();
    let mut push_all = |raw: &str| {
        for seg in raw.split(';') {
            let seg = seg.trim();
            if seg.is_empty() {
                continue;
            }
            let key = seg.to_ascii_lowercase();
            if seen.insert(key) {
                merged.push(seg.to_string());
            }
        }
    };

    let inherited = std::env::var("PATH").unwrap_or_default();
    let inherited_segs = if inherited.is_empty() {
        0
    } else {
        inherited
            .split(';')
            .filter(|s| !s.trim().is_empty())
            .count()
    };
    push_all(&inherited);
    if let Some(system) = read_path(
        RegKey::predef(HKEY_LOCAL_MACHINE),
        r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment",
    ) {
        push_all(&system);
    }
    if let Some(user) = read_path(RegKey::predef(HKEY_CURRENT_USER), "Environment") {
        push_all(&user);
    }

    // 持久化：记录补全前后段数（补入的多为用户级 PATH，如 node/deno 安装目录）。
    log_info!(
        "[ytdlp-env] PATH 补全: 继承 {inherited_segs} 段 → 合并 {} 段（+{} 段来自注册表）",
        merged.len(),
        merged.len().saturating_sub(inherited_segs)
    );

    if !merged.is_empty() {
        cmd.env("PATH", merged.join(";"));
    }
}

/// 登录 shell 输出 PATH 的前后标记：shell 配置自身打印的横幅 / 提示不会混进结果。
#[cfg(not(windows))]
const PATH_BEGIN: &str = "__FLUXDOWN_PATH_BEGIN__";
#[cfg(not(windows))]
const PATH_END: &str = "__FLUXDOWN_PATH_END__";
/// 登录 shell 取 PATH 的超时：配置异常（等输入、卡网络）时放弃补全，不拖住工具调用。
#[cfg(not(windows))]
const LOGIN_SHELL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// 登录 shell 的 PATH（进程内只取一次；取不到为 `None`，此后沿用继承的 PATH）。
#[cfg(not(windows))]
static LOGIN_SHELL_PATH: tokio::sync::OnceCell<Option<String>> = tokio::sync::OnceCell::const_new();

/// 为受管外部工具子进程补全 `PATH`：登录 shell 的 PATH 在前（与用户终端里的查找顺序
/// 一致），继承的 PATH 中其余目录在后，去重合并。
#[cfg(not(windows))]
pub(super) async fn apply_full_path_env(cmd: &mut Command) {
    let Some(login) = LOGIN_SHELL_PATH.get_or_init(read_login_shell_path).await else {
        return;
    };
    let inherited = std::env::var("PATH").unwrap_or_default();
    cmd.env("PATH", merge_path_lists(&[login, &inherited]));
}

/// 以交互式登录 shell（`$SHELL -l -i -c`，与终端新开窗口加载同一组配置）打印 PATH。
/// stdin 为空、stderr 丢弃；失败一律记日志后返回 `None`。
#[cfg(not(windows))]
async fn read_login_shell_path() -> Option<String> {
    use std::process::Stdio;

    let Some(shell) = std::env::var_os("SHELL").filter(|s| !s.is_empty()) else {
        log_warn!("[ytdlp-env] 未设置 SHELL，外部工具沿用继承的 PATH");
        return None;
    };
    let script = format!("printf '%s' '{PATH_BEGIN}'; printenv PATH; printf '%s' '{PATH_END}'");
    let mut cmd = Command::new(&shell);
    cmd.args(["-l", "-i", "-c", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let shell = shell.to_string_lossy();
    let child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            log_warn!("[ytdlp-env] 启动登录 shell {shell} 失败，外部工具沿用继承的 PATH: {e}");
            return None;
        }
    };
    let output = match tokio::time::timeout(LOGIN_SHELL_TIMEOUT, child.wait_with_output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => {
            log_warn!("[ytdlp-env] 登录 shell {shell} 执行失败，外部工具沿用继承的 PATH: {e}");
            return None;
        }
        Err(_) => {
            log_warn!(
                "[ytdlp-env] 登录 shell {shell} 超过 {}s 未返回 PATH，外部工具沿用继承的 PATH",
                LOGIN_SHELL_TIMEOUT.as_secs()
            );
            return None;
        }
    };
    match extract_marked_path(&String::from_utf8_lossy(&output.stdout)) {
        Some(path) => {
            log_info!(
                "[ytdlp-env] 已取登录 shell {shell} 的 PATH（{} 段）",
                path.split(':').filter(|s| !s.is_empty()).count()
            );
            Some(path)
        }
        None => {
            log_warn!(
                "[ytdlp-env] 登录 shell {shell} 未输出 PATH（退出码 {:?}），外部工具沿用继承的 PATH",
                output.status.code()
            );
            None
        }
    }
}

/// 取两个标记之间的 PATH（去首尾空白）；缺标记或为空时 `None`。
#[cfg(not(windows))]
fn extract_marked_path(stdout: &str) -> Option<String> {
    let start = stdout.rfind(PATH_BEGIN)? + PATH_BEGIN.len();
    let rest = &stdout[start..];
    let path = rest[..rest.find(PATH_END)?].trim();
    (!path.is_empty()).then(|| path.to_string())
}

/// 按顺序合并多份 `:` 分隔的 PATH，去掉空段与重复目录（先出现者优先）。
#[cfg(not(windows))]
fn merge_path_lists(lists: &[&str]) -> String {
    let mut seen = std::collections::HashSet::new();
    let mut merged: Vec<&str> = Vec::new();
    for seg in lists.iter().flat_map(|list| list.split(':')) {
        if !seg.is_empty() && seen.insert(seg) {
            merged.push(seg);
        }
    }
    merged.join(":")
}

#[cfg(all(test, not(windows)))]
mod tests {
    use super::{PATH_BEGIN, PATH_END, extract_marked_path, merge_path_lists};

    #[test]
    fn marked_path_ignores_shell_banner_and_trailing_newline() {
        let out = format!("Welcome!\n{PATH_BEGIN}/opt/homebrew/bin:/usr/bin\n{PATH_END}");
        assert_eq!(
            extract_marked_path(&out).as_deref(),
            Some("/opt/homebrew/bin:/usr/bin")
        );
    }

    #[test]
    fn marked_path_rejects_missing_marker_or_empty_value() {
        assert_eq!(extract_marked_path("/usr/bin:/bin"), None);
        assert_eq!(extract_marked_path(&format!("{PATH_BEGIN}/usr/bin")), None);
        assert_eq!(
            extract_marked_path(&format!("{PATH_BEGIN}\n{PATH_END}")),
            None
        );
    }

    #[test]
    fn merge_keeps_login_order_first_and_drops_duplicates() {
        assert_eq!(
            merge_path_lists(&[
                "/Users/u/.bun/bin:/opt/homebrew/bin:/usr/bin:/bin",
                "/usr/bin:/bin:/usr/sbin:/sbin::",
            ]),
            "/Users/u/.bun/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin"
        );
    }
}
