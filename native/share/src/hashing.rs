//! 4 MiB 分块 BLAKE3 哈希与块校验。
//!
//! - 块大小固定 [`CHUNK_SIZE`]（4 MiB），最后一块可更短，空文件没有块。
//! - 每块哈希是该块字节的 BLAKE3。
//! - **根哈希** = `BLAKE3(chunk_hash_0 ‖ chunk_hash_1 ‖ …)`，即所有块哈希的 32 字节原始值
//!   按块序拼接后再做一次 BLAKE3；空文件的根哈希是空输入的 BLAKE3。
//!   选用“哈希的哈希”而非 BLAKE3 树根，是为了接收方只凭清单里的块哈希列表就能复核根哈希，
//!   不依赖 BLAKE3 内部树结构。

use std::fs::File;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result};
use tokio_util::sync::CancellationToken;

use crate::types::ShareFile;

/// 块大小：4 MiB。
pub const CHUNK_SIZE: u64 = 4 * 1024 * 1024;

/// 读文件的缓冲区大小。
const READ_BUF: usize = 1024 * 1024;

/// 哈希进度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HashProgress {
    /// 已处理字节数。
    pub done: u64,
    /// 文件总字节数。
    pub total: u64,
}

/// 块校验失败原因。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChunkError {
    /// 块下标超出清单。
    #[error("chunk index {0} is out of range")]
    OutOfRange(u64),
    /// 清单里该块没有合法的哈希。
    #[error("manifest has no valid hash for chunk {0}")]
    MissingHash(u64),
    /// 数据长度与该块应有的长度不符。
    #[error("chunk {index} has {got} bytes, expected {expected}")]
    BadLength {
        /// 块下标。
        index: u64,
        /// 应有字节数。
        expected: u64,
        /// 实际字节数。
        got: u64,
    },
    /// BLAKE3 不一致。
    #[error("chunk {0} failed BLAKE3 verification")]
    HashMismatch(u64),
}

/// 小写 hex 编码。
pub fn to_hex(bytes: &[u8]) -> String {
    data_encoding::HEXLOWER.encode(bytes)
}

/// 解析 64 字符 hex 为 32 字节哈希；大小写均接受，非法返回 `None`。
pub fn parse_hash(hex: &str) -> Option<[u8; 32]> {
    let bytes = data_encoding::HEXLOWER_PERMISSIVE
        .decode(hex.as_bytes())
        .ok()?;
    <[u8; 32]>::try_from(bytes.as_slice()).ok()
}

/// 由块哈希（32 字节原始值）计算根哈希。
pub fn root_of_chunks<'a>(chunk_hashes: impl IntoIterator<Item = &'a [u8; 32]>) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    for hash in chunk_hashes {
        hasher.update(hash);
    }
    *hasher.finalize().as_bytes()
}

/// 复核清单中某个文件的根哈希与其块哈希列表是否自洽。
pub fn manifest_root_is_consistent(file: &ShareFile) -> bool {
    let Some(chunks) = file
        .chunk_hashes
        .iter()
        .map(|h| parse_hash(h))
        .collect::<Option<Vec<_>>>()
    else {
        return false;
    };
    if chunks.len() as u64 != file.chunk_count() {
        return false;
    }
    parse_hash(&file.root_hash) == Some(root_of_chunks(chunks.iter()))
}

/// 校验第 `index` 块的数据：长度与 BLAKE3 都必须与清单一致。
pub fn verify_chunk(file: &ShareFile, index: u64, data: &[u8]) -> Result<(), ChunkError> {
    let (start, end) = file
        .chunk_range(index)
        .ok_or(ChunkError::OutOfRange(index))?;
    let expected = end - start;
    let got = data.len() as u64;
    if got != expected {
        return Err(ChunkError::BadLength {
            index,
            expected,
            got,
        });
    }
    let want = file
        .chunk_hash(index)
        .ok_or(ChunkError::MissingHash(index))?;
    if blake3::hash(data).as_bytes() != &want {
        return Err(ChunkError::HashMismatch(index));
    }
    Ok(())
}

/// 文件修改时间的 Unix 毫秒数（早于 1970 为负）。
///
/// 清单登记与发送方“文件被改”检测必须使用同一个函数，保证两边口径一致。
pub fn file_mtime_ms(meta: &std::fs::Metadata) -> std::io::Result<i64> {
    let modified = meta.modified()?;
    Ok(match modified.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_millis()).unwrap_or(i64::MAX),
        Err(e) => i64::try_from(e.duration().as_millis()).map_or(i64::MIN, |v| -v),
    })
}

/// 同步流式哈希：一次顺序读，边读边喂块哈希器。
///
/// 读取前后各取一次元数据，传输期间文件被改会返回错误（避免登记出内容与哈希不符的清单）。
/// `on_progress` 每读满一个缓冲区调用一次；`cancel` 被取消时返回错误。
pub fn hash_file_blocking(
    path: &Path,
    name: Option<String>,
    mut on_progress: impl FnMut(HashProgress),
    cancel: &CancellationToken,
) -> Result<ShareFile> {
    let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let before = file.metadata().context("stat before hashing")?;
    anyhow::ensure!(before.is_file(), "{} is not a regular file", path.display());
    let size = before.len();
    let mtime_ms = file_mtime_ms(&before).context("read mtime")?;

    let mut chunk_hashes: Vec<[u8; 32]> = Vec::new();
    let mut hasher = blake3::Hasher::new();
    let mut in_chunk: u64 = 0;
    let mut done: u64 = 0;
    let mut buf = vec![0u8; READ_BUF];
    loop {
        anyhow::ensure!(!cancel.is_cancelled(), "hashing cancelled");
        let n = file.read(&mut buf).context("read file")?;
        if n == 0 {
            break;
        }
        let mut rest = buf.get(..n).context("read returned more than the buffer")?;
        while !rest.is_empty() {
            let room = usize::try_from(CHUNK_SIZE - in_chunk).unwrap_or(usize::MAX);
            let take = room.min(rest.len());
            let (head, tail) = rest.split_at(take);
            hasher.update(head);
            in_chunk += take as u64;
            rest = tail;
            if in_chunk == CHUNK_SIZE {
                chunk_hashes.push(*hasher.finalize().as_bytes());
                hasher.reset();
                in_chunk = 0;
            }
        }
        done += n as u64;
        on_progress(HashProgress { done, total: size });
    }
    if in_chunk > 0 {
        chunk_hashes.push(*hasher.finalize().as_bytes());
    }

    let after = file.metadata().context("stat after hashing")?;
    anyhow::ensure!(
        done == size && after.len() == size && file_mtime_ms(&after).ok() == Some(mtime_ms),
        "{} changed while it was being hashed",
        path.display()
    );

    let root = root_of_chunks(chunk_hashes.iter());
    let name = match name {
        Some(n) => n,
        None => path
            .file_name()
            .and_then(|n| n.to_str())
            .map(str::to_owned)
            .with_context(|| format!("{} has no UTF-8 file name", path.display()))?,
    };
    Ok(ShareFile {
        name,
        size,
        mtime_ms,
        chunk_hashes: chunk_hashes.iter().map(|h| to_hex(h)).collect(),
        root_hash: to_hex(&root),
    })
}

/// 在阻塞线程上哈希文件，不占用异步运行时线程。
///
/// `on_progress` 在阻塞线程内同步调用，应当很轻（例如发一个 channel 消息）。
pub async fn hash_file(
    path: std::path::PathBuf,
    name: Option<String>,
    on_progress: impl FnMut(HashProgress) + Send + 'static,
    cancel: CancellationToken,
) -> Result<ShareFile> {
    tokio::task::spawn_blocking(move || hash_file_blocking(&path, name, on_progress, &cancel))
        .await
        .context("hashing task failed")?
}
