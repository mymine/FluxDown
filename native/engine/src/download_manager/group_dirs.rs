//! 删除任务组成员（带文件）后清理组留下的空目录。
//!
//! 组成员落在 `组根目录/相对路径` 下，组根目录（`base/sanitize(组名)`）与子目录都是建组时由
//! 引擎创建的；只删成员文件会留下一棵空目录树。删除成功后从每个成员的保存目录向上逐级
//! 移除**空**目录，止于组根目录：组名为空时组根就是用户选的基础目录，只清到其下一级为止。
//! 目录里只剩系统生成的元数据文件（`.DS_Store` 等）也视为空；有任何其他条目的目录原样保留。

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use super::DownloadManager;

/// 系统 / 文件管理器自动生成、不算用户数据的文件名（大小写不敏感）。
const DISPOSABLE_ENTRIES: [&str; 3] = [".ds_store", "thumbs.db", "desktop.ini"];

/// 一个成员的清理范围：从 `dir` 向上到 `root`（`include_root` 决定组根本身是否可删）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct GroupDirTarget {
    pub(super) task_id: String,
    pub(super) dir: PathBuf,
    pub(super) root: PathBuf,
    pub(super) include_root: bool,
}

impl DownloadManager {
    /// 删除前登记组成员的目录（任务行删除后就查不到组归属了）；读库失败只记日志，不阻止删除。
    pub(super) async fn group_dir_targets(&self, task_ids: &[String]) -> Vec<GroupDirTarget> {
        let tasks = match self.db.load_tasks_by_ids(task_ids).await {
            Ok(tasks) => tasks,
            Err(error) => {
                crate::logger::report_warning("download-manager", "load group members", &error);
                return Vec::new();
            }
        };
        if tasks.iter().all(|task| task.group_id.is_empty()) {
            return Vec::new();
        }
        let groups: HashMap<String, (String, String)> = match self.db.load_all_groups().await {
            Ok(groups) => groups
                .into_iter()
                .map(|group| (group.group_id, (group.name, group.save_dir)))
                .collect(),
            Err(error) => {
                crate::logger::report_warning("download-manager", "load task groups", &error);
                return Vec::new();
            }
        };
        tasks
            .into_iter()
            .filter_map(|task| {
                let (name, root) = groups.get(&task.group_id)?;
                (!root.trim().is_empty()).then(|| GroupDirTarget {
                    task_id: task.task_id,
                    dir: PathBuf::from(task.save_dir),
                    root: PathBuf::from(root),
                    include_root: !name.trim().is_empty(),
                })
            })
            .collect()
    }

    /// 已删除成员的空目录清理（阻塞 I/O 放到 blocking 线程）。
    pub(super) async fn prune_group_dirs(&self, targets: Vec<GroupDirTarget>, deleted: &[String]) {
        let deleted: HashSet<&str> = deleted.iter().map(String::as_str).collect();
        let targets: Vec<GroupDirTarget> = targets
            .into_iter()
            .filter(|target| deleted.contains(target.task_id.as_str()))
            .collect();
        if targets.is_empty() {
            return;
        }
        if let Err(error) = tokio::task::spawn_blocking(move || prune_targets(&targets)).await {
            crate::logger::report_warning("download-manager", "prune group directories", &error);
        }
    }
}

/// 同一组的成员共享上层目录：先去重，再从最深的目录开始清理。
fn prune_targets(targets: &[GroupDirTarget]) {
    let mut seen = HashSet::new();
    let mut unique: Vec<&GroupDirTarget> = targets
        .iter()
        .filter(|target| seen.insert((&target.dir, &target.root)))
        .collect();
    unique.sort_by_key(|target| std::cmp::Reverse(target.dir.components().count()));
    for target in unique {
        prune_empty_dirs(&target.dir, &target.root, target.include_root);
    }
}

/// 从 `dir` 向上逐级删除空目录，不越过 `root`；遇到非空目录即停。
pub(super) fn prune_empty_dirs(dir: &Path, root: &Path, include_root: bool) {
    let mut current = dir;
    while current.starts_with(root) {
        let is_root = current == root;
        if (is_root && !include_root) || !remove_if_empty(current) || is_root {
            return;
        }
        match current.parent() {
            Some(parent) => current = parent,
            None => return,
        }
    }
}

/// 目录为空（或只剩可丢弃的系统文件）时删除它；目录已不存在视为成功。
fn remove_if_empty(dir: &Path) -> bool {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return true,
        Err(error) => {
            crate::logger::report_warning("download-manager", "read group directory", &error);
            return false;
        }
    };
    let mut disposable = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                crate::logger::report_warning("download-manager", "read group directory", &error);
                return false;
            }
        };
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if !DISPOSABLE_ENTRIES.contains(&name.as_str()) {
            return false;
        }
        disposable.push(entry.path());
    }
    for path in disposable {
        if let Err(error) = std::fs::remove_file(&path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            crate::logger::report_warning("download-manager", "remove folder metadata", &error);
            return false;
        }
    }
    match std::fs::remove_dir(dir) {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(error) => {
            crate::logger::report_warning(
                "download-manager",
                "remove empty group directory",
                &error,
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::prune_empty_dirs;

    fn fixture(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("fluxdown-group-dirs-{name}-{}", std::process::id()));
        if let Err(error) = fs::remove_dir_all(&dir)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            panic!("reset fixture: {error}");
        }
        fs::create_dir_all(&dir).expect("create fixture");
        dir
    }

    #[test]
    fn removes_empty_tree_up_to_named_group_root() {
        let base = fixture("named");
        let root = base.join("Pack");
        let sub = root.join("extras").join("b");
        fs::create_dir_all(&sub).expect("create subdirs");
        fs::write(root.join("extras").join(".DS_Store"), b"x").expect("write metadata");

        prune_empty_dirs(&sub, &root, true);

        assert!(!root.exists(), "empty group root is removed");
        assert!(base.exists(), "the user's base directory stays");
        fs::remove_dir_all(&base).expect("cleanup");
    }

    #[test]
    fn keeps_directories_with_user_files_and_unnamed_roots() {
        let base = fixture("kept");
        let sub = base.join("extras");
        fs::create_dir_all(&sub).expect("create subdir");
        fs::write(base.join("other.txt"), b"x").expect("write user file");

        // 组名为空：组根就是用户目录，只清到其下一级。
        prune_empty_dirs(&sub, &base, false);
        assert!(!sub.exists());
        assert!(base.join("other.txt").exists());

        let root = base.join("Pack");
        fs::create_dir_all(root.join("a")).expect("create subdir");
        fs::write(root.join("keep.bin"), b"x").expect("write user file");
        prune_empty_dirs(&root.join("a"), &root, true);
        assert!(!root.join("a").exists());
        assert!(root.join("keep.bin").exists(), "non-empty group root stays");
        fs::remove_dir_all(&base).expect("cleanup");
    }
}
