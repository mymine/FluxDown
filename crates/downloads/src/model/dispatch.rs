//! 跨设备下发 / 远程任务控制的纯规则：目标保存目录校验、批量结果汇总、命令适用状态。

use fluxdown_protocol::{PathStyle, RemoteCommandAction, RemoteTaskStatus, RpcErrorData};

/// 远端保存目录输入的校验结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RemoteDirCheck {
    /// 输入为空：提交时不带 `saveDir`，目标设备用它自己的默认目录。
    UseDefault,
    /// 合法目录（已去首尾空白）。
    Explicit(String),
    /// 不是目标路径风格下的绝对路径。
    Invalid,
}

/// 按目标设备的路径风格校验保存目录。风格未知（如 Web 端 / 新平台）时无法判断，
/// 交给目标设备接单时回退到默认目录，这里只要求非空即放行。
#[must_use]
pub(crate) fn check_remote_save_dir(input: &str, style: Option<PathStyle>) -> RemoteDirCheck {
    let dir = input.trim();
    if dir.is_empty() {
        return RemoteDirCheck::UseDefault;
    }
    match style.filter(|style| *style != PathStyle::Unknown) {
        Some(style) if !style.is_absolute(dir) => RemoteDirCheck::Invalid,
        _ => RemoteDirCheck::Explicit(dir.to_owned()),
    }
}

/// 一批命令（逐条链接下发 / 批量任务控制）的结果汇总：成功数、失败数与**首个**错误。
/// 后完成的成功不会覆盖已有失败。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DispatchSummary {
    pub(crate) succeeded: usize,
    pub(crate) failed: usize,
    pub(crate) first_error: Option<RpcErrorData>,
}

impl DispatchSummary {
    pub(crate) fn record<T>(&mut self, result: &Result<T, RpcErrorData>) {
        match result {
            Ok(_) => self.succeeded += 1,
            Err(error) => {
                self.failed += 1;
                if self.first_error.is_none() {
                    self.first_error = Some(error.clone());
                }
            }
        }
    }

    /// 批量成功：`count` 条已确认。
    pub(crate) fn record_succeeded(&mut self, count: usize) {
        self.succeeded += count;
    }

    /// 批量失败：`count` 条，首个错误只记一次。
    pub(crate) fn record_failed(&mut self, count: usize, error: &RpcErrorData) {
        if count == 0 {
            return;
        }
        self.failed += count;
        if self.first_error.is_none() {
            self.first_error = Some(error.clone());
        }
    }

    pub(crate) fn has_failure(&self) -> bool {
        self.failed > 0
    }
}

/// 远程任务行是否可以执行该动作。云端状态未知（旧客户端不认识的新状态）的任务不控制；
/// 暂停只适用于未结束且未暂停的任务，继续只适用于已暂停的任务，删除对任何已知状态有效
/// （云端直接删除记录，目标设备据此删除其本地任务，不要求目标在线）。
#[must_use]
pub(crate) fn remote_action_applies(status: RemoteTaskStatus, action: RemoteCommandAction) -> bool {
    match (status, action) {
        (RemoteTaskStatus::Unknown, _) => false,
        (
            RemoteTaskStatus::Pending | RemoteTaskStatus::Accepted | RemoteTaskStatus::Downloading,
            RemoteCommandAction::Pause | RemoteCommandAction::Cancel,
        ) => true,
        (RemoteTaskStatus::Paused, RemoteCommandAction::Resume | RemoteCommandAction::Cancel) => {
            true
        }
        (_, RemoteCommandAction::Delete) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use fluxdown_protocol::{ApplicationErrorCode, ErrorReason};

    use super::*;

    #[test]
    fn remote_save_dir_follows_target_path_style() {
        use RemoteDirCheck::{Explicit, Invalid, UseDefault};
        assert_eq!(
            check_remote_save_dir("  ", Some(PathStyle::Posix)),
            UseDefault
        );
        assert_eq!(
            check_remote_save_dir(" /mnt/dl ", Some(PathStyle::Posix)),
            Explicit("/mnt/dl".to_owned())
        );
        // 本机是 Windows 路径、目标是 mac：不能原样下发（#691）。
        assert_eq!(
            check_remote_save_dir(r"C:\Downloads", Some(PathStyle::Posix)),
            Invalid
        );
        assert_eq!(
            check_remote_save_dir("/Users/me", Some(PathStyle::Windows)),
            Invalid
        );
        assert_eq!(
            check_remote_save_dir("relative/dir", Some(PathStyle::Posix)),
            Invalid
        );
        assert_eq!(
            check_remote_save_dir(r"D:\data", Some(PathStyle::Windows)),
            Explicit(r"D:\data".to_owned())
        );
        // 风格未知：无法判断，放行非空输入。
        assert_eq!(
            check_remote_save_dir("whatever", None),
            Explicit("whatever".to_owned())
        );
        assert_eq!(
            check_remote_save_dir("whatever", Some(PathStyle::Unknown)),
            Explicit("whatever".to_owned())
        );
    }

    #[test]
    fn summary_keeps_first_error_when_later_calls_succeed() {
        let offline = RpcErrorData::new(ApplicationErrorCode::Unavailable, true)
            .with_reason(ErrorReason::TargetDeviceOffline);
        let conflict = RpcErrorData::new(ApplicationErrorCode::Conflict, false)
            .with_reason(ErrorReason::TaskStateConflict);
        let mut summary = DispatchSummary::default();
        summary.record::<()>(&Err(offline.clone()));
        summary.record::<()>(&Ok(()));
        summary.record::<()>(&Err(conflict));
        assert_eq!(summary.succeeded, 1);
        assert_eq!(summary.failed, 2);
        assert!(summary.has_failure());
        assert_eq!(summary.first_error, Some(offline));
    }

    #[test]
    fn remote_actions_only_apply_to_matching_known_states() {
        use RemoteCommandAction::{Delete, Pause, Resume};
        use RemoteTaskStatus::{Completed, Downloading, Paused, Pending, Unknown};
        assert!(remote_action_applies(Downloading, Pause));
        assert!(remote_action_applies(Pending, Pause));
        assert!(!remote_action_applies(Paused, Pause));
        assert!(remote_action_applies(Paused, Resume));
        assert!(!remote_action_applies(Completed, Resume));
        assert!(!remote_action_applies(Completed, Pause));
        assert!(remote_action_applies(Completed, Delete));
        // 未知状态一律不可控制，包括删除。
        for action in [Pause, Resume, Delete] {
            assert!(!remote_action_applies(Unknown, action));
        }
    }
}
