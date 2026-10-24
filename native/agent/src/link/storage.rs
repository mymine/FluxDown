//! agent 私有状态文件里的设备互联存储：本机身份种子 + 已配对设备名册。
//!
//! 名册沿用 daemon 迁移时写入 `AgentState::linked_devices` 的 JSON 形状（`fingerprint` /
//! `identityPubB64` / `name` / `platform` / `linkSecretB64` / `candidatesJson` / `pairedAt` /
//! `lastSeenAt`），并向后兼容地追加可选字段 `defaultSaveDir` / `pathStyle` / `appVersion` /
//! `cloudDeviceId`（对端经已认证链路自报）、`customName`（本机备注名）、`revoked`（对端已解除
//! 配对）与控制授权 `grantsControl`（该对端可经 `/rpc` 控制本机）/ `controllable`（本机可控制该
//! 对端），后两者 `false` 时不落盘。旧记录没有这些字段，读回时取默认值。链路密钥只存在这里与内存里，
//! 永远不进快照 / 事件。

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use fluxdown_link::{LinkError, LinkResult, LinkStorage, PeerCandidate, PeerRecord, RecordUpdate};
use fluxdown_protocol::{LinkDeviceInfo, PathStyle};
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::state::{AgentState, StateStore};

/// 基于 agent 状态文件的 [`LinkStorage`]。
pub struct AgentLinkStorage {
    state: Arc<Mutex<AgentState>>,
    store: Arc<StateStore>,
}

impl AgentLinkStorage {
    #[must_use]
    pub fn new(state: Arc<Mutex<AgentState>>, store: Arc<StateStore>) -> Self {
        Self { state, store }
    }

    async fn persist(&self, state: &AgentState) -> LinkResult<()> {
        self.store
            .save(state)
            .await
            .map_err(|error| LinkError::Store(format!("{error:#}")))
    }

    /// 名册改动后落盘；失败时把内存里的名册恢复成改动之前的样子。否则界面 / 数据面看到的名册与
    /// 磁盘不一致：这次改动在重启后凭空消失（新配对的设备、被撤销的标记、解除的配对都会「复活」）。
    async fn persist_roster(&self, state: &mut AgentState, previous: Vec<Value>) -> LinkResult<()> {
        let result = self.persist(state).await;
        if result.is_err() {
            state.linked_devices = previous;
        }
        result
    }
}

/// 身份种子可能是 daemon 迁移来的 base64 字符串，也可能是早期 agent 自行生成的
/// `{ "secretB64": … }` 对象；两种都认。
fn seed_from_identity(value: &Value) -> Option<[u8; 32]> {
    let text = match value {
        Value::String(text) => text.as_str(),
        Value::Object(map) => map.get("secretB64")?.as_str()?,
        _ => return None,
    };
    let bytes = B64.decode(text.trim()).ok()?;
    <[u8; 32]>::try_from(bytes.as_slice()).ok()
}

fn path_style_text(style: PathStyle) -> Option<&'static str> {
    match style {
        PathStyle::Windows => Some("windows"),
        PathStyle::Posix => Some("posix"),
        PathStyle::Unknown => None,
    }
}

fn path_style_from_text(text: &str) -> Option<PathStyle> {
    match text {
        "windows" => Some(PathStyle::Windows),
        "posix" => Some(PathStyle::Posix),
        _ => None,
    }
}

/// 布尔字段：缺失或类型不对一律 `false`（旧数据没有这些键）。
fn bool_field(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// 把名册里的一条 JSON 解析为强类型记录；缺关键字段（指纹 / 公钥 / 密钥）的坏条目返回
/// `None`（不让一条坏数据毒死整份名册）。
pub(super) fn record_from_value(value: &Value) -> Option<PeerRecord> {
    let text = |key: &str| value.get(key).and_then(Value::as_str);
    let non_empty = |key: &str| text(key).filter(|text| !text.is_empty()).map(str::to_owned);
    let bytes = |key: &str| text(key).and_then(|s| B64.decode(s).ok());
    let fingerprint = text("fingerprint")?.to_owned();
    let identity_pub = bytes("identityPubB64")?;
    let link_secret = bytes("linkSecretB64").filter(|secret| !secret.is_empty())?;
    let candidates: Vec<PeerCandidate> = text("candidatesJson")
        .and_then(|json| serde_json::from_str(json).ok())
        .unwrap_or_default();
    Some(PeerRecord {
        fingerprint,
        identity_pub,
        name: text("name").unwrap_or_default().to_owned(),
        platform: non_empty("platform"),
        link_secret,
        candidates,
        paired_at: value.get("pairedAt").and_then(Value::as_i64).unwrap_or(0),
        last_seen_at: value.get("lastSeenAt").and_then(Value::as_i64).unwrap_or(0),
        app_version: non_empty("appVersion"),
        cloud_device_id: non_empty("cloudDeviceId"),
        default_save_dir: non_empty("defaultSaveDir"),
        path_style: text("pathStyle")
            .filter(|style| path_style_from_text(style).is_some())
            .map(str::to_owned),
        custom_name: non_empty("customName"),
        revoked: bool_field(value, "revoked"),
        grants_control: bool_field(value, "grantsControl"),
        controllable: bool_field(value, "controllable"),
    })
}

pub(super) fn value_from_record(record: &PeerRecord) -> Value {
    let mut value = json!({
        "fingerprint": record.fingerprint,
        "identityPubB64": B64.encode(&record.identity_pub),
        "name": record.name,
        "platform": record.platform.clone().unwrap_or_default(),
        "linkSecretB64": B64.encode(&record.link_secret),
        "candidatesJson": serde_json::to_string(&record.candidates).unwrap_or_else(|_| "[]".to_owned()),
        "pairedAt": record.paired_at,
        "lastSeenAt": record.last_seen_at,
    });
    let optional = [
        ("defaultSaveDir", &record.default_save_dir),
        ("pathStyle", &record.path_style),
        ("appVersion", &record.app_version),
        ("cloudDeviceId", &record.cloud_device_id),
        ("customName", &record.custom_name),
    ];
    for (key, field) in optional {
        if let Some(text) = field {
            value[key] = Value::String(text.clone());
        }
    }
    for (key, flag) in [
        ("revoked", record.revoked),
        ("grantsControl", record.grants_control),
        ("controllable", record.controllable),
    ] {
        if flag {
            value[key] = Value::Bool(true);
        }
    }
    value
}

fn records(state: &AgentState) -> Vec<PeerRecord> {
    let mut records: Vec<PeerRecord> = state
        .linked_devices
        .iter()
        .filter_map(record_from_value)
        .collect();
    records.sort_by_key(|record| std::cmp::Reverse(record.last_seen_at));
    records
}

fn position(state: &AgentState, fingerprint: &str) -> Option<usize> {
    state
        .linked_devices
        .iter()
        .position(|value| value.get("fingerprint").and_then(Value::as_str) == Some(fingerprint))
}

/// 名册记录 → 对外视图（严禁透出链路密钥 / 公钥）。
#[must_use]
pub fn device_info(record: &PeerRecord, online: bool) -> LinkDeviceInfo {
    LinkDeviceInfo {
        fingerprint: record.fingerprint.clone(),
        name: record.effective_name().to_owned(),
        platform: record.platform.clone(),
        // 被对端解除配对的设备不可达也不可下发：不论探测结果都不算在线。
        online: online && !record.revoked,
        paired_at: record.paired_at,
        last_seen_at: record.last_seen_at,
        default_save_dir: record.default_save_dir.clone(),
        path_style: record.path_style.as_deref().and_then(path_style_from_text),
        custom_name: record.custom_name.clone().filter(|name| !name.is_empty()),
        app_version: record.app_version.clone(),
        cloud_device_id: record.cloud_device_id.clone(),
        revoked: record.revoked,
        grants_control: record.grants_control,
        controllable: record.controllable,
    }
}

/// 运行期名册视图（启动初始快照用，在线状态未知一律 `false`；之后由 `LinkService`
/// 探测后推送带真实 `online` 的 `LinkedDevicesChanged`）。
#[must_use]
pub fn public_devices(state: &AgentState) -> Vec<LinkDeviceInfo> {
    records(state)
        .iter()
        .map(|record| device_info(record, false))
        .collect()
}

/// 名册视图（带在线状态表）。
#[must_use]
pub(super) fn devices_with_online(
    state: &AgentState,
    online: &HashMap<String, bool>,
) -> Vec<LinkDeviceInfo> {
    records(state)
        .iter()
        .map(|record| {
            device_info(
                record,
                online.get(&record.fingerprint).copied().unwrap_or(false),
            )
        })
        .collect()
}

/// 本机路径风格的名册文本形式（对端信息交换用）。
#[must_use]
pub(super) fn local_path_style_text() -> &'static str {
    path_style_text(PathStyle::current()).unwrap_or("posix")
}

#[async_trait]
impl LinkStorage for AgentLinkStorage {
    async fn load_identity_seed(&self) -> LinkResult<Option<[u8; 32]>> {
        let state = self.state.lock().await;
        match state.link_identity.as_ref() {
            None | Some(Value::Null) => Ok(None),
            // 种子存在但读不出来：报错而不是当作「没有」——否则互联层会生成新身份覆盖它，
            // 指纹一变，所有已配对设备立即失效。
            Some(value) => seed_from_identity(value).map(Some).ok_or_else(|| {
                LinkError::Store(
                    "the stored link identity seed is present but unreadable".to_owned(),
                )
            }),
        }
    }

    async fn save_identity_seed(&self, seed: &[u8; 32]) -> LinkResult<()> {
        let mut state = self.state.lock().await;
        let previous = state.link_identity.take();
        state.link_identity = Some(Value::String(B64.encode(seed)));
        let result = self.persist(&state).await;
        if result.is_err() {
            state.link_identity = previous;
        }
        result
    }

    async fn upsert(&self, record: &PeerRecord) -> LinkResult<()> {
        let mut state = self.state.lock().await;
        let previous = state.linked_devices.clone();
        let mut fresh = record.clone();
        let existing = position(&state, &record.fingerprint);
        // 重新配对不带对端自报信息 / 备注名：沿用旧记录里已有的，直到下一次交换刷新；
        // `revoked` 不沿用——重新配对就是重新建立信任。
        if let Some(old) = existing
            .and_then(|index| state.linked_devices.get(index))
            .and_then(record_from_value)
        {
            fresh.default_save_dir = fresh.default_save_dir.or(old.default_save_dir);
            fresh.path_style = fresh.path_style.or(old.path_style);
            fresh.app_version = fresh.app_version.or(old.app_version);
            fresh.cloud_device_id = fresh.cloud_device_id.or(old.cloud_device_id);
            fresh.custom_name = fresh.custom_name.or(old.custom_name);
        }
        let value = value_from_record(&fresh);
        match existing {
            Some(index) => state.linked_devices[index] = value,
            None => state.linked_devices.push(value),
        }
        self.persist_roster(&mut state, previous).await
    }

    async fn list(&self) -> LinkResult<Vec<PeerRecord>> {
        Ok(records(&*self.state.lock().await))
    }

    async fn get(&self, fingerprint: &str) -> LinkResult<Option<PeerRecord>> {
        let state = self.state.lock().await;
        Ok(position(&state, fingerprint)
            .and_then(|index| state.linked_devices.get(index))
            .and_then(record_from_value))
    }

    async fn remove(&self, fingerprint: &str) -> LinkResult<bool> {
        let mut state = self.state.lock().await;
        let Some(index) = position(&state, fingerprint) else {
            return Ok(false);
        };
        let previous = state.linked_devices.clone();
        state.linked_devices.remove(index);
        self.persist_roster(&mut state, previous).await?;
        Ok(true)
    }

    async fn touch(&self, fingerprint: &str, at: i64) -> LinkResult<()> {
        // 最近活跃时间只更新内存：它随下一次名册写盘落地，不值得为每次探活 fsync 整份状态。
        let mut state = self.state.lock().await;
        if let Some(index) = position(&state, fingerprint) {
            state.linked_devices[index]["lastSeenAt"] = json!(at);
        }
        Ok(())
    }

    async fn update_candidates(
        &self,
        fingerprint: &str,
        candidates: &[PeerCandidate],
    ) -> LinkResult<bool> {
        let mut state = self.state.lock().await;
        let Some(index) = position(&state, fingerprint) else {
            return Ok(false);
        };
        let previous = state.linked_devices.clone();
        state.linked_devices[index]["candidatesJson"] =
            Value::String(serde_json::to_string(candidates).unwrap_or_else(|_| "[]".to_owned()));
        self.persist_roster(&mut state, previous).await?;
        Ok(true)
    }

    async fn update_record(
        &self,
        fingerprint: &str,
        update: RecordUpdate<'_>,
    ) -> LinkResult<Option<bool>> {
        let mut state = self.state.lock().await;
        let Some(index) = position(&state, fingerprint) else {
            return Ok(None);
        };
        let Some(mut record) = state.linked_devices.get(index).and_then(record_from_value) else {
            return Ok(None);
        };
        // 读-改-写在同一把状态锁内完成；没有改动就不写盘（空闲时的信息交换零写入）。
        if !update(&mut record) {
            return Ok(Some(false));
        }
        let previous = state.linked_devices.clone();
        state.linked_devices[index] = value_from_record(&record);
        self.persist_roster(&mut state, previous).await?;
        Ok(Some(true))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use fluxdown_link::TransportKind;

    use super::*;

    fn record() -> PeerRecord {
        PeerRecord {
            fingerprint: "fp-1".into(),
            identity_pub: vec![1; 32],
            name: "Laptop".into(),
            platform: Some("macos".into()),
            link_secret: vec![2; 32],
            candidates: vec![PeerCandidate {
                kind: TransportKind::Direct,
                address: "192.168.1.5:17800".into(),
            }],
            paired_at: 100,
            last_seen_at: 200,
            ..PeerRecord::default()
        }
    }

    async fn storage(
        label: &str,
    ) -> (AgentLinkStorage, Arc<Mutex<AgentState>>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "fluxdown-link-storage-{label}-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let store = Arc::new(StateStore::open(dir.clone()).await.unwrap());
        let state = Arc::new(Mutex::new(AgentState::default()));
        (AgentLinkStorage::new(state.clone(), store), state, dir)
    }

    #[test]
    fn migrated_daemon_roster_shape_is_readable() {
        // daemon `link_migration_export` 写入的形状（没有 defaultSaveDir / pathStyle 等新增字段）。
        let value = json!({
            "fingerprint": "fp-1",
            "identityPubB64": B64.encode([1u8; 32]),
            "name": "Phone",
            "platform": "",
            "linkSecretB64": B64.encode([2u8; 32]),
            "candidatesJson": r#"[{"kind":"Direct","address":"10.0.0.2:17800"}]"#,
            "pairedAt": 5,
            "lastSeenAt": 6,
        });
        let record = record_from_value(&value).unwrap();
        assert_eq!(record.platform, None);
        assert_eq!(record.candidates.len(), 1);
        assert_eq!(record.default_save_dir, None);
        assert_eq!(record.custom_name, None);
        assert!(!record.revoked);
        // 旧数据没有控制标志：读回恒为 false，且视图里不出现。
        assert!(!record.grants_control && !record.controllable);
        assert!(value.get("grantsControl").is_none());
        let info = device_info(&record, false);
        assert!(info.default_save_dir.is_none());
        assert!(!info.revoked);
        assert_eq!(info.name, "Phone");
    }

    #[test]
    fn broken_roster_entries_are_skipped_not_fatal() {
        assert!(record_from_value(&json!({"fingerprint": "x"})).is_none());
        let empty_secret = json!({
            "fingerprint": "x",
            "identityPubB64": B64.encode([1u8; 32]),
            "linkSecretB64": "",
        });
        assert!(record_from_value(&empty_secret).is_none());
    }

    #[test]
    fn peer_info_survives_a_roster_roundtrip_and_maps_to_dto() {
        let mut original = record();
        original.default_save_dir = Some("/mnt/downloads".into());
        original.path_style = Some("posix".into());
        original.app_version = Some("1.2.3".into());
        original.cloud_device_id = Some("cloud-9".into());
        original.custom_name = Some("书房".into());
        original.revoked = true;
        let back = record_from_value(&value_from_record(&original)).unwrap();
        assert_eq!(back.default_save_dir, original.default_save_dir);
        assert_eq!(back.cloud_device_id, original.cloud_device_id);
        assert_eq!(back.custom_name, original.custom_name);
        assert!(back.revoked);
        let dto = device_info(&back, true);
        // 备注名优先于对端自报名；被对端解除的设备恒为离线。
        assert_eq!(dto.name, "书房");
        assert_eq!(dto.custom_name.as_deref(), Some("书房"));
        assert_eq!(dto.app_version.as_deref(), Some("1.2.3"));
        assert_eq!(dto.cloud_device_id.as_deref(), Some("cloud-9"));
        assert!(dto.revoked);
        assert!(!dto.online);
        assert_eq!(dto.default_save_dir.as_deref(), Some("/mnt/downloads"));
        assert_eq!(dto.path_style, Some(PathStyle::Posix));
        let mut healthy = back;
        healthy.revoked = false;
        assert!(device_info(&healthy, true).online);
    }

    #[test]
    fn control_flags_round_trip_with_the_documented_keys() {
        let mut original = record();
        original.grants_control = true;
        original.controllable = true;
        let value = value_from_record(&original);
        assert_eq!(value["grantsControl"], json!(true));
        assert_eq!(value["controllable"], json!(true));
        let back = record_from_value(&value).unwrap();
        assert!(back.grants_control && back.controllable);
        let dto = device_info(&back, true);
        assert!(dto.grants_control && dto.controllable);

        // false 不落盘（与旧数据同形）；类型不对的值按 false 处理。
        let plain = value_from_record(&record());
        assert!(plain.get("grantsControl").is_none() && plain.get("controllable").is_none());
        let mut malformed = value_from_record(&original);
        malformed["grantsControl"] = json!("yes");
        malformed["controllable"] = json!(1);
        let back = record_from_value(&malformed).unwrap();
        assert!(!back.grants_control && !back.controllable);
        let dto = device_info(&record(), true);
        assert!(!dto.grants_control && !dto.controllable);
    }

    #[tokio::test]
    async fn repairing_takes_the_new_control_flags_instead_of_inheriting() {
        let (storage, _state, dir) = storage("control-repair").await;
        let mut granted = record();
        granted.grants_control = true;
        granted.controllable = true;
        storage.upsert(&granted).await.unwrap();
        let stored = storage.get("fp-1").await.unwrap().unwrap();
        assert!(stored.grants_control && stored.controllable);
        // 其它字段沿用旧记录，控制标志只取本次配对结果。
        storage.upsert(&record()).await.unwrap();
        let repaired = storage.get("fp-1").await.unwrap().unwrap();
        assert!(!repaired.grants_control && !repaired.controllable);
        // `update_record` 的读-改-写不会丢控制标志。
        storage.upsert(&granted).await.unwrap();
        storage
            .update_record(
                "fp-1",
                Box::new(|record| {
                    record.custom_name = Some("书房".into());
                    true
                }),
            )
            .await
            .unwrap();
        let stored = storage.get("fp-1").await.unwrap().unwrap();
        assert!(stored.grants_control && stored.controllable);
        if let Err(error) = std::fs::remove_dir_all(&dir) {
            tracing::warn!(%error, "link storage test cleanup failed");
        }
    }

    #[tokio::test]
    async fn update_record_writes_only_on_change_and_repairing_clears_revoked() {
        let (storage, state, dir) = storage("update").await;
        storage.upsert(&record()).await.unwrap();
        let snapshot = |state: &Arc<Mutex<AgentState>>| {
            let state = state.clone();
            async move { state.lock().await.linked_devices.clone() }
        };
        let before = snapshot(&state).await;
        // 无变化：返回 Some(false)，名册原样。
        assert_eq!(
            storage
                .update_record("fp-1", Box::new(|_| false))
                .await
                .unwrap(),
            Some(false)
        );
        assert_eq!(snapshot(&state).await, before);
        // 有变化：落盘并可读回。
        assert_eq!(
            storage
                .update_record(
                    "fp-1",
                    Box::new(|record| {
                        record.revoked = true;
                        record.custom_name = Some("书房".into());
                        true
                    })
                )
                .await
                .unwrap(),
            Some(true)
        );
        let stored = storage.get("fp-1").await.unwrap().unwrap();
        assert!(stored.revoked);
        assert_eq!(stored.custom_name.as_deref(), Some("书房"));
        // 未知设备：None。
        assert_eq!(
            storage
                .update_record("missing", Box::new(|_| true))
                .await
                .unwrap(),
            None
        );
        // 重新配对：覆盖记录并清除 revoked，但沿用本机备注名。
        storage.upsert(&record()).await.unwrap();
        let repaired = storage.get("fp-1").await.unwrap().unwrap();
        assert!(!repaired.revoked);
        assert_eq!(repaired.custom_name.as_deref(), Some("书房"));
        if let Err(error) = std::fs::remove_dir_all(&dir) {
            tracing::warn!(%error, "link storage test cleanup failed");
        }
    }

    #[tokio::test]
    async fn unreadable_identity_seed_is_an_error_not_a_missing_seed() {
        let (storage, state, dir) = storage("seed").await;
        // 从未生成：没有种子。
        assert_eq!(storage.load_identity_seed().await.unwrap(), None);
        // 存在但读不出来（既不是 base64 字符串也不是 `{secretB64}` 对象）：报错，绝不当作没有。
        state.lock().await.link_identity = Some(Value::String("not base64!".into()));
        assert!(matches!(
            storage.load_identity_seed().await,
            Err(LinkError::Store(_))
        ));
        state.lock().await.link_identity = Some(json!({"unexpected": true}));
        assert!(matches!(
            storage.load_identity_seed().await,
            Err(LinkError::Store(_))
        ));
        // 失败不会改写已有值。
        assert_eq!(
            state.lock().await.link_identity,
            Some(json!({"unexpected": true}))
        );
        let seed = [7u8; 32];
        state.lock().await.link_identity = Some(Value::String(B64.encode(seed)));
        assert_eq!(storage.load_identity_seed().await.unwrap(), Some(seed));
        if let Err(error) = std::fs::remove_dir_all(&dir) {
            tracing::warn!(%error, "link storage test cleanup failed");
        }
    }

    #[test]
    fn identity_seed_accepts_daemon_string_and_legacy_object() {
        let seed = [9u8; 32];
        let text = B64.encode(seed);
        assert_eq!(seed_from_identity(&Value::String(text.clone())), Some(seed));
        assert_eq!(
            seed_from_identity(&json!({"secretB64": text, "publicB64": "ignored"})),
            Some(seed)
        );
        assert_eq!(seed_from_identity(&Value::Null), None);
        assert_eq!(
            seed_from_identity(&Value::String("not base64!".into())),
            None
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod rollback_tests {
    use fluxdown_link::TransportKind;

    use super::*;

    fn device(fingerprint: &str) -> PeerRecord {
        PeerRecord {
            fingerprint: fingerprint.into(),
            identity_pub: vec![1; 32],
            name: "Laptop".into(),
            link_secret: vec![2; 32],
            candidates: vec![PeerCandidate {
                kind: TransportKind::Direct,
                address: "192.168.1.5:17800".into(),
            }],
            ..PeerRecord::default()
        }
    }

    #[tokio::test]
    async fn failed_persistence_rolls_the_in_memory_roster_back() {
        let dir = std::env::temp_dir().join(format!(
            "fluxdown-link-rollback-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let store = Arc::new(StateStore::open(dir.clone()).await.unwrap());
        let state = Arc::new(Mutex::new(AgentState::default()));
        let storage = AgentLinkStorage::new(state.clone(), store);
        storage.upsert(&device("fp-1")).await.unwrap();
        let before = state.lock().await.linked_devices.clone();

        // 让落盘必然失败：数据目录被替换成同名普通文件。
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::write(&dir, b"not a directory").unwrap();

        let failed = |result: LinkResult<()>| assert!(matches!(result, Err(LinkError::Store(_))));
        failed(storage.upsert(&device("fp-2")).await);
        failed(
            storage
                .update_record(
                    "fp-1",
                    Box::new(|record| {
                        record.revoked = true;
                        record.grants_control = true;
                        true
                    }),
                )
                .await
                .map(|_| ()),
        );
        failed(storage.update_candidates("fp-1", &[]).await.map(|_| ()));
        failed(storage.remove("fp-1").await.map(|_| ()));
        failed(storage.save_identity_seed(&[7; 32]).await);

        // 内存里的名册和身份都停在改动之前，与磁盘上最后一次成功写入一致。
        assert_eq!(state.lock().await.linked_devices, before);
        assert!(state.lock().await.link_identity.is_none());
        let kept = storage.get("fp-1").await.unwrap().unwrap();
        assert!(!kept.revoked && !kept.grants_control);
        assert_eq!(kept.candidates.len(), 1);
        assert!(storage.get("fp-2").await.unwrap().is_none());

        if let Err(error) = std::fs::remove_file(&dir) {
            tracing::warn!(%error, "link rollback test cleanup failed");
        }
    }
}
