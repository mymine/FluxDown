//! 插件 yt-dlp 桥集成测试：EngineBridge.run_ytdlp 端到端。
//!
//! 覆盖：
//! - 危险开关（--exec 等）/ 越牢路径参数在 spawn 前被拒（确定性，无需 yt-dlp 二进制）。
//! - URL 参数放行（yt-dlp 本职，区别于 ffmpeg 封网）——经真实二进制间接验证。
//! - 真实 yt-dlp 可用时：`--version` 退出码 0、stdout 非空（牢笼由 bridge 自持）。
//!
//! 仅 `plugins` feature 下编译运行。真实执行经 `FLUXDOWN_TEST_YTDLP=<绝对路径>` 注入。
#![cfg(feature = "plugins")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use fluxdown_engine::db::Db;
use fluxdown_engine::plugin::PluginBridge;
use fluxdown_engine::plugin::bridge::EngineBridge;
use fluxdown_engine::plugin::runtime::YtdlpSpec;
use fluxdown_engine::proxy_config::ProxyConfig;

fn unique_dir(tag: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let mut d = std::env::temp_dir();
    d.push(format!("fluxdown_yt_{}_{}_{}", tag, std::process::id(), n));
    std::fs::create_dir_all(&d).expect("mkdir temp");
    d
}

async fn make_bridge(data_dir: &Path) -> EngineBridge {
    let db = Db::open(data_dir).await.expect("open db");
    // 测试用真实 yt-dlp：`FLUXDOWN_TEST_YTDLP=<绝对路径>` 时经 config 手动指定，
    // 使 resolve_ytdlp 命中（CI/本机无系统 yt-dlp 时的确定性执行入口）。
    if let Ok(p) = std::env::var("FLUXDOWN_TEST_YTDLP") {
        db.set_config(fluxdown_engine::components::CONFIG_YTDLP_PATH, &p)
            .await
            .expect("seed yt-dlp path");
    }
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    EngineBridge::new(db, &ProxyConfig::default(), tx, data_dir.to_path_buf()).expect("bridge")
}

fn spec(args: &[&str]) -> YtdlpSpec {
    YtdlpSpec {
        args: args.iter().map(|s| s.to_string()).collect(),
        subdir: None,
        timeout_ms: Some(30_000),
        wall_pause: None,
        egress: None,
    }
}

/// 危险开关 / 越牢路径参数在 spawn 前被拒——无论 yt-dlp 是否安装（校验先于二进制解析）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejects_dangerous_and_escape_args() {
    let data_dir = unique_dir("data_reject");
    let bridge = make_bridge(&data_dir).await;

    for args in [
        // 执行外部程序 / 加载任意配置或插件 / 读浏览器凭据的开关。
        vec!["--exec", "rm -rf x", "https://x/y"],
        vec!["--downloader", "aria2c", "https://x/y"],
        vec!["--config-location", "cfg", "https://x/y"],
        vec!["--plugin-dirs", "plugins", "https://x/y"],
        vec!["--ffmpeg-location", "ff", "https://x/y"],
        vec!["-a", "urls.txt"],
        vec!["--cookies-from-browser", "chrome", "https://x/y"],
        // 越牢文件路径。
        vec!["-o", "/etc/passwd", "https://x/y"],
        vec!["-o", "../escape.%(ext)s", "https://x/y"],
        vec!["--paths", "home:/abs", "https://x/y"],
        vec!["-o", "C:\\Windows\\x", "https://x/y"],
    ] {
        let r = bridge.run_ytdlp("test@yt", spec(&args)).await;
        assert!(r.is_err(), "args {args:?} must be rejected before spawn");
    }

    // 空参数也拒。
    assert!(bridge.run_ytdlp("test@yt", spec(&[])).await.is_err());
}

/// 真实 yt-dlp 可用时：`--version` 退出码 0、stdout 非空（离线可跑）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runs_real_ytdlp_version() {
    let data_dir = unique_dir("data_run");
    let bridge = make_bridge(&data_dir).await;

    // 探测可用性；无 yt-dlp 环境（多数 CI）直接跳过——执行部分依赖真实二进制。
    let avail = bridge.ytdlp_available().await;
    let Some(a) = avail else { return };
    if !a.available {
        eprintln!("[skip] yt-dlp 不可用，跳过真实执行断言");
        return;
    }

    let out = bridge
        .run_ytdlp("test@yt", spec(&["--version"]))
        .await
        .expect("run_ytdlp");

    assert!(!out.timed_out, "should not time out");
    assert_eq!(out.code, 0, "yt-dlp exit non-zero; stderr: {}", out.stderr);
    assert!(
        !out.stdout.trim().is_empty(),
        "version stdout should be non-empty"
    );
}
/// flux.fs：插件工作区（= yt-dlp cwd 同根）通用读写，取代旧 cookies_text。
/// 确定性、无需 yt-dlp 二进制：写 cookie 文件 → 落在工作区 → 读回 → 列出 → 删除。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fs_roundtrip_in_workspace() {
    let data_dir = unique_dir("data_fs");
    let bridge = make_bridge(&data_dir).await;

    let body = "# Netscape HTTP Cookie File\n.youtube.com\tTRUE\t/\tTRUE\t0\tTEST\tval\n";
    bridge
        .fs_write("cookie@yt", "cookies.txt", body.to_string())
        .await
        .expect("fs_write");

    // 工作区 = <data_dir>/plugins-work/cookie_40yt（身份按字节无碰撞编码，与 run_ytdlp 的 cwd 同根）。
    let path = data_dir
        .join("plugins-work")
        .join("cookie_40yt")
        .join("cookies.txt");
    assert!(path.is_file(), "file must land in workspace: {path:?}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), body);

    assert_eq!(
        bridge.fs_read("cookie@yt", "cookies.txt").await.as_deref(),
        Some(body)
    );
    assert!(
        bridge
            .fs_list("cookie@yt")
            .await
            .contains(&"cookies.txt".to_string())
    );

    bridge
        .fs_remove("cookie@yt", "cookies.txt")
        .await
        .expect("fs_remove");
    assert!(!path.exists(), "file must be removed");
    assert_eq!(bridge.fs_read("cookie@yt", "cookies.txt").await, None);
    // 删除不存在的文件视为成功（幂等）。
    bridge
        .fs_remove("cookie@yt", "cookies.txt")
        .await
        .expect("idempotent remove");
}

/// flux.fs 越牢 / 非法文件名一律拒绝（写与删均校验，读返回 None）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fs_rejects_unsafe_names() {
    let data_dir = unique_dir("data_fs_reject");
    let bridge = make_bridge(&data_dir).await;

    for name in ["", ".", "..", "../escape", "a/b", "a\\b", "C:evil", "x\0y"] {
        assert!(
            bridge
                .fs_write("bad@yt", name, "x".to_string())
                .await
                .is_err(),
            "write name {name:?} must be rejected"
        );
        assert!(
            bridge.fs_remove("bad@yt", name).await.is_err(),
            "remove name {name:?} must be rejected"
        );
        assert_eq!(bridge.fs_read("bad@yt", name).await, None);
    }
}

/// 端到端安装冒烟（需网络，默认忽略）：
/// `cargo test -p fluxdown_engine --features plugins,components --test engine plugin_ytdlp -- --ignored ytdlp_install_smoke`
/// 从 GitHub 下载当前平台官方 yt-dlp 二进制 → 校验托管状态 → 经 bridge 跑 `--version`。
#[cfg(feature = "components")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn ytdlp_install_smoke() {
    let data_dir = unique_dir("data_install_smoke");
    let db = Db::open(&data_dir).await.expect("open db");
    let egress = fluxdown_engine::egress::Egress::new(db.clone());
    let progress = |d: u64, t: u64| eprintln!("[install] {d}/{t}");

    let status =
        fluxdown_engine::components::install_ytdlp(&db, &data_dir, &egress, None, &progress)
            .await
            .expect("install yt-dlp");
    assert_eq!(status.source.as_str(), "managed");
    assert!(
        !status.managed_version.is_empty(),
        "managed version recorded"
    );
    assert!(!status.version.is_empty(), "probed --version non-empty");
    eprintln!("[install] installed yt-dlp {}", status.version);

    // resolve_ytdlp 命中托管二进制；bridge 经它跑 `--version`。
    assert!(
        fluxdown_engine::components::resolve_ytdlp(&db, &data_dir)
            .await
            .is_some(),
        "resolve_ytdlp must find managed binary"
    );
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let bridge =
        EngineBridge::new(db, &ProxyConfig::default(), tx, data_dir.clone()).expect("bridge");
    let out = bridge
        .run_ytdlp("test@yt", spec(&["--version"]))
        .await
        .expect("run_ytdlp --version");
    assert_eq!(
        out.code, 0,
        "yt-dlp --version exit non-zero: {}",
        out.stderr
    );
    assert!(!out.stdout.trim().is_empty());
}

/// 快速网络冒烟（需网络，默认忽略）：拉取 yt-dlp Release 列表并解析版本 tag。
/// `cargo test -p fluxdown_engine --features plugins,components --test engine plugin_ytdlp -- --ignored ytdlp_list_versions_smoke`
#[cfg(feature = "components")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn ytdlp_list_versions_smoke() {
    let data_dir = unique_dir("data_list_versions_smoke");
    let db = Db::open(&data_dir).await.expect("open db");
    let egress = fluxdown_engine::egress::Egress::new(db.clone());
    let v = fluxdown_engine::components::list_ytdlp_versions(&db, &egress)
        .await
        .expect("list yt-dlp versions");
    assert!(!v.versions.is_empty(), "at least one version tag");
    assert_eq!(v.latest_stable, v.versions[0], "latest = first (newest)");
    assert_eq!(
        v.latest_stable.split('.').count(),
        3,
        "unexpected tag shape: {}",
        v.latest_stable
    );
    eprintln!(
        "[list] latest={} count={}",
        v.latest_stable,
        v.versions.len()
    );
}

/// stdout 上限边界：用一个可控的假 yt-dlp 夹具输出任意大小的合法 JSON，验证
/// `truncated_stdout` 语义（确定性、离线、无需真实 yt-dlp）。
///
/// 覆盖审查建议：>4 MiB 且 <16 MiB → `truncated_stdout == false` 且 JSON 可解析；
/// >16 MiB → `truncated_stdout == true`。用脚本而非真实 yt-dlp，是因为本桥不提供
/// 「任意大 stdout」的真实开关（`MAX_YTDLP_ARG_LEN` = 8 KiB 限制了参数尺寸）。
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stdout_cap_boundary() {
    use std::io::Write;

    let data_dir = unique_dir("data_cap");
    // 假 yt-dlp：忽略参数，向 stdout 写出 SIZE 指定字节数的合法 JSON 对象。
    // 参数校验在 spawn 前完成，因此仍需传真 yt-dlp 也能接受的参数（-J + URL）。
    let script = data_dir.join("fake-ytdlp.sh");
    {
        let mut f = std::fs::File::create(&script).expect("create fake yt-dlp");
        f.write_all(
            b"#!/bin/sh\n\
              # fake yt-dlp: scan args for the download URL and size stdout off it.\n\
              for arg in \"$@\"; do case \"$arg\" in --version) echo 9999.99.99; exit 0;; esac; done\n\
              size=64\n\
              for arg in \"$@\"; do\n\
                case \"$arg\" in\n\
                */big) size=$((5*1024*1024));;\n\
                */huge) size=$((17*1024*1024));;\n\
                esac\n\
              done\n\
              printf '{\"pad\":\"'; \n\
              head -c $size /dev/zero | tr '\\0' 'a'; \n\
              printf '\"}\n'; \n",
        )
        .expect("write script");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("chmod fake yt-dlp");
    }

    // 用假二进制作为「手动路径」注入，使 resolve_ytdlp 命中它。
    let db = Db::open(&data_dir).await.expect("open db");
    db.set_config(
        fluxdown_engine::components::CONFIG_YTDLP_PATH,
        script.to_str().expect("script path utf8"),
    )
    .await
    .expect("seed fake yt-dlp path");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let bridge =
        EngineBridge::new(db, &ProxyConfig::default(), tx, data_dir.clone()).expect("bridge");

    // >4 MiB 且 <16 MiB：不截断。用 5 MiB 落在旧上限（4 MiB）与新上限（16 MiB）之间。
    let out = bridge
        .run_ytdlp("test@yt", spec(&["-J", "https://example.com/big"]))
        .await
        .expect("run_ytdlp 5MiB");
    assert_eq!(out.code, 0, "fake yt-dlp exit; stderr: {}", out.stderr);
    assert!(
        !out.truncated_stdout,
        "5 MiB stdout must NOT be truncated under the 16 MiB cap"
    );
    assert!(
        serde_json::from_str::<serde_json::Value>(out.stdout.trim()).is_ok(),
        "5 MiB stdout must be valid JSON (proves no truncation)"
    );

    // >16 MiB：截断。
    let out2 = bridge
        .run_ytdlp("test@yt", spec(&["-J", "https://example.com/huge"]))
        .await
        .expect("run_ytdlp 17MiB");
    assert!(
        out2.truncated_stdout,
        "17 MiB stdout must be flagged truncated"
    );
}

/// 动态上限：单插件 config `plugin.<id>.ytdlp.stdout_cap` 覆盖默认，且越界夹到区间。
/// 同一假 yt-dlp 的 5 MiB 输出，在默认上限下不截断，在小上限下必须截断。
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stdout_cap_honours_per_plugin_config() {
    use std::io::Write;

    let data_dir = unique_dir("data_cap_cfg");
    let script = data_dir.join("fake-ytdlp.sh");
    {
        let mut f = std::fs::File::create(&script).expect("create fake yt-dlp");
        f.write_all(
            b"#!/bin/sh\n\
              for arg in \"$@\"; do case \"$arg\" in --version) echo 9999.99.99; exit 0;; esac; done\n\
              printf '{\"pad\":\"'; \n\
              head -c $((5*1024*1024)) /dev/zero | tr '\\0' 'a'; \n\
              printf '\"}\n'; \n",
        )
        .expect("write script");
    }
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("chmod fake yt-dlp");
    }

    let db = Db::open(&data_dir).await.expect("open db");
    db.set_config(
        fluxdown_engine::components::CONFIG_YTDLP_PATH,
        script.to_str().expect("script path utf8"),
    )
    .await
    .expect("seed fake yt-dlp path");
    let db_handle = db.clone();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let bridge =
        EngineBridge::new(db, &ProxyConfig::default(), tx, data_dir.clone()).expect("bridge");

    // 默认 16 MiB：5 MiB 输出不截断。
    let out = bridge
        .run_ytdlp("cap@yt", spec(&["-J", "https://example.com/x"]))
        .await
        .expect("run_ytdlp default cap");
    assert!(!out.truncated_stdout, "5 MiB under default 16 MiB cap");

    // 单插件覆盖为 1 MiB（下限）：同一 5 MiB 输出必须被截断。
    db_handle
        .set_config("plugin.cap@yt.ytdlp.stdout_cap", "1m")
        .await
        .expect("seed per-plugin cap");
    let out2 = bridge
        .run_ytdlp("cap@yt", spec(&["-J", "https://example.com/x"]))
        .await
        .expect("run_ytdlp per-plugin cap");
    assert!(
        out2.truncated_stdout,
        "5 MiB output must be truncated under a 1 MiB per-plugin cap"
    );

    // 另一插件不受该单插件覆盖影响（仍用默认）。
    let out3 = bridge
        .run_ytdlp("other@yt", spec(&["-J", "https://example.com/x"]))
        .await
        .expect("run_ytdlp other plugin");
    assert!(
        !out3.truncated_stdout,
        "per-plugin cap must not leak to other plugins"
    );
}
