//! 桌面宿主：自身程序文件消失后完全退出。
//!
//! 程序文件所在位置可能在 agent 运行期间失效：macOS Gatekeeper 路径随机化（App Translocation）
//! 的只读副本被系统回收、用户删除 App；Linux AppImage 挂载点被卸载、系统包或便携目录被删除。
//! 此时 agent / daemon 仍占着端口与数据目录锁，却再也拉不起同级桌面程序与 daemon：新打开的
//! 应用与浏览器扩展都连到这个无界面的「幽灵」实例。程序文件连续两次检查都不存在即走完全退出
//! （先关停 daemon），下一次启动从真实位置重新拉起。
//!
//! - 每轮重新解析程序路径：Linux `/proc/self/exe` 跟随改名（目录被移走后同级程序仍可达，不算
//!   消失）；系统包升级后带 ` (deleted)` 后缀，按去后缀的路径检查，新文件已就位即视为存在，
//!   版本差异由桌面握手替换旧 agent 处理。
//! - Windows 运行中的程序文件无法删除，只在自更新时被改名让位并由新文件立即顶上，检查天然
//!   不触发；统一启用，保持各平台行为一致。
//! - server 模式不启用：程序由 systemd / 容器 / NAS 包管理，替换后由其重启。
//!
//! 只 `stat` 程序文件，不触碰 data_dir / save_dir，不影响空闲静默。

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::lifecycle::Lifecycle;

/// 检查间隔。
const CHECK_INTERVAL: Duration = Duration::from_secs(30);
/// 连续缺失多少次才退出：自更新替换程序的两次改名之间有极短的空窗。
const MISSES_TO_QUIT: u32 = 2;

/// 启动监视（桌面宿主调用）。
pub(crate) fn spawn(lifecycle: Arc<Lifecycle>, cancel: CancellationToken) -> JoinHandle<()> {
    tokio::spawn(watch(lifecycle, cancel))
}

async fn watch(lifecycle: Arc<Lifecycle>, cancel: CancellationToken) {
    let mut misses = 0_u32;
    loop {
        tokio::select! {
            () = cancel.cancelled() => return,
            () = tokio::time::sleep(CHECK_INTERVAL) => {}
        }
        let Some(exe) = crate::update::install::current_exe_path() else {
            tracing::debug!("cannot resolve the agent executable path");
            misses = 0;
            continue;
        };
        misses = next_misses(misses, executable_present(&exe).await);
        if misses >= MISSES_TO_QUIT {
            tracing::warn!(
                exe = %exe.display(),
                "the executable this agent runs from is gone (macOS App Translocation copy reclaimed, AppImage unmounted, or app removed); quitting"
            );
            lifecycle.request_quit();
            return;
        }
    }
}

/// 无法判断（权限等 I/O 错误）按「存在」处理：宁可多留一轮也不误退出。
async fn executable_present(exe: &Path) -> bool {
    match tokio::fs::try_exists(exe).await {
        Ok(present) => present,
        Err(error) => {
            tracing::debug!(exe = %exe.display(), %error, "cannot check the agent executable");
            true
        }
    }
}

fn next_misses(misses: u32, present: bool) -> u32 {
    if present { 0 } else { misses.saturating_add(1) }
}

#[cfg(test)]
mod tests {
    use super::{MISSES_TO_QUIT, next_misses};

    #[test]
    fn a_single_miss_during_bundle_swap_does_not_quit() {
        let misses = next_misses(0, false);
        assert!(misses < MISSES_TO_QUIT);
        assert_eq!(next_misses(misses, true), 0);
        assert_eq!(next_misses(next_misses(0, false), false), MISSES_TO_QUIT);
    }
}
