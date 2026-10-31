//! 共享内容表：本机能向别人提供哪些内容（按文件根哈希索引）。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, PoisonError, RwLock};

use super::chunkset::ChunkSet;
use crate::types::ShareFile;

/// 内容的来源。
#[derive(Debug, Clone)]
pub enum ContentSource {
    /// 磁盘上的完整文件；大小或修改时间与登记不符即视为已变。
    Complete {
        /// 文件路径。
        path: PathBuf,
        /// 登记时的修改时间（Unix 毫秒）。
        mtime_ms: i64,
    },
    /// 下载中的文件：只提供 `have` 里的块，读盘后按块哈希再校验一次。
    Partial {
        /// 下载中的临时文件路径。
        path: PathBuf,
        /// 清单里的文件（提供块哈希）。
        file: ShareFile,
        /// 已校验并写入的块。
        have: Arc<ChunkSet>,
    },
}

/// 一项可提供的内容。
#[derive(Debug, Clone)]
pub struct ContentEntry {
    /// 文件大小（字节）。
    pub size: u64,
    /// 来源。
    pub source: ContentSource,
}

/// 内容表。可克隆，克隆体共享同一份内容；宿主整体替换，服务端只读。
#[derive(Clone, Default)]
pub struct ContentRegistry {
    inner: Arc<RwLock<HashMap<[u8; 32], Arc<ContentEntry>>>>,
}

impl std::fmt::Debug for ContentRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContentRegistry")
            .field("entries", &self.len())
            .finish()
    }
}

impl ContentRegistry {
    /// 空内容表。
    pub fn new() -> Self {
        Self::default()
    }

    /// 整体替换。同一 root 出现多次时完整内容优先，其余取先出现者。
    pub fn replace_all(&self, entries: Vec<([u8; 32], ContentEntry)>) {
        let mut next: HashMap<[u8; 32], Arc<ContentEntry>> = HashMap::with_capacity(entries.len());
        for (root, entry) in entries {
            let complete = matches!(entry.source, ContentSource::Complete { .. });
            match next.get(&root) {
                None => {
                    next.insert(root, Arc::new(entry));
                }
                Some(existing)
                    if complete && matches!(existing.source, ContentSource::Partial { .. }) =>
                {
                    next.insert(root, Arc::new(entry));
                }
                Some(_) => {}
            }
        }
        *self.inner.write().unwrap_or_else(PoisonError::into_inner) = next;
    }

    /// 查找内容。
    pub fn get(&self, root: &[u8; 32]) -> Option<Arc<ContentEntry>> {
        self.inner
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(root)
            .cloned()
    }

    fn len(&self) -> usize {
        self.inner
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// 是否没有任何内容。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
