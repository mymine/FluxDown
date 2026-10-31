//! PAC 脚本的 `FindProxyForURL` 执行器（QuickJS）。
//!
//! 每次调用新建 Runtime + Context（无状态、无跨线程问题）；内存上限 + 同一截止时间的
//! 中断处理器兜住死循环与 OOM。PAC 标准函数与微软扩展以 JS 实现注入（见 [`PRELUDE`]），
//! DNS / 本机地址 / 当前时间由 Rust 原生函数提供（DNS 走有界解析，总时限含脚本内 DNS）。
#![cfg(feature = "plugins")]

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rquickjs::{CatchResultExt, Context, Function, Runtime};

/// PAC 脚本内存上限。
const MEMORY_LIMIT: usize = 64 * 1024 * 1024;

/// 注入给 PAC 脚本的标准函数与微软扩展（Mozilla / Chromium 语义）。
const PRELUDE: &str = r#"
function __fdParseV4(s) {
  var m = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/.exec(s);
  if (!m) return null;
  var o = [];
  for (var i = 1; i <= 4; i++) {
    var n = parseInt(m[i], 10);
    if (n > 255) return null;
    o.push(n);
  }
  return o;
}
function __fdGroups(part) {
  if (part === '') return [];
  var out = [];
  var p = part.split(':');
  for (var i = 0; i < p.length; i++) {
    var g = p[i];
    if (i === p.length - 1 && g.indexOf('.') >= 0) {
      var v4 = __fdParseV4(g);
      if (!v4) return null;
      out.push(v4[0] * 256 + v4[1], v4[2] * 256 + v4[3]);
      continue;
    }
    if (!/^[0-9a-fA-F]{1,4}$/.test(g)) return null;
    out.push(parseInt(g, 16));
  }
  return out;
}
function __fdParseV6(s) {
  if (s.indexOf(':') < 0) return null;
  var z = s.indexOf('%');
  if (z >= 0) s = s.substring(0, z);
  var dbl = s.split('::');
  if (dbl.length > 2) return null;
  var head = __fdGroups(dbl[0]);
  if (head === null) return null;
  var w;
  if (dbl.length === 2) {
    var tail = __fdGroups(dbl[1]);
    if (tail === null) return null;
    var fill = 8 - head.length - tail.length;
    if (fill < 1) return null;
    w = head.concat(new Array(fill).fill(0), tail);
  } else {
    if (head.length !== 8) return null;
    w = head;
  }
  var b = [];
  for (var i = 0; i < 8; i++) b.push(w[i] >> 8, w[i] & 255);
  return b;
}
function __fdParseIP(s) {
  if (typeof s !== 'string') return null;
  return __fdParseV4(s) || __fdParseV6(s);
}
function __fdInCidr(ip, cidr) {
  var i = String(cidr).indexOf('/');
  if (i < 0) return false;
  var net = __fdParseIP(cidr.substring(0, i));
  var p = cidr.substring(i + 1);
  if (net === null || !/^\d{1,3}$/.test(p)) return false;
  var bits = parseInt(p, 10);
  if (net.length !== ip.length || bits > net.length * 8) return false;
  for (var k = 0; k < net.length && bits > 0; k++, bits -= 8) {
    var m = bits >= 8 ? 255 : ((0xff << (8 - bits)) & 255);
    if ((ip[k] & m) !== (net[k] & m)) return false;
  }
  return true;
}
function __fdArgs(args) {
  var a = Array.prototype.slice.call(args);
  var gmt = false;
  if (a.length && a[a.length - 1] === 'GMT') { gmt = true; a.pop(); }
  return { a: a, gmt: gmt };
}
function __fdNow() { return new Date(__fd_now()); }

function isPlainHostName(host) { return String(host).indexOf('.') < 0; }
function dnsDomainIs(host, domain) {
  host = String(host); domain = String(domain);
  return host.length >= domain.length &&
    host.substring(host.length - domain.length) === domain;
}
function localHostOrDomainIs(host, hostdom) {
  host = String(host); hostdom = String(hostdom);
  return host === hostdom ||
    (host.indexOf('.') < 0 && hostdom.substring(0, host.length + 1) === host + '.');
}
function dnsResolve(host) {
  var r = __fd_dns(String(host));
  return r == null ? null : r;
}
function isResolvable(host) { return dnsResolve(host) !== null; }
function myIpAddress() { return __fd_myip(); }
function dnsDomainLevels(host) { return String(host).split('.').length - 1; }
function shExpMatch(str, pat) {
  pat = String(pat);
  var re = '^';
  for (var i = 0; i < pat.length; i++) {
    var c = pat.charAt(i);
    if (c === '*') re += '[\\s\\S]*';
    else if (c === '?') re += '[\\s\\S]';
    else re += c.replace(/[^A-Za-z0-9_]/, '\\$&');
  }
  return new RegExp(re + '$').test(String(str));
}
function convert_addr(ip) {
  var o = __fdParseV4(String(ip));
  if (!o) return 0;
  return ((o[0] * 16777216) + (o[1] * 65536) + (o[2] * 256) + o[3]);
}
function isInNet(host, pattern, mask) {
  host = String(host);
  var ip = __fdParseV4(host) ? host : dnsResolve(host);
  if (ip === null || !__fdParseV4(ip)) return false;
  var p = __fdParseV4(String(pattern)), m = __fdParseV4(String(mask));
  if (!p || !m) return false;
  var a = __fdParseV4(ip);
  for (var i = 0; i < 4; i++) {
    if ((a[i] & m[i]) !== (p[i] & m[i])) return false;
  }
  return true;
}
function weekdayRange() {
  var r = __fdArgs(arguments), a = r.a;
  var names = ['SUN', 'MON', 'TUE', 'WED', 'THU', 'FRI', 'SAT'];
  if (a.length < 1 || a.length > 2) return false;
  var d1 = names.indexOf(String(a[0]).toUpperCase());
  var d2 = a.length === 2 ? names.indexOf(String(a[1]).toUpperCase()) : d1;
  if (d1 < 0 || d2 < 0) return false;
  var n = __fdNow();
  var cur = r.gmt ? n.getUTCDay() : n.getDay();
  return d1 <= d2 ? (cur >= d1 && cur <= d2) : (cur >= d1 || cur <= d2);
}
function dateRange() {
  var r = __fdArgs(arguments), a = r.a;
  var months = ['JAN', 'FEB', 'MAR', 'APR', 'MAY', 'JUN', 'JUL', 'AUG', 'SEP', 'OCT', 'NOV', 'DEC'];
  var k = [];
  for (var i = 0; i < a.length; i++) {
    var v = a[i], x = null;
    if (typeof v === 'string') {
      var mi = months.indexOf(v.toUpperCase());
      if (mi >= 0) x = { t: 'm', v: mi };
    } else {
      var num = Number(v);
      if (num >= 1 && num <= 31) x = { t: 'd', v: num };
      else if (num > 31) x = { t: 'y', v: num };
    }
    if (x === null) return false;
    k.push(x);
  }
  var n = __fdNow();
  var cd = r.gmt ? n.getUTCDate() : n.getDate();
  var cm = r.gmt ? n.getUTCMonth() : n.getMonth();
  var cy = r.gmt ? n.getUTCFullYear() : n.getFullYear();
  function within(c, s, e, wrap) {
    return s <= e ? (c >= s && c <= e) : (wrap && (c >= s || c <= e));
  }
  if (k.length === 1) {
    return k[0].t === 'd' ? cd === k[0].v : k[0].t === 'm' ? cm === k[0].v : cy === k[0].v;
  }
  if (k.length === 2) {
    if (k[0].t !== k[1].t) return false;
    var cur = k[0].t === 'd' ? cd : k[0].t === 'm' ? cm : cy;
    return within(cur, k[0].v, k[1].v, k[0].t !== 'y');
  }
  if (k.length === 4) {
    if (k[0].t === 'd' && k[1].t === 'm' && k[2].t === 'd' && k[3].t === 'm') {
      return within(cm * 32 + cd, k[1].v * 32 + k[0].v, k[3].v * 32 + k[2].v, true);
    }
    if (k[0].t === 'm' && k[1].t === 'y' && k[2].t === 'm' && k[3].t === 'y') {
      return within(cy * 12 + cm, k[1].v * 12 + k[0].v, k[3].v * 12 + k[2].v, false);
    }
    return false;
  }
  if (k.length === 6) {
    var ty = 'dmydmy';
    for (var j = 0; j < 6; j++) if (k[j].t !== ty.charAt(j)) return false;
    return within(cy * 512 + cm * 32 + cd,
      k[2].v * 512 + k[1].v * 32 + k[0].v,
      k[5].v * 512 + k[4].v * 32 + k[3].v, false);
  }
  return false;
}
function timeRange() {
  var r = __fdArgs(arguments), a = r.a.map(Number);
  for (var i = 0; i < a.length; i++) if (isNaN(a[i])) return false;
  var n = __fdNow();
  var h = r.gmt ? n.getUTCHours() : n.getHours();
  var m = r.gmt ? n.getUTCMinutes() : n.getMinutes();
  var s = r.gmt ? n.getUTCSeconds() : n.getSeconds();
  var cur = h * 3600 + m * 60 + s, s1, s2;
  switch (a.length) {
    case 1: s1 = a[0] * 3600; s2 = a[0] * 3600 + 3599; break;
    case 2: s1 = a[0] * 3600; s2 = a[1] * 3600 + 3599; break;
    case 4: s1 = a[0] * 3600 + a[1] * 60; s2 = a[2] * 3600 + a[3] * 60 + 59; break;
    case 6: s1 = a[0] * 3600 + a[1] * 60 + a[2]; s2 = a[3] * 3600 + a[4] * 60 + a[5]; break;
    default: return false;
  }
  return s1 <= s2 ? (cur >= s1 && cur <= s2) : (cur >= s1 || cur <= s2);
}
function alert(msg) { __fd_alert(String(msg)); }

function dnsResolveEx(host) { return __fd_dns_ex(String(host)); }
function isResolvableEx(host) { return dnsResolveEx(host) !== ''; }
function myIpAddressEx() { return __fd_myip(); }
function isInNetEx(host, cidr) {
  host = String(host);
  var list = __fdParseIP(host) ? [host] : dnsResolveEx(host).split(';');
  for (var i = 0; i < list.length; i++) {
    var b = __fdParseIP(list[i]);
    if (b && __fdInCidr(b, String(cidr))) return true;
  }
  return false;
}
function sortIpAddressList(list) {
  var items = String(list).split(';'), parsed = [];
  for (var i = 0; i < items.length; i++) {
    var s = items[i].trim();
    var b = __fdParseIP(s);
    if (!b) return '';
    parsed.push({ s: s, b: b });
  }
  parsed.sort(function (x, y) {
    if (x.b.length !== y.b.length) return x.b.length - y.b.length;
    for (var i = 0; i < x.b.length; i++) if (x.b[i] !== y.b[i]) return x.b[i] - y.b[i];
    return 0;
  });
  return parsed.map(function (p) { return p.s; }).join(';');
}
function getClientVersion() { return '1.0'; }
"#;

/// 入口调度：`FindProxyForURLEx` 优先，其次 `FindProxyForURL`；返回值必须是字符串。
const DISPATCH: &str = r#"
(function () {
  var f = typeof FindProxyForURLEx === 'function' ? FindProxyForURLEx
    : typeof FindProxyForURL === 'function' ? FindProxyForURL : null;
  if (f === null) throw new Error('PAC 脚本未定义 FindProxyForURL');
  var r = f(__fd_url, __fd_host);
  if (typeof r !== 'string') throw new Error('FindProxyForURL 返回了非字符串: ' + typeof r);
  return r;
})()
"#;

/// 域名解析器：名称 → 地址列表（在有界解析线程里调用）。
type Resolver = Arc<dyn Fn(&str) -> std::io::Result<Vec<IpAddr>> + Send + Sync>;

/// 外部依赖注入点（生产用系统时间与系统解析器；测试可固定）。
struct Env {
    /// 固定的当前 epoch 毫秒；`None` = 系统时间。
    now_ms: Option<f64>,
    resolver: Resolver,
}

impl Env {
    fn system() -> Self {
        Self {
            now_ms: None,
            resolver: Arc::new(|name| {
                (name, 0u16)
                    .to_socket_addrs()
                    .map(|it| it.map(|a| a.ip()).collect())
            }),
        }
    }
}

/// 执行 PAC 脚本的 FindProxyForURL（脚本定义了 FindProxyForURLEx 时优先调用它）。
/// 阻塞函数：调用方在 spawn_blocking 中调用。`budget` 为总时限（含脚本内 DNS）。
/// 返回脚本原样的返回字符串（如 "PROXY a:1; SOCKS5 b:2; DIRECT"），解析由调用方负责。
pub(crate) fn find_proxy_for_url(
    script: &str,
    url: &str,
    host: &str,
    budget: Duration,
) -> Result<String, String> {
    run(script, url, host, budget, Env::system())
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // 缓存只是读写 HashMap，持锁线程 panic 后数据仍一致，直接取回内部值。
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// 解析 `name`：返回全部地址（字面量原样）。失败/超时 → `None`。
fn resolve_all(name: &str, deadline: Instant, resolver: &Resolver) -> Option<Vec<IpAddr>> {
    let bare = name.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = bare.parse::<IpAddr>() {
        return Some(vec![ip]);
    }
    let r = Arc::clone(resolver);
    match crate::pac::resolve_bounded(name, 0, deadline, move |n, p| {
        r(&n).map(|ips| ips.into_iter().map(|ip| SocketAddr::new(ip, p)).collect())
    }) {
        Ok(addrs) => Some(addrs.into_iter().map(|a| a.ip()).collect()),
        Err(e) => {
            tracing::debug!("[pac_js] DNS 解析失败: {e}");
            None
        }
    }
}

/// 本机出口地址：UDP connect 公网地址（不发包）后读本地端点；失败回落 127.0.0.1。
fn local_ip() -> String {
    UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| {
            s.connect("8.8.8.8:53")?;
            s.local_addr()
        })
        .map_or_else(|_| "127.0.0.1".to_string(), |a| a.ip().to_string())
}

fn system_now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64() * 1000.0)
}

fn run(script: &str, url: &str, host: &str, budget: Duration, env: Env) -> Result<String, String> {
    let deadline = Instant::now() + budget;
    let rt = Runtime::new().map_err(|e| format!("创建 JS 运行时失败: {e}"))?;
    rt.set_memory_limit(MEMORY_LIMIT);
    rt.set_interrupt_handler(Some(Box::new(move || Instant::now() >= deadline)));
    let ctx = Context::full(&rt).map_err(|e| format!("创建 JS 上下文失败: {e}"))?;

    let result = ctx.with(|ctx| -> Result<String, String> {
        let globals = ctx.globals();
        let set_err = |name: &str, e: rquickjs::Error| format!("注入 {name} 失败: {e}");

        // 同一次调用内按名缓存解析结果（None = 解析失败，同样缓存，避免重复耗预算）。
        let cache: Arc<Mutex<HashMap<String, Option<Vec<IpAddr>>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let lookup = {
            let resolver = Arc::clone(&env.resolver);
            move |name: &str| -> Option<Vec<IpAddr>> {
                if let Some(hit) = lock(&cache).get(name) {
                    return hit.clone();
                }
                let got = resolve_all(name, deadline, &resolver);
                lock(&cache).insert(name.to_string(), got.clone());
                got
            }
        };
        let lookup = Arc::new(lookup);

        let l = Arc::clone(&lookup);
        let dns = Function::new(ctx.clone(), move |name: String| -> Option<String> {
            let addrs = l(&name)?;
            // 字面量（含 IPv6）原样返回；域名取第一个 IPv4。
            let bare = name.trim_start_matches('[').trim_end_matches(']');
            if bare.parse::<IpAddr>().is_ok() {
                return Some(bare.to_string());
            }
            addrs
                .into_iter()
                .find(IpAddr::is_ipv4)
                .map(|ip| ip.to_string())
        })
        .map_err(|e| set_err("__fd_dns", e))?;
        globals
            .set("__fd_dns", dns)
            .map_err(|e| set_err("__fd_dns", e))?;

        let l = Arc::clone(&lookup);
        let dns_ex = Function::new(ctx.clone(), move |name: String| -> String {
            l(&name)
                .map(|ips| {
                    ips.iter()
                        .map(IpAddr::to_string)
                        .collect::<Vec<_>>()
                        .join(";")
                })
                .unwrap_or_default()
        })
        .map_err(|e| set_err("__fd_dns_ex", e))?;
        globals
            .set("__fd_dns_ex", dns_ex)
            .map_err(|e| set_err("__fd_dns_ex", e))?;

        let myip = Function::new(ctx.clone(), local_ip).map_err(|e| set_err("__fd_myip", e))?;
        globals
            .set("__fd_myip", myip)
            .map_err(|e| set_err("__fd_myip", e))?;

        let fixed = env.now_ms;
        let now = Function::new(ctx.clone(), move || fixed.unwrap_or_else(system_now_ms))
            .map_err(|e| set_err("__fd_now", e))?;
        globals
            .set("__fd_now", now)
            .map_err(|e| set_err("__fd_now", e))?;

        let alert = Function::new(ctx.clone(), |msg: String| {
            tracing::debug!("[pac_js] alert: {msg}");
        })
        .map_err(|e| set_err("__fd_alert", e))?;
        globals
            .set("__fd_alert", alert)
            .map_err(|e| set_err("__fd_alert", e))?;

        globals
            .set("__fd_url", url)
            .map_err(|e| set_err("__fd_url", e))?;
        globals
            .set("__fd_host", host)
            .map_err(|e| set_err("__fd_host", e))?;

        ctx.eval::<(), _>(PRELUDE)
            .catch(&ctx)
            .map_err(|e| format!("PAC 标准函数初始化失败: {e}"))?;
        ctx.eval::<(), _>(script)
            .catch(&ctx)
            .map_err(|e| format!("PAC 脚本加载失败: {e}"))?;
        ctx.eval::<String, _>(DISPATCH)
            .catch(&ctx)
            .map_err(|e| format!("PAC 执行失败: {e}"))
    });

    match result {
        Err(_) if Instant::now() >= deadline => Err(format!("PAC 执行超时（{budget:?}）")),
        other => other,
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    const BUDGET: Duration = Duration::from_secs(5);

    fn env_with(
        now_ms: Option<f64>,
        table: &'static [(&'static str, &'static [&'static str])],
    ) -> Env {
        Env {
            now_ms,
            resolver: Arc::new(move |name| {
                table
                    .iter()
                    .find(|(n, _)| *n == name)
                    .map(|(_, ips)| ips.iter().map(|s| s.parse().expect("ip")).collect())
                    .ok_or_else(|| std::io::Error::other("NXDOMAIN"))
            }),
        }
    }

    fn call(script: &str, url: &str, host: &str) -> Result<String, String> {
        run(script, url, host, BUDGET, env_with(None, &[]))
    }

    /// 以表达式为返回值的单函数 PAC，便于直接断言各标准函数。
    fn eval_expr(expr: &str, now_ms: Option<f64>) -> String {
        let script = format!("function FindProxyForURL(url, host) {{ return String({expr}); }}");
        run(
            &script,
            "http://x/",
            "x",
            BUDGET,
            env_with(
                now_ms,
                &[
                    ("a.test", &["10.1.2.3", "2001:db8::1"]),
                    ("v6.test", &["2001:db8::2"]),
                ],
            ),
        )
        .expect("script ok")
    }

    /// 用 JS 计算「本地时区」下指定时刻的 epoch 毫秒，避免测试依赖机器时区。
    fn epoch(local: bool, y: i32, mo: i32, d: i32, h: i32, mi: i32, s: i32) -> f64 {
        let rt = Runtime::new().expect("rt");
        let ctx = Context::full(&rt).expect("ctx");
        let ctor = if local { "new Date" } else { "Date.UTC" };
        let src = if local {
            format!("{ctor}({y},{},{d},{h},{mi},{s}).getTime()", mo - 1)
        } else {
            format!("{ctor}({y},{},{d},{h},{mi},{s})", mo - 1)
        };
        ctx.with(|c| c.eval::<f64, _>(src).expect("eval"))
    }

    #[test]
    fn clash_style_single_return() {
        let s = r#"function FindProxyForURL(url, host) { return "PROXY 127.0.0.1:7890; DIRECT"; }"#;
        assert_eq!(
            call(s, "http://a.com/", "a.com").unwrap(),
            "PROXY 127.0.0.1:7890; DIRECT"
        );
    }

    #[test]
    fn gfwlist_style_rules_route_per_host() {
        let s = r#"
            var proxy = "SOCKS5 127.0.0.1:1080; DIRECT";
            var domains = ["*.google.com", "twitter.com"];
            function FindProxyForURL(url, host) {
              if (isPlainHostName(host)) return "DIRECT";
              if (dnsDomainIs(host, ".github.com")) return proxy;
              for (var i = 0; i < domains.length; i++)
                if (shExpMatch(host, domains[i])) return proxy;
              return "DIRECT";
            }"#;
        let p = "SOCKS5 127.0.0.1:1080; DIRECT";
        assert_eq!(call(s, "u", "mail.google.com").unwrap(), p);
        assert_eq!(call(s, "u", "twitter.com").unwrap(), p);
        assert_eq!(call(s, "u", "gist.github.com").unwrap(), p);
        assert_eq!(call(s, "u", "github.com").unwrap(), "DIRECT");
        assert_eq!(call(s, "u", "intranet").unwrap(), "DIRECT");
        assert_eq!(call(s, "u", "example.org").unwrap(), "DIRECT");
    }

    #[test]
    fn sh_exp_match_escapes_regex_metachars() {
        assert_eq!(
            eval_expr(r#"shExpMatch("aXb.com", "a.b.com")"#, None),
            "false"
        );
        assert_eq!(
            eval_expr(r#"shExpMatch("a.b.com", "a.b.com")"#, None),
            "true"
        );
        assert_eq!(eval_expr(r#"shExpMatch("a+b", "a+b")"#, None), "true");
        assert_eq!(eval_expr(r#"shExpMatch("aab", "a+b")"#, None), "false");
        assert_eq!(
            eval_expr(r#"shExpMatch("x.com/p?q=1", "*.com/p?q=?")"#, None),
            "true"
        );
        assert_eq!(eval_expr(r#"shExpMatch("abc", "a?")"#, None), "false");
        assert_eq!(eval_expr(r#"shExpMatch("(a)", "(a)")"#, None), "true");
        // 通配符须整串匹配。
        assert_eq!(eval_expr(r#"shExpMatch("xab", "ab*")"#, None), "false");
    }

    #[test]
    fn dns_domain_and_misc_helpers() {
        assert_eq!(
            eval_expr(r#"dnsDomainIs("www.a.com", ".a.com")"#, None),
            "true"
        );
        assert_eq!(
            eval_expr(r#"dnsDomainIs("a.com", ".a.com")"#, None),
            "false"
        );
        assert_eq!(
            eval_expr(r#"localHostOrDomainIs("www", "www.a.com")"#, None),
            "true"
        );
        assert_eq!(
            eval_expr(r#"localHostOrDomainIs("www.b.com", "www.a.com")"#, None),
            "false"
        );
        assert_eq!(eval_expr(r#"dnsDomainLevels("a.b.c")"#, None), "2");
        assert_eq!(eval_expr(r#"convert_addr("10.1.2.3")"#, None), "167838211");
        assert_eq!(
            eval_expr(r#"convert_addr("255.255.255.255")"#, None),
            "4294967295"
        );
        assert_eq!(eval_expr("getClientVersion()", None), "1.0");
    }

    #[test]
    fn is_in_net_literal_and_mask_boundary() {
        // 字面量不触发 DNS（解析表为空也能判定）。
        assert_eq!(
            eval_expr(r#"isInNet("10.1.2.3", "10.0.0.0", "255.0.0.0")"#, None),
            "true"
        );
        assert_eq!(
            eval_expr(r#"isInNet("11.0.0.1", "10.0.0.0", "255.0.0.0")"#, None),
            "false"
        );
        assert_eq!(
            eval_expr(
                r#"isInNet("192.168.1.255", "192.168.1.0", "255.255.255.0")"#,
                None
            ),
            "true"
        );
        assert_eq!(
            eval_expr(
                r#"isInNet("192.168.2.0", "192.168.1.0", "255.255.255.0")"#,
                None
            ),
            "false"
        );
        assert_eq!(
            eval_expr(
                r#"isInNet("172.31.255.255", "172.16.0.0", "255.240.0.0")"#,
                None
            ),
            "true"
        );
        assert_eq!(
            eval_expr(
                r#"isInNet("172.32.0.0", "172.16.0.0", "255.240.0.0")"#,
                None
            ),
            "false"
        );
        // 域名走解析（a.test → 10.1.2.3 取第一个 IPv4）。
        assert_eq!(
            eval_expr(r#"isInNet("a.test", "10.0.0.0", "255.0.0.0")"#, None),
            "true"
        );
        assert_eq!(
            eval_expr(r#"isInNet("nx.test", "10.0.0.0", "255.0.0.0")"#, None),
            "false"
        );
    }

    #[test]
    fn is_in_net_ex_cidr_v4_and_v6() {
        assert_eq!(
            eval_expr(r#"isInNetEx("10.1.2.3", "10.0.0.0/8")"#, None),
            "true"
        );
        assert_eq!(
            eval_expr(r#"isInNetEx("10.1.2.3", "10.1.2.4/30")"#, None),
            "false"
        );
        assert_eq!(
            eval_expr(r#"isInNetEx("10.1.2.7", "10.1.2.4/30")"#, None),
            "true"
        );
        assert_eq!(
            eval_expr(r#"isInNetEx("2001:db8::2", "2001:db8::/32")"#, None),
            "true"
        );
        assert_eq!(
            eval_expr(r#"isInNetEx("2001:db9::2", "2001:db8::/32")"#, None),
            "false"
        );
        assert_eq!(
            eval_expr(r#"isInNetEx("a.test", "2001:db8::/32")"#, None),
            "true"
        );
        assert_eq!(
            eval_expr(r#"isInNetEx("10.1.2.3", "2001:db8::/32")"#, None),
            "false"
        );
    }

    #[test]
    fn dns_resolve_semantics() {
        assert_eq!(eval_expr(r#"dnsResolve("a.test")"#, None), "10.1.2.3");
        assert_eq!(eval_expr(r#"dnsResolve("1.2.3.4")"#, None), "1.2.3.4");
        assert_eq!(eval_expr(r#"dnsResolve("nx.test") === null"#, None), "true");
        assert_eq!(eval_expr(r#"isResolvable("nx.test")"#, None), "false");
        assert_eq!(eval_expr(r#"isResolvable("a.test")"#, None), "true");
        // 仅有 IPv6 记录：dnsResolve 无 IPv4 → null，Ex 返回地址。
        assert_eq!(eval_expr(r#"dnsResolve("v6.test") === null"#, None), "true");
        assert_eq!(
            eval_expr(r#"dnsResolveEx("a.test")"#, None),
            "10.1.2.3;2001:db8::1"
        );
        assert_eq!(eval_expr(r#"dnsResolveEx("nx.test")"#, None), "");
        assert_eq!(eval_expr(r#"isResolvableEx("v6.test")"#, None), "true");
        assert_eq!(eval_expr(r#"isResolvableEx("nx.test")"#, None), "false");
    }

    #[test]
    fn dns_results_are_cached_within_one_call() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let count = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&count);
        let env = Env {
            now_ms: None,
            resolver: Arc::new(move |_| {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(vec!["1.1.1.1".parse().expect("ip")])
            }),
        };
        let s = "function FindProxyForURL(u, h) { dnsResolve('x.test'); dnsResolve('x.test'); return dnsResolve('x.test'); }";
        assert_eq!(run(s, "u", "h", BUDGET, env).unwrap(), "1.1.1.1");
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn sort_ip_address_list_orders_numerically() {
        assert_eq!(
            eval_expr(
                r#"sortIpAddressList("10.0.0.10;9.255.0.1;10.0.0.9;2001:db8::1")"#,
                None
            ),
            "9.255.0.1;10.0.0.9;10.0.0.10;2001:db8::1"
        );
        assert_eq!(eval_expr(r#"sortIpAddressList("not-an-ip")"#, None), "");
    }

    #[test]
    fn weekday_range_plain_wrap_and_gmt() {
        // 2026-10-24 是周六（UTC 12:00）。
        let sat = Some(epoch(false, 2026, 10, 24, 12, 0, 0));
        assert_eq!(eval_expr(r#"weekdayRange("SAT", "GMT")"#, sat), "true");
        assert_eq!(
            eval_expr(r#"weekdayRange("MON", "FRI", "GMT")"#, sat),
            "false"
        );
        assert_eq!(
            eval_expr(r#"weekdayRange("FRI", "MON", "GMT")"#, sat),
            "true"
        ); // 回绕
        assert_eq!(eval_expr(r#"weekdayRange("SUN", "GMT")"#, sat), "false");
        // GMT 与本地可跨日：UTC 周六 23:30 在 UTC+1 以东已是周日——只断言 GMT 路径不受时区影响。
        let late = Some(epoch(false, 2026, 10, 24, 23, 30, 0));
        assert_eq!(eval_expr(r#"weekdayRange("SAT", "GMT")"#, late), "true");
        // 本地时区：同一本地时刻按本地断言。
        let local_sat = Some(epoch(true, 2026, 10, 24, 12, 0, 0));
        assert_eq!(eval_expr(r#"weekdayRange("SAT")"#, local_sat), "true");
        assert_eq!(
            eval_expr(r#"weekdayRange("SUN", "FRI")"#, local_sat),
            "false"
        );
    }

    #[test]
    fn date_range_forms_wrap_and_gmt() {
        let t = Some(epoch(false, 2026, 10, 24, 12, 0, 0));
        assert_eq!(eval_expr(r#"dateRange(24, "GMT")"#, t), "true");
        assert_eq!(eval_expr(r#"dateRange(25, "GMT")"#, t), "false");
        assert_eq!(eval_expr(r#"dateRange("OCT", "GMT")"#, t), "true");
        assert_eq!(eval_expr(r#"dateRange(2026, "GMT")"#, t), "true");
        assert_eq!(eval_expr(r#"dateRange(1, 15, "GMT")"#, t), "false");
        assert_eq!(eval_expr(r#"dateRange(20, 28, "GMT")"#, t), "true");
        assert_eq!(eval_expr(r#"dateRange(22, 10, "GMT")"#, t), "true"); // 日回绕（22..下月10）
        assert_eq!(eval_expr(r#"dateRange(25, 20, "GMT")"#, t), "false");
        assert_eq!(eval_expr(r#"dateRange("NOV", "FEB", "GMT")"#, t), "false");
        assert_eq!(eval_expr(r#"dateRange("SEP", "FEB", "GMT")"#, t), "true"); // 月回绕
        assert_eq!(eval_expr(r#"dateRange(2025, 2027, "GMT")"#, t), "true");
        assert_eq!(eval_expr(r#"dateRange(2020, 2025, "GMT")"#, t), "false");
        assert_eq!(
            eval_expr(r#"dateRange(24, "OCT", 31, "OCT", "GMT")"#, t),
            "true"
        );
        assert_eq!(
            eval_expr(r#"dateRange(25, "OCT", 31, "DEC", "GMT")"#, t),
            "false"
        );
        assert_eq!(
            eval_expr(r#"dateRange(1, "DEC", 31, "JAN", "GMT")"#, t),
            "false"
        );
        assert_eq!(
            eval_expr(r#"dateRange(1, "OCT", 31, "JAN", "GMT")"#, t),
            "true"
        );
        assert_eq!(
            eval_expr(r#"dateRange("SEP", 2026, "NOV", 2026, "GMT")"#, t),
            "true"
        );
        assert_eq!(
            eval_expr(r#"dateRange("NOV", 2026, "DEC", 2026, "GMT")"#, t),
            "false"
        );
        assert_eq!(
            eval_expr(r#"dateRange(24, "OCT", 2026, 24, "OCT", 2026, "GMT")"#, t),
            "true"
        );
        assert_eq!(
            eval_expr(r#"dateRange(25, "OCT", 2026, 1, "JAN", 2027, "GMT")"#, t),
            "false"
        );
        // 本地时区。
        let lt = Some(epoch(true, 2026, 10, 24, 12, 0, 0));
        assert_eq!(eval_expr(r#"dateRange(24)"#, lt), "true");
        assert_eq!(eval_expr(r#"dateRange("OCT", "OCT")"#, lt), "true");
    }

    #[test]
    fn time_range_forms_wrap_and_gmt() {
        let t = Some(epoch(false, 2026, 10, 24, 9, 30, 15));
        assert_eq!(eval_expr(r#"timeRange(9, "GMT")"#, t), "true");
        assert_eq!(eval_expr(r#"timeRange(10, "GMT")"#, t), "false");
        assert_eq!(eval_expr(r#"timeRange(9, 17, "GMT")"#, t), "true");
        assert_eq!(eval_expr(r#"timeRange(10, 17, "GMT")"#, t), "false");
        assert_eq!(eval_expr(r#"timeRange(0, 9, "GMT")"#, t), "true"); // 末小时含到 9:59:59
        assert_eq!(eval_expr(r#"timeRange(9, 0, 9, 29, "GMT")"#, t), "false");
        assert_eq!(eval_expr(r#"timeRange(9, 0, 9, 30, "GMT")"#, t), "true");
        assert_eq!(eval_expr(r#"timeRange(9, 31, 10, 0, "GMT")"#, t), "false");
        assert_eq!(
            eval_expr(r#"timeRange(9, 30, 15, 9, 30, 15, "GMT")"#, t),
            "true"
        );
        assert_eq!(
            eval_expr(r#"timeRange(9, 30, 16, 9, 31, 0, "GMT")"#, t),
            "false"
        );
        // 跨午夜回绕：22:00–06:59 含 09:30？否；含 03:00？是。
        assert_eq!(eval_expr(r#"timeRange(22, 6, "GMT")"#, t), "false");
        let night = Some(epoch(false, 2026, 10, 24, 3, 0, 0));
        assert_eq!(eval_expr(r#"timeRange(22, 6, "GMT")"#, night), "true");
        let late = Some(epoch(false, 2026, 10, 24, 23, 0, 0));
        assert_eq!(eval_expr(r#"timeRange(22, 0, 6, 0, "GMT")"#, late), "true");
        // 本地时区。
        let lt = Some(epoch(true, 2026, 10, 24, 9, 30, 15));
        assert_eq!(eval_expr(r#"timeRange(9, 10)"#, lt), "true");
        assert_eq!(eval_expr(r#"timeRange(11, 12)"#, lt), "false");
    }

    #[test]
    fn find_proxy_for_url_ex_takes_precedence() {
        let s = r#"
            function FindProxyForURL(u, h) { return "DIRECT"; }
            function FindProxyForURLEx(u, h) { return "PROXY ex:1"; }"#;
        assert_eq!(call(s, "u", "h").unwrap(), "PROXY ex:1");
    }

    #[test]
    fn url_and_host_are_passed_through() {
        let s = "function FindProxyForURL(u, h) { return u + '|' + h; }";
        assert_eq!(
            call(s, "http://a.com/x?y=\"1\"", "a.com").unwrap(),
            "http://a.com/x?y=\"1\"|a.com"
        );
    }

    #[test]
    fn infinite_loop_is_interrupted_within_budget() {
        let s = "function FindProxyForURL(u, h) { while (true) {} }";
        let started = Instant::now();
        let err = run(s, "u", "h", Duration::from_millis(300), env_with(None, &[])).unwrap_err();
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        assert!(err.contains("超时"), "{err}");
    }

    #[test]
    fn top_level_infinite_loop_is_interrupted() {
        let started = Instant::now();
        let err = run(
            "for (;;) {}",
            "u",
            "h",
            Duration::from_millis(300),
            env_with(None, &[]),
        )
        .unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(err.contains("超时"), "{err}");
    }

    #[test]
    fn memory_bomb_returns_err() {
        let s = "function FindProxyForURL(u, h) { var a = 'x'; for (;;) { a = a + a; } }";
        assert!(run(s, "u", "h", Duration::from_secs(20), env_with(None, &[])).is_err());
    }

    #[test]
    fn thrown_exception_is_err() {
        let s = "function FindProxyForURL(u, h) { throw new Error('boom'); }";
        let err = call(s, "u", "h").unwrap_err();
        assert!(err.contains("boom"), "{err}");
    }

    #[test]
    fn syntax_error_and_missing_function_are_err() {
        assert!(call("function (", "u", "h").is_err());
        let err = call("var x = 1;", "u", "h").unwrap_err();
        assert!(err.contains("FindProxyForURL"), "{err}");
    }

    #[test]
    fn non_string_return_is_err() {
        for body in [
            "return 1;",
            "return null;",
            "return undefined;",
            "return true;",
            "return ['DIRECT'];",
        ] {
            let s = format!("function FindProxyForURL(u, h) {{ {body} }}");
            assert!(call(&s, "u", "h").is_err(), "{body}");
        }
    }

    #[test]
    fn alert_does_not_interrupt_execution() {
        let s = "function FindProxyForURL(u, h) { alert('hi'); return 'DIRECT'; }";
        assert_eq!(call(s, "u", "h").unwrap(), "DIRECT");
    }

    #[test]
    fn my_ip_address_returns_ipv4_literal() {
        let ip = eval_expr("myIpAddress()", None);
        assert!(ip.parse::<std::net::Ipv4Addr>().is_ok(), "{ip}");
    }
}
