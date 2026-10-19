//! 进程级跨平台辅助：子进程生成与文件描述符软限制。
//!
//! 引擎会拉起若干**控制台子进程**（ffmpeg / ffprobe / yt-dlp / tar，以及组件
//! 版本探测 `-version` / `--version`）。在 Windows 上，若不显式设置
//! `CREATE_NO_WINDOW`，每次拉起都会闪现一个黑色控制台窗口——打开设置「组件」
//! 页做版本探测时尤其高频、肉眼可见。
//!
//! 因此引擎内所有经 [`tokio::process::Command`] 拉起控制台程序的调用点都
//! **必须**先经 [`no_console_window`] 处理，切勿直接 `.spawn()` / `.output()`。

#[cfg(feature = "bt")]
use crate::logger::log_info;

/// 进程级只提升一次 `RLIMIT_NOFILE` 软限制（至硬限制）。
///
/// BT（每文件 / 每 peer 常驻 FD）与高线程 HTTP 任务（单任务最多 512 条连接）
/// 都会撞 Linux 默认 1024 / macOS GUI 默认 256 的软限制，EMFILE 会连带拖垮同进程
/// 的 SQLite 与其他下载。失败（硬限制更低等）只记日志，不影响下载继续。
///
/// 提升动作由 librqbit 提供（内部用 `rlimit`）；没有 `bt` feature 的构建不链接
/// librqbit，也没有其它直接依赖或 std API 能不借 `unsafe` 完成 `setrlimit`，
/// 故此时为空操作（沿用系统默认软限制）。
#[cfg(feature = "bt")]
pub(crate) fn raise_nofile_limit_once() {
    static NOFILE_LIMIT_ONCE: std::sync::Once = std::sync::Once::new();
    NOFILE_LIMIT_ONCE.call_once(|| match librqbit::try_increase_nofile_limit() {
        Ok(limit) => log_info!("[proc] RLIMIT_NOFILE soft limit raised to {limit}"),
        Err(e) => log_info!("[proc] failed to raise RLIMIT_NOFILE: {e:#}"),
    });
}

/// 无 `bt` feature 时的空操作，见上方说明。
#[cfg(not(feature = "bt"))]
pub(crate) fn raise_nofile_limit_once() {}

/// 为控制台子进程设置 Windows `CREATE_NO_WINDOW`，避免闪现黑色控制台窗口。
///
/// GUI 子系统的可执行程序不受此标志影响；非 Windows 平台为空操作。
/// 引擎内所有拉起外部控制台程序（ffmpeg/ffprobe/yt-dlp/tar/版本探测）的
/// [`tokio::process::Command`] **必须**先经此函数处理，否则会在 Windows 上闪窗。
pub(crate) fn no_console_window(cmd: &mut tokio::process::Command) {
    #[cfg(target_os = "windows")]
    {
        /// `CREATE_NO_WINDOW`：不为控制台子进程分配/闪现窗口。
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = cmd;
    }
}
