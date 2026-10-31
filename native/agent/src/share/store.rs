//! FluxMesh的本机持久化：iroh 私钥（独立 0600 文件）、本机设置、分享登记、接收记录。
//!
//! 全部在 `<agent 数据目录>/share/` 下，与 `agent-state.json` 分离：私钥不随云同步 / 状态快照，
//! 进度类数据从不落盘（只在状态变化时写入这些小文件）；云端暂存上传另有分片完成时才写的断点文件。

use std::path::{Path, PathBuf};

use fluxdown_protocol::{ShareDto, ShareLocalConfigDto, ShareProvideState, ShareReceiveRecordDto};
use fluxdown_share::{SecretKey, ShareManifest};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::upload_state::UploadState;

/// 持久化失败（带操作上下文的文本）。
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct StoreError(String);

impl StoreError {
    fn new(what: &str, path: &Path, error: impl std::fmt::Display) -> Self {
        Self(format!("{what} {}: {error}", path.display()))
    }
}

pub(super) type Result<T, E = StoreError> = std::result::Result<T, E>;

/// 一个本机登记的分享：云端 DTO + 清单 + 本机文件路径（重启后据此重新登记发送方）。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct StoredShare {
    pub dto: ShareDto,
    pub manifest: ShareManifest,
    pub paths: Vec<PathBuf>,
    /// 未登录时以游客发送方身份创建：云端归属游客 endpoint，修改 / 停止 / 对账必须用游客令牌。
    #[serde(default)]
    pub guest_owned: bool,
}

/// `provide_daily.json`：当日序号与已共享上传的字节。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct ProvideDaily {
    pub day: i64,
    pub used: u64,
}

/// 接收记录保留上限。
pub(super) const RECORD_LIMIT: usize = 200;

/// 一次接收的共享状态：持有领取时的票据（登记共享时证明「确实领取过」），文件随下载完成逐个加入。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct StoredProvide {
    pub record_id: String,
    pub share_id: String,
    pub ticket: String,
    /// 原分享的到期时间（RFC 3339）；到期后共享自动结束。
    #[serde(default)]
    pub share_expires_at: Option<String>,
    #[serde(default)]
    pub files: Vec<ProvideFile>,
    pub state: ShareProvideState,
}

/// 已完整落盘、可提供给他人的一个文件。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProvideFile {
    pub task_id: String,
    pub root_hash: String,
    pub size: u64,
    pub mtime_ms: i64,
    pub path: PathBuf,
}

#[derive(Clone)]
pub(super) struct ShareStore {
    dir: PathBuf,
}

impl ShareStore {
    pub(super) fn new(data_dir: &Path) -> Self {
        Self {
            dir: data_dir.join("share"),
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    async fn ensure_dir(&self) -> Result<()> {
        tokio::fs::create_dir_all(&self.dir)
            .await
            .map_err(|e| StoreError::new("create", &self.dir, e))?;
        crate::state::set_private_dir_permissions(&self.dir)
            .await
            .map_err(|e| StoreError::new("restrict", &self.dir, e))
    }

    /// 读取（或首次生成）iroh 私钥：32 字节 hex，文件权限 0600。
    pub(super) async fn load_or_create_secret(&self) -> Result<SecretKey> {
        let path = self.path("secret.key");
        match tokio::fs::read_to_string(&path).await {
            Ok(text) => {
                let bytes = fluxdown_share::hashing::parse_hash(text.trim())
                    .ok_or_else(|| StoreError::new("parse", &path, "not a 32-byte hex key"))?;
                return Ok(SecretKey::from_bytes(&bytes));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(StoreError::new("read", &path, error)),
        }
        let key = SecretKey::generate();
        let hex = fluxdown_share::hashing::to_hex(&key.to_bytes());
        self.write_private(&path, hex.as_bytes()).await?;
        Ok(key)
    }

    pub(super) async fn load_config(&self) -> ShareLocalConfigDto {
        self.read_json("config.json").await.unwrap_or_default()
    }

    pub(super) async fn save_config(&self, config: &ShareLocalConfigDto) -> Result<()> {
        self.write_json("config.json", config).await
    }

    /// 共享提供的当日用量（反 PCDN 每日上限的跨重启延续）。
    pub(super) async fn load_daily(&self) -> Option<ProvideDaily> {
        self.read_json("provide_daily.json").await
    }

    pub(super) async fn save_daily(&self, daily: &ProvideDaily) -> Result<()> {
        self.write_json("provide_daily.json", daily).await
    }

    pub(super) async fn load_shares(&self) -> Vec<StoredShare> {
        self.read_json("shares.json").await.unwrap_or_default()
    }

    pub(super) async fn save_shares(&self, shares: &[StoredShare]) -> Result<()> {
        self.write_json("shares.json", shares).await
    }

    pub(super) async fn load_records(&self) -> Vec<ShareReceiveRecordDto> {
        self.read_json("receive-records.json")
            .await
            .unwrap_or_default()
    }

    pub(super) async fn save_records(&self, records: &[ShareReceiveRecordDto]) -> Result<()> {
        self.write_json("receive-records.json", records).await
    }

    pub(super) async fn load_provides(&self) -> Vec<StoredProvide> {
        self.read_json("provides.json").await.unwrap_or_default()
    }

    pub(super) async fn save_provides(&self, provides: &[StoredProvide]) -> Result<()> {
        self.write_json("provides.json", provides).await
    }

    // ───────────── 云端暂存上传断点（`uploads/<shareId>.json`） ─────────────

    /// 断点文件名：分享 ID 来自云端，仍只放行文件名安全的字符。
    fn upload_name(share_id: &str) -> Result<String> {
        if share_id.is_empty()
            || !share_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        {
            return Err(StoreError::new(
                "upload state name",
                Path::new(share_id),
                "unsafe share id",
            ));
        }
        Ok(format!("uploads/{share_id}.json"))
    }

    pub(super) async fn load_upload(&self, share_id: &str) -> Option<UploadState> {
        match Self::upload_name(share_id) {
            Ok(name) => self.read_json(&name).await,
            Err(error) => {
                tracing::warn!(error = %error, "upload state skipped");
                None
            }
        }
    }

    pub(super) async fn save_upload(&self, share_id: &str, state: &UploadState) -> Result<()> {
        let name = Self::upload_name(share_id)?;
        self.ensure_dir().await?;
        let dir = self.dir.join("uploads");
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| StoreError::new("create", &dir, e))?;
        crate::state::set_private_dir_permissions(&dir)
            .await
            .map_err(|e| StoreError::new("restrict", &dir, e))?;
        self.write_json(&name, state).await
    }

    /// 删除断点文件；本来就没有不算错。
    pub(super) async fn remove_upload(&self, share_id: &str) -> Result<()> {
        let path = self.path(&Self::upload_name(share_id)?);
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(StoreError::new("remove", &path, error)),
        }
    }

    /// 缺失 → `None`；损坏 → 记录警告后当作缺失（文件很小且可重建，不阻塞启动）。
    async fn read_json<T: DeserializeOwned>(&self, name: &str) -> Option<T> {
        let path = self.path(name);
        let text = match tokio::fs::read_to_string(&path).await {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
            Err(error) => {
                tracing::warn!(path = %path.display(), error = %error, "share state file unreadable");
                return None;
            }
        };
        match serde_json::from_str(&text) {
            Ok(value) => Some(value),
            Err(error) => {
                tracing::warn!(path = %path.display(), error = %error, "share state file is corrupt; ignored");
                None
            }
        }
    }

    async fn write_json<T: Serialize + ?Sized>(&self, name: &str, value: &T) -> Result<()> {
        let path = self.path(name);
        let bytes =
            serde_json::to_vec(value).map_err(|e| StoreError::new("serialize", &path, e))?;
        self.write_private(&path, &bytes).await
    }

    /// 临时文件写满再改名：崩溃不会留下半截文件。
    async fn write_private(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        self.ensure_dir().await?;
        let tmp = path.with_extension("tmp");
        tokio::fs::write(&tmp, bytes)
            .await
            .map_err(|e| StoreError::new("write", &tmp, e))?;
        crate::state::set_private_file_permissions(&tmp)
            .await
            .map_err(|e| StoreError::new("restrict", &tmp, e))?;
        tokio::fs::rename(&tmp, path)
            .await
            .map_err(|e| StoreError::new("replace", path, e))
    }
}
