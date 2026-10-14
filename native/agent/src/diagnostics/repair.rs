//! Doctor 修复动作的执行，只在用户点「修复」时调用。
//!
//! 权限修复遵循同一顺序：先以普通权限修（补属主权限位、原子替换旧文件）；仍被拒才请求管理员
//! 授权，而授权只用来「释放」——把路径还给当前用户（见 [`crate::permission`]）；随后回到普通
//! 权限完成重写，并重新探测确认。重新探测仍未通过时如实报告，不报假成功。

use std::path::{Path, PathBuf};

use fluxdown_protocol::{
    ApplicationErrorCode, ComponentProbeDto, ComponentRepairParams, RpcErrorData, StorageProbeDto,
    StorageProbeFailure, StorageProbeRole,
};
use serde_json::{Value, json};

use super::{
    ACTION_ENABLE_AUTOSTART, ACTION_FIX_COMSPEC, ACTION_FIX_DIR_ACCESS, ACTION_OPEN_SETTINGS,
    ACTION_REREGISTER, DiagnosticsError, DiagnosticsService, PERMISSION_PROBE_TIMEOUT,
    category_probe_targets, join_error, spawn_blocking_platform,
};
use crate::permission::{
    self, DirFacts, DirPlan, DirScope, Os, PermissionError, ReleaseOp, ScopeRules,
};

impl DiagnosticsService {
    /// 需要桌面会话（授权对话框 / 系统设置页 / 通知）的动作在 headless 宿主上拒绝。
    fn require_desktop(&self, action: &str) -> Result<(), DiagnosticsError> {
        if self.desktop.is_some() {
            Ok(())
        } else {
            Err(DiagnosticsError::InvalidAction(format!(
                "{action} needs a desktop session"
            )))
        }
    }

    /// 重新跑一遍目录写入探测（含 agent 偏好里的分类目录）。
    async fn probe_storage(&self) -> Result<Vec<StorageProbeDto>, DiagnosticsError> {
        let category_dirs = self.events.inspect(category_probe_targets);
        self.probe_permissions(category_dirs)
            .await
            .map(|result| result.storage)
            .map_err(|error| {
                tracing::warn!(error = %error, "permission probe unavailable during repair");
                DiagnosticsError::Daemon(RpcErrorData::new(ApplicationErrorCode::Unavailable, true))
            })
    }

    /// `fix_dir_access`：目标路径只用来在重新探测的结果里选中一项，实际修改的目录来自探测。
    pub(super) async fn repair_dir_access(&self, target: &str) -> Result<Value, DiagnosticsError> {
        self.require_desktop(ACTION_FIX_DIR_ACCESS)?;
        let target = target.trim().to_owned();
        let probes = self.probe_storage().await?;
        let probe = find_probe(&probes, &target)
            .ok_or_else(|| {
                DiagnosticsError::InvalidAction(format!("not a probed directory: {target}"))
            })?
            .clone();
        if probe.failure.is_none() {
            return Ok(json!({ "ok": true, "changed": false }));
        }
        if probe.failure != Some(StorageProbeFailure::PermissionDenied)
            || probe.probed_dir.is_empty()
        {
            return Err(PermissionError::NotApplicable(
                "the directory does not fail with a permission error".to_owned(),
            )
            .into());
        }
        let rules = self
            .scope_rules(owned_roots(&probes, self.store.data_dir()))
            .await?;
        let mut facts = tokio::task::spawn_blocking(move || dir_facts(&probe, &rules))
            .await
            .map_err(join_error)?;
        if facts.os == Os::Windows || facts.scope == DirScope::Shared {
            facts.principal = Some(permission::current_principal().await?);
        }
        match permission::plan_dir_repair(&facts) {
            DirPlan::ChmodOwner(dir) => chmod_owner(dir).await?,
            DirPlan::Release(op) => {
                refuse_when_root()?;
                permission::release(&op).await?;
            }
            DirPlan::PrivacySettings => {
                return Err(PermissionError::NotApplicable(
                    "macOS privacy permission must be granted in System Settings".to_owned(),
                )
                .into());
            }
            DirPlan::NotApplicable(reason) => {
                return Err(PermissionError::NotApplicable(reason.to_owned()).into());
            }
        }
        let after = self.probe_storage().await?;
        match find_probe(&after, &target) {
            Some(probe) if probe.failure.is_some() => Err(DiagnosticsError::RepairIncomplete(
                format!("{}: {}", probe.failed_step, probe.error),
            )),
            _ => Ok(json!({ "ok": true, "changed": true })),
        }
    }

    /// `reregister` / `use_this_install`：普通权限重写注册；写入被拒时请求授权把这些目录还给
    /// 当前用户再重写；最后像浏览器那样拉起一次确认。
    pub(super) async fn repair_nmh(&self) -> Result<Value, DiagnosticsError> {
        #[cfg(unix)]
        if let Err(error) =
            tokio::task::spawn_blocking(crate::nmh::registry::ensure_relay_executable)
                .await
                .map_err(join_error)?
        {
            tracing::warn!(error = %error, "could not restore the NMH relay's execute permission");
        }
        let issues = register().await?;
        if !issues.denied.is_empty() {
            if self.require_desktop(ACTION_REREGISTER).is_err() {
                return Err(DiagnosticsError::RepairIncomplete(
                    "registration is blocked by file permissions".to_owned(),
                ));
            }
            self.release_nmh(&issues.denied).await?;
            let again = register().await?;
            if !again.denied.is_empty() {
                return Err(DiagnosticsError::RepairIncomplete(format!(
                    "still denied: {}",
                    join_paths(again.denied.iter().map(PathBuf::as_path))
                )));
            }
        }
        if self.desktop.is_some() {
            let diagnosis = tokio::task::spawn_blocking(crate::nmh::registry::diagnose)
                .await
                .map_err(join_error)?;
            if let Some(Err(error)) = crate::nmh::registry::probe_browser_launch(&diagnosis).await {
                return Err(DiagnosticsError::RepairIncomplete(error.to_string()));
            }
        }
        Ok(json!({ "ok": true }))
    }

    /// `fix_comspec`：只改 `HKCU\Environment`（无需管理员）让新启动的程序拿到可用的 `ComSpec`，
    /// 再广播 `WM_SETTINGCHANGE` 让资源管理器刷新环境块——否则之后从开始菜单 / 任务栏打开的
    /// 浏览器仍拿到旧值。注册表已正确时只重新广播（用户用注册表编辑器或脚本改过、没有通知
    /// 资源管理器）。最后按注册表配置值像浏览器那样拉起一次确认；已在运行的浏览器与本进程
    /// 仍沿用旧值，只能由用户重启浏览器解决，诊断里另以 `nmh_comspec_restart` 提示。
    pub(super) async fn repair_comspec(&self) -> Result<Value, DiagnosticsError> {
        self.require_desktop(ACTION_FIX_COMSPEC)?;
        apply_comspec_fix().await
    }

    /// 把写入被拒的 NMH 目录还给当前用户：类 Unix 先以普通权限给本就属于自己的目录补上
    /// `u+rwx`，只有属于其他用户的目录才请求授权改回属主（只改目录本身、只限家目录内；目录里
    /// 属于其他用户的旧文件由随后的「临时文件 + rename」替换）；Windows 加 ACL。
    async fn release_nmh(&self, denied: &[PathBuf]) -> Result<(), DiagnosticsError> {
        let rules = self.scope_rules(Vec::new()).await?;
        let denied = denied.to_vec();
        let foreign = tokio::task::spawn_blocking(move || {
            let mut foreign = Vec::new();
            for path in denied {
                let path = normalize(&path);
                if !matches!(rules.classify(&path), DirScope::Home | DirScope::Owned) {
                    return Err(DiagnosticsError::from(PermissionError::NotApplicable(
                        format!("{} is outside the home directory", path.display()),
                    )));
                }
                if owned_by_self(&path) {
                    restore_owner_bits(&path)?;
                } else {
                    foreign.push(path);
                }
            }
            Ok(foreign)
        })
        .await
        .map_err(join_error)??;
        if foreign.is_empty() {
            return Ok(());
        }
        if Os::CURRENT == Os::Windows {
            let principal = permission::current_principal().await?;
            for dir in foreign {
                permission::release(&ReleaseOp::Grant {
                    principal: principal.clone(),
                    dir,
                })
                .await?;
            }
            return Ok(());
        }
        refuse_when_root()?;
        let uid = current_uid()?;
        let entries = foreign.into_iter().map(|path| (path, false)).collect();
        permission::release(&ReleaseOp::Chown { uid, entries }).await?;
        Ok(())
    }

    /// `fix_component`：由 daemon 补上托管组件的执行权限并重新探测。
    pub(super) async fn repair_component(&self, name: &str) -> Result<Value, DiagnosticsError> {
        let params = ComponentRepairParams {
            name: name.trim().to_owned(),
        };
        let probe = tokio::time::timeout(
            PERMISSION_PROBE_TIMEOUT,
            self.daemon
                .call::<ComponentRepairParams, ComponentProbeDto>(
                    fluxdown_protocol::method::DAEMON_DIAGNOSTICS_FIX_COMPONENT,
                    Some(params),
                ),
        )
        .await
        .map_err(|_| {
            DiagnosticsError::Daemon(RpcErrorData::new(ApplicationErrorCode::Timeout, true))
        })?
        .map_err(DiagnosticsError::Daemon)?;
        if probe.error.is_empty() {
            Ok(json!({ "ok": true }))
        } else {
            Err(DiagnosticsError::RepairIncomplete(probe.error))
        }
    }

    /// `enable_autostart`：用户在 Doctor 里明确要求重新启用（写入条目并清除系统级禁用标记），
    /// 之后重新读取确认。
    pub(super) async fn repair_autostart(&self) -> Result<Value, DiagnosticsError> {
        self.require_desktop(ACTION_ENABLE_AUTOSTART)?;
        spawn_blocking_platform(|| crate::platform::set_autostart(true)).await?;
        let state = tokio::task::spawn_blocking(crate::platform::autostart_state)
            .await
            .map_err(join_error)?;
        if state == crate::platform::AutostartState::Enabled {
            Ok(json!({ "ok": true }))
        } else {
            Err(DiagnosticsError::RepairIncomplete(format!(
                "autostart is {state:?} after re-enabling"
            )))
        }
    }

    /// `open_settings`：打开只能由用户本人操作的系统设置页。
    pub(super) async fn open_settings(&self, target: &str) -> Result<Value, DiagnosticsError> {
        self.require_desktop(ACTION_OPEN_SETTINGS)?;
        let pane = crate::platform::SettingsPane::from_name(target.trim()).ok_or_else(|| {
            DiagnosticsError::InvalidAction(format!("unknown settings page: {target}"))
        })?;
        spawn_blocking_platform(move || crate::platform::open_system_settings(pane)).await?;
        Ok(json!({ "ok": true }))
    }

    /// 当前平台的目录范围规则：家目录与 FluxDown 自有目录都先规范化。
    async fn scope_rules(&self, owned_roots: Vec<PathBuf>) -> Result<ScopeRules, DiagnosticsError> {
        tokio::task::spawn_blocking(move || {
            let home = directories::BaseDirs::new().map(|dirs| normalize(dirs.home_dir()));
            let owned = owned_roots.iter().map(|root| normalize(root)).collect();
            ScopeRules::for_current_os(home, owned)
        })
        .await
        .map_err(join_error)
    }
}

/// FluxDown 自有目录：daemon 数据目录（探测里的 `DataDir` 项）与 agent 数据目录。
fn owned_roots(probes: &[StorageProbeDto], agent_data_dir: &Path) -> Vec<PathBuf> {
    probes
        .iter()
        .filter(|probe| probe.role == StorageProbeRole::DataDir)
        .map(|probe| PathBuf::from(&probe.path))
        .chain(std::iter::once(agent_data_dir.to_path_buf()))
        .collect()
}

/// 在探测结果里按配置路径（忽略首尾空白与末尾分隔符）选中目标。
fn find_probe<'a>(probes: &'a [StorageProbeDto], target: &str) -> Option<&'a StorageProbeDto> {
    let key = |path: &str| {
        let trimmed = path.trim();
        let stripped = trimmed.trim_end_matches(['/', '\\']);
        if stripped.is_empty() || stripped.ends_with(':') {
            trimmed.to_owned()
        } else {
            stripped.to_owned()
        }
    };
    let target = key(target);
    if target.is_empty() {
        return None;
    }
    probes.iter().find(|probe| key(&probe.path) == target)
}

/// 解析符号链接与 `..`，Windows 去掉 `\\?\` 前缀；目录不存在时保留原样。
fn normalize(path: &Path) -> PathBuf {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let text = canonical.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(stripped) if !stripped.starts_with("UNC\\") => PathBuf::from(stripped),
        _ => canonical,
    }
}

/// 规划所需的只读事实（阻塞：规范化路径、读属主）。
fn dir_facts(probe: &StorageProbeDto, rules: &ScopeRules) -> DirFacts {
    let dir = normalize(Path::new(&probe.probed_dir));
    let scope = rules.classify(&dir);
    #[cfg(unix)]
    let (owner, self_uid) = (
        permission::owner_uid(&dir),
        permission::effective_uid().ok(),
    );
    #[cfg(not(unix))]
    let (owner, self_uid) = (None, None);
    DirFacts {
        os: Os::CURRENT,
        os_error: probe.os_error,
        dir,
        scope,
        owner,
        self_uid,
        principal: None,
    }
}

#[cfg(unix)]
async fn chmod_owner(dir: PathBuf) -> Result<(), DiagnosticsError> {
    tokio::task::spawn_blocking(move || permission::chmod_owner_rwx(&dir))
        .await
        .map_err(join_error)??;
    Ok(())
}

#[cfg(not(unix))]
async fn chmod_owner(_dir: PathBuf) -> Result<(), DiagnosticsError> {
    Err(PermissionError::NotApplicable("permission bits do not exist on Windows".to_owned()).into())
}

/// 目录属于当前用户（读不到属主时按「不是」处理，交给提权路径重新判定）。Windows 无属主位，
/// 一律走 ACL 授权。
#[cfg(unix)]
fn owned_by_self(dir: &Path) -> bool {
    match (permission::owner_uid(dir), permission::effective_uid()) {
        (Some(owner), Ok(uid)) => owner == uid,
        _ => false,
    }
}

#[cfg(not(unix))]
fn owned_by_self(_dir: &Path) -> bool {
    false
}

#[cfg(unix)]
fn restore_owner_bits(dir: &Path) -> Result<(), DiagnosticsError> {
    Ok(permission::chmod_owner_rwx(dir)?)
}

#[cfg(not(unix))]
fn restore_owner_bits(_dir: &Path) -> Result<(), DiagnosticsError> {
    Ok(())
}

/// 以 root 运行时「还给当前用户」没有意义，且会继续写出 root 所有的文件。
fn refuse_when_root() -> Result<(), DiagnosticsError> {
    #[cfg(unix)]
    if permission::effective_uid().is_ok_and(|uid| uid == 0) {
        return Err(PermissionError::RunningElevated.into());
    }
    Ok(())
}

#[cfg(unix)]
fn current_uid() -> Result<u32, DiagnosticsError> {
    Ok(permission::effective_uid()?)
}

#[cfg(not(unix))]
fn current_uid() -> Result<u32, DiagnosticsError> {
    Err(PermissionError::NotApplicable("uid".to_owned()).into())
}

async fn register() -> Result<crate::nmh::registry::RegisterIssues, DiagnosticsError> {
    Ok(tokio::task::spawn_blocking(crate::nmh::registry::register)
        .await
        .map_err(join_error)??)
}

#[cfg(windows)]
async fn apply_comspec_fix() -> Result<Value, DiagnosticsError> {
    let change = tokio::task::spawn_blocking(crate::nmh::comspec::repair)
        .await
        .map_err(join_error)??;
    let broadcast = tokio::task::spawn_blocking(crate::platform::notify_environment_changed)
        .await
        .map_err(join_error)?;
    // 注册表已改好；广播失败只影响资源管理器何时刷新（注销后必然生效），不算修复失败。
    if let Err(error) = &broadcast {
        tracing::warn!(error = %error, "could not broadcast the environment change");
    }
    let diagnosis = tokio::task::spawn_blocking(crate::nmh::registry::diagnose)
        .await
        .map_err(join_error)?;
    if let Some(issue) = diagnosis
        .comspec
        .as_ref()
        .and_then(|state| state.configured_issue)
    {
        return Err(DiagnosticsError::RepairIncomplete(format!(
            "ComSpec is still unusable: {issue}"
        )));
    }
    if let Some(Err(error)) = crate::nmh::registry::probe_browser_launch(&diagnosis).await {
        return Err(DiagnosticsError::RepairIncomplete(error.to_string()));
    }
    Ok(json!({ "ok": true, "change": change.name(), "broadcast": broadcast.is_ok() }))
}

#[cfg(not(windows))]
async fn apply_comspec_fix() -> Result<Value, DiagnosticsError> {
    Err(DiagnosticsError::InvalidAction(format!(
        "{ACTION_FIX_COMSPEC} only applies to Windows"
    )))
}

fn join_paths<'a>(paths: impl Iterator<Item = &'a Path>) -> String {
    paths
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use fluxdown_protocol::{StorageProbeDto, StorageProbeRole};

    use super::{find_probe, owned_roots};

    fn probe(role: StorageProbeRole, path: &str) -> StorageProbeDto {
        StorageProbeDto {
            role,
            label: String::new(),
            path: path.to_owned(),
            exists: true,
            probed_dir: path.to_owned(),
            failed_step: String::new(),
            failure: None,
            error: String::new(),
            available_bytes: None,
            os_error: None,
        }
    }

    #[test]
    fn repair_target_selects_a_probed_directory_only() {
        let probes = [
            probe(StorageProbeRole::DefaultSaveDir, "/data/dl"),
            probe(StorageProbeRole::DataDir, "/data/fluxdown"),
        ];
        assert_eq!(
            find_probe(&probes, " /data/dl/ ").map(|probe| probe.role),
            Some(StorageProbeRole::DefaultSaveDir)
        );
        assert!(find_probe(&probes, "/etc").is_none());
        assert!(find_probe(&probes, "  ").is_none());
        assert_eq!(
            owned_roots(&probes, Path::new("/agent")),
            vec![PathBuf::from("/data/fluxdown"), PathBuf::from("/agent")]
        );
    }
}
