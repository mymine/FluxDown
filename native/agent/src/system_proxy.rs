//! agent 自身外网客户端「跟随系统代理」的显式装配。
//!
//! reqwest 0.12 未设代理时交给 hyper-util `Matcher::from_system`：环境变量逐槽优先，macOS 再从
//! SCDynamicStore 补 HTTP / HTTPS 代理地址——但**从不读 `ExceptionsList`**，直连名单只剩
//! `NO_PROXY` 环境变量。GUI 进程没有该变量，于是系统设置「忽略这些主机与域的代理设置」里的回环、
//! 局域网、`.local` 全被送进代理（云端 / 更新请求连本机地址被代理接走即此）。
//!
//! 这里按 hyper-util 同一优先级重建代理，只修直连名单：
//! - 环回恒直连（同引擎 `proxy_config::loopback_no_proxy`）；
//! - 两槽都并入 `NO_PROXY` / `no_proxy`；
//! - 取自 macOS 系统设置的槽再并入归一化后的 `ExceptionsList`。
//!
//! 名单之外的目标（海外站点等）照常走代理，代理软件按自身规则分流。`ExcludeSimpleHostnames`
//! （不含点的主机名直连）`NoProxy` 无法表达，不处理。Windows 由 hyper-util 读注册表
//! `ProxyOverride`，保持 reqwest 默认。

use reqwest::ClientBuilder;

/// 给 `builder` 装上系统代理。macOS 会起 `scutil` 子进程，须在阻塞上下文调用。
#[cfg(not(windows))]
pub fn follow_system(builder: ClientBuilder) -> ClientBuilder {
    SystemProxy::resolve(&|name| std::env::var(name).ok(), os_settings).apply(builder)
}

/// Windows：hyper-util 已读注册表代理与 `ProxyOverride`，沿用 reqwest 默认。
#[cfg(windows)]
pub fn follow_system(builder: ClientBuilder) -> ClientBuilder {
    builder
}

#[cfg(not(windows))]
use imp::{SystemProxy, os_settings};

#[cfg(not(windows))]
mod imp {
    use reqwest::{ClientBuilder, NoProxy, Proxy};

    /// 环回恒直连：本机服务绝不能被转发到外部代理。
    const LOOPBACK: [&str; 3] = ["localhost", "127.0.0.0/8", "::1"];

    /// 操作系统代理设置（目前只有 macOS 有来源）。
    #[derive(Debug, Default, Clone, PartialEq, Eq)]
    pub(super) struct OsProxy {
        /// `host[:port]`，未启用为 `None`。
        pub http: Option<String>,
        pub https: Option<String>,
        /// 原样的例外列表条目。
        pub exceptions: Vec<String>,
    }

    /// 按目标协议构造代理（`Proxy::http` / `Proxy::https`）。
    type MakeProxy = fn(&str) -> reqwest::Result<Proxy>;

    /// 一个目标协议的代理及其直连名单（`NoProxy` 语法）。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(super) struct Route {
        pub url: String,
        pub no_proxy: String,
    }

    #[derive(Debug, Default, Clone, PartialEq, Eq)]
    pub(super) struct SystemProxy {
        pub http: Option<Route>,
        pub https: Option<Route>,
    }

    impl SystemProxy {
        /// 与 hyper-util `Builder::from_system().build()` 同一优先级：每槽先取环境变量，空槽才读
        /// 系统设置，仍无效再退到 `ALL_PROXY`；CGI 环境（`REQUEST_METHOD`）一律不用代理。
        pub(super) fn resolve(
            env: &dyn Fn(&str) -> Option<String>,
            os: impl FnOnce() -> OsProxy,
        ) -> Self {
            if env("REQUEST_METHOD").is_some() {
                return Self::default();
            }
            let first = |names: [&str; 2]| {
                names
                    .into_iter()
                    .find_map(env)
                    .filter(|value| !value.trim().is_empty())
            };
            let env_bypass = first(["NO_PROXY", "no_proxy"]).unwrap_or_default();
            let all = first(["ALL_PROXY", "all_proxy"]);
            let mut http = first(["HTTP_PROXY", "http_proxy"]).map(|value| (value, false));
            let mut https = first(["HTTPS_PROXY", "https_proxy"]).map(|value| (value, false));
            let mut exceptions = Vec::new();
            if http.is_none() || https.is_none() {
                let os = os();
                http = http.or(os.http.map(|value| (value, true)));
                https = https.or(os.https.map(|value| (value, true)));
                exceptions = os.exceptions;
            }
            let route = |slot: Option<(String, bool)>| {
                let (url, from_os) = slot
                    .and_then(|(value, from_os)| proxy_url(&value).map(|url| (url, from_os)))
                    .or_else(|| all.as_deref().and_then(proxy_url).map(|url| (url, false)))?;
                let os_bypass: &[String] = if from_os { &exceptions } else { &[] };
                Some(Route {
                    url,
                    no_proxy: bypass_list(&env_bypass, os_bypass),
                })
            };
            Self {
                http: route(http),
                https: route(https),
            }
        }

        pub(super) fn apply(&self, mut builder: ClientBuilder) -> ClientBuilder {
            let slots: [(&Option<Route>, MakeProxy); 2] = [
                (&self.http, |url| Proxy::http(url)),
                (&self.https, |url| Proxy::https(url)),
            ];
            let mut installed = false;
            for (route, make) in slots {
                let Some(route) = route else { continue };
                match make(&route.url) {
                    Ok(proxy) => {
                        builder =
                            builder.proxy(proxy.no_proxy(NoProxy::from_string(&route.no_proxy)));
                        installed = true;
                    }
                    // 地址可能含认证信息，不写日志。
                    Err(error) => {
                        tracing::warn!(%error, "system proxy address rejected; connecting directly")
                    }
                }
            }
            // 已完整评估过环境与系统设置：没有代理就显式直连，不再让 reqwest 重读一遍。
            if installed {
                builder
            } else {
                builder.no_proxy()
            }
        }
    }

    /// hyper-util `parse_env_uri` 接受的代理写法：http(s) / socks4(a) / socks5(h)，无 scheme 视为 http。
    fn proxy_url(value: &str) -> Option<String> {
        let value = value.trim();
        let candidate = if value.contains("://") {
            value.to_owned()
        } else {
            format!("http://{value}")
        };
        let url = reqwest::Url::parse(&candidate).ok()?;
        let supported = matches!(
            url.scheme(),
            "http" | "https" | "socks4" | "socks4a" | "socks5" | "socks5h"
        );
        (supported && url.host_str().is_some_and(|host| !host.is_empty())).then_some(candidate)
    }

    /// 环回 + 归一化后的 `NO_PROXY` + 系统例外列表，去重保序。
    fn bypass_list(env_bypass: &str, os_exceptions: &[String]) -> String {
        let mut out: Vec<String> = LOOPBACK.iter().map(|entry| (*entry).to_owned()).collect();
        let extra =
            normalize_no_proxy(env_bypass) + "," + &normalize_no_proxy(&os_exceptions.join(","));
        for entry in extra.split(',') {
            if !entry.is_empty() && !out.iter().any(|known| known.eq_ignore_ascii_case(entry)) {
                out.push(entry.to_owned());
            }
        }
        out.join(",")
    }

    /// 把系统 / 用户写的绕过列表归一化为 `reqwest::NoProxy` 语法（`*.x` → `.x`、`a.b.*` 与缩写
    /// 网段 `169.254/16` → CIDR、`<local>` → 环回 + 私网段近似），`;` 统一为 `,`。幂等。
    ///
    /// 镜像：`native/engine/src/proxy_config.rs::normalize_no_proxy`（agent 不依赖引擎）。
    pub(super) fn normalize_no_proxy(list: &str) -> String {
        let mut out: Vec<String> = Vec::new();
        for raw in list.split([',', ';']) {
            let entry = raw.trim();
            if entry.is_empty() {
                continue;
            }
            if entry.eq_ignore_ascii_case("<local>") {
                out.extend(
                    [
                        "localhost",
                        "127.0.0.0/8",
                        "::1",
                        "10.0.0.0/8",
                        "172.16.0.0/12",
                        "192.168.0.0/16",
                    ]
                    .map(str::to_owned),
                );
                continue;
            }
            if let Some(rest) = entry.strip_prefix("*.") {
                out.push(format!(".{rest}"));
                continue;
            }
            if let Some(prefix) = entry.strip_suffix(".*")
                && let Some(cidr) = ipv4_prefix_cidr(prefix, None)
            {
                out.push(cidr);
                continue;
            }
            if let Some((prefix, bits)) = entry.split_once('/')
                && let Some(cidr) = ipv4_prefix_cidr(prefix, Some(bits))
            {
                out.push(cidr);
                continue;
            }
            out.push(entry.to_owned());
        }
        out.join(",")
    }

    /// 1–3 段 IPv4 前缀补足为四段 CIDR；`bits` 缺省按段数 × 8。
    fn ipv4_prefix_cidr(prefix: &str, bits: Option<&str>) -> Option<String> {
        let mut octets: Vec<&str> = prefix.split('.').collect();
        if !(1..=3).contains(&octets.len())
            || !octets.iter().all(|octet| octet.parse::<u8>().is_ok())
        {
            return None;
        }
        let bits = match bits {
            Some(bits) => bits.parse::<u8>().ok().filter(|bits| *bits <= 32)?,
            None => u8::try_from(octets.len() * 8).ok()?,
        };
        octets.resize(4, "0");
        Some(format!("{}/{bits}", octets.join(".")))
    }

    /// `scutil --proxy` 输出：顶层 `HTTP(S)Enable / Proxy / Port` 与 `ExceptionsList` 数组。
    #[cfg(any(test, target_os = "macos"))]
    pub(super) fn parse_scutil(text: &str) -> OsProxy {
        let mut scalars = std::collections::HashMap::new();
        let mut exceptions = Vec::new();
        let mut depth = 0usize;
        let mut in_exceptions = false;
        for line in text.lines().map(str::trim) {
            if line == "}" {
                depth = depth.saturating_sub(1);
                in_exceptions = in_exceptions && depth > 1;
                continue;
            }
            let pair = line
                .split_once(':')
                .map(|(key, value)| (key.trim(), value.trim()));
            if line.ends_with('{') {
                in_exceptions = depth == 1 && pair.is_some_and(|(key, _)| key == "ExceptionsList");
                depth += 1;
                continue;
            }
            match pair {
                Some((key, value)) if depth == 1 => {
                    scalars.insert(key, value);
                }
                Some((_, value)) if in_exceptions && depth == 2 && !value.is_empty() => {
                    exceptions.push(value.to_owned());
                }
                _ => {}
            }
        }
        let slot = |enable: &str, host: &str, port: &str| {
            if scalars.get(enable) != Some(&"1") {
                return None;
            }
            let host = scalars.get(host).filter(|host| !host.is_empty())?;
            Some(
                match scalars.get(port).and_then(|port| port.parse::<u16>().ok()) {
                    Some(port) => format!("{host}:{port}"),
                    None => (*host).to_owned(),
                },
            )
        };
        OsProxy {
            http: slot("HTTPEnable", "HTTPProxy", "HTTPPort"),
            https: slot("HTTPSEnable", "HTTPSProxy", "HTTPSPort"),
            exceptions,
        }
    }

    /// macOS 系统代理：读失败按「没有系统代理」处理（同 hyper-util 取不到 SCDynamicStore 时）。
    #[cfg(target_os = "macos")]
    pub(super) fn os_settings() -> OsProxy {
        match scutil_output() {
            Ok(text) => parse_scutil(&text),
            Err(error) => {
                tracing::warn!(%error, "read macOS proxy settings");
                OsProxy::default()
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    pub(super) fn os_settings() -> OsProxy {
        OsProxy::default()
    }

    /// 读本地配置的轻量命令，正常数十毫秒；超时兜住异常慢的系统，绝不拖住客户端构建。
    #[cfg(target_os = "macos")]
    const SCUTIL_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1500);

    #[cfg(target_os = "macos")]
    fn scutil_output() -> std::io::Result<String> {
        use std::io::Read;
        use std::process::{Command, Stdio};

        let mut child = Command::new("/usr/sbin/scutil")
            .arg("--proxy")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let deadline = std::time::Instant::now() + SCUTIL_TIMEOUT;
        loop {
            if let Some(status) = child.try_wait()? {
                // 输出远小于管道缓冲，退出后再读不会死锁。
                let mut text = String::new();
                if let Some(mut stdout) = child.stdout.take() {
                    stdout.read_to_string(&mut text)?;
                }
                return if status.success() {
                    Ok(text)
                } else {
                    Err(std::io::Error::other(format!(
                        "scutil --proxy exited with {status}"
                    )))
                };
            }
            if std::time::Instant::now() >= deadline {
                child.kill()?;
                child.wait()?;
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "scutil --proxy timed out",
                ));
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

#[cfg(all(test, not(windows)))]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::net::SocketAddr;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::imp::{OsProxy, Route, SystemProxy, parse_scutil};

    fn no_env(_: &str) -> Option<String> {
        None
    }

    fn scutil(proxy_port: u16, exceptions: &[&str]) -> String {
        let items: String = exceptions
            .iter()
            .enumerate()
            .map(|(index, entry)| format!("    {index} : {entry}\n"))
            .collect();
        format!(
            "<dictionary> {{\n  ExceptionsList : <array> {{\n{items}  }}\n  ExcludeSimpleHostnames : 1\n  HTTPEnable : 1\n  HTTPPort : {proxy_port}\n  HTTPProxy : 127.0.0.1\n  HTTPSEnable : 1\n  HTTPSPort : {proxy_port}\n  HTTPSProxy : 127.0.0.1\n  SOCKSEnable : 1\n  SOCKSPort : 1086\n  SOCKSProxy : 127.0.0.1\n}}\n"
        )
    }

    /// 应答固定正文的最小 HTTP 服务：正文标明请求落在源站还是代理。
    async fn serve(body: &'static str) -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut request = Vec::new();
                    let mut buffer = [0u8; 1024];
                    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                        match socket.read(&mut buffer).await {
                            Ok(0) | Err(_) => return,
                            Ok(read) => request.extend_from_slice(&buffer[..read]),
                        }
                    }
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    socket.write_all(response.as_bytes()).await.unwrap();
                });
            }
        });
        address
    }

    /// 系统设置的代理只放过例外列表与环回：例外域名、未列入的环回地址直连源站，其余（海外站点）
    /// 仍经代理。此前 macOS 例外列表被整段丢弃，连本机地址也被代理接走。
    #[tokio::test]
    async fn system_proxy_bypasses_only_exceptions_and_loopback() {
        let origin = serve("origin").await;
        let proxy = serve("proxy").await;
        let text = scutil(proxy.port(), &["*.corp.example", "169.254/16"]);
        let client = SystemProxy::resolve(&no_env, || parse_scutil(&text))
            .apply(reqwest::Client::builder())
            .resolve("nas.corp.example", origin)
            .resolve("cdn.global.example", origin)
            .build()
            .unwrap();
        let port = origin.port();
        for (url, expected) in [
            (format!("http://cdn.global.example:{port}/"), "proxy"),
            (format!("http://nas.corp.example:{port}/"), "origin"),
            (format!("http://127.0.0.1:{port}/"), "origin"),
        ] {
            let body = client.get(&url).send().await.unwrap().text().await.unwrap();
            assert_eq!(body, expected, "{url}");
        }
    }

    /// 环境变量逐槽优先（同 hyper-util）：环境给的槽只用 `NO_PROXY`，系统补上的槽才并入例外列表；
    /// 两槽都有环境值时不读系统设置；CGI 环境不用代理。
    #[test]
    fn environment_slots_take_precedence_over_system_settings() {
        let text = scutil(7897, &["*.local", "<local>"]);
        let env = |name: &str| match name {
            "HTTP_PROXY" => Some("socks5h://user:pw@10.0.0.2:1080".to_owned()),
            "no_proxy" => Some("*.corp.example".to_owned()),
            _ => None,
        };
        let resolved = SystemProxy::resolve(&env, || parse_scutil(&text));
        assert_eq!(
            resolved.http,
            Some(Route {
                url: "socks5h://user:pw@10.0.0.2:1080".to_owned(),
                no_proxy: "localhost,127.0.0.0/8,::1,.corp.example".to_owned(),
            })
        );
        assert_eq!(
            resolved.https,
            Some(Route {
                url: "http://127.0.0.1:7897".to_owned(),
                no_proxy: "localhost,127.0.0.0/8,::1,.corp.example,.local,10.0.0.0/8,172.16.0.0/12,192.168.0.0/16"
                    .to_owned(),
            })
        );

        let both = |name: &str| match name {
            "https_proxy" | "http_proxy" => Some("10.0.0.2:3128".to_owned()),
            _ => None,
        };
        let read_system = std::cell::Cell::new(false);
        let resolved = SystemProxy::resolve(&both, || {
            read_system.set(true);
            OsProxy::default()
        });
        assert!(!read_system.get(), "system settings must not be read");
        assert_eq!(
            resolved.https.map(|route| route.url).as_deref(),
            Some("http://10.0.0.2:3128")
        );

        let cgi = |name: &str| (name == "REQUEST_METHOD").then(|| "GET".to_owned());
        assert_eq!(
            SystemProxy::resolve(&cgi, || parse_scutil(&text)),
            SystemProxy::default()
        );
    }

    #[test]
    fn disabled_system_proxy_and_unusable_env_fall_back_to_direct_or_all_proxy() {
        let disabled =
            "<dictionary> {\n  HTTPEnable : 0\n  HTTPProxy : 127.0.0.1\n  HTTPPort : 7897\n}\n";
        assert_eq!(
            SystemProxy::resolve(&no_env, || parse_scutil(disabled)),
            SystemProxy::default()
        );
        let env = |name: &str| match name {
            "HTTP_PROXY" => Some("ftp://10.0.0.2:21".to_owned()),
            "ALL_PROXY" => Some("socks5://10.0.0.3:1080".to_owned()),
            _ => None,
        };
        let resolved = SystemProxy::resolve(&env, OsProxy::default);
        assert_eq!(
            resolved.http.map(|route| route.url).as_deref(),
            Some("socks5://10.0.0.3:1080")
        );
        assert_eq!(
            resolved.https.map(|route| route.url).as_deref(),
            Some("socks5://10.0.0.3:1080")
        );
    }
}
