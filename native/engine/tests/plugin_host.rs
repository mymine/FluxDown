//! 常驻插件宿主（`main`）：可移植视图会话、命令、RPC、`flux.tasks` 限流、宿主回收与渲染失败。
//!
//! 仅 `plugins` feature 下编译运行。

#![cfg(feature = "plugins")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fluxdown_engine::bt_config::BtConfig;
use fluxdown_engine::events::{EngineEvent, EventSink};
use fluxdown_engine::plugin::{PluginError, PluginManager, PluginTaskPort};
use fluxdown_engine::proxy_config::ProxyConfig;
use fluxdown_engine::{Engine, EngineConfig, NoopSelection};
use serde_json::{Value, json};

const IDENTITY: &str = "tester@host";

const MANIFEST: &str = r#"{
  "identity": "tester@host",
  "name": "Host Test",
  "version": "1.0.0",
  "permissions": ["tasks"],
  "main": { "entry": "main.js" },
  "contributes": {
    "views": [
      { "id": "counter", "title": "Counter", "location": "activity" },
      { "id": "flaky", "title": "Flaky", "location": "activity" },
      { "id": "ghost", "title": "Ghost", "location": "activity" }
    ],
    "commands": [
      { "id": "run", "title": "Run" },
      { "id": "spam", "title": "Spam" }
    ]
  }
}"#;

const MAIN_JS: &str = r#"
let ran = false;
let spamResults = [];

flux.ui.view("counter", {
  init: async () => ({ n: 0 }),
  render: (st) =>
    flux.ui.h("stack", { direction: "column" },
      flux.ui.h("text", { text: String(st.n) }),
      flux.ui.h("button", { text: "+" })),
  event: async (st, ev) => {
    if (ev.type === "press") st.n += 1;
  },
});

flux.ui.view("flaky", {
  init: () => ({ fail: false }),
  render: (st) => {
    if (st.fail) throw new Error("boom");
    return flux.ui.h("text", { text: "ok" });
  },
  event: (st) => { st.fail = true; },
});

flux.commands.register("run", async (ctx) => { ran = ctx.taskIds.join(","); });
flux.commands.register("spam", async () => {
  spamResults = [];
  for (let i = 0; i < 21; i++) {
    try {
      await flux.tasks.create({ url: "https://example.com/" + i });
      spamResults.push("ok");
    } catch (e) {
      spamResults.push(String(e.message));
    }
  }
});

flux.rpc.handle("ran", async () => ran);
flux.rpc.handle("spam", async () => spamResults);
flux.rpc.handle("echo", async (args) => ({ got: args }));
"#;

/// 记录全部事件的 sink。
#[derive(Default)]
struct CaptureSink(Mutex<Vec<EngineEvent>>);

impl EventSink for CaptureSink {
    fn emit(&self, event: EngineEvent) {
        self.0.lock().unwrap().push(event);
    }
}

impl CaptureSink {
    /// 轮询等待第一条被 `pick` 选中的事件。
    async fn wait_for<T>(&self, pick: impl Fn(&EngineEvent) -> Option<T>) -> T {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(found) = self.0.lock().unwrap().iter().find_map(&pick) {
                return found;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for event"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

/// 记录 daemon 调用的任务端口桩。
#[derive(Default)]
struct RecordingPort(Mutex<Vec<(String, Value)>>);

#[async_trait::async_trait]
impl PluginTaskPort for RecordingPort {
    async fn call(&self, method: &str, params: Value) -> Result<Value, PluginError> {
        self.0.lock().unwrap().push((method.to_string(), params));
        Ok(json!({ "id": "t1" }))
    }
}

fn uniq() -> String {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{}-{}", std::process::id(), n)
}

async fn make_engine(work: &Path, sink: Arc<CaptureSink>) -> Engine {
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
    Engine::new(cfg, sink, Arc::new(NoopSelection))
        .await
        .expect("engine")
}

struct Fixture {
    pm: Arc<PluginManager>,
    sink: Arc<CaptureSink>,
    port: Arc<RecordingPort>,
    _engine: Engine,
}

/// 安装并授权测试插件，装好任务端口。
async fn fixture() -> Fixture {
    fixture_with(MANIFEST, MAIN_JS, &["tasks"]).await
}

async fn fixture_with(manifest: &str, main_js: &str, permissions: &[&str]) -> Fixture {
    let work = std::env::temp_dir().join(format!("fluxdown-plugin-host-{}", uniq()));
    tokio::fs::create_dir_all(&work).await.expect("work dir");
    let src = work.join("plugin-src");
    tokio::fs::create_dir_all(&src).await.expect("src dir");
    tokio::fs::write(src.join("manifest.json"), manifest)
        .await
        .expect("manifest");
    tokio::fs::write(src.join("main.js"), main_js)
        .await
        .expect("main.js");

    let sink = Arc::new(CaptureSink::default());
    let engine = make_engine(&work, sink.clone()).await;
    let pm = engine.manager.plugin_manager().expect("pm installed");
    let port = Arc::new(RecordingPort::default());
    pm.set_task_port(port.clone());
    pm.install_from_dir(&src).await.expect("install");
    // 新装插件的权限需要授权，走正式 consent API。
    let granted: Vec<String> = permissions.iter().map(|p| (*p).to_string()).collect();
    pm.grant_consent(IDENTITY, &granted, &[])
        .await
        .expect("consent");
    Fixture {
        pm,
        sink,
        port,
        _engine: engine,
    }
}

fn text_of_first_child(tree: &Value) -> String {
    tree["c"][0]["p"]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plugin_host_counter_view_renders_and_updates_on_press() {
    let fx = fixture().await;
    let (session, tree) = fx
        .pm
        .open_view(IDENTITY, "counter", json!({}), None, "en")
        .await
        .expect("open view");
    assert_eq!(tree["k"], "stack");
    assert_eq!(text_of_first_child(&tree), "0");

    fx.pm
        .view_event(&session, json!({ "node": "root/1", "type": "press" }))
        .expect("queue event");
    let rendered = fx
        .sink
        .wait_for(|event| match event {
            EngineEvent::PluginViewRendered { session: s, tree } if *s == session => {
                Some(tree.clone())
            }
            _ => None,
        })
        .await;
    assert_eq!(text_of_first_child(&rendered), "1");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plugin_host_runs_registered_command_and_rpc() {
    let fx = fixture().await;
    fx.pm
        .run_command(IDENTITY, "run", vec!["a".to_string(), "b".to_string()])
        .await
        .expect("run command");
    let ran = fx
        .pm
        .invoke(IDENTITY, "ran", Value::Null)
        .await
        .expect("invoke ran");
    assert_eq!(ran, json!("a,b"));

    let echoed = fx
        .pm
        .invoke(IDENTITY, "echo", json!({ "x": 1 }))
        .await
        .expect("invoke echo");
    assert_eq!(echoed, json!({ "got": { "x": 1 } }));

    // 未声明的命令被拒绝，未注册的 RPC 报错。
    assert!(fx.pm.run_command(IDENTITY, "nope", vec![]).await.is_err());
    assert!(
        fx.pm
            .invoke(IDENTITY, "missing", Value::Null)
            .await
            .is_err()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plugin_host_rate_limits_task_creation() {
    let fx = fixture().await;
    fx.pm
        .run_command(IDENTITY, "spam", vec![])
        .await
        .expect("spam command");
    let calls = fx.port.0.lock().unwrap().clone();
    let creates = calls
        .iter()
        .filter(|(method, _)| method == "daemon.task.create")
        .count();
    assert_eq!(creates, 20, "the 21st create must not reach the port");
    assert_eq!(calls[0].1["url"], "https://example.com/0");

    let results = fx
        .pm
        .invoke(IDENTITY, "spam", Value::Null)
        .await
        .expect("spam results");
    let results = results.as_array().expect("array");
    assert_eq!(results.len(), 21);
    assert_eq!(results[19], "ok");
    assert_eq!(results[20], "tasks.create rate limited");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plugin_host_disable_closes_sessions_with_plugin_unloaded() {
    let fx = fixture().await;
    let (session, _) = fx
        .pm
        .open_view(IDENTITY, "counter", json!({}), None, "en")
        .await
        .expect("open view");
    fx.pm.set_enabled(IDENTITY, false).await.expect("disable");
    let reason = fx
        .sink
        .wait_for(|event| match event {
            EngineEvent::PluginViewClosed { session: s, reason } if *s == session => {
                Some(reason.clone())
            }
            _ => None,
        })
        .await;
    assert_eq!(reason, "pluginUnloaded");
    // 禁用后不能再打开视图。
    assert!(
        fx.pm
            .open_view(IDENTITY, "counter", json!({}), None, "en")
            .await
            .is_err()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plugin_host_render_error_emits_view_failed() {
    let fx = fixture().await;
    let (session, _) = fx
        .pm
        .open_view(IDENTITY, "flaky", json!({}), None, "en")
        .await
        .expect("open view");
    fx.pm
        .view_event(&session, json!({ "node": "root", "type": "press" }))
        .expect("queue event");
    let message = fx
        .sink
        .wait_for(|event| match event {
            EngineEvent::PluginViewFailed {
                session: s,
                message,
            } if *s == session => Some(message.clone()),
            _ => None,
        })
        .await;
    assert!(message.contains("boom"), "unexpected message: {message}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plugin_host_rejects_unregistered_and_undeclared_views() {
    let fx = fixture().await;
    // manifest 声明了 ghost，但 main 没有注册。
    let err = fx
        .pm
        .open_view(IDENTITY, "ghost", json!({}), None, "en")
        .await
        .expect_err("ghost is not registered");
    assert!(
        matches!(&err, PluginError::Runtime(m) if m == "view not registered: ghost"),
        "unexpected error: {err:?}"
    );
    // manifest 没有声明的 view。
    let err = fx
        .pm
        .open_view(IDENTITY, "other", json!({}), None, "en")
        .await
        .expect_err("view is not declared");
    assert!(matches!(err, PluginError::Runtime(_)));
}

const EVENTS_MANIFEST: &str = r#"{
  "identity": "tester@host",
  "name": "Events Test",
  "version": "1.0.0",
  "main": { "entry": "main.js", "events": ["onDone"] },
  "contributes": {
    "views": [{ "id": "v", "title": "V", "location": "activity" }]
  }
}"#;

const EVENTS_JS: &str = r#"
let seen = [];
let ticks = 0;
flux.events.on("onDone", async (ev) => {
  seen.push(ev.taskId);
  flux.ui.toast("success", "done " + ev.taskId);
});
flux.timers.setTimeout(() => { ticks += 1; }, 30);
flux.rpc.handle("state", async () => ({ seen, ticks }));
flux.ui.view("v", { render: () => flux.ui.h("text", { text: "x" }) });
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plugin_host_delivers_declared_events_timers_and_toasts() {
    let fx = fixture_with(EVENTS_MANIFEST, EVENTS_JS, &[]).await;
    // 激活宿主（main 顶层注册了一个 30ms 定时器）。
    fx.pm
        .invoke(IDENTITY, "state", Value::Null)
        .await
        .expect("activate host");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let state = fx
        .pm
        .invoke(IDENTITY, "state", Value::Null)
        .await
        .expect("state after timer");
    assert_eq!(state["ticks"], 1);

    fx.pm
        .notify(fluxdown_engine::plugin::PluginEvent::Done {
            task_id: "t1".to_string(),
            url: "https://example.com/f".to_string(),
            file_path: "/tmp/f".to_string(),
            audio_path: None,
            muxed: false,
        })
        .await;
    let text = fx
        .sink
        .wait_for(|event| match event {
            EngineEvent::PluginToast {
                identity,
                level,
                text,
            } if identity == IDENTITY && level == "success" => Some(text.clone()),
            _ => None,
        })
        .await;
    assert_eq!(text, "done t1");
    let state = fx
        .pm
        .invoke(IDENTITY, "state", Value::Null)
        .await
        .expect("state after event");
    assert_eq!(state["seen"], json!(["t1"]));
}
