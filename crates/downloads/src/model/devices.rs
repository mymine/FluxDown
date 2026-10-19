//! 侧栏设备区与「下载到」目标共用的设备投影：过滤本机、去重、合并、同名消歧。
//!
//! 「其他设备」= 云账号设备（去掉本机；同一台机器的直连记录挂接其上）+ 独立的局域网已配对
//! 设备（排除对方已解除的）。选择 id 与远程任务的 `to_device` 同一空间：云设备用
//! `CloudDevice::device_id`，独立已配对设备用指纹。

use std::collections::{HashMap, HashSet};

use fluxdown_protocol::{CloudDevice, LinkDeviceInfo, PathStyle};

/// 其他设备的来源。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceKind {
    /// 同账号的云设备（经 FluxCloud 中转）。
    Cloud,
    /// 局域网直连配对设备。
    Paired,
}

/// 「下载到」目标：本机、云设备或已配对设备。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum DispatchTarget {
    #[default]
    Local,
    /// `CloudDevice::device_id`。
    Cloud(String),
    /// 已配对设备指纹。
    Paired(String),
}

impl DispatchTarget {
    /// 设备本地偏好里的持久化编码：`local` / `cloud:<id>` / `link:<fingerprint>`。
    #[must_use]
    pub fn to_pref(&self) -> String {
        match self {
            Self::Local => "local".to_owned(),
            Self::Cloud(id) => format!("cloud:{id}"),
            Self::Paired(fingerprint) => format!("link:{fingerprint}"),
        }
    }

    /// [`Self::to_pref`] 的逆运算；空 id 或未知前缀视为无记录。
    #[must_use]
    pub fn from_pref(value: &str) -> Option<Self> {
        match value.split_once(':') {
            None if value == "local" => Some(Self::Local),
            Some(("cloud", id)) if !id.is_empty() => Some(Self::Cloud(id.to_owned())),
            Some(("link", id)) if !id.is_empty() => Some(Self::Paired(id.to_owned())),
            _ => None,
        }
    }
}

/// 一台其他设备（已去重、名称已消歧）。
///
/// 云条目可挂接同一台机器的直连记录（`link_fingerprint`）：显示云设备名、在线取两者并集，
/// 直连在线时下发走局域网（见 [`Self::route`]）。没有对应云条目的直连记录是独立的
/// [`DeviceKind::Paired`] 条目。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceEntry {
    /// 云设备 `device_id`（云条目，含挂接）/ 已配对设备指纹（独立直连条目）；
    /// 同时是选择 id 与远程任务的 `to_device`。
    pub id: String,
    pub kind: DeviceKind,
    /// 挂接到本云条目的直连记录指纹；独立条目与未挂接的云条目为 `None`。
    pub link_fingerprint: Option<String>,
    /// 挂接直连记录的在线状态；服务未就绪（状态未知）为 `None`。
    pub link_online: Option<bool>,
    /// 显示名；与其他设备同名时追加短码。
    pub label: String,
    pub online: Option<bool>,
    /// 目标自报的默认下载目录（远程下发不填目录时使用）。
    pub default_save_dir: Option<String>,
    /// 目标的路径风格；未上报时已按平台推断，仍未知则为 `None`。
    pub path_style: Option<PathStyle>,
}

impl DeviceEntry {
    /// 选择 / 持久化用的目标（挂接条目为 `cloud:<id>`）。
    #[must_use]
    pub fn target(&self) -> DispatchTarget {
        match self.kind {
            DeviceKind::Cloud => DispatchTarget::Cloud(self.id.clone()),
            DeviceKind::Paired => DispatchTarget::Paired(self.id.clone()),
        }
    }

    /// 实际下发路由：挂接直连且直连在线时走局域网直连（一次性投递），否则按条目本身
    /// （云条目走云端，独立直连条目走直连）。
    #[must_use]
    pub fn route(&self) -> DispatchTarget {
        match (&self.link_fingerprint, self.link_online) {
            (Some(fingerprint), Some(true)) => DispatchTarget::Paired(fingerprint.clone()),
            _ => self.target(),
        }
    }

    /// 「下载到」下拉里要带「直连」标签：只有独立直连条目（挂接条目显示云设备名，不带标签）。
    #[must_use]
    pub fn is_standalone_link(&self) -> bool {
        self.kind == DeviceKind::Paired
    }
}

/// 在线状态合并：任一在线 → 在线；否则任一未知 → 未知；否则离线。
fn merge_online(cloud: Option<bool>, link: Option<bool>) -> Option<bool> {
    match (cloud, link) {
        (Some(true), _) | (_, Some(true)) => Some(true),
        (None, _) | (_, None) => None,
        (Some(false), Some(false)) => Some(false),
    }
}

fn clean_dir(dir: Option<&str>) -> Option<String> {
    dir.map(str::trim)
        .filter(|dir| !dir.is_empty())
        .map(str::to_owned)
}

/// 设备 id 的短码（末 4 位字母数字），用于同名设备消歧。
fn short_code(id: &str) -> String {
    let mut tail: Vec<char> = id
        .chars()
        .rev()
        .filter(char::is_ascii_alphanumeric)
        .take(4)
        .collect();
    tail.reverse();
    tail.into_iter().collect()
}

fn paired_style(device: &LinkDeviceInfo) -> Option<PathStyle> {
    device
        .path_style
        .filter(|style| *style != PathStyle::Unknown)
        .or_else(|| {
            device
                .platform
                .as_deref()
                .and_then(PathStyle::from_platform)
        })
}

/// 云设备 + 已配对设备 → 「其他设备」列表。
///
/// - 云条目：去掉 `is_current`（名册包含请求设备自身）、按 id 去重（首条保留）；
/// - 直连记录：指纹非空、未被对方解除（`revoked`）、按指纹去重；其 `cloud_device_id` 命中
///   一个尚未挂接的云条目时挂接到该条目，否则成为独立直连条目；
/// - 顺序：云条目（含挂接）按云名册顺序，随后独立直连条目按名册顺序；
/// - 名称为空用短码代替；多台同名（忽略大小写与首尾空白）时在合并后的整表上全部追加 ` · 短码`。
#[must_use]
pub(crate) fn other_devices(
    cloud: &[CloudDevice],
    linked: &[LinkDeviceInfo],
    cloud_current: bool,
    service_current: bool,
) -> Vec<DeviceEntry> {
    let mut seen_cloud = HashSet::new();
    let mut entries: Vec<DeviceEntry> = Vec::with_capacity(cloud.len() + linked.len());
    let mut cloud_index: HashMap<&str, usize> = HashMap::new();
    for device in cloud
        .iter()
        .filter(|device| !device.is_current && !device.device_id.is_empty())
    {
        if !seen_cloud.insert(device.device_id.as_str()) {
            continue;
        }
        cloud_index.insert(device.device_id.as_str(), entries.len());
        entries.push(DeviceEntry {
            id: device.device_id.clone(),
            kind: DeviceKind::Cloud,
            link_fingerprint: None,
            link_online: None,
            label: device.name.trim().to_owned(),
            online: (service_current && cloud_current).then_some(device.is_online),
            default_save_dir: clean_dir(device.default_save_dir.as_deref()),
            path_style: device.effective_path_style(),
        });
    }
    let mut seen_link = HashSet::new();
    let mut standalone = Vec::new();
    for device in linked
        .iter()
        .filter(|device| !device.fingerprint.is_empty() && !device.revoked)
    {
        if !seen_link.insert(device.fingerprint.as_str()) {
            continue;
        }
        let link_online = service_current.then_some(device.online);
        let attach_to = device
            .cloud_device_id
            .as_deref()
            .and_then(|cloud_id| cloud_index.get(cloud_id).copied())
            .filter(|index| entries[*index].link_fingerprint.is_none());
        if let Some(index) = attach_to {
            let entry = &mut entries[index];
            entry.link_fingerprint = Some(device.fingerprint.clone());
            entry.link_online = link_online;
            entry.online = merge_online(entry.online, link_online);
            if entry.default_save_dir.is_none() {
                entry.default_save_dir = clean_dir(device.default_save_dir.as_deref());
            }
            if entry.path_style.is_none() {
                entry.path_style = paired_style(device);
            }
            continue;
        }
        standalone.push(DeviceEntry {
            id: device.fingerprint.clone(),
            kind: DeviceKind::Paired,
            link_fingerprint: None,
            link_online: None,
            label: device.name.trim().to_owned(),
            online: link_online,
            default_save_dir: clean_dir(device.default_save_dir.as_deref()),
            path_style: paired_style(device),
        });
    }
    entries.extend(standalone);
    disambiguate(&mut entries);
    entries
}

/// 把持久化 / 旧选择解析到当前存在的目标：`link:<fp>` 已挂接到云条目时解析为该云条目；
/// 目标已不存在返回 `None`（调用方回退本机）。
#[must_use]
pub(crate) fn resolve_target(
    target: DispatchTarget,
    entries: &[DeviceEntry],
) -> Option<DispatchTarget> {
    match &target {
        DispatchTarget::Local => Some(target),
        DispatchTarget::Cloud(id) => entries
            .iter()
            .any(|entry| entry.kind == DeviceKind::Cloud && entry.id == *id)
            .then_some(target),
        DispatchTarget::Paired(fingerprint) => entries.iter().find_map(|entry| {
            let standalone = entry.kind == DeviceKind::Paired && entry.id == *fingerprint;
            let attached = entry.link_fingerprint.as_deref() == Some(fingerprint.as_str());
            (standalone || attached).then(|| entry.target())
        }),
    }
}

fn disambiguate(entries: &mut [DeviceEntry]) {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for entry in entries.iter() {
        *counts.entry(entry.label.to_lowercase()).or_default() += 1;
    }
    for entry in entries.iter_mut() {
        let duplicated = counts
            .get(&entry.label.to_lowercase())
            .is_some_and(|n| *n > 1);
        if entry.label.is_empty() {
            entry.label = short_code(&entry.id);
        } else if duplicated {
            entry.label = format!("{} · {}", entry.label, short_code(&entry.id));
        }
    }
}

/// 本机的云设备 id：会话里的本机设备优先，否则名册中的 `is_current`。
#[must_use]
pub(crate) fn local_device_id(
    session_device: Option<&str>,
    cloud: &[CloudDevice],
) -> Option<String> {
    session_device
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            cloud
                .iter()
                .find(|device| device.is_current && !device.device_id.is_empty())
                .map(|device| device.device_id.clone())
        })
}

/// 侧栏设备区可见性：偏好显式设置则按值；未设置时只要存在「其他设备」就显示（账号里只有
/// 本机一台、没有已配对设备时不显示，和只看名册人数不同）。
#[must_use]
pub(crate) fn devices_section_visible(pref: Option<bool>, other_devices: usize) -> bool {
    pref.unwrap_or(other_devices > 0)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn devices_section_defaults_to_visible_only_with_other_devices() {
        assert!(!devices_section_visible(None, 0));
        assert!(devices_section_visible(None, 1));
        // 显式偏好优先于自动判定。
        assert!(!devices_section_visible(Some(false), 3));
        assert!(devices_section_visible(Some(true), 0));
    }

    fn cloud(value: serde_json::Value) -> CloudDevice {
        serde_json::from_value(value).expect("cloud device")
    }

    fn linked(value: serde_json::Value) -> LinkDeviceInfo {
        serde_json::from_value(value).expect("link device")
    }

    #[test]
    fn cloud_disconnect_does_not_hide_targets_or_invalidate_lan_presence() {
        let cloud_devices = [cloud(json!({"id":"1","deviceId":"cloud","isOnline":true}))];
        let paired = [linked(
            json!({"fingerprint":"lan","name":"LAN","online":true,"pairedAt":0,"lastSeenAt":0}),
        )];
        let entries = other_devices(&cloud_devices, &paired, false, true);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].online, None);
        assert_eq!(
            entries[0].target(),
            DispatchTarget::Cloud("cloud".to_owned())
        );
        assert_eq!(entries[1].online, Some(true));
        let stale = other_devices(&cloud_devices, &paired, false, false);
        assert!(stale.iter().all(|entry| entry.online.is_none()));
        assert_eq!(
            other_devices(&cloud_devices, &paired, true, true)[0].online,
            Some(true)
        );
    }

    #[test]
    fn other_devices_drop_current_and_duplicate_ids() {
        let devices = [
            cloud(json!({"id":"1","deviceId":"dev-me","name":"Me","isCurrent":true})),
            cloud(
                json!({"id":"2","deviceId":"dev-a","name":"Mac","isOnline":true,"platform":"macos"}),
            ),
            cloud(json!({"id":"3","deviceId":"dev-a","name":"Mac dup"})),
            cloud(json!({"id":"4","deviceId":"","name":"No id"})),
        ];
        let entries = other_devices(&devices, &[], true, true);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "dev-a");
        assert_eq!(entries[0].label, "Mac");
        assert_eq!(entries[0].online, Some(true));
        assert_eq!(entries[0].path_style, Some(PathStyle::Posix));
    }

    #[test]
    fn same_named_devices_get_short_codes_and_unnamed_fall_back_to_code() {
        let devices = [
            cloud(json!({"id":"1","deviceId":"aaaa-1111","name":"PC"})),
            cloud(json!({"id":"2","deviceId":"bbbb-2222","name":" pc "})),
            cloud(json!({"id":"3","deviceId":"cccc-3333","name":"Laptop"})),
            cloud(json!({"id":"4","deviceId":"dddd-4444","name":""})),
        ];
        let entries = other_devices(&devices, &[], true, true);
        let labels: Vec<_> = entries.iter().map(|entry| entry.label.as_str()).collect();
        assert_eq!(labels, ["PC · 1111", "pc · 2222", "Laptop", "4444"]);
    }

    #[test]
    fn paired_devices_keep_fingerprint_ids_and_dedupe_across_kinds_independently() {
        let cloud_devices = [cloud(json!({"id":"1","deviceId":"same","name":"Box"}))];
        let paired = [
            linked(
                json!({"fingerprint":"same","name":"Box","online":true,"pairedAt":0,"lastSeenAt":0,
                "pathStyle":"windows"}),
            ),
            linked(
                json!({"fingerprint":"same","name":"Box again","online":false,"pairedAt":0,"lastSeenAt":0}),
            ),
        ];
        let entries = other_devices(&cloud_devices, &paired, true, true);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].kind, DeviceKind::Cloud);
        assert_eq!(entries[1].kind, DeviceKind::Paired);
        assert_eq!(entries[1].path_style, Some(PathStyle::Windows));
        // 云设备与已配对设备同名 → 各自追加短码消歧。
        assert_eq!(entries[0].label, "Box · same");
        assert!(entries[1].label.starts_with("Box · "));
    }

    fn link_for(fp: &str, cloud_id: Option<&str>, online: bool) -> LinkDeviceInfo {
        linked(json!({
            "fingerprint": fp, "name": "Link name", "online": online,
            "pairedAt": 0, "lastSeenAt": 0, "cloudDeviceId": cloud_id,
            "defaultSaveDir": "/link/dir", "platform": "windows",
        }))
    }

    #[test]
    fn link_with_matching_cloud_id_attaches_and_keeps_cloud_name_and_dir() {
        let cloud_devices = [cloud(json!({
            "id":"1","deviceId":"dev-a","name":"Mac","isOnline":false,
            "defaultSaveDir":"/cloud/dir"
        }))];
        let paired = [link_for("fp1", Some("dev-a"), true)];
        let entries = other_devices(&cloud_devices, &paired, true, true);
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.kind, DeviceKind::Cloud);
        assert_eq!(entry.label, "Mac");
        assert_eq!(entry.link_fingerprint.as_deref(), Some("fp1"));
        assert_eq!(entry.link_online, Some(true));
        assert_eq!(entry.default_save_dir.as_deref(), Some("/cloud/dir"));
        assert!(!entry.is_standalone_link());
    }

    #[test]
    fn attached_online_is_any_true_then_any_unknown_then_false() {
        let cloud_devices = [cloud(json!({"id":"1","deviceId":"dev-a","isOnline":false}))];
        // 云离线 + 直连在线 → 在线。
        let on = other_devices(
            &cloud_devices,
            &[link_for("f", Some("dev-a"), true)],
            true,
            true,
        );
        assert_eq!(on[0].online, Some(true));
        // 云连接不可信（未知）+ 直连离线 → 未知。
        let unknown = other_devices(
            &cloud_devices,
            &[link_for("f", Some("dev-a"), false)],
            false,
            true,
        );
        assert_eq!(unknown[0].online, None);
        // 云未知 + 直连在线 → 在线。
        let link_on = other_devices(
            &cloud_devices,
            &[link_for("f", Some("dev-a"), true)],
            false,
            true,
        );
        assert_eq!(link_on[0].online, Some(true));
        // 两者都离线 → 离线。
        let off = other_devices(
            &cloud_devices,
            &[link_for("f", Some("dev-a"), false)],
            true,
            true,
        );
        assert_eq!(off[0].online, Some(false));
        // 服务未就绪 → 全部未知。
        let stale = other_devices(
            &cloud_devices,
            &[link_for("f", Some("dev-a"), true)],
            true,
            false,
        );
        assert_eq!(stale[0].online, None);
        assert_eq!(stale[0].link_online, None);
    }

    #[test]
    fn route_prefers_link_only_when_attached_link_is_online() {
        let cloud_devices = [cloud(json!({"id":"1","deviceId":"dev-a","isOnline":true}))];
        let online = other_devices(
            &cloud_devices,
            &[link_for("fp", Some("dev-a"), true)],
            true,
            true,
        );
        assert_eq!(online[0].route(), DispatchTarget::Paired("fp".to_owned()));
        assert_eq!(
            online[0].target(),
            DispatchTarget::Cloud("dev-a".to_owned())
        );
        let offline = other_devices(
            &cloud_devices,
            &[link_for("fp", Some("dev-a"), false)],
            true,
            true,
        );
        assert_eq!(
            offline[0].route(),
            DispatchTarget::Cloud("dev-a".to_owned())
        );
        let unknown = other_devices(
            &cloud_devices,
            &[link_for("fp", Some("dev-a"), true)],
            true,
            false,
        );
        assert_eq!(
            unknown[0].route(),
            DispatchTarget::Cloud("dev-a".to_owned())
        );
        // 独立直连条目与未挂接的云条目按自身路由。
        let plain = other_devices(&cloud_devices, &[link_for("fp2", None, true)], true, true);
        assert_eq!(plain[0].route(), DispatchTarget::Cloud("dev-a".to_owned()));
        assert_eq!(plain[1].route(), DispatchTarget::Paired("fp2".to_owned()));
        assert!(plain[1].is_standalone_link());
    }

    #[test]
    fn link_pref_resolves_to_the_attached_cloud_entry() {
        let cloud_devices = [cloud(json!({"id":"1","deviceId":"dev-a"}))];
        let paired = [
            link_for("fp1", Some("dev-a"), true),
            link_for("fp2", None, true),
        ];
        let entries = other_devices(&cloud_devices, &paired, true, true);
        assert_eq!(
            resolve_target(DispatchTarget::Paired("fp1".to_owned()), &entries),
            Some(DispatchTarget::Cloud("dev-a".to_owned()))
        );
        assert_eq!(
            resolve_target(DispatchTarget::Paired("fp2".to_owned()), &entries),
            Some(DispatchTarget::Paired("fp2".to_owned()))
        );
        assert_eq!(
            resolve_target(DispatchTarget::Cloud("dev-a".to_owned()), &entries),
            Some(DispatchTarget::Cloud("dev-a".to_owned()))
        );
        assert_eq!(
            resolve_target(DispatchTarget::Paired("gone".to_owned()), &entries),
            None
        );
        assert_eq!(
            resolve_target(DispatchTarget::Cloud("gone".to_owned()), &entries),
            None
        );
        assert_eq!(
            resolve_target(DispatchTarget::Local, &entries),
            Some(DispatchTarget::Local)
        );
    }

    #[test]
    fn revoked_links_are_excluded_and_second_link_for_same_cloud_id_stays_standalone() {
        let cloud_devices = [cloud(json!({"id":"1","deviceId":"dev-a","name":"Mac"}))];
        let mut revoked = link_for("fp-revoked", Some("dev-a"), true);
        revoked.revoked = true;
        let entries = other_devices(&cloud_devices, &[revoked], true, true);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].link_fingerprint, None);

        let paired = [
            link_for("fp1", Some("dev-a"), true),
            link_for("fp2", Some("dev-a"), true),
        ];
        let entries = other_devices(&cloud_devices, &paired, true, true);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].link_fingerprint.as_deref(), Some("fp1"));
        assert_eq!(entries[1].kind, DeviceKind::Paired);
        assert_eq!(entries[1].id, "fp2");
    }

    #[test]
    fn names_are_disambiguated_after_merging_not_before() {
        let cloud_devices = [
            cloud(json!({"id":"1","deviceId":"aaaa-1111","name":"Mac"})),
            cloud(json!({"id":"2","deviceId":"bbbb-2222","name":"Studio"})),
        ];
        // 与云条目同一台机器（挂接）的直连记录不应让「Mac」重名；
        // 另一台独立直连设备恰好叫「Studio」才触发消歧。
        let mut same_machine = link_for("fp1", Some("aaaa-1111"), true);
        same_machine.name = "Mac".to_owned();
        let mut other = link_for("fp2", None, true);
        other.name = "studio".to_owned();
        let entries = other_devices(&cloud_devices, &[same_machine, other], true, true);
        let labels: Vec<_> = entries.iter().map(|entry| entry.label.as_str()).collect();
        assert_eq!(labels, ["Mac", "Studio · 2222", "studio · fp2"]);
    }

    #[test]
    fn local_device_id_prefers_session_then_current_flag() {
        let devices = [
            cloud(json!({"id":"1","deviceId":"dev-a"})),
            cloud(json!({"id":"2","deviceId":"dev-me","isCurrent":true})),
        ];
        assert_eq!(
            local_device_id(Some("session-dev"), &devices).as_deref(),
            Some("session-dev")
        );
        assert_eq!(local_device_id(None, &devices).as_deref(), Some("dev-me"));
        assert_eq!(local_device_id(Some(""), &[]), None);
    }

    #[test]
    fn dispatch_target_pref_round_trips_and_rejects_garbage() {
        for target in [
            DispatchTarget::Local,
            DispatchTarget::Cloud("a:b".to_owned()),
            DispatchTarget::Paired("f0f0".to_owned()),
        ] {
            assert_eq!(DispatchTarget::from_pref(&target.to_pref()), Some(target));
        }
        assert_eq!(DispatchTarget::from_pref("cloud:"), None);
        assert_eq!(DispatchTarget::from_pref("other:1"), None);
        assert_eq!(DispatchTarget::from_pref(""), None);
    }
}
