//! 失败的 dev 安装只能回退自己写入的 dev 登记：同 identity 已安装的插件目录与
//! 启停设置必须原样保留。
//!
//! 仅 `plugins` feature 下编译运行。

#![cfg(feature = "plugins")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;

use fluxdown_engine::bt_config::BtConfig;
use fluxdown_engine::plugin::PluginError;
use fluxdown_engine::proxy_config::ProxyConfig;
use fluxdown_engine::{Engine, EngineConfig, NoopSelection, NoopSink};

const IDENTITY: &str = "tester@rollback";

fn uniq() -> String {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{}-{}", std::process::id(), n)
}

async fn make_engine(work: &Path) -> Engine {
    let cfg = EngineConfig {
        max_concurrent: 4,
        speed_limit_bps: 0,
        upload_limit_bps: 0,
        default_save_dir: work.to_string_lossy().into_owned(),
        app_data_dir: work.to_string_lossy().into_owned(),
        bt_config: BtConfig::default(),
        proxy_config: ProxyConfig::default(),
        user_agent: String::new(),
        data_dir_override: Some(work.to_path_buf()),
        database_url: None,
    };
    Engine::new(cfg, Arc::new(NoopSink), Arc::new(NoopSelection))
        .await
        .expect("engine")
}

async fn write_plugin(dir: &Path, version: &str, script: &str) {
    tokio::fs::create_dir_all(dir).await.expect("mkdir");
    let manifest = format!(
        r#"{{
      "identity": "{IDENTITY}",
      "name": "Rollback Test",
      "version": "{version}",
      "permissions": ["auth"],
      "auth": {{ "entry": "auth.js" }}
    }}"#
    );
    tokio::fs::write(dir.join("manifest.json"), manifest)
        .await
        .expect("write manifest");
    tokio::fs::write(dir.join("auth.js"), script)
        .await
        .expect("write auth.js");
}

const VALID_SCRIPT: &str =
    r#"globalThis.authenticate = async (ctx) => JSON.stringify({ status: "success" });"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_dev_install_keeps_installed_plugin_and_settings() {
    let work = std::env::temp_dir().join(format!("fluxdown-install-rollback-{}", uniq()));
    let installed_src = work.join("installed-src");
    let dev_src = work.join("dev-src");
    write_plugin(&installed_src, "1.0.0", VALID_SCRIPT).await;
    write_plugin(&dev_src, "9.0.0", "globalThis.authenticate = (;").await;

    let engine = make_engine(&work).await;
    let pm = engine.manager.plugin_manager().expect("pm installed");
    pm.install_from_dir(&installed_src).await.expect("install");
    pm.set_enabled(IDENTITY, false).await.expect("disable");

    let err = pm.install_dev(&dev_src).await.expect_err("broken dev copy");
    assert!(matches!(err, PluginError::CompileFailed(_)), "got {err:?}");

    // 已安装版本仍在、仍是正式版、手动禁用状态未被抹掉。
    let info = pm
        .list()
        .await
        .into_iter()
        .find(|p| p.identity == IDENTITY)
        .expect("installed plugin must survive failed dev install");
    assert_eq!(info.version, "1.0.0");
    assert_eq!(info.load_status, "Loaded");
    assert!(!info.dev_mode);
    assert!(!info.enabled);

    // 失败的 dev 登记已回退。
    let err = pm
        .reload_dev(IDENTITY)
        .await
        .expect_err("no dev registration");
    assert!(matches!(err, PluginError::NotDevPlugin(_)), "got {err:?}");

    if let Err(error) = tokio::fs::remove_dir_all(&work).await
        && error.kind() != std::io::ErrorKind::NotFound
    {
        eprintln!("best-effort test directory cleanup: {error}");
    }
}
