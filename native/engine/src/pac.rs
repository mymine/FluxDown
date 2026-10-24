//! 系统 PAC 脚本的「代理端点」提取。
//!
//! `ProxyMode::Auto` 自己按 host 实测/竞速选路，只需要「PAC 指向哪个代理端点」，
//! 不需要逐 URL 执行 `FindProxyForURL`（那需要 JS 引擎，且 System 模式的逐 URL
//! 语义不在本模块范围）。因此这里只做三件事：
//!
//! 1. 有界地取回 PAC 文本（`http://` 走 std `TcpStream`，`file://` 读文件；
//!    `https://` 不支持——没有可用的同步 TLS 客户端，记一次日志后放弃）；
//! 2. 从脚本里扫出字符串字面量中的代理指令
//!    （`PROXY|HTTP|HTTPS|SOCKS|SOCKS4|SOCKS5 host:port`），跳过注释与 `DIRECT`；
//! 3. 按 PAC URL 缓存结果，避免 actor 线程反复取脚本。
//!
//! WPAD 自动发现（系统配置里没有 PAC URL）不在范围内。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Ipv6Addr, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use url::Url;

use crate::logger::log_info;
use crate::proxy_config::{ProxyConfig, ProxyMode, ProxyType};

/// 取脚本的总时限（连接 + 读写）。本函数在 actor 线程同步调用，必须有界。
const FETCH_BUDGET: Duration = Duration::from_millis(1500);
/// PAC 脚本体上限；超限直接拒绝（截断后的脚本可能缺失关键分支，不如不用）。
const MAX_PAC_BYTES: usize = 2 * 1024 * 1024;
/// 响应头上限，防止对端无限吐头部。
const MAX_HEADER_BYTES: usize = 16 * 1024;
/// 成功结果缓存时长。
const SUCCESS_TTL: Duration = Duration::from_secs(300);
/// 失败（取不到/无端点）负缓存时长，避免 PAC 服务挂掉时每次探测都等满时限。
const FAILURE_TTL: Duration = Duration::from_secs(30);
/// 缓存条目数上限；PAC URL 实际只有 1~2 个，超限直接清空重来。
const CACHE_MAX_ENTRIES: usize = 16;

type PacCache = HashMap<String, (Instant, Option<ProxyConfig>)>;

static CACHE: Mutex<Option<PacCache>> = Mutex::new(None);

/// 解析 PAC URL 指向的第一个可用代理端点（带缓存）。无端点/取不到返回 `None`。
pub fn resolve_pac_endpoint(pac_url: &str) -> Option<ProxyConfig> {
    let now = Instant::now();
    {
        let guard = CACHE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((expires, cached)) = guard.as_ref().and_then(|m| m.get(pac_url))
            && now < *expires
        {
            return cached.clone();
        }
    }
    // 取脚本期间不持锁：慢 PAC 服务器不能阻塞其它调用方读缓存。
    let resolved = load_endpoint(pac_url);
    let ttl = if resolved.is_some() {
        SUCCESS_TTL
    } else {
        FAILURE_TTL
    };
    let mut guard = CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let map = guard.get_or_insert_with(HashMap::new);
    if map.len() >= CACHE_MAX_ENTRIES && !map.contains_key(pac_url) {
        map.clear();
    }
    map.insert(
        pac_url.to_string(),
        (Instant::now() + ttl, resolved.clone()),
    );
    resolved
}

fn load_endpoint(pac_url: &str) -> Option<ProxyConfig> {
    let text = match fetch_pac_text(pac_url, FETCH_BUDGET, MAX_PAC_BYTES) {
        Ok(text) => text,
        Err(reason) => {
            log_info!("[pac] 取 PAC 脚本失败 {pac_url}: {reason}");
            return None;
        }
    };
    let endpoint = extract_proxy_endpoints(&text).into_iter().next();
    match &endpoint {
        Some(cfg) => log_info!(
            "[pac] {pac_url} -> {}://{}:{}",
            cfg.proxy_type.scheme(),
            cfg.host,
            cfg.port
        ),
        None => log_info!("[pac] {pac_url} 中没有可用的代理端点"),
    }
    endpoint
}

/// 按 URL scheme 取 PAC 文本。`budget` 为总时限，`max_bytes` 为脚本体上限。
fn fetch_pac_text(pac_url: &str, budget: Duration, max_bytes: usize) -> Result<String, String> {
    let url = Url::parse(pac_url.trim()).map_err(|e| format!("URL 非法: {e}"))?;
    match url.scheme() {
        "http" => fetch_http(&url, budget, max_bytes),
        "file" => fetch_file(&url, max_bytes),
        "https" => Err("不支持 https:// PAC".to_string()),
        other => Err(format!("不支持的 PAC scheme: {other}")),
    }
}

fn fetch_file(url: &Url, max_bytes: usize) -> Result<String, String> {
    let path = url
        .to_file_path()
        .map_err(|()| "file:// 路径非法".to_string())?;
    let file =
        std::fs::File::open(&path).map_err(|e| format!("打开 {} 失败: {e}", path.display()))?;
    let mut buf = Vec::new();
    // 多读 1 字节用来判断是否超限。
    file.take(max_bytes as u64 + 1)
        .read_to_end(&mut buf)
        .map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
    if buf.len() > max_bytes {
        return Err(format!("PAC 文件超过 {max_bytes} 字节上限"));
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// 剩余预算；耗尽返回错误（`set_*_timeout` 不接受 0 时长）。
fn remaining(deadline: Instant) -> Result<Duration, String> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        Err("超时".to_string())
    } else {
        Ok(left)
    }
}

fn fetch_http(url: &Url, budget: Duration, max_bytes: usize) -> Result<String, String> {
    let deadline = Instant::now() + budget;
    let host = url.host_str().ok_or_else(|| "URL 缺少 host".to_string())?;
    let port = url.port_or_known_default().unwrap_or(80);
    let addrs = match url.host() {
        Some(url::Host::Ipv4(ip)) => vec![SocketAddr::new(ip.into(), port)],
        Some(url::Host::Ipv6(ip)) => vec![SocketAddr::new(ip.into(), port)],
        Some(url::Host::Domain(name)) => resolve_bounded(name, port, deadline, |n, p| {
            (n.as_str(), p).to_socket_addrs().map(Iterator::collect)
        })?,
        None => return Err("URL 缺少 host".to_string()),
    };
    let mut stream = None;
    let mut last_err = String::from("无可用地址");
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, remaining(deadline)?) {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(e) => last_err = format!("连接 {addr} 失败: {e}"),
        }
    }
    let mut stream = stream.ok_or(last_err)?;

    let mut target = url.path().to_string();
    if let Some(q) = url.query() {
        target.push('?');
        target.push_str(q);
    }
    let host_header = match url.port() {
        Some(p) => format!("{host}:{p}"),
        None => host.to_string(),
    };
    let request = format!(
        "GET {target} HTTP/1.0\r\nHost: {host_header}\r\nAccept: */*\r\nConnection: close\r\n\r\n"
    );
    stream
        .set_write_timeout(Some(remaining(deadline)?))
        .map_err(|e| format!("设置写超时失败: {e}"))?;
    stream
        .write_all(request.as_bytes())
        .map_err(|e| format!("发送请求失败: {e}"))?;

    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    // (正文起点, Content-Length)
    let mut head: Option<(usize, Option<usize>)> = None;
    loop {
        if head.is_none()
            && let Some(pos) = find_header_end(&buf)
        {
            let (status_ok, content_length) = parse_head(buf.get(..pos).unwrap_or_default())?;
            if !status_ok {
                return Err("HTTP 状态码不是 200".to_string());
            }
            if content_length.is_some_and(|n| n > max_bytes) {
                return Err(format!("PAC 响应超过 {max_bytes} 字节上限"));
            }
            head = Some((pos + 4, content_length));
        }
        match head {
            Some((start, length)) => {
                let body_len = buf.len().saturating_sub(start);
                if body_len > max_bytes {
                    return Err(format!("PAC 响应超过 {max_bytes} 字节上限"));
                }
                if length.is_some_and(|n| body_len >= n) {
                    break;
                }
            }
            None if buf.len() > MAX_HEADER_BYTES => {
                return Err("响应头过长".to_string());
            }
            None => {}
        }
        stream
            .set_read_timeout(Some(remaining(deadline)?))
            .map_err(|e| format!("设置读超时失败: {e}"))?;
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(chunk.get(..n).unwrap_or_default()),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                return Err("读取超时".to_string());
            }
            Err(e) => return Err(format!("读取失败: {e}")),
        }
    }

    let Some((start, length)) = head else {
        return Err("响应在头部结束前被关闭".to_string());
    };
    let body = buf.get(start..).unwrap_or_default();
    let body = match length {
        Some(n) => body.get(..n).ok_or_else(|| "响应体被截断".to_string())?,
        None => body,
    };
    Ok(String::from_utf8_lossy(body).into_owned())
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

/// 解析响应头，返回（状态码是否 200，Content-Length）。
fn parse_head(head: &[u8]) -> Result<(bool, Option<usize>), String> {
    let text = String::from_utf8_lossy(head);
    let mut lines = text.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let mut parts = status_line.split_whitespace();
    let version = parts.next().unwrap_or_default();
    if !version.starts_with("HTTP/") {
        return Err("响应不是 HTTP".to_string());
    }
    let status_ok = parts.next() == Some("200");
    let content_length = lines.find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name.trim().eq_ignore_ascii_case("content-length") {
            value.trim().parse::<usize>().ok()
        } else {
            None
        }
    });
    Ok((status_ok, content_length))
}

/// 从 PAC 脚本里按出现顺序提取去重后的代理端点（纯函数）。
///
/// 只看代码里的字符串字面量（`"…"`/`'…'`/`` `…` ``），注释内容整体跳过；字面量按
/// `;` 切成指令，形如 `<KEYWORD> host:port` 才算，`DIRECT` 与非法端口忽略。
pub(crate) fn extract_proxy_endpoints(script: &str) -> Vec<ProxyConfig> {
    let mut out: Vec<ProxyConfig> = Vec::new();
    for literal in string_literals(script) {
        for directive in literal.split(';') {
            let mut tokens = directive.split_whitespace();
            let (Some(keyword), Some(endpoint), None) =
                (tokens.next(), tokens.next(), tokens.next())
            else {
                continue;
            };
            let proxy_type = match keyword.to_ascii_uppercase().as_str() {
                "PROXY" | "HTTP" => ProxyType::Http,
                "HTTPS" => ProxyType::Https,
                "SOCKS4" => ProxyType::Socks4,
                "SOCKS" | "SOCKS5" => ProxyType::Socks5,
                _ => continue,
            };
            let Some((host, port)) = parse_endpoint(endpoint) else {
                continue;
            };
            if out
                .iter()
                .any(|c| c.proxy_type == proxy_type && c.host == host && c.port == port)
            {
                continue;
            }
            out.push(ProxyConfig {
                mode: ProxyMode::Manual,
                proxy_type,
                host,
                port,
                ..Default::default()
            });
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

/// 在独立线程里解析域名，主线程最多等到 `deadline`；超时按失败处理，解析线程
/// 自然结束、不 join。`localhost` 直接解析（走 hosts，不会卡网络）。
fn resolve_bounded<F>(
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
    use std::net::TcpListener;

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

    /// 起一个只服务一次连接的本地服务器：读完请求头后写出 `response`，
    /// `stall` 为真时读完请求后不回任何字节并保持连接。
    fn serve_once(response: Vec<u8>, stall: bool) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        std::thread::spawn(move || {
            let Ok((mut conn, _)) = listener.accept() else {
                return;
            };
            let mut req = Vec::new();
            let mut chunk = [0u8; 1024];
            while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                match conn.read(&mut chunk) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => req.extend_from_slice(&chunk[..n]),
                }
            }
            if stall {
                std::thread::sleep(Duration::from_secs(3));
                return;
            }
            if let Err(e) = conn.write_all(&response) {
                eprintln!("test server write failed: {e}");
            }
        });
        port
    }

    fn http_response(status: &str, headers: &str, body: &str) -> Vec<u8> {
        format!("HTTP/1.0 {status}\r\n{headers}\r\n{body}").into_bytes()
    }

    fn fetch(port: u16, budget: Duration, max: usize) -> Result<String, String> {
        fetch_pac_text(
            &format!("http://127.0.0.1:{port}/proxy.pac?x=1"),
            budget,
            max,
        )
    }

    #[test]
    fn http_fetch_reads_body_by_content_length_and_eof() {
        let body = "return \"PROXY 1.2.3.4:80\";";
        let port = serve_once(
            http_response(
                "200 OK",
                &format!("Content-Length: {}\r\n", body.len()),
                body,
            ),
            false,
        );
        assert_eq!(
            fetch(port, Duration::from_secs(2), 1024).as_deref(),
            Ok(body)
        );

        // 无 Content-Length：读到 EOF。
        let port = serve_once(http_response("200 OK", "", body), false);
        assert_eq!(
            fetch(port, Duration::from_secs(2), 1024).as_deref(),
            Ok(body)
        );
    }

    #[test]
    fn http_fetch_rejects_non_200() {
        let port = serve_once(http_response("404 Not Found", "", "nope"), false);
        assert!(fetch(port, Duration::from_secs(2), 1024).is_err());
    }

    #[test]
    fn http_fetch_rejects_oversize_body_both_ways() {
        // 声明的 Content-Length 超限。
        let port = serve_once(
            http_response("200 OK", "Content-Length: 5000\r\n", &"a".repeat(5000)),
            false,
        );
        assert!(fetch(port, Duration::from_secs(2), 1024).is_err());
        // 不声明长度，实际体超限。
        let port = serve_once(http_response("200 OK", "", &"a".repeat(5000)), false);
        assert!(fetch(port, Duration::from_secs(2), 1024).is_err());
        // 恰好等于上限则接受。
        let port = serve_once(http_response("200 OK", "", &"a".repeat(1024)), false);
        assert_eq!(
            fetch(port, Duration::from_secs(2), 1024).map(|s| s.len()),
            Ok(1024)
        );
    }

    #[test]
    fn http_fetch_rejects_truncated_body() {
        let port = serve_once(
            http_response("200 OK", "Content-Length: 100\r\n", "short"),
            false,
        );
        assert!(fetch(port, Duration::from_secs(2), 1024).is_err());
    }

    #[test]
    fn http_fetch_times_out_on_silent_server() {
        let port = serve_once(Vec::new(), true);
        let started = Instant::now();
        assert!(fetch(port, Duration::from_millis(300), 1024).is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn https_and_unknown_schemes_are_unsupported() {
        assert!(fetch_pac_text("https://example.com/p.pac", FETCH_BUDGET, 1024).is_err());
        assert!(fetch_pac_text("ftp://example.com/p.pac", FETCH_BUDGET, 1024).is_err());
        assert!(fetch_pac_text("not a url", FETCH_BUDGET, 1024).is_err());
    }

    #[test]
    fn file_fetch_enforces_size_cap() {
        let dir = std::env::temp_dir().join(format!("fluxdown-pac-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("p.pac");
        std::fs::write(&path, "return \"PROXY 1.2.3.4:80\";").expect("write");
        let url = Url::from_file_path(&path).expect("file url").to_string();
        assert!(fetch_pac_text(&url, FETCH_BUDGET, 1024).is_ok());
        assert!(fetch_pac_text(&url, FETCH_BUDGET, 8).is_err());
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn resolve_caches_failure_without_refetching() {
        // 端口 1 无监听 → 连接失败；第二次必须命中负缓存（同一 URL）。
        let url = "http://127.0.0.1:1/never.pac";
        assert!(resolve_pac_endpoint(url).is_none());
        let started = Instant::now();
        assert!(resolve_pac_endpoint(url).is_none());
        assert!(started.elapsed() < Duration::from_millis(50));
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
