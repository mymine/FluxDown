//! 本机文件做种：把用户磁盘上已有的文件 / 文件夹登记为「已完成 + 做种」的 BT 任务。
//!
//! 两种来源，殊途同归（见 [`crate::download_manager::DownloadManager::create_local_seed`]）：
//! - 只给内容路径：由 [`build_torrent`] 现场为其生成种子（对大文件是分钟级的哈希，
//!   必须在 actor 之外调用）；
//! - 同时给出现成的 `.torrent`：只校验它与所选路径的形态匹配，哈希由续种路径异步校验。
//!
//! 内容路径的布局与完成态任务一致：任务 `save_dir` = 路径的父目录、`file_name` = 路径末段，
//! 续种时单文件落在 `save_dir`、多文件以 `save_dir/<file_name>` 为根。

use std::path::Path;

use thiserror::Error;

/// 本机做种的准备 / 登记错误。
#[derive(Debug, Error)]
pub enum LocalSeedError {
    /// 参数不合法（路径不存在、种子与路径不匹配、BT 被禁用…），调用方应原样回显。
    #[error("{0}")]
    InvalidArgument(String),
    /// 生成种子或落库失败。
    #[error("{0}")]
    Operation(String),
    /// 同一个种子（info hash）已被另一任务持有，未重复创建。
    #[error("duplicate torrent: already managed by task {task_id}")]
    Duplicate {
        /// 已持有该种子的任务 id。
        task_id: String,
        /// 该任务的显示名（可能为空）。
        name: String,
    },
}

/// 通过校验、可直接落库的本机做种计划。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalSeedPlan {
    /// `.torrent` 字节。
    pub torrent_bytes: Vec<u8>,
    /// 任务保存目录（内容路径的父目录）。
    pub save_dir: String,
    /// 任务文件名（内容路径末段，与种子 `name` 一致）。
    pub file_name: String,
    /// 种子内全部文件总大小。
    pub total_bytes: i64,
}

/// 内容路径的末段名与父目录。
fn split_content_path(path: &Path) -> Result<(String, String), LocalSeedError> {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| LocalSeedError::InvalidArgument("path has no file name".to_owned()))?;
    let parent = path
        .parent()
        .map(|parent| parent.to_string_lossy().into_owned())
        .filter(|parent| !parent.is_empty())
        .ok_or_else(|| {
            LocalSeedError::InvalidArgument("path has no parent directory".to_owned())
        })?;
    Ok((parent, name))
}

fn ensure_content_path(path: &Path) -> Result<(), LocalSeedError> {
    if !path.is_absolute() {
        return Err(LocalSeedError::InvalidArgument(
            "path must be absolute".to_owned(),
        ));
    }
    if !path.exists() {
        return Err(LocalSeedError::InvalidArgument(
            "path does not exist".to_owned(),
        ));
    }
    Ok(())
}
/// 任务的分享材料：info hash、展示名与可直接发给别人的磁力链接。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TorrentShare {
    /// 小写十六进制 info hash。
    pub info_hash: String,
    /// 种子名。
    pub name: String,
    /// 磁力链接：种子自带 tracker + 本机会话会追加的 tracker（自定义 / 内置公共 / 订阅，
    /// 对齐 qBittorrent「自动追加 tracker」），让对方不只靠 DHT 就能找到本机。
    pub magnet: String,
}

/// 由 `.torrent` 字节生成分享材料。`extra_trackers` 为会话追加的 tracker 列表。
///
/// # Errors
/// 种子无法解析。
pub fn torrent_share(
    torrent_bytes: &[u8],
    extra_trackers: &[String],
) -> Result<TorrentShare, LocalSeedError> {
    let parsed = librqbit::torrent_from_bytes(torrent_bytes).map_err(|error| {
        LocalSeedError::InvalidArgument(format!("torrent parse error: {error}"))
    })?;
    let info_hash = parsed.info_hash.as_string();
    let name = parsed
        .info
        .data
        .name
        .as_ref()
        .map(|name| String::from_utf8_lossy(name.as_ref()).into_owned())
        .unwrap_or_default();
    let mut magnet = format!("magnet:?xt=urn:btih:{info_hash}");
    if !name.is_empty() {
        magnet.push_str("&dn=");
        magnet.extend(url::form_urlencoded::byte_serialize(name.as_bytes()));
    }
    let own: Vec<String> = parsed
        .iter_announce()
        .map(|tracker| String::from_utf8_lossy(tracker.as_ref()).into_owned())
        .filter(|tracker| !tracker.is_empty())
        .collect();
    let magnet = crate::bt_downloader::magnet_with_trackers(&magnet, &own);
    let magnet = crate::bt_downloader::magnet_with_trackers(&magnet, extra_trackers);
    Ok(TorrentShare {
        info_hash,
        name,
        magnet,
    })
}

/// 为 `path`（文件或文件夹）生成 `.torrent` 字节。`trackers` 为空即无 tracker 种子
/// （靠 DHT / PEX 发现）。哈希在调用线程上做，耗时与数据量成正比，不得在 actor 上调用。
///
/// # Errors
/// 路径不存在 / 非绝对路径、读盘或编码失败。
pub async fn build_torrent(path: &Path, trackers: Vec<String>) -> Result<Vec<u8>, LocalSeedError> {
    ensure_content_path(path)?;
    let (_, name) = split_content_path(path)?;
    let created = librqbit::create_torrent(
        path,
        librqbit::CreateTorrentOptions {
            name: Some(&name),
            trackers,
            piece_length: None,
        },
        &librqbit::spawn_utils::BlockingSpawner::new(1),
    )
    .await
    .map_err(|error| LocalSeedError::Operation(format!("create torrent: {error:#}")))?;
    created
        .as_bytes()
        .map(|bytes| bytes.to_vec())
        .map_err(|error| LocalSeedError::Operation(format!("encode torrent: {error:#}")))
}

/// 校验 `torrent_bytes` 与内容路径 `path` 形态匹配并产出落库计划。
///
/// 文件夹可对应任意多文件种子；单个文件必须对应「只含该文件、名字相同」的单文件种子，
/// 否则续种校验必然失败，不如在这里就给出明确原因。
///
/// # Errors
/// 路径不可用、种子无法解析、种子与路径不匹配。
pub fn plan_local_seed(
    path: &Path,
    torrent_bytes: Vec<u8>,
) -> Result<LocalSeedPlan, LocalSeedError> {
    ensure_content_path(path)?;
    let (save_dir, file_name) = split_content_path(path)?;
    let meta = crate::bt_downloader::probe_torrent_meta(String::new(), torrent_bytes.clone());
    if !meta.error.is_empty() {
        return Err(LocalSeedError::InvalidArgument(meta.error));
    }
    if path.is_file() {
        let single = meta.files.len() == 1
            && meta.name == file_name
            && meta
                .files
                .first()
                .is_some_and(|file| file.path == file_name);
        if !single {
            return Err(LocalSeedError::InvalidArgument(
                "the torrent does not describe the selected file".to_owned(),
            ));
        }
    } else if meta.files.is_empty() {
        return Err(LocalSeedError::InvalidArgument(
            "the torrent contains no files".to_owned(),
        ));
    }
    Ok(LocalSeedPlan {
        torrent_bytes,
        save_dir,
        file_name,
        total_bytes: meta.total_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "fluxdown-local-seed-{tag}-{}",
            uuid::Uuid::now_v7()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn built_torrent_matches_its_own_file_and_folder() {
        let root = temp_dir("plan");
        let file = root.join("one.bin");
        std::fs::write(&file, vec![7u8; 100_000]).expect("file");
        let folder = root.join("pack");
        std::fs::create_dir_all(&folder).expect("folder");
        std::fs::write(folder.join("a.bin"), vec![1u8; 40_000]).expect("a");
        std::fs::write(folder.join("b.bin"), vec![2u8; 40_000]).expect("b");

        let bytes = build_torrent(&file, Vec::new())
            .await
            .expect("file torrent");
        let plan = plan_local_seed(&file, bytes).expect("file plan");
        assert_eq!(plan.file_name, "one.bin");
        assert_eq!(plan.total_bytes, 100_000);
        assert_eq!(plan.save_dir, root.to_string_lossy());

        let bytes = build_torrent(&folder, vec!["udp://t.example:80/announce".into()])
            .await
            .expect("folder torrent");
        let plan = plan_local_seed(&folder, bytes.clone()).expect("folder plan");
        assert_eq!(plan.file_name, "pack");
        assert_eq!(plan.total_bytes, 80_000);

        // 多文件种子不能配到单个文件上；单文件种子配到别名文件同样拒绝。
        assert!(matches!(
            plan_local_seed(&file, bytes),
            Err(LocalSeedError::InvalidArgument(_))
        ));
        let other = root.join("other.bin");
        std::fs::write(&other, vec![7u8; 100_000]).expect("other");
        let single = build_torrent(&file, Vec::new()).await.expect("torrent");
        assert!(matches!(
            plan_local_seed(&other, single),
            Err(LocalSeedError::InvalidArgument(_))
        ));
        assert!(matches!(
            plan_local_seed(&root.join("missing"), Vec::new()),
            Err(LocalSeedError::InvalidArgument(_))
        ));
        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn share_magnet_carries_hash_name_own_and_extra_trackers_once() {
        let root = temp_dir("share");
        let file = root.join("my file.bin");
        std::fs::write(&file, vec![9u8; 50_000]).expect("file");
        let own = "udp://own.example:6969/announce".to_owned();
        let bytes = build_torrent(&file, vec![own.clone()])
            .await
            .expect("torrent");
        let extra = vec![own.clone(), "https://extra.example/announce".to_owned()];
        let share = torrent_share(&bytes, &extra).expect("share");

        let magnet = librqbit::Magnet::parse(&share.magnet).expect("parsable magnet");
        assert_eq!(
            magnet.as_id20().map(|id| id.as_string()),
            Some(share.info_hash.clone())
        );
        assert_eq!(share.name, "my file.bin");
        // 自带 tracker 与追加 tracker 合并且不重复。
        assert_eq!(
            magnet.trackers,
            vec![own, "https://extra.example/announce".to_owned()]
        );
        assert!(torrent_share(b"nope", &[]).is_err());
        std::fs::remove_dir_all(&root).expect("cleanup");
    }
}
