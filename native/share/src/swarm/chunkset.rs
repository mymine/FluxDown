//! 块位图：记录一个文件的哪些 4 MiB 块已经持有（并通过校验）。

use std::sync::atomic::{AtomicU64, Ordering};

/// 线程安全的块位图。可在下载线程置位的同时被共享服务端 / 调度器读取。
///
/// 线上位序：第 `i` 块对应第 `i / 8` 字节的 `1 << (i % 8)` 位，字节数 `ceil(len / 8)`。
#[derive(Debug)]
pub struct ChunkSet {
    words: Box<[AtomicU64]>,
    len: u64,
    count: AtomicU64,
}

impl ChunkSet {
    /// 容纳 `len` 个块的空集合。`len` 大到无法分配时返回零长度集合（不 panic）。
    pub fn new(len: u64) -> Self {
        let words = len
            .div_ceil(64)
            .try_into()
            .ok()
            .map(|n: usize| (0..n).map(|_| AtomicU64::new(0)).collect::<Box<[_]>>());
        match words {
            Some(words) => Self {
                words,
                len,
                count: AtomicU64::new(0),
            },
            None => Self {
                words: Box::default(),
                len: 0,
                count: AtomicU64::new(0),
            },
        }
    }

    /// 块总数。
    pub fn len(&self) -> u64 {
        self.len
    }

    /// 是否没有任何块位（`len == 0`）。
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 已持有的块数。
    pub fn count(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }

    fn slot(&self, index: u64) -> Option<(&AtomicU64, u64)> {
        if index >= self.len {
            return None;
        }
        let word = self.words.get(usize::try_from(index / 64).ok()?)?;
        Some((word, 1u64 << (index % 64)))
    }

    /// 第 `index` 块是否已持有；越界为 `false`。
    pub fn contains(&self, index: u64) -> bool {
        self.slot(index)
            .is_some_and(|(word, mask)| word.load(Ordering::Acquire) & mask != 0)
    }

    /// 置位；返回是否由未持有变为持有。越界返回 `false`。
    pub fn insert(&self, index: u64) -> bool {
        let Some((word, mask)) = self.slot(index) else {
            return false;
        };
        let fresh = word.fetch_or(mask, Ordering::AcqRel) & mask == 0;
        if fresh {
            self.count.fetch_add(1, Ordering::Relaxed);
        }
        fresh
    }

    /// 清位；返回是否由持有变为未持有。越界返回 `false`。
    pub fn remove(&self, index: u64) -> bool {
        let Some((word, mask)) = self.slot(index) else {
            return false;
        };
        let had = word.fetch_and(!mask, Ordering::AcqRel) & mask != 0;
        if had {
            self.count.fetch_sub(1, Ordering::Relaxed);
        }
        had
    }

    /// 按线上位序导出，长度 `ceil(len / 8)`。
    pub fn to_bytes(&self) -> Vec<u8> {
        let bytes = usize::try_from(self.len.div_ceil(8)).unwrap_or(0);
        let mut out = Vec::with_capacity(bytes);
        for word in self.words.iter() {
            out.extend_from_slice(&word.load(Ordering::Acquire).to_le_bytes());
        }
        out.truncate(bytes);
        out
    }

    /// 从线上位图还原。长度不是 `ceil(len / 8)`，或有 `>= len` 的位被置位时返回 `None`。
    pub fn from_bytes(len: u64, bytes: &[u8]) -> Option<Self> {
        if u64::try_from(bytes.len()).ok()? != len.div_ceil(8) {
            return None;
        }
        let set = Self::new(len);
        if set.len != len {
            return None;
        }
        let mut count = 0u64;
        for (word, part) in set.words.iter().zip(bytes.chunks(8)) {
            let mut raw = [0u8; 8];
            raw[..part.len()].copy_from_slice(part);
            let value = u64::from_le_bytes(raw);
            count += u64::from(value.count_ones());
            word.store(value, Ordering::Relaxed);
        }
        // 末尾填充位必须为 0：位总数不得超过 len，且最高位不得越界。
        if count > len {
            return None;
        }
        if let Some(last) = set.words.last() {
            let used = len - (set.words.len() as u64 - 1) * 64;
            if used < 64 && last.load(Ordering::Relaxed) >> used != 0 {
                return None;
            }
        }
        set.count.store(count, Ordering::Relaxed);
        Some(set)
    }
}
