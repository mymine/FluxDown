//! 系统 PAC（代理自动配置）：脚本获取、按下载链接决策、失败代理记忆。
//!
//! # 获取
//!
//! PAC 文本一律在异步上下文取回：`http://` / `https://` 经统一出口
//! （[`crate::egress::client_builder`]，直连——PAC 定义代理，不能经代理取），
//! `file://` 读文件；脚本体上限 2 MiB。按 PAC URL 缓存（成功 5min / 失败 30s）。
//! actor 线程上的同步调用方（Auto 候选解析、route_health 端点 epoch）只读缓存，
//! 未命中/过期时在后台刷新——绝不在 actor 线程上做网络 IO；下载任务起跑时
//! （spawned task 内，见 [`crate::pac_route`]）会 await 一次获取，首个任务也拿得到脚本。
//!
//! # 决策
//!
//! 带 `plugins` feature（桌面 / NAS）时经 `pac_js` 执行 `FindProxyForURL`，
//! 按 Chromium 约定传 URL（https 只传 `scheme://host[:port]/`，不暴露路径与查询），
//! 结果按 (PAC URL, URL) 缓存 60s。无 JS 引擎（手机 / CLI）或脚本执行失败时
//! 退化为「脚本里出现的代理端点」（`evaluated = false`）：Auto 用它作系统代理
//! 候选，System 模式不据此改道（保持历史行为）。
//!
//! # 失败代理
//!
//! 与 Chromium 一致：经 PAC 选中的代理传输失败后 5 分钟内排到列表最后；列表里
//! 全是坏代理时按原顺序取。
//!
//! WPAD：系统开启「自动发现」且没有显式 PAC URL 时，由 [`crate::proxy_config`]
//! 给出 [`WPAD_URL`]（同 Chromium 的 DNS 方式；DHCP 方式需要系统底层接口，不支持）。

use std::collections::HashMap;
use std::net::{Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use url::Url;

use crate::logger::log_info;
use crate::proxy_config::{ProxyConfig, ProxyMode, ProxyType};

/// WPAD 的 DNS 发现地址：解析 `wpad` 时由系统 DNS 套用搜索域。
pub(crate) const WPAD_URL: &str = "http://wpad/wpad.dat";
/// PAC 脚本体上限；超限直接拒绝（截断后的脚本可能缺失关键分支，不如不用）。
const MAX_PAC_BYTES: usize = 2 * 1024 * 1024;
/// 取脚本的总时限。
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
/// 任务起跑时等待显式 PAC 脚本的上限（超时先放行，获取在后台继续）。
const TASK_FETCH_WAIT: Duration = Duration::from_secs(3);
/// 脚本成功缓存时长。
const SUCCESS_TTL: Duration = Duration::from_secs(300);
/// 失败负缓存时长：PAC 服务挂掉时不在每个任务起跑时都重新等满超时。
const FAILURE_TTL: Duration = Duration::from_secs(30);
/// 脚本缓存条目上限；PAC URL 实际只有 1~2 个，超限直接清空重来。
const CACHE_MAX_ENTRIES: usize = 16;
/// 单次 `FindProxyForURL` 执行的总预算（含脚本内 DNS）。
#[cfg(feature = "plugins")]
const EVAL_BUDGET: Duration = Duration::from_secs(3);
/// 决策缓存时长。
#[cfg(feature = "plugins")]
const DECISION_TTL: Duration = Duration::from_secs(60);
/// 决策缓存条目上限。
#[cfg(feature = "plugins")]
const DECISION_CACHE_MAX: usize = 256;
/// 失败代理的冷却时长（Chromium 默认 5 分钟）。
const BAD_PROXY_TTL: Duration = Duration::from_secs(300);

/// PAC 返回列表里的一条指令。
#[derive(Debug, Clone)]
pub(crate) enum PacEntry {
    Direct,
    Proxy(ProxyConfig),
}

/// 一次 PAC 决策。`evaluated = false` 表示没有真正执行脚本（无 JS 引擎或执行
/// 失败），`entries` 只是脚本里出现过的代理端点，不代表该 URL 的分流结论。
#[derive(Debug, Clone)]
pub(crate) struct PacDecision {
    pub entries: Vec<PacEntry>,
    pub evaluated: bool,
}

struct ScriptSlot {
    expires: Instant,
    text: Option<Arc<str>>,
    refreshing: bool,
}

static SCRIPTS: Mutex<Option<HashMap<String, ScriptSlot>>> = Mutex::new(None);
static BAD_PROXIES: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);

/// 缓存只是可重算的快照：锁中毒直接取内部值。
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

enum Cached {
    Fresh(Option<Arc<str>>),
    Stale(Option<Arc<str>>),
    Missing,
}

fn cached(pac_url: &str) -> Cached {
    let guard = lock(&SCRIPTS);
    match guard.as_ref().and_then(|map| map.get(pac_url)) {
        Some(slot) if Instant::now() < slot.expires => Cached::Fresh(slot.text.clone()),
        Some(slot) => Cached::Stale(slot.text.clone()),
        None => Cached::Missing,
    }
}

fn store(pac_url: &str, text: Option<Arc<str>>) {
    // WPAD 是系统默认开启的「碰运气」发现（Windows 默认勾选自动检测），绝大多数
    // 网络里都没有，失败按成功时长缓存，避免反复查询。
    let ttl = if text.is_some() || pac_url == WPAD_URL {
        SUCCESS_TTL
    } else {
        FAILURE_TTL
    };
    let mut guard = lock(&SCRIPTS);
    let map = guard.get_or_insert_with(HashMap::new);
    if map.len() >= CACHE_MAX_ENTRIES && !map.contains_key(pac_url) {
        map.clear();
    }
    map.insert(
        pac_url.to_string(),
        ScriptSlot {
            expires: Instant::now() + ttl,
            text,
            refreshing: false,
        },
    );
}

/// 标记后台刷新进行中；已有刷新在途返回 `false`（不重复发起）。
fn begin_refresh(pac_url: &str) -> bool {
    let mut guard = lock(&SCRIPTS);
    let map = guard.get_or_insert_with(HashMap::new);
    let slot = map.entry(pac_url.to_string()).or_insert(ScriptSlot {
        expires: Instant::now(),
        text: None,
        refreshing: false,
    });
    if slot.refreshing {
        return false;
    }
    slot.refreshing = true;
    true
}

fn end_refresh(pac_url: &str) {
    if let Some(slot) = lock(&SCRIPTS).as_mut().and_then(|map| map.get_mut(pac_url)) {
        slot.refreshing = false;
    }
}

async fn refresh(pac_url: &str) -> Option<Arc<str>> {
    let fetched = fetch_pac_text(pac_url, MAX_PAC_BYTES)
        .await
        .and_then(|text| {
            // 运营商 DNS 劫持会让 `wpad` 等不存在的名字返回广告页：不含入口函数的
            // 响应不是 PAC 脚本。
            if text.contains("FindProxyForURL") {
                Ok(text)
            } else {
                Err("响应不是 PAC 脚本（缺少 FindProxyForURL）".to_string())
            }
        });
    let text = match fetched {
        Ok(text) => Some(Arc::<str>::from(text)),
        Err(reason) => {
            log_info!("[pac] 取 PAC 脚本失败 {pac_url}: {reason}");
            None
        }
    };
    store(pac_url, text.clone());
    text
}

/// 任务起跑时取 PAC 文本，保证不明显推迟任务：WPAD（系统默认开启的猜测式
/// 发现）只读缓存、后台刷新，绝不让任务等它；显式 PAC 最多等
/// [`TASK_FETCH_WAIT`]，超时先用缓存（可能过期）的文本，获取在后台继续。
async fn script_for_task(pac_url: &str) -> Option<Arc<str>> {
    if pac_url == WPAD_URL {
        return cached_script_text(pac_url);
    }
    if let Cached::Fresh(text) = cached(pac_url) {
        return text;
    }
    let owned = pac_url.to_string();
    let fetch = tokio::spawn(async move { refresh(&owned).await });
    match tokio::time::timeout(TASK_FETCH_WAIT, fetch).await {
        Ok(Ok(text)) => text,
        Ok(Err(error)) => {
            log_info!("[pac] 取 PAC 脚本任务异常 {pac_url}: {error}");
            None
        }
        Err(_elapsed) => match cached(pac_url) {
            Cached::Fresh(text) | Cached::Stale(text) => text,
            Cached::Missing => None,
        },
    }
}

/// 同步只读缓存（actor 线程用）：未命中 / 过期时在后台刷新，过期但有旧文本
/// 时先返回旧文本；无 tokio 运行时则只读不刷新。
pub(crate) fn cached_script_text(pac_url: &str) -> Option<Arc<str>> {
    let (text, needs_refresh) = match cached(pac_url) {
        Cached::Fresh(text) => (text, false),
        Cached::Stale(text) => (text, true),
        Cached::Missing => (None, true),
    };
    if needs_refresh && begin_refresh(pac_url) {
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                let url = pac_url.to_string();
                handle.spawn(async move {
                    if refresh(&url).await.is_none() {
                        tracing::debug!("background PAC refresh produced no script: {url}");
                    }
                });
            }
            Err(_) => end_refresh(pac_url),
        }
    }
    text
}

/// PAC 脚本里出现的第一个代理端点（只读缓存，见 [`cached_script_text`]）。
/// Auto 模式的系统代理候选在 actor 线程上用它判定「有没有系统代理」。
pub fn resolve_pac_endpoint(pac_url: &str) -> Option<ProxyConfig> {
    let script = cached_script_text(pac_url)?;
    extract_proxy_endpoints(&script).into_iter().next()
}

async fn fetch_pac_text(pac_url: &str, max_bytes: usize) -> Result<String, String> {
    let url = Url::parse(pac_url.trim()).map_err(|e| format!("URL 非法: {e}"))?;
    match url.scheme() {
        "http" | "https" => fetch_http(url, max_bytes).await,
        "file" => fetch_file(&url, max_bytes).await,
        other => Err(format!("不支持的 PAC scheme: {other}")),
    }
}

async fn fetch_http(url: Url, max_bytes: usize) -> Result<String, String> {
    // PAC 定义代理本身，必须直连取回（与浏览器一致）。
    let client = crate::egress::client_builder(&ProxyConfig::default(), "")
        .map_err(|e| format!("构建 HTTP 客户端失败: {e}"))?
        .timeout(FETCH_TIMEOUT)
        .build()
        .map_err(|e| format!("构建 HTTP 客户端失败: {e}"))?;
    let mut resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("请求失败: {}", crate::logger::format_error_chain(&e)))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    if resp
        .content_length()
        .is_some_and(|n| n > u64::try_from(max_bytes).unwrap_or(u64::MAX))
    {
        return Err(format!("PAC 响应超过 {max_bytes} 字节上限"));
    }
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("读取失败: {}", crate::logger::format_error_chain(&e)))?
    {
        if buf.len().saturating_add(chunk.len()) > max_bytes {
            return Err(format!("PAC 响应超过 {max_bytes} 字节上限"));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

async fn fetch_file(url: &Url, max_bytes: usize) -> Result<String, String> {
    use tokio::io::AsyncReadExt;
    let path = url
        .to_file_path()
        .map_err(|()| "file:// 路径非法".to_string())?;
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|e| format!("打开 {} 失败: {e}", path.display()))?;
    let mut buf = Vec::new();
    // 多读 1 字节用来判断是否超限。
    file.take(
        u64::try_from(max_bytes)
            .unwrap_or(u64::MAX)
            .saturating_add(1),
    )
    .read_to_end(&mut buf)
    .await
    .map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
    if buf.len() > max_bytes {
        return Err(format!("PAC 文件超过 {max_bytes} 字节上限"));
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// 按下载链接做一次 PAC 决策。脚本取不到 → `None`（调用方按「没有 PAC」处理）。
pub(crate) async fn decide(pac_url: &str, url: &str) -> Option<PacDecision> {
    let script = script_for_task(pac_url).await?;
    #[cfg(feature = "plugins")]
    if let Some((target, host)) = pac_target(url) {
        match evaluate(&script, pac_url, target, host).await {
            Ok(entries) => {
                return Some(PacDecision {
                    entries,
                    evaluated: true,
                });
            }
            Err(reason) => {
                log_info!("[pac] 执行 FindProxyForURL 失败（退化为端点提取）: {reason}");
            }
        }
    }
    #[cfg(not(feature = "plugins"))]
    let _no_js_engine = url;
    Some(PacDecision {
        entries: extract_proxy_endpoints(&script)
            .into_iter()
            .map(PacEntry::Proxy)
            .collect(),
        evaluated: false,
    })
}

#[cfg(feature = "plugins")]
struct DecisionSlot {
    script: Arc<str>,
    expires: Instant,
    entries: Vec<PacEntry>,
}

#[cfg(feature = "plugins")]
type DecisionCache = HashMap<(String, String), DecisionSlot>;

#[cfg(feature = "plugins")]
static DECISIONS: Mutex<Option<DecisionCache>> = Mutex::new(None);

#[cfg(feature = "plugins")]
async fn evaluate(
    script: &Arc<str>,
    pac_url: &str,
    target: String,
    host: String,
) -> Result<Vec<PacEntry>, String> {
    let key = (pac_url.to_string(), target.clone());
    {
        let guard = lock(&DECISIONS);
        // 缓存条目持有脚本 Arc：`ptr_eq` 判定「同一份脚本」不会因地址复用误判。
        if let Some(slot) = guard.as_ref().and_then(|map| map.get(&key))
            && Arc::ptr_eq(&slot.script, script)
            && Instant::now() < slot.expires
        {
            return Ok(slot.entries.clone());
        }
    }
    let owned = Arc::clone(script);
    let raw = tokio::task::spawn_blocking(move || {
        crate::pac_js::find_proxy_for_url(&owned, &target, &host, EVAL_BUDGET)
    })
    .await
    .map_err(|e| format!("执行线程异常: {e}"))??;
    let entries = parse_pac_result(&raw);
    if entries.is_empty() {
        return Err(format!("返回值没有可用指令: {raw:?}"));
    }
    let mut guard = lock(&DECISIONS);
    let map = guard.get_or_insert_with(HashMap::new);
    if map.len() >= DECISION_CACHE_MAX {
        map.clear();
    }
    map.insert(
        key,
        DecisionSlot {
            script: Arc::clone(script),
            expires: Instant::now() + DECISION_TTL,
            entries: entries.clone(),
        },
    );
    Ok(entries)
}

/// 下载链接 → 传给 `FindProxyForURL` 的 `(url, host)`（Chromium 约定）：https/wss
/// 只传 `scheme://host[:port]/`；其余协议去掉用户信息与片段后原样传；host 小写、
/// IPv6 不带方括号。非层级 URL（无 host）→ `None`。
#[cfg_attr(not(feature = "plugins"), allow(dead_code))] // 仅执行 PAC 脚本（plugins）时调用；其余构建保留供单测覆盖
pub(crate) fn pac_target(url: &str) -> Option<(String, String)> {
    let mut parsed = Url::parse(url).ok()?;
    let host = match parsed.host()? {
        url::Host::Domain(domain) => domain.to_ascii_lowercase(),
        url::Host::Ipv4(addr) => addr.to_string(),
        url::Host::Ipv6(addr) => addr.to_string(),
    };
    let target = if matches!(parsed.scheme(), "https" | "wss") {
        let authority = parsed.host_str()?;
        match parsed.port() {
            Some(port) => format!("{}://{authority}:{port}/", parsed.scheme()),
            None => format!("{}://{authority}/", parsed.scheme()),
        }
    } else {
        parsed.set_fragment(None);
        parsed.set_username("").ok()?;
        parsed.set_password(None).ok()?;
        parsed.to_string()
    };
    Some((target, host))
}

/// 解析 `FindProxyForURL` 的返回串（`"PROXY a:1; SOCKS5 b:2; DIRECT"`），按顺序
/// 保留可用指令；无法识别的指令跳过（与浏览器一致）。
#[cfg_attr(not(feature = "plugins"), allow(dead_code))] // 仅执行 PAC 脚本（plugins）时调用；其余构建保留供单测覆盖
pub(crate) fn parse_pac_result(raw: &str) -> Vec<PacEntry> {
    raw.split(';').filter_map(parse_directive).collect()
}

/// 单条 PAC 指令：`DIRECT` 或 `<KEYWORD> host:port`。
fn parse_directive(directive: &str) -> Option<PacEntry> {
    let mut tokens = directive.split_whitespace();
    let keyword = tokens.next()?;
    if keyword.eq_ignore_ascii_case("DIRECT") {
        return tokens.next().is_none().then_some(PacEntry::Direct);
    }
    let (Some(endpoint), None) = (tokens.next(), tokens.next()) else {
        return None;
    };
    let proxy_type = match keyword.to_ascii_uppercase().as_str() {
        "PROXY" | "HTTP" => ProxyType::Http,
        "HTTPS" => ProxyType::Https,
        "SOCKS4" => ProxyType::Socks4,
        "SOCKS" | "SOCKS5" => ProxyType::Socks5,
        _ => return None,
    };
    let (host, port) = parse_endpoint(endpoint)?;
    Some(PacEntry::Proxy(ProxyConfig {
        mode: ProxyMode::Manual,
        proxy_type,
        host,
        port,
        ..Default::default()
    }))
}

fn proxy_key(cfg: &ProxyConfig) -> String {
    format!("{}://{}:{}", cfg.proxy_type.scheme(), cfg.host, cfg.port)
}

/// 记录一次经 PAC 选中的代理传输失败：5 分钟内排到列表最后。
pub(crate) fn mark_bad(cfg: &ProxyConfig) {
    let mut guard = lock(&BAD_PROXIES);
    let map = guard.get_or_insert_with(HashMap::new);
    let now = Instant::now();
    map.retain(|_, until| *until > now);
    map.insert(proxy_key(cfg), now + BAD_PROXY_TTL);
}

fn is_bad(cfg: &ProxyConfig, now: Instant) -> bool {
    lock(&BAD_PROXIES)
        .as_ref()
        .and_then(|map| map.get(&proxy_key(cfg)))
        .is_some_and(|until| *until > now)
}

/// 从 PAC 列表里挑本次使用的条目：第一个非坏代理（`DIRECT` 永不算坏）；全坏时
/// 按原顺序取第一个（Chromium 语义：坏代理只降序、不剔除）。
pub(crate) fn pick(entries: &[PacEntry]) -> Option<&PacEntry> {
    let now = Instant::now();
    entries
        .iter()
        .find(|entry| match entry {
            PacEntry::Direct => true,
            PacEntry::Proxy(cfg) => !is_bad(cfg, now),
        })
        .or_else(|| entries.first())
}

/// 从 PAC 脚本里按出现顺序提取去重后的代理端点（纯函数）。
///
/// 只看代码里的字符串字面量（`"…"`/`'…'`/`` `…` ``），注释内容整体跳过；字面量按
/// `;` 切成指令，形如 `<KEYWORD> host:port` 才算，`DIRECT` 与非法端口忽略。
pub(crate) fn extract_proxy_endpoints(script: &str) -> Vec<ProxyConfig> {
    let mut out: Vec<ProxyConfig> = Vec::new();
    for literal in string_literals(script) {
        for directive in literal.split(';') {
            let Some(PacEntry::Proxy(cfg)) = parse_directive(directive) else {
                continue;
            };
            if out
                .iter()
                .any(|c| c.proxy_type == cfg.proxy_type && c.host == cfg.host && c.port == cfg.port)
            {
                continue;
            }
            out.push(cfg);
        }
    }
    out
}

/// 按出现顺序收集代码中的字符串字面量内容，跳过 `//` 与 `/* */` 注释。
/// 字符串内的 `//`（如 `"http://x"`）不是注释，所以必须按词法状态扫描。
fn string_literals(script: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = script.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '/' => match chars.peek() {
                Some('/') => {
                    for n in chars.by_ref() {
                        if n == '\n' {
                            break;
                        }
                    }
                }
                Some('*') => {
                    chars.next();
                    let mut prev = ' ';
                    for n in chars.by_ref() {
                        if prev == '*' && n == '/' {
                            break;
                        }
                        prev = n;
                    }
                }
                _ => {}
            },
            '"' | '\'' | '`' => {
                let mut s = String::new();
                while let Some(n) = chars.next() {
                    if n == '\\' {
                        if let Some(escaped) = chars.next() {
                            s.push(escaped);
                        }
                        continue;
                    }
                    if n == c || (n == '\n' && c != '`') {
                        break;
                    }
                    s.push(n);
                }
                out.push(s);
            }
            _ => {}
        }
    }
    out
}

/// 解析 `host:port`；host 可为 IPv4/域名/`[IPv6]`（IPv6 按现有约定保留方括号）。
fn parse_endpoint(raw: &str) -> Option<(String, u16)> {
    let (host, port_text) = if let Some(rest) = raw.strip_prefix('[') {
        let (inner, after) = rest.split_once(']')?;
        inner.parse::<Ipv6Addr>().ok()?;
        (format!("[{inner}]"), after.strip_prefix(':')?)
    } else {
        let (host, port_text) = raw.rsplit_once(':')?;
        let valid = !host.is_empty()
            && host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'));
        if !valid {
            return None;
        }
        (host.to_string(), port_text)
    };
    if port_text.is_empty() || !port_text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let port = port_text.parse::<u16>().ok().filter(|p| *p != 0)?;
    Some((host, port))
}

/// 剩余预算；耗尽返回错误（`recv_timeout` 的 0 时长没有意义）。
#[cfg_attr(not(feature = "plugins"), allow(dead_code))] // 仅 PAC 脚本内 DNS（plugins）调用
fn remaining(deadline: Instant) -> Result<Duration, String> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        Err("超时".to_string())
    } else {
        Ok(left)
    }
}

/// 在独立线程里解析域名，调用方最多等到 `deadline`；超时按失败处理，解析线程
/// 自然结束、不 join。`localhost` 直接解析（走 hosts，不会卡网络）。PAC 脚本里
/// 的 `dnsResolve` 等经此保证有界。
#[cfg_attr(not(feature = "plugins"), allow(dead_code))] // 仅 PAC 脚本内 DNS（plugins）调用
pub(crate) fn resolve_bounded<F>(
    name: &str,
    port: u16,
    deadline: Instant,
    resolver: F,
) -> Result<Vec<SocketAddr>, String>
where
    F: FnOnce(String, u16) -> std::io::Result<Vec<SocketAddr>> + Send + 'static,
{
    if name.eq_ignore_ascii_case("localhost") {
        return resolver(name.to_string(), port).map_err(|e| format!("解析 {name} 失败: {e}"));
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let owned = name.to_string();
    std::thread::Builder::new()
        .name("pac-dns".to_string())
        .spawn(move || {
            // 接收端超时后已丢弃，发送失败属预期。
            if tx.send(resolver(owned, port)).is_err() {
                crate::logger::log_info!("[pac] DNS 结果在超时后到达，已丢弃");
            }
        })
        .map_err(|e| format!("启动解析线程失败: {e}"))?;
    match rx.recv_timeout(remaining(deadline)?) {
        Ok(Ok(addrs)) => Ok(addrs),
        Ok(Err(e)) => Err(format!("解析 {name} 失败: {e}")),
        Err(_) => Err(format!("解析 {name} 超时")),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    fn first(script: &str) -> Option<(ProxyType, String, u16)> {
        extract_proxy_endpoints(script)
            .into_iter()
            .next()
            .map(|c| (c.proxy_type, c.host, c.port))
    }

    #[test]
    fn clash_style_returns_first_proxy_in_order() {
        let script = r#"function FindProxyForURL(url, host) {
            return "PROXY 127.0.0.1:7890; SOCKS5 127.0.0.1:7891; DIRECT";
        }"#;
        let eps = extract_proxy_endpoints(script);
        assert_eq!(eps.len(), 2);
        assert_eq!(
            first(script),
            Some((ProxyType::Http, "127.0.0.1".to_string(), 7890))
        );
        assert_eq!(eps[1].proxy_type, ProxyType::Socks5);
        assert_eq!(eps[1].port, 7891);
    }

    #[test]
    fn gfwlist_style_constant_dedups_socks_and_socks5() {
        let script = r#"
            var proxy = "SOCKS5 127.0.0.1:1080; SOCKS 127.0.0.1:1080; DIRECT;";
            var rules = [["http://www.google.com", "x"]];
            function FindProxyForURL(url, host) { if (x) return proxy; return "DIRECT"; }
        "#;
        let eps = extract_proxy_endpoints(script);
        // SOCKS 与 SOCKS5 同映射 Socks5，同端点只保留一条。
        assert_eq!(eps.len(), 1);
        assert_eq!(eps[0].proxy_type, ProxyType::Socks5);
        assert_eq!(eps[0].host, "127.0.0.1");
        assert_eq!(eps[0].port, 1080);
    }

    #[test]
    fn directives_inside_comments_are_ignored() {
        let script = r#"
            // return "PROXY 1.1.1.1:1111";
            /* var p = "PROXY 2.2.2.2:2222"; */
            var u = "http://example.com/x"; // "PROXY 3.3.3.3:3333"
            return "PROXY 4.4.4.4:4444";
        "#;
        let eps = extract_proxy_endpoints(script);
        assert_eq!(eps.len(), 1);
        assert_eq!(eps[0].host, "4.4.4.4");
    }

    #[test]
    fn slashes_inside_strings_do_not_start_comments() {
        let script = r#"var a = "http://x//y"; return "PROXY 5.5.5.5:5555";"#;
        assert_eq!(
            first(script),
            Some((ProxyType::Http, "5.5.5.5".to_string(), 5555))
        );
    }

    #[test]
    fn invalid_ports_and_all_direct_yield_none() {
        assert!(first(r#"return "PROXY 127.0.0.1:0";"#).is_none());
        assert!(first(r#"return "PROXY 127.0.0.1:65536";"#).is_none());
        assert!(first(r#"return "PROXY 127.0.0.1:+80";"#).is_none());
        assert!(first(r#"return "PROXY 127.0.0.1";"#).is_none());
        assert!(first(r#"return "DIRECT";"#).is_none());
        assert!(first(r#"return "DIRECT; DIRECT";"#).is_none());
    }

    #[test]
    fn keywords_are_case_insensitive_and_variants_map() {
        let script = r#"return "http a.example.com:80; Https b.example.com:443; socks4 [::1]:1081; SoCkS5 c-d.example:1";"#;
        let eps = extract_proxy_endpoints(script);
        let got: Vec<_> = eps
            .iter()
            .map(|c| (c.proxy_type.clone(), c.host.as_str(), c.port))
            .collect();
        assert_eq!(
            got,
            vec![
                (ProxyType::Http, "a.example.com", 80),
                (ProxyType::Https, "b.example.com", 443),
                (ProxyType::Socks4, "[::1]", 1081),
                (ProxyType::Socks5, "c-d.example", 1),
            ]
        );
    }

    #[test]
    fn bad_hosts_are_skipped_not_fatal() {
        let script = r#"return "PROXY [zz]:80; PROXY ho st:80; PROXY :80; PROXY 9.9.9.9:9";"#;
        assert_eq!(
            first(script),
            Some((ProxyType::Http, "9.9.9.9".to_string(), 9))
        );
    }

    fn describe(entries: &[PacEntry]) -> Vec<String> {
        entries
            .iter()
            .map(|e| match e {
                PacEntry::Direct => "DIRECT".to_string(),
                PacEntry::Proxy(c) => format!("{}:{}", c.host, c.port),
            })
            .collect()
    }

    #[test]
    fn pac_result_keeps_order_and_skips_unknown_directives() {
        let entries = parse_pac_result("PROXY a.example:1;  ; QUIC q:443; direct; SOCKS5 [::1]:2");
        assert_eq!(describe(&entries), vec!["a.example:1", "DIRECT", "[::1]:2"]);
        assert!(parse_pac_result("").is_empty());
        assert!(parse_pac_result("DIRECT extra").is_empty());
    }

    #[test]
    fn pac_target_hides_https_path_and_strips_credentials() {
        assert_eq!(
            pac_target("https://User:pw@Dl.Example.com:8443/a/b?token=x#f"),
            Some((
                "https://dl.example.com:8443/".to_string(),
                "dl.example.com".to_string()
            ))
        );
        assert_eq!(
            pac_target("http://u:p@Example.com/f.bin?x=1#frag"),
            Some((
                "http://example.com/f.bin?x=1".to_string(),
                "example.com".to_string()
            ))
        );
        assert_eq!(
            pac_target("http://[::1]:8080/x").map(|(_, h)| h),
            Some("::1".to_string())
        );
        assert_eq!(pac_target("magnet:?xt=urn:btih:abc"), None);
    }

    #[test]
    fn bad_proxies_sink_to_the_end_but_never_vanish() {
        let entries = parse_pac_result("PROXY bad-a.pac-test:1; PROXY ok-b.pac-test:2; DIRECT");
        let PacEntry::Proxy(bad) = &entries[0] else {
            panic!("first entry must be a proxy");
        };
        mark_bad(bad);
        assert_eq!(
            describe(std::slice::from_ref(pick(&entries).unwrap())),
            vec!["ok-b.pac-test:2"]
        );
        // 列表只剩坏代理：仍按原顺序取，不得返回 None。
        let only_bad = parse_pac_result("PROXY bad-a.pac-test:1");
        assert_eq!(
            describe(std::slice::from_ref(pick(&only_bad).unwrap())),
            vec!["bad-a.pac-test:1"]
        );
        assert!(pick(&[]).is_none());
    }

    #[tokio::test]
    async fn file_fetch_enforces_size_cap_and_unknown_schemes_fail() {
        let dir = std::env::temp_dir().join(format!("fluxdown-pac-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("p.pac");
        std::fs::write(&path, "return \"PROXY 1.2.3.4:80\";").expect("write");
        let url = Url::from_file_path(&path).expect("file url").to_string();
        assert!(fetch_pac_text(&url, 1024).await.is_ok());
        assert!(fetch_pac_text(&url, 8).await.is_err());
        assert!(
            fetch_pac_text("ftp://example.com/p.pac", 1024)
                .await
                .is_err()
        );
        assert!(fetch_pac_text("not a url", 1024).await.is_err());
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// 本地一次性 HTTP 服务：读完请求头后回 `response`。
    async fn serve_once(response: String) -> u16 {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().expect("addr").port();
        tokio::spawn(async move {
            let Ok((mut conn, _)) = listener.accept().await else {
                return;
            };
            let mut req = Vec::new();
            let mut chunk = [0u8; 1024];
            while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                match conn.read(&mut chunk).await {
                    Ok(0) | Err(_) => return,
                    Ok(n) => req.extend_from_slice(&chunk[..n]),
                }
            }
            if let Err(e) = conn.write_all(response.as_bytes()).await {
                eprintln!("test server write failed: {e}");
            }
        });
        port
    }

    #[tokio::test]
    async fn http_fetch_checks_status_and_size() {
        let body = "return \"PROXY 1.2.3.4:80\";";
        let ok = serve_once(format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ))
        .await;
        assert_eq!(
            fetch_pac_text(&format!("http://127.0.0.1:{ok}/p.pac"), 1024)
                .await
                .as_deref(),
            Ok(body)
        );
        let missing = serve_once(
            "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
        )
        .await;
        assert!(
            fetch_pac_text(&format!("http://127.0.0.1:{missing}/p.pac"), 1024)
                .await
                .is_err()
        );
        let big = "a".repeat(5000);
        let oversize =
            serve_once(format!("HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{big}")).await;
        assert!(
            fetch_pac_text(&format!("http://127.0.0.1:{oversize}/p.pac"), 1024)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn failed_fetch_is_negatively_cached() {
        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
            listener.local_addr().expect("addr").port()
        };
        let url = format!("http://127.0.0.1:{port}/never.pac");
        assert!(refresh(&url).await.is_none());
        assert!(matches!(cached(&url), Cached::Fresh(None)), "失败须负缓存");
    }

    #[tokio::test]
    async fn hijacked_non_pac_response_is_rejected() {
        // 运营商 DNS 劫持返回的广告页：HTTP 200 但不是 PAC。
        let html = "<html><body>ad</body></html>";
        let port = serve_once(format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
            html.len()
        ))
        .await;
        assert!(
            refresh(&format!("http://127.0.0.1:{port}/wpad.dat"))
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn slow_explicit_pac_never_holds_a_task_longer_than_the_wait_cap() {
        // 只接受连接、永不应答的 PAC 服务。
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().expect("addr").port();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((conn, _)) = listener.accept().await {
                held.push(conn);
            }
        });
        let started = Instant::now();
        let decision = decide(&format!("http://127.0.0.1:{port}/p.pac"), "http://a.test/f").await;
        assert!(decision.is_none());
        assert!(started.elapsed() < TASK_FETCH_WAIT + Duration::from_secs(1));
    }

    #[test]
    fn dns_resolution_is_bounded_by_the_deadline() {
        let started = Instant::now();
        let result = resolve_bounded(
            "slow.invalid",
            80,
            Instant::now() + Duration::from_millis(200),
            |_, _| {
                std::thread::sleep(Duration::from_secs(3));
                Ok(Vec::new())
            },
        );
        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn dns_resolution_returns_resolver_addresses_in_time() {
        let addr: SocketAddr = "127.0.0.1:80".parse().expect("addr");
        let got = resolve_bounded(
            "fast.invalid",
            80,
            Instant::now() + Duration::from_secs(1),
            move |_, _| Ok(vec![addr]),
        );
        assert_eq!(got, Ok(vec![addr]));
    }
}
