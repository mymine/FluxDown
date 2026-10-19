//! 宿主自身下载的统一出口：载荷地址按下载任务同一套 resolver 规则交给插件改写，
//! 只接受单一 https 直链，插件出错 / 不命中一律原地址。
//!
//! 仅 `plugins` feature 下编译运行。

#![cfg(feature = "plugins")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use fluxdown_engine::bt_config::BtConfig;
use fluxdown_engine::egress::{Egress, EgressAttempt};
use fluxdown_engine::proxy_config::ProxyConfig;
use fluxdown_engine::{Engine, EngineConfig, NoopSelection, NoopSink};

async fn write_plugin(dir: &std::path::Path, identity: &str, pattern: &str, resolve_js: &str) {
    tokio::fs::create_dir_all(dir).await.expect("mkdir plugin");
    let manifest = format!(
        r#"{{
      "identity": "{identity}",
      "name": "Plugin {identity}",
      "version": "1.0.0",
      "description": "egress routing fixture",
      "resolvers": [{{ "match": {{ "urls": ["{pattern}"] }}, "entry": "resolve.js", "timeoutMs": 5000 }}]
    }}"#
    );
    tokio::fs::write(dir.join("manifest.json"), manifest)
        .await
        .expect("write manifest");
    tokio::fs::write(dir.join("resolve.js"), resolve_js)
        .await
        .expect("write resolve.js");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn verified_payloads_follow_plugin_resolvers_with_original_fallback() {
    let work =
        std::env::temp_dir().join(format!("fluxdown-egress-{}", uuid::Uuid::new_v4().simple()));
    tokio::fs::create_dir_all(&work).await.expect("mkdir work");
    let engine = Engine::new(
        EngineConfig {
            max_concurrent: 2,
            speed_limit_bps: 0,
            upload_limit_bps: 0,
            default_save_dir: work.to_string_lossy().into_owned(),
            app_data_dir: work.to_string_lossy().into_owned(),
            bt_config: BtConfig::default(),
            proxy_config: ProxyConfig::default(),
            user_agent: String::new(),
            data_dir_override: Some(work.clone()),
            database_url: None,
        },
        Arc::new(NoopSink),
        Arc::new(NoopSelection),
    )
    .await
    .expect("engine");
    let pm = engine.manager.plugin_manager().expect("plugin manager");
    for (identity, pattern, script) in [
        (
            "accel@test",
            "*://secure.origin.test/*",
            "globalThis.resolve = async (ctx) => ({ url: 'https://mirror.test/' + ctx.url.split('/').pop(), extraHeaders: { 'x-mirror': '1' } });",
        ),
        (
            "plain@test",
            "*://plain.origin.test/*",
            "globalThis.resolve = async () => ({ url: 'http://mirror.test/a.bin' });",
        ),
        (
            "broken@test",
            "*://broken.origin.test/*",
            "globalThis.resolve = async () => { throw new Error('boom'); };",
        ),
    ] {
        let src = work.join(format!("src-{identity}"));
        write_plugin(&src, identity, pattern, script).await;
        pm.install_from_dir(&src).await.expect("install");
    }
    let egress = Egress::new(engine.db.clone()).with_plugins(pm);

    let source = "https://secure.origin.test/releases/a.bin";
    let route = egress.route_verified(source).await;
    assert_eq!(
        route.attempts(),
        vec![
            EgressAttempt {
                url: "https://mirror.test/a.bin".to_owned(),
                headers: vec![("x-mirror".to_owned(), "1".to_owned())],
                plugin: Some("accel@test".to_owned()),
            },
            EgressAttempt {
                url: source.to_owned(),
                headers: Vec::new(),
                plugin: None,
            },
        ]
    );

    // 插件把 https 降级成 http、插件抛错、无插件命中：都按原地址下载。
    for direct in [
        "https://plain.origin.test/a.bin",
        "https://broken.origin.test/a.bin",
        "https://elsewhere.test/a.bin",
    ] {
        let route = egress.route_verified(direct).await;
        assert!(route.rewrite().is_none(), "{direct} must not be rewritten");
        assert_eq!(route.attempts().len(), 1);
    }

    // 完整性锚（非已校验载荷）只给代理，地址不变。
    let plan = egress.plan(source, false).await.expect("plan");
    assert!(plan.route.rewrite().is_none());
    assert_eq!(plan.proxy, None);

    drop(engine);
    if let Err(error) = tokio::fs::remove_dir_all(&work).await {
        eprintln!("egress test directory removal failed: {error}");
    }
}
