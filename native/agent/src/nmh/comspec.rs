//! Windows 命令解释器环境变量 `ComSpec`。
//!
//! Chrome / Edge（未开 `NativeHostsExecutablesLaunchDirectly` 时）以
//! `%ComSpec% /d /c <host>` 启动原生消息主机，只读自身进程环境里的 `ComSpec`、不回退默认值；
//! 值坏了（空、以分隔符结尾、文件不存在）浏览器只报 "Failed to start native messaging host."。
//!
//! 生效时机：注册表（用户变量覆盖系统变量）是**新启动**的程序拿到的值；已在运行的进程一直
//! 沿用启动时继承的环境块，只有处理 `WM_SETTINGCHANGE("Environment")` 的资源管理器会重新
//! 读取注册表。因此：
//! - 诊断与浏览器式拉起读注册表里的配置值，而不是本进程环境（修好后本进程仍是旧值）；
//! - 本进程环境仍是坏值时单独提示「修改前启动的程序仍沿用旧值」；
//! - 修复写注册表后由调用方广播 `WM_SETTINGCHANGE`，此后从开始菜单 / 任务栏启动的浏览器
//!   才拿得到新值。

use std::fmt;

/// Windows 安装时写入系统变量的默认值（`REG_EXPAND_SZ`）。
pub const DEFAULT_COMSPEC: &str = r"%SystemRoot%\system32\cmd.exe";

/// 生效值来自哪一级环境变量。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComSpecSource {
    /// `HKCU\Environment`：存在时覆盖系统变量。
    User,
    /// `HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Environment`。
    System,
}

impl ComSpecSource {
    /// 诊断详情里的位置说明。
    #[must_use]
    pub fn location(self) -> &'static str {
        match self {
            Self::User => r"user variable, HKCU\Environment",
            Self::System => {
                r"system variable, HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Environment"
            }
        }
    }
}

/// `ComSpec` 不能用来启动命令解释器的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComSpecIssue {
    /// 未设置或为空：浏览器直接放弃启动。
    Missing,
    /// 仍含 `%VAR%`：`REG_SZ` 不展开，或引用了不存在的变量。
    Unexpanded,
    /// 以 `\` / `/` 结尾，指向目录而不是程序。
    TrailingSeparator,
    /// 不是带盘符或 UNC 的绝对路径。
    NotAbsolute,
    /// 文件不存在（或是目录）。
    NotFound,
}

impl fmt::Display for ComSpecIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Missing => "not set",
            Self::Unexpanded => "contains an unexpanded %variable%",
            Self::TrailingSeparator => {
                "ends with a path separator, so it names a folder instead of a program"
            }
            Self::NotAbsolute => "is not an absolute path",
            Self::NotFound => "the file does not exist",
        })
    }
}

/// 注册表里新启动的程序会拿到的值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfiguredComSpec {
    pub source: ComSpecSource,
    /// 注册表原文（`REG_EXPAND_SZ` 未展开）。
    pub raw: String,
    /// 展开后的路径；`REG_SZ` 与原文相同。
    pub path: String,
}

/// `ComSpec` 诊断快照。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComSpecState {
    /// 新启动的程序会拿到的值；`None` = 用户与系统变量都未设置。
    pub configured: Option<ConfiguredComSpec>,
    /// `configured` 不能用的原因；未设置为 [`ComSpecIssue::Missing`]。
    pub configured_issue: Option<ComSpecIssue>,
    /// 本进程启动时继承的值；`None` = 未设置。
    pub process: Option<String>,
    /// `process` 不能用的原因；未设置为 [`ComSpecIssue::Missing`]。
    pub process_issue: Option<ComSpecIssue>,
}

impl ComSpecState {
    /// 浏览器式拉起应使用的命令解释器：配置值可用时为其展开路径。
    #[must_use]
    pub fn launcher(&self) -> Option<&str> {
        match (&self.configured, self.configured_issue) {
            (Some(configured), None) => Some(&configured.path),
            _ => None,
        }
    }
}

/// [`plan_repair`] 的决定，也是修复结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComSpecRepair {
    /// 注册表已可用，只需重新广播让资源管理器刷新。
    Unchanged,
    /// 删除坏的用户变量，回落到可用的系统变量。
    RemoveUserOverride,
    /// 写入用户变量 [`DEFAULT_COMSPEC`]：系统变量缺失或损坏，而改它需要管理员权限，
    /// 用户变量对当前用户同样生效。
    WriteUserDefault,
}

impl ComSpecRepair {
    /// 修复结果里的机器可读名称。
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Unchanged => "unchanged",
            Self::RemoveUserOverride => "removed_user_override",
            Self::WriteUserDefault => "wrote_user_default",
        }
    }
}

/// 按两级变量的现状决定修复：`None` = 该级未设置，`Some(usable)` = 已设置及其是否可用。
/// 用户变量存在即覆盖系统变量，所以只看用户变量是否可用；不可用且系统变量可用时删掉它，
/// 否则写一个可用的用户变量。
#[must_use]
pub fn plan_repair(user: Option<bool>, system: Option<bool>) -> ComSpecRepair {
    match (user, system) {
        (Some(true), _) | (None, Some(true)) => ComSpecRepair::Unchanged,
        (Some(false), Some(true)) => ComSpecRepair::RemoveUserOverride,
        _ => ComSpecRepair::WriteUserDefault,
    }
}

/// 与 `ExpandEnvironmentStringsW` 相同的展开：已定义的 `%NAME%` 替换为其值，未定义的与
/// 落单的 `%` 原样保留。
#[must_use]
pub fn expand(raw: &str, lookup: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('%') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let name = &after[..end];
        match (!name.is_empty()).then(|| lookup(name)).flatten() {
            Some(value) => {
                out.push_str(&value);
                rest = &after[end + 1..];
            }
            // 结尾的 `%` 可能是下一个变量的开头，从它继续扫描。
            None => {
                out.push('%');
                out.push_str(name);
                rest = &after[end..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// 检查展开后的值能否作为命令解释器启动；`is_file` 判断文件是否存在。
#[must_use]
pub fn validate(path: &str, is_file: impl Fn(&str) -> bool) -> Option<ComSpecIssue> {
    if path.is_empty() {
        Some(ComSpecIssue::Missing)
    } else if has_variable_reference(path) {
        Some(ComSpecIssue::Unexpanded)
    } else if path.ends_with(['\\', '/']) {
        Some(ComSpecIssue::TrailingSeparator)
    } else if !is_windows_absolute(path) {
        Some(ComSpecIssue::NotAbsolute)
    } else if !is_file(path) {
        Some(ComSpecIssue::NotFound)
    } else {
        None
    }
}

/// 是否还有 `%NAME%` 形式的变量引用（`%%` 不算）。
fn has_variable_reference(text: &str) -> bool {
    let mut rest = text;
    while let Some(start) = rest.find('%') {
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(0) => rest = &after[1..],
            Some(_) => return true,
            None => return false,
        }
    }
    false
}

/// `X:\…` / `X:/…` 或 UNC `\\server\…`；与宿主平台无关，便于测试。
fn is_windows_absolute(path: &str) -> bool {
    let bytes = path.as_bytes();
    let drive = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/');
    drive || path.starts_with(r"\\") || path.starts_with("//")
}

#[cfg(windows)]
pub use self::windows::{inspect, repair};

#[cfg(windows)]
mod windows {
    use std::io;
    use std::path::Path;

    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE, RegType};
    use winreg::types::FromRegValue;
    use winreg::{HKEY, RegKey, RegValue};

    use super::{
        ComSpecIssue, ComSpecRepair, ComSpecSource, ComSpecState, ConfiguredComSpec,
        DEFAULT_COMSPEC, expand, plan_repair, validate,
    };

    const VALUE_NAME: &str = "ComSpec";
    const USER_ENVIRONMENT: &str = "Environment";
    const SYSTEM_ENVIRONMENT: &str =
        r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment";

    /// 注册表里的一个字符串变量。
    struct RegString {
        text: String,
        expandable: bool,
    }

    impl RegString {
        /// 系统构建环境块时只展开 `REG_EXPAND_SZ`。引用的变量（`%SystemRoot%` 等）按本进程
        /// 环境展开：它们是开机即定的系统变量，不随这次修改变化。
        fn resolved(&self) -> String {
            if self.expandable {
                expand(&self.text, |name| std::env::var(name).ok())
            } else {
                self.text.clone()
            }
        }
    }

    fn is_file(path: &str) -> bool {
        Path::new(path).is_file()
    }

    /// 读取一级变量；键或值不存在为 `None`。非字符串类型系统建环境块时同样忽略。
    fn read(root: HKEY, path: &str) -> io::Result<Option<RegString>> {
        let key = match RegKey::predef(root).open_subkey_with_flags(path, KEY_READ) {
            Ok(key) => key,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let value = match key.get_raw_value(VALUE_NAME) {
            Ok(value) => value,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let expandable = match value.vtype {
            RegType::REG_EXPAND_SZ => true,
            RegType::REG_SZ => false,
            _ => return Ok(None),
        };
        Ok(Some(RegString {
            text: String::from_reg_value(&value)?,
            expandable,
        }))
    }

    /// 读不出的一级按未设置处理并记日志：诊断仍要给出其余结论。
    fn read_logged(root: HKEY, path: &str) -> Option<RegString> {
        read(root, path).unwrap_or_else(|error| {
            tracing::warn!(path, error = %error, "could not read ComSpec from the registry");
            None
        })
    }

    /// 只读快照：注册表配置值（用户变量覆盖系统变量）与本进程继承的值。
    #[must_use]
    pub fn inspect() -> ComSpecState {
        let configured = read_logged(HKEY_CURRENT_USER, USER_ENVIRONMENT)
            .map(|value| (ComSpecSource::User, value))
            .or_else(|| {
                read_logged(HKEY_LOCAL_MACHINE, SYSTEM_ENVIRONMENT)
                    .map(|value| (ComSpecSource::System, value))
            })
            .map(|(source, value)| ConfiguredComSpec {
                source,
                path: value.resolved(),
                raw: value.text,
            });
        let configured_issue = configured
            .as_ref()
            .map_or(Some(ComSpecIssue::Missing), |configured| {
                validate(&configured.path, is_file)
            });
        let process =
            std::env::var_os(VALUE_NAME).map(|value| value.to_string_lossy().into_owned());
        let process_issue = process
            .as_deref()
            .map_or(Some(ComSpecIssue::Missing), |value| {
                validate(value, is_file)
            });
        ComSpecState {
            configured,
            configured_issue,
            process,
            process_issue,
        }
    }

    /// `REG_EXPAND_SZ`：UTF-16LE 加结尾 NUL。
    fn expand_sz(text: &str) -> RegValue {
        RegValue {
            bytes: text
                .encode_utf16()
                .chain(std::iter::once(0))
                .flat_map(u16::to_le_bytes)
                .collect(),
            vtype: RegType::REG_EXPAND_SZ,
        }
    }

    /// 让当前用户新启动的程序拿到可用的 `ComSpec`，只改 `HKCU\Environment`（无需管理员）。
    /// 广播 `WM_SETTINGCHANGE` 由调用方负责。
    pub fn repair() -> io::Result<ComSpecRepair> {
        let usable = |value: &RegString| validate(&value.resolved(), is_file).is_none();
        let user = read(HKEY_CURRENT_USER, USER_ENVIRONMENT)?;
        let system = read(HKEY_LOCAL_MACHINE, SYSTEM_ENVIRONMENT)?;
        let plan = plan_repair(user.as_ref().map(usable), system.as_ref().map(usable));
        match plan {
            ComSpecRepair::Unchanged => {}
            ComSpecRepair::RemoveUserOverride => {
                RegKey::predef(HKEY_CURRENT_USER)
                    .open_subkey_with_flags(USER_ENVIRONMENT, KEY_WRITE)?
                    .delete_value(VALUE_NAME)?;
            }
            ComSpecRepair::WriteUserDefault => {
                let default = RegString {
                    text: DEFAULT_COMSPEC.to_owned(),
                    expandable: true,
                };
                let path = default.resolved();
                if let Some(issue) = validate(&path, is_file) {
                    return Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        format!("the default command prompt {path} is unusable: {issue}"),
                    ));
                }
                let (key, _) = RegKey::predef(HKEY_CURRENT_USER)
                    .create_subkey_with_flags(USER_ENVIRONMENT, KEY_WRITE)?;
                key.set_raw_value(VALUE_NAME, &expand_sz(DEFAULT_COMSPEC))?;
            }
        }
        tracing::info!(change = plan.name(), "ComSpec repaired");
        Ok(plan)
    }
}

#[cfg(test)]
mod tests {
    use super::{ComSpecIssue, ComSpecRepair, expand, plan_repair, validate};

    fn lookup(name: &str) -> Option<String> {
        // Windows 环境变量名不区分大小写。
        name.eq_ignore_ascii_case("SystemRoot")
            .then(|| r"C:\Windows".to_owned())
    }

    #[test]
    fn expand_matches_expand_environment_strings() {
        assert_eq!(
            expand(r"%SystemRoot%\system32\cmd.exe", lookup),
            r"C:\Windows\system32\cmd.exe"
        );
        assert_eq!(
            expand(r"%systemroot%\cmd.exe", lookup),
            r"C:\Windows\cmd.exe"
        );
        // 未定义的变量原样保留，其结尾的 `%` 仍可开启下一个变量。
        assert_eq!(expand(r"%Nope%SystemRoot%\x", lookup), r"%NopeC:\Windows\x");
        assert_eq!(expand("100%", lookup), "100%");
        assert_eq!(expand("%%", lookup), "%%");
    }

    #[test]
    fn validate_reports_the_first_blocking_issue() {
        let exists = |path: &str| path.eq_ignore_ascii_case(r"C:\Windows\system32\cmd.exe");
        assert_eq!(validate(r"C:\Windows\system32\cmd.exe", exists), None);
        assert_eq!(validate("", exists), Some(ComSpecIssue::Missing));
        // REG_SZ 不展开：浏览器拿到的是字面上的 `%SystemRoot%`。
        assert_eq!(
            validate(r"%SystemRoot%\system32\cmd.exe", exists),
            Some(ComSpecIssue::Unexpanded)
        );
        // 用户反馈的现场：Rust / Chrome 都拒绝以分隔符结尾的程序路径。
        assert_eq!(
            validate(r"C:\Windows\system32\", exists),
            Some(ComSpecIssue::TrailingSeparator)
        );
        assert_eq!(validate("cmd.exe", exists), Some(ComSpecIssue::NotAbsolute));
        assert_eq!(
            validate(r"D:\tools\cmd.exe", exists),
            Some(ComSpecIssue::NotFound)
        );
        assert_eq!(
            validate(r"\\server\share\cmd.exe", exists),
            Some(ComSpecIssue::NotFound)
        );
    }

    #[test]
    fn repair_plan_prefers_the_user_override_and_never_needs_admin() {
        assert_eq!(
            plan_repair(Some(true), Some(false)),
            ComSpecRepair::Unchanged
        );
        assert_eq!(plan_repair(None, Some(true)), ComSpecRepair::Unchanged);
        assert_eq!(
            plan_repair(Some(false), Some(true)),
            ComSpecRepair::RemoveUserOverride
        );
        // 系统变量坏了或缺失：写用户变量而不是改 HKLM。
        assert_eq!(
            plan_repair(Some(false), Some(false)),
            ComSpecRepair::WriteUserDefault
        );
        assert_eq!(
            plan_repair(None, Some(false)),
            ComSpecRepair::WriteUserDefault
        );
        assert_eq!(plan_repair(None, None), ComSpecRepair::WriteUserDefault);
    }
}
