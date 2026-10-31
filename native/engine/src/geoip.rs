//! IP → 国家（对齐 qBittorrent 的 peer 国旗）：读 DB-IP「国家 lite」`.mmdb`（MaxMind DB 格式），
//! 并按月自动下载。
//!
//! - 读取：自带的最小 MaxMind DB 读取器（只支持国家库用到的类型与 24 / 28 / 32 位记录），不引入依赖。
//! - 下载：`https://download.db-ip.com/free/dbip-country-lite-YYYY-MM.mmdb.gz`，经宿主统一出口
//!   （全局代理 / 站点规则同样生效），解压后校验可读再原子落盘到 `<data_dir>/geoip/`；当月文件不存在就试
//!   当月，当月未发布时回退上月。失败保留旧库，且 1 小时内不重试。
//! - 触发：只在有人查看 BT peer 列表时（[`ensure_background`]），不新增任何周期任务（空闲静默）。
//!
//! 数据来源 DB-IP.com（CC BY 4.0），署名见关于页 / 开源许可。

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, LazyLock, RwLock};
use std::time::Duration;

/// MaxMind DB 元数据起始标记。
const METADATA_MARKER: &[u8] = b"\xAB\xCD\xEFMaxMind.com";
/// 数据区与搜索树之间的分隔长度。
const DATA_SEPARATOR: usize = 16;
/// 嵌套解码深度上限（防畸形文件）。
const MAX_DEPTH: u8 = 8;
/// 下载失败后的重试间隔（秒）。
const RETRY_SECS: i64 = 3600;
/// 下载超时。
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);
/// 压缩包与解压后大小上限（防异常响应）。
const MAX_GZ_BYTES: usize = 64 * 1024 * 1024;
const MAX_DB_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
enum Value {
    Str(String),
    Map(Vec<(String, Value)>),
    Uint(u64),
    Other,
}

/// 已加载的国家库。
#[derive(Debug)]
pub struct GeoDb {
    data: Vec<u8>,
    node_count: usize,
    record_size: usize,
    tree_size: usize,
    /// IPv6 树里 `::/96`（IPv4 映射区）的起点节点；IPv4 库为 0。
    ipv4_start: usize,
    ipv6: bool,
}

impl GeoDb {
    /// 解析 `.mmdb` 字节。
    ///
    /// # Errors
    /// 文件不是支持的 MaxMind DB。
    pub fn from_bytes(data: Vec<u8>) -> Result<Self, String> {
        let marker = data
            .windows(METADATA_MARKER.len())
            .rposition(|window| window == METADATA_MARKER)
            .ok_or("not a MaxMind DB (metadata marker missing)")?;
        let meta_start = marker + METADATA_MARKER.len();
        let (meta, _) = decode(&data[..], meta_start, meta_start, 0).ok_or("bad metadata")?;
        let Value::Map(meta) = meta else {
            return Err("metadata is not a map".to_owned());
        };
        let uint = |key: &str| {
            meta.iter().find_map(|(name, value)| match value {
                Value::Uint(number) if name == key => usize::try_from(*number).ok(),
                _ => None,
            })
        };
        let node_count = uint("node_count").ok_or("node_count missing")?;
        let record_size = uint("record_size").ok_or("record_size missing")?;
        let ip_version = uint("ip_version").ok_or("ip_version missing")?;
        if !matches!(record_size, 24 | 28 | 32) || !matches!(ip_version, 4 | 6) {
            return Err(format!(
                "unsupported database (record_size={record_size}, ip_version={ip_version})"
            ));
        }
        let tree_size = node_count
            .checked_mul(record_size / 4)
            .ok_or("tree size overflow")?;
        if tree_size + DATA_SEPARATOR > marker {
            return Err("search tree exceeds file".to_owned());
        }
        let mut db = Self {
            data,
            node_count,
            record_size,
            tree_size,
            ipv4_start: 0,
            ipv6: ip_version == 6,
        };
        if db.ipv6 {
            // IPv4 地址在 IPv6 树里位于 ::/96：沿 96 个 0 比特走到起点。
            let mut node = 0usize;
            for _ in 0..96 {
                if node >= db.node_count {
                    break;
                }
                node = db.record(node, 0).ok_or("corrupt search tree")?;
            }
            db.ipv4_start = node;
        }
        Ok(db)
    }

    /// 读取 `node` 的左（0）/ 右（1）记录。
    fn record(&self, node: usize, bit: u8) -> Option<usize> {
        let size = self.record_size / 4;
        let base = node.checked_mul(size)?;
        let bytes = self.data.get(base..base + size)?;
        let value = match self.record_size {
            24 => {
                let at = usize::from(bit) * 3;
                (usize::from(bytes[at]) << 16)
                    | (usize::from(bytes[at + 1]) << 8)
                    | usize::from(bytes[at + 2])
            }
            28 => {
                if bit == 0 {
                    ((usize::from(bytes[3]) & 0xF0) << 20)
                        | (usize::from(bytes[0]) << 16)
                        | (usize::from(bytes[1]) << 8)
                        | usize::from(bytes[2])
                } else {
                    ((usize::from(bytes[3]) & 0x0F) << 24)
                        | (usize::from(bytes[4]) << 16)
                        | (usize::from(bytes[5]) << 8)
                        | usize::from(bytes[6])
                }
            }
            _ => {
                let at = usize::from(bit) * 4;
                (usize::from(bytes[at]) << 24)
                    | (usize::from(bytes[at + 1]) << 16)
                    | (usize::from(bytes[at + 2]) << 8)
                    | usize::from(bytes[at + 3])
            }
        };
        Some(value)
    }

    /// 查询 IP 的 ISO 3166-1 两位国家码（大写）；未收录 / 内网地址返回 `None`。
    #[must_use]
    pub fn country(&self, ip: IpAddr) -> Option<String> {
        let (bits, start): (Vec<u8>, usize) = match ip {
            IpAddr::V4(v4) => (v4.octets().to_vec(), self.ipv4_start),
            IpAddr::V6(v6) => {
                if !self.ipv6 {
                    return None;
                }
                (v6.octets().to_vec(), 0)
            }
        };
        let mut node = start;
        for bit_index in 0..bits.len() * 8 {
            if node >= self.node_count {
                break;
            }
            let byte = bits[bit_index / 8];
            let bit = (byte >> (7 - (bit_index % 8))) & 1;
            node = self.record(node, bit)?;
        }
        if node <= self.node_count {
            return None;
        }
        let offset = self.tree_size + (node - self.node_count);
        let data_start = self.tree_size + DATA_SEPARATOR;
        if offset < data_start {
            return None;
        }
        let (value, _) = decode(&self.data, data_start, offset, 0)?;
        let Value::Map(root) = value else {
            return None;
        };
        let country = root.iter().find_map(|(key, value)| match value {
            Value::Map(inner) if key == "country" => Some(inner),
            _ => None,
        })?;
        country.iter().find_map(|(key, value)| match value {
            Value::Str(code) if key == "iso_code" => Some(code.to_ascii_uppercase()),
            _ => None,
        })
    }
}

/// 解码 `offset` 处的值，返回值与下一个位置。`base` 是指针的相对基址（数据区起点）。
fn decode(data: &[u8], base: usize, offset: usize, depth: u8) -> Option<(Value, usize)> {
    if depth > MAX_DEPTH {
        return None;
    }
    let ctrl = *data.get(offset)?;
    let mut pos = offset + 1;
    let mut kind = ctrl >> 5;

    if kind == 1 {
        // 指针：大小位决定宽度，目标相对数据区起点。
        let width = usize::from((ctrl >> 3) & 0x3);
        let low = usize::from(ctrl & 0x7);
        let raw = data.get(pos..pos + width + 1)?;
        pos += raw.len();
        let mut number = if width == 3 { 0 } else { low };
        for byte in raw {
            number = (number << 8) | usize::from(*byte);
        }
        number += match width {
            1 => 2048,
            2 => 526_336,
            _ => 0,
        };
        let (value, _) = decode(data, base, base.checked_add(number)?, depth + 1)?;
        return Some((value, pos));
    }
    if kind == 0 {
        kind = 7u8.checked_add(*data.get(pos)?)?;
        pos += 1;
    }
    let mut size = usize::from(ctrl & 0x1F);
    if size >= 29 {
        let extra = size - 28;
        let raw = data.get(pos..pos + extra)?;
        pos += extra;
        let mut number = 0usize;
        for byte in raw {
            number = (number << 8) | usize::from(*byte);
        }
        size = number
            + match extra {
                1 => 29,
                2 => 285,
                _ => 65_821,
            };
    }
    match kind {
        2 => {
            let raw = data.get(pos..pos.checked_add(size)?)?;
            Some((
                Value::Str(String::from_utf8_lossy(raw).into_owned()),
                pos + size,
            ))
        }
        5 | 6 | 9 => {
            let raw = data.get(pos..pos.checked_add(size)?)?;
            let mut number = 0u64;
            for byte in raw {
                number = number.checked_shl(8)? | u64::from(*byte);
            }
            Some((Value::Uint(number), pos + size))
        }
        7 => {
            let mut entries = Vec::with_capacity(size.min(16));
            for _ in 0..size {
                let (key, next) = decode(data, base, pos, depth + 1)?;
                let (value, next) = decode(data, base, next, depth + 1)?;
                pos = next;
                if let Value::Str(key) = key {
                    entries.push((key, value));
                }
            }
            Some((Value::Map(entries), pos))
        }
        11 => {
            for _ in 0..size {
                let (_, next) = decode(data, base, pos, depth + 1)?;
                pos = next;
            }
            Some((Value::Other, pos))
        }
        // 14 布尔：值在大小位里，无负载；其余（double / bytes / i32 / u128 / float 等）按长度跳过。
        14 => Some((Value::Other, pos)),
        3 | 4 | 8 | 10 | 15 => Some((Value::Other, pos.checked_add(size)?)),
        _ => None,
    }
}

// ───────────────────────── 全局库与下载 ─────────────────────────

static CURRENT: LazyLock<RwLock<Option<Arc<GeoDb>>>> = LazyLock::new(|| RwLock::new(None));
static LAST_ATTEMPT: AtomicI64 = AtomicI64::new(0);
static RUNNING: AtomicBool = AtomicBool::new(false);

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_secs()).ok())
        .unwrap_or(0)
}

/// 当前加载的库（未下载 / 未加载为 `None`）。
#[must_use]
pub fn current() -> Option<Arc<GeoDb>> {
    match CURRENT.read() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

/// 查询 peer 地址（`ip:port`，IPv6 带方括号）的国家码。
#[must_use]
pub fn lookup_addr(addr: &str) -> Option<String> {
    let ip = addr
        .parse::<std::net::SocketAddr>()
        .map(|socket| socket.ip())
        .ok()?;
    current()?.country(ip)
}

fn install(db: GeoDb) {
    let value = Some(Arc::new(db));
    match CURRENT.write() {
        Ok(mut guard) => *guard = value,
        Err(poisoned) => *poisoned.into_inner() = value,
    }
}

fn dir(data_dir: &Path) -> PathBuf {
    data_dir.join("geoip")
}

fn file_name(year: i32, month: u32) -> String {
    format!("dbip-country-lite-{year:04}-{month:02}.mmdb")
}

/// 当月与上月的 `(年, 月)`。
fn recent_months() -> [(i32, u32); 2] {
    use chrono::Datelike as _;
    let today = chrono::Utc::now().date_naive();
    let (year, month) = (today.year(), today.month());
    let previous = if month == 1 {
        (year - 1, 12)
    } else {
        (year, month - 1)
    };
    [(year, month), previous]
}

/// 确保国家库可用：已有当月文件则加载；否则后台下载（最多 1 小时一次）。立即返回，不阻塞调用方。
/// 已有较旧的库时先加载它（国旗立即可用），再尝试更新。
pub fn ensure_background(data_dir: PathBuf, proxy: crate::proxy_config::ProxyConfig) {
    let months = recent_months();
    let current_file = dir(&data_dir).join(file_name(months[0].0, months[0].1));
    if current().is_some() && current_file.exists() {
        return;
    }
    if RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    tokio::spawn(async move {
        let outcome = ensure(&data_dir, &proxy, months).await;
        RUNNING.store(false, Ordering::SeqCst);
        if let Err(error) = outcome {
            crate::log_warn!("[geoip] {error}");
        }
    });
}

async fn ensure(
    data_dir: &Path,
    proxy: &crate::proxy_config::ProxyConfig,
    months: [(i32, u32); 2],
) -> Result<(), String> {
    let folder = dir(data_dir);
    // 先加载磁盘上最新的一份（当月 → 上月 → 目录里任何 dbip 文件）。
    if current().is_none() {
        let candidates = months.map(|(year, month)| folder.join(file_name(year, month)));
        let mut loaded = false;
        for path in candidates {
            if load_file(&path).await.is_ok() {
                loaded = true;
                break;
            }
        }
        if !loaded && let Some(path) = newest_in(&folder).await {
            loaded = load_file(&path).await.is_ok();
        }
        let _ = loaded;
    }
    let current_file = folder.join(file_name(months[0].0, months[0].1));
    if current_file.exists() && current().is_some() {
        return Ok(());
    }
    let now = now_secs();
    let last = LAST_ATTEMPT.load(Ordering::Relaxed);
    if now - last < RETRY_SECS && last != 0 {
        return Ok(());
    }
    LAST_ATTEMPT.store(now, Ordering::Relaxed);

    let clients =
        crate::egress::RoutedClients::new(proxy, "", |builder| builder.timeout(DOWNLOAD_TIMEOUT))
            .map_err(|error| format!("egress: {error:#}"))?;
    let mut last_error = String::from("no month available");
    for (year, month) in months {
        let name = file_name(year, month);
        let url = format!("https://download.db-ip.com/free/{name}.gz");
        match download(&clients, &url).await {
            Ok(bytes) => {
                let db = GeoDb::from_bytes(bytes.clone())?;
                tokio::fs::create_dir_all(&folder)
                    .await
                    .map_err(|error| format!("create {}: {error}", folder.display()))?;
                let target = folder.join(&name);
                let temp = folder.join(format!("{name}.part"));
                tokio::fs::write(&temp, &bytes)
                    .await
                    .map_err(|error| format!("write {}: {error}", temp.display()))?;
                tokio::fs::rename(&temp, &target)
                    .await
                    .map_err(|error| format!("rename {}: {error}", target.display()))?;
                install(db);
                prune_old(&folder, &name).await;
                crate::log_info!("[geoip] installed {name}");
                return Ok(());
            }
            Err(error) => last_error = format!("{url}: {error}"),
        }
    }
    Err(last_error)
}

async fn load_file(path: &Path) -> Result<(), String> {
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|error| error.to_string())?;
    let db = tokio::task::spawn_blocking(move || GeoDb::from_bytes(bytes))
        .await
        .map_err(|error| error.to_string())??;
    install(db);
    Ok(())
}

async fn newest_in(folder: &Path) -> Option<PathBuf> {
    let mut entries = tokio::fs::read_dir(folder).await.ok()?;
    let mut best: Option<PathBuf> = None;
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        let is_db = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("dbip-country-lite-") && name.ends_with(".mmdb"));
        if is_db && best.as_ref().is_none_or(|current| path > *current) {
            best = Some(path);
        }
    }
    best
}

async fn prune_old(folder: &Path, keep: &str) {
    let Ok(mut entries) = tokio::fs::read_dir(folder).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        let stale = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name != keep && name.starts_with("dbip-country-lite-"));
        if stale && let Err(error) = tokio::fs::remove_file(&path).await {
            crate::log_warn!("[geoip] remove {}: {error}", path.display());
        }
    }
}

async fn download(clients: &crate::egress::RoutedClients, url: &str) -> Result<Vec<u8>, String> {
    let client = clients
        .client(url)
        .await
        .map_err(|error| error.to_string())?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }
    let gz = response.bytes().await.map_err(|error| error.to_string())?;
    if gz.len() > MAX_GZ_BYTES {
        return Err("response too large".to_owned());
    }
    tokio::task::spawn_blocking(move || {
        use std::io::Read as _;
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(&gz[..])
            .take(MAX_DB_BYTES)
            .read_to_end(&mut out)
            .map_err(|error| format!("gunzip: {error}"))?;
        Ok(out)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 为 `1.2.3.0/24 → XX` 手工构造一个 IPv4 / 24 位记录的最小 MaxMind DB。
    fn tiny_db() -> Vec<u8> {
        let node_count = 24usize;
        let mut tree = Vec::new();
        let bits: Vec<u8> = [1u8, 2, 3]
            .iter()
            .flat_map(|byte| (0..8).map(move |i| (byte >> (7 - i)) & 1))
            .collect();
        let data_record = node_count + DATA_SEPARATOR; // 数据区偏移 0
        for (index, bit) in bits.iter().enumerate() {
            let next = if index + 1 == bits.len() {
                data_record
            } else {
                index + 1
            };
            let (left, right) = if *bit == 0 {
                (next, node_count)
            } else {
                (node_count, next)
            };
            for record in [left, right] {
                tree.extend_from_slice(&[(record >> 16) as u8, (record >> 8) as u8, record as u8]);
            }
        }
        let mut data = vec![0u8; DATA_SEPARATOR];
        // {"country": {"iso_code": "xx"}}
        data.push(0xE1); // map, 1 entry
        data.push(0x40 | 7);
        data.extend_from_slice(b"country");
        data.push(0xE1);
        data.push(0x40 | 8);
        data.extend_from_slice(b"iso_code");
        data.push(0x40 | 2);
        data.extend_from_slice(b"xx");
        // 元数据：{"node_count": u32, "record_size": u16, "ip_version": u16}
        let mut meta = METADATA_MARKER.to_vec();
        meta.push(0xE3);
        meta.push(0x40 | 10);
        meta.extend_from_slice(b"node_count");
        meta.extend_from_slice(&[0xC1, 24]);
        meta.push(0x40 | 11);
        meta.extend_from_slice(b"record_size");
        meta.extend_from_slice(&[0xA1, 24]);
        meta.push(0x40 | 10);
        meta.extend_from_slice(b"ip_version");
        meta.extend_from_slice(&[0xA1, 4]);
        let mut file = tree;
        file.extend(data);
        file.extend(meta);
        file
    }

    #[test]
    fn reads_country_from_minimal_ipv4_tree() {
        let db = GeoDb::from_bytes(tiny_db()).expect("db");
        assert_eq!(
            db.country("1.2.3.4".parse().expect("ip")),
            Some("XX".into())
        );
        assert_eq!(
            db.country("1.2.3.255".parse().expect("ip")),
            Some("XX".into())
        );
        assert_eq!(db.country("1.2.4.4".parse().expect("ip")), None);
        assert_eq!(db.country("8.8.8.8".parse().expect("ip")), None);
        assert_eq!(
            db.country("::1".parse().expect("ip")),
            None,
            "IPv4 库不查 IPv6"
        );
    }

    #[test]
    fn rejects_garbage() {
        assert!(GeoDb::from_bytes(vec![1, 2, 3]).is_err());
    }

    /// 真实 DB-IP 库（`FLUXDOWN_TEST_MMDB=<abs>` 注入，默认跳过）：IPv4 / IPv6 / 内网。
    #[test]
    fn real_dbip_resolves_known_addresses() {
        let Ok(path) = std::env::var("FLUXDOWN_TEST_MMDB") else {
            return;
        };
        let db = GeoDb::from_bytes(std::fs::read(path).expect("read mmdb")).expect("db");
        assert_eq!(db.record_size, 24);
        assert_eq!(
            db.country("8.8.8.8".parse().expect("ip")),
            Some("US".into())
        );
        assert_eq!(
            db.country("114.114.114.114".parse().expect("ip")),
            Some("CN".into())
        );
        assert_eq!(
            db.country("2408:8256::1".parse().expect("ip")),
            Some("CN".into())
        );
        assert_eq!(db.country("192.168.1.1".parse().expect("ip")), None);
        // 带方括号的 IPv6 套接字地址能解析成国家码（具体国家不断言：任播地址的归属随库版本变化）。
        assert!(
            lookup_addr_with(&db, "[2001:4860:4860::8888]:443").is_some_and(|code| code.len() == 2)
        );
    }

    fn lookup_addr_with(db: &GeoDb, addr: &str) -> Option<String> {
        let ip = addr.parse::<std::net::SocketAddr>().ok()?.ip();
        db.country(ip)
    }

    /// 联网冒烟（`cargo nextest run --run-ignored all geoip_download`）：真实下载 → 解压 → 校验 → 落盘 → 查询。
    #[tokio::test]
    #[ignore = "needs network access to download.db-ip.com"]
    async fn geoip_download_installs_and_resolves() {
        let dir = std::env::temp_dir().join(format!("fluxdown-geoip-{}", uuid::Uuid::now_v7()));
        let months = recent_months();
        ensure(&dir, &crate::proxy_config::ProxyConfig::default(), months)
            .await
            .expect("download");
        let db = current().expect("installed");
        assert_eq!(
            db.country("114.114.114.114".parse().expect("ip")),
            Some("CN".into())
        );
        assert!(
            dir.join("geoip").read_dir().expect("dir").count() == 1,
            "exactly one db file kept"
        );
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}
