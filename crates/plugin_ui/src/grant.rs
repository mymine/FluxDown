//! 插件界面脚本的授权：用户选定的「装即信任」模型（同 Tern / Neovim）。
//!
//! gpui-shell 默认全拒；这里给出它能表达的最大授权：整个文件系统读写、任意进程、
//! 剪贴板读写、localStorage。两项上游无法表达的边界：网络只接受逐个主机名（无通配，
//! 取该视图 `contributes.views[].desktop.networkHosts`），以及 `process.exit` 永不授予（会退出整个桌面端）。

use std::path::{Path, PathBuf};

use gpui_shell::{Capabilities, ExecuteGrant};

/// 构造一个插件界面的授权。
pub(crate) fn full_trust(network_hosts: &[String]) -> Capabilities {
    let roots = filesystem_roots();
    Capabilities::new()
        .read_roots(roots.clone())
        .write_roots(roots)
        .execute(ExecuteGrant::Unrestricted)
        .network_hosts(network_hosts.iter().cloned())
        .storage(true)
        .clipboard_read(true)
        .clipboard_write(true)
        .exit(false)
}

/// 授权摘要（写入审计 `desktop.load` 的 detail）：与 [`full_trust`] 给出的能力一一对应。
/// 例：`fs:all process:all network:api.x.com clipboard:rw`，无网络主机时为 `network:none`。
pub(crate) fn summary(network_hosts: &[String]) -> String {
    let network = if network_hosts.is_empty() {
        "none".to_owned()
    } else {
        network_hosts.join(",")
    };
    format!("fs:all process:all network:{network} clipboard:rw")
}

/// 本机全部文件系统根：Unix 为 `/`，Windows 为存在的盘符根。
fn filesystem_roots() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        (b'A'..=b'Z')
            .map(|letter| PathBuf::from(format!("{}:\\", char::from(letter))))
            .filter(|root| root.is_dir())
            .collect()
    }
    #[cfg(not(windows))]
    {
        vec![PathBuf::from("/")]
    }
}

/// 插件界面的数据目录：`<root>/<编码后的 identity>`。
///
/// 编码与引擎 `plugin_workspace` 同规则（无碰撞）：`[A-Za-z0-9-]` 原样，其余字节写成
/// `_XX`（小写十六进制），`_` 自身也编码。桌面端不依赖引擎，故在此镜像。
pub(crate) fn data_dir(root: &Path, identity: &str) -> PathBuf {
    let mut encoded = String::with_capacity(identity.len());
    for byte in identity.bytes() {
        if byte.is_ascii_alphanumeric() || byte == b'-' {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("_{byte:02x}"));
        }
    }
    root.join(encoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_encoding_is_collision_free() {
        let root = Path::new("/data");
        assert_eq!(
            data_dir(root, "zerx_lab@ytdlp"),
            PathBuf::from("/data/zerx_5flab_40ytdlp")
        );
        assert_ne!(data_dir(root, "a_b@c"), data_dir(root, "a.b@c"));
    }

    #[test]
    fn summary_lists_every_granted_capability() {
        assert_eq!(
            summary(&["api.x.com".to_owned()]),
            "fs:all process:all network:api.x.com clipboard:rw"
        );
        assert_eq!(
            summary(&["a.com".to_owned(), "b.com".to_owned()]),
            "fs:all process:all network:a.com,b.com clipboard:rw"
        );
        assert_eq!(summary(&[]), "fs:all process:all network:none clipboard:rw");
    }
    #[test]
    fn grant_never_allows_exiting_the_desktop() {
        let grant = full_trust(&["api.example.com".to_owned()]);
        assert!(!grant.may_exit());
        assert!(grant.may_run("ffprobe"));
        assert!(grant.may_reach("api.example.com"));
        assert!(!grant.may_reach("other.example.com"));
    }
}
