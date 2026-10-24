//! 持久化抽象：本机身份种子 + 已配对设备名册。
//!
//! 宿主实现：agent 落在自己的私有状态文件。[`crate::LinkManager`] 只依赖本 trait。

use async_trait::async_trait;

use super::error::LinkResult;
use super::types::{PeerCandidate, PeerRecord};

/// [`LinkStorage::update_record`] 的原地修改闭包：返回 `true` = 记录有改动，需要写回。
pub type RecordUpdate<'a> = Box<dyn FnOnce(&mut PeerRecord) -> bool + Send + 'a>;

/// 设备互联持久化后端。
///
/// 所有方法都应是幂等、并发安全的；`LinkManager` 会在多个任务里同时调用。
#[async_trait]
pub trait LinkStorage: Send + Sync + 'static {
    /// 读取已持久化的本机身份私钥 seed（32 字节）；从未生成过返回 `None`。
    async fn load_identity_seed(&self) -> LinkResult<Option<[u8; 32]>>;

    /// 持久化新生成的身份私钥 seed。
    async fn save_identity_seed(&self, seed: &[u8; 32]) -> LinkResult<()>;

    /// 落库（新增或覆盖）一条已配对设备。
    async fn upsert(&self, record: &PeerRecord) -> LinkResult<()>;

    /// 读取全部已配对设备（最近活跃降序）。
    async fn list(&self) -> LinkResult<Vec<PeerRecord>>;

    /// 按指纹读取单台设备。
    async fn get(&self, fingerprint: &str) -> LinkResult<Option<PeerRecord>>;

    /// 解除配对。返回是否删到行。
    async fn remove(&self, fingerprint: &str) -> LinkResult<bool>;

    /// 刷新最近活跃时间（Unix 秒）。
    async fn touch(&self, fingerprint: &str, at: i64) -> LinkResult<()>;

    /// 覆盖一台已配对设备的候选端点。返回是否命中已配对设备。
    async fn update_candidates(
        &self,
        fingerprint: &str,
        candidates: &[PeerCandidate],
    ) -> LinkResult<bool>;

    /// 读-改-写一条已配对设备记录：`update` 就地修改记录并返回是否真的有改动，**只有改动时
    /// 才写回 / 落盘**（空闲时的对端信息交换因此零写入）。返回 `None` = 设备不存在；
    /// `Some(changed)` = 设备存在，`changed` 即闭包的返回值。
    ///
    /// 默认实现是 `get` + `upsert`（非原子）；有共享状态锁的后端（agent）应覆盖成锁内原子
    /// 读改写。
    async fn update_record(
        &self,
        fingerprint: &str,
        update: RecordUpdate<'_>,
    ) -> LinkResult<Option<bool>> {
        let Some(mut record) = self.get(fingerprint).await? else {
            return Ok(None);
        };
        if !update(&mut record) {
            return Ok(Some(false));
        }
        self.upsert(&record).await?;
        Ok(Some(true))
    }
}

/// 仅供本 crate 测试的内存后端。
#[cfg(test)]
pub(crate) mod memory {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use async_trait::async_trait;

    use super::{LinkStorage, RecordUpdate};
    use crate::error::{LinkError, LinkResult};
    use crate::types::{PeerCandidate, PeerRecord};

    #[derive(Default)]
    pub(crate) struct MemoryLinkStorage {
        seed: Mutex<Option<[u8; 32]>>,
        peers: Mutex<HashMap<String, PeerRecord>>,
        pub(crate) fail_metadata_writes: std::sync::atomic::AtomicBool,
        /// 身份种子「存在但无法解析」：`load_identity_seed` 报错（而不是当作不存在）。
        pub(crate) seed_unreadable: std::sync::atomic::AtomicBool,
        /// 名册写入次数（upsert / 候选 / 记录更新）：验证「空闲时零写入」。
        pub(crate) writes: std::sync::atomic::AtomicUsize,
    }

    impl MemoryLinkStorage {
        /// 直接读种子（不经 trait，绕过 `seed_unreadable`）。
        pub(crate) fn raw_seed(&self) -> Option<[u8; 32]> {
            self.seed.lock().ok().and_then(|seed| *seed)
        }

        pub(crate) fn write_count(&self) -> usize {
            self.writes.load(std::sync::atomic::Ordering::Relaxed)
        }
    }

    fn poisoned<T>(_: std::sync::PoisonError<T>) -> LinkError {
        LinkError::Store("memory storage poisoned".into())
    }

    #[async_trait]
    impl LinkStorage for MemoryLinkStorage {
        async fn load_identity_seed(&self) -> LinkResult<Option<[u8; 32]>> {
            if self
                .seed_unreadable
                .load(std::sync::atomic::Ordering::Relaxed)
            {
                return Err(LinkError::Store("identity seed is unreadable".into()));
            }
            Ok(*self.seed.lock().map_err(poisoned)?)
        }

        async fn save_identity_seed(&self, seed: &[u8; 32]) -> LinkResult<()> {
            *self.seed.lock().map_err(poisoned)? = Some(*seed);
            Ok(())
        }

        async fn upsert(&self, record: &PeerRecord) -> LinkResult<()> {
            self.peers
                .lock()
                .map_err(poisoned)?
                .insert(record.fingerprint.clone(), record.clone());
            self.writes
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Ok(())
        }

        async fn list(&self) -> LinkResult<Vec<PeerRecord>> {
            let mut all: Vec<PeerRecord> = self
                .peers
                .lock()
                .map_err(poisoned)?
                .values()
                .cloned()
                .collect();
            all.sort_by_key(|record| std::cmp::Reverse(record.last_seen_at));
            Ok(all)
        }

        async fn get(&self, fingerprint: &str) -> LinkResult<Option<PeerRecord>> {
            Ok(self
                .peers
                .lock()
                .map_err(poisoned)?
                .get(fingerprint)
                .cloned())
        }

        async fn remove(&self, fingerprint: &str) -> LinkResult<bool> {
            Ok(self
                .peers
                .lock()
                .map_err(poisoned)?
                .remove(fingerprint)
                .is_some())
        }

        async fn touch(&self, fingerprint: &str, at: i64) -> LinkResult<()> {
            if let Some(record) = self.peers.lock().map_err(poisoned)?.get_mut(fingerprint) {
                record.last_seen_at = at;
            }
            Ok(())
        }

        async fn update_candidates(
            &self,
            fingerprint: &str,
            candidates: &[PeerCandidate],
        ) -> LinkResult<bool> {
            if self
                .fail_metadata_writes
                .load(std::sync::atomic::Ordering::Relaxed)
            {
                return Err(LinkError::Store("metadata write rejected".into()));
            }
            let mut peers = self.peers.lock().map_err(poisoned)?;
            match peers.get_mut(fingerprint) {
                Some(record) => {
                    record.candidates = candidates.to_vec();
                    self.writes
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    Ok(true)
                }
                None => Ok(false),
            }
        }

        async fn update_record(
            &self,
            fingerprint: &str,
            update: RecordUpdate<'_>,
        ) -> LinkResult<Option<bool>> {
            let mut peers = self.peers.lock().map_err(poisoned)?;
            let Some(record) = peers.get_mut(fingerprint) else {
                return Ok(None);
            };
            // 先在副本上试改：写失败注入只在「确有改动」时触发，与真实后端的落盘语义一致。
            let mut edited = record.clone();
            if !update(&mut edited) {
                return Ok(Some(false));
            }
            if self
                .fail_metadata_writes
                .load(std::sync::atomic::Ordering::Relaxed)
            {
                return Err(LinkError::Store("metadata write rejected".into()));
            }
            *record = edited;
            self.writes
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Ok(Some(true))
        }
    }
}
