//! 插件包修订号：桌面端据此判断插件包代码是否被替换、需要重新挂载桌面脚本视图。
//!
//! 只在包代码被替换的生命周期动作成功后递增（安装 / 升级、dev 安装、dev 重载），
//! 且只递增被操作的那个插件；启停、授权、撤回、熔断等触发的 `load_all` 不递增，
//! 其他插件的界面状态因此不受影响。
//!
//! 值只用于相等比较。起点取进程启动时的 Unix 微秒数，此后每次递增从同一全局计数器
//! 取下一个值：daemon 重启后的新起点远大于旧进程分出的任何值（旧值 ≤ 旧起点 + 用户
//! 操作次数），重启前后不会撞值，桌面端不会漏掉重新挂载。

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

pub(super) struct PackageRevisions {
    inner: Mutex<Inner>,
}

struct Inner {
    /// 本进程内未递增过的插件共用的修订号。
    base: u64,
    /// 最近分出的修订号（全局单调）。
    last: u64,
    by_identity: HashMap<String, u64>,
}

impl PackageRevisions {
    pub(super) fn new() -> Self {
        let base = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX / 2))
            .unwrap_or(0);
        Self::starting_at(base)
    }

    fn starting_at(base: u64) -> Self {
        Self {
            inner: Mutex::new(Inner {
                base,
                last: base,
                by_identity: HashMap::new(),
            }),
        }
    }

    /// 插件包代码已被替换：为该插件分配新的修订号。
    pub(super) fn bump(&self, identity: &str) {
        let mut inner = self.lock();
        inner.last = inner.last.wrapping_add(1);
        let next = inner.last;
        inner.by_identity.insert(identity.to_owned(), next);
    }

    pub(super) fn get(&self, identity: &str) -> u64 {
        let inner = self.lock();
        inner
            .by_identity
            .get(identity)
            .copied()
            .unwrap_or(inner.base)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // 临界区只做整数与哈希表操作、不会 panic；即便中毒，数据仍然自洽。
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::PackageRevisions;

    #[test]
    fn bump_changes_only_the_target_plugin() {
        let revisions = PackageRevisions::starting_at(100);
        assert_eq!(revisions.get("a@x"), revisions.get("b@x"));
        let before_b = revisions.get("b@x");
        let before_a = revisions.get("a@x");

        revisions.bump("a@x");

        assert_ne!(revisions.get("a@x"), before_a);
        assert_eq!(revisions.get("b@x"), before_b);
    }

    #[test]
    fn every_bump_yields_a_fresh_value() {
        let revisions = PackageRevisions::starting_at(100);
        revisions.bump("a@x");
        let first = revisions.get("a@x");
        revisions.bump("b@x");
        revisions.bump("a@x");
        let second = revisions.get("a@x");
        assert_ne!(first, second);
        assert_ne!(second, revisions.get("b@x"));
    }

    #[test]
    fn a_restarted_process_never_reuses_earlier_values() {
        let old = PackageRevisions::starting_at(1_000);
        for _ in 0..5 {
            old.bump("a@x");
        }
        let restarted = PackageRevisions::new();
        assert!(restarted.get("a@x") > old.get("a@x"));
    }
}
