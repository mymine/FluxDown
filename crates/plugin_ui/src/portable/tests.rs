use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use fluxdown_protocol::{PluginViewFrameDto, ViewOp};
use fluxdown_ui_i18n::I18nCatalog;
use gpui::{AppContext as _, Entity, TestApp};
use serde_json::{Value, json};

use super::{PluginViewHost, dispatch_closed, dispatch_frame, install, render, resync_all};
use crate::{PluginUiPort, PortFuture};

fn tree(text: &str) -> Value {
    json!({
        "id": "root", "k": "stack", "p": {}, "c": [
            {"id": "root/0", "k": "text", "p": {"text": text}, "c": []}
        ]
    })
}

/// 记录全部调用；`view.open` 返回会话 `s1`，`view.snapshot` 返回 seq 4 的树。
#[derive(Default)]
struct MockPort {
    calls: Mutex<Vec<(String, Value)>>,
    session_exists: Mutex<bool>,
}

impl MockPort {
    fn methods(&self) -> Vec<String> {
        self.calls
            .lock()
            .map(|calls| calls.iter().map(|(method, _)| method.clone()).collect())
            .unwrap_or_default()
    }

    fn count(&self, method: &str) -> usize {
        self.methods().iter().filter(|m| *m == method).count()
    }
}

impl PluginUiPort for MockPort {
    fn call(&self, method: String, params: Value) -> PortFuture<Value> {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push((method.clone(), params));
        }
        let exists = self.session_exists.lock().map(|e| *e).unwrap_or(true);
        let reply = match method.as_str() {
            "daemon.plugin.view.open" => Ok(json!({"session": "s1", "seq": 0, "tree": tree("0")})),
            "daemon.plugin.view.snapshot" if exists => Ok(json!({"seq": 4, "tree": tree("4")})),
            "daemon.plugin.view.snapshot" => Err(fluxdown_protocol::RpcErrorData::new(
                fluxdown_protocol::ApplicationErrorCode::InvalidArgument,
                false,
            )),
            _ => Ok(json!({})),
        };
        Box::pin(async move { reply })
    }
}

fn frame(seq: u64, text: &str) -> PluginViewFrameDto {
    let mut set = serde_json::Map::new();
    set.insert("text".to_owned(), json!(text));
    PluginViewFrameDto {
        session: "s1".to_owned(),
        seq,
        ops: vec![ViewOp::Props {
            id: "root/0".to_owned(),
            set,
            unset: Vec::new(),
        }],
    }
}

fn text_of(host: &PluginViewHost) -> String {
    host.tree()
        .and_then(|tree| tree.c.first())
        .and_then(|node| node.p.get("text"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned()
}

fn setup() -> (TestApp, Arc<MockPort>, Entity<PluginViewHost>) {
    let port = Arc::new(MockPort {
        session_exists: Mutex::new(true),
        ..MockPort::default()
    });
    let mut app = TestApp::new();
    let host = app.update(|cx| {
        let catalog = Arc::new(I18nCatalog::load_embedded().expect("embedded translations"));
        let translator = cx.new(|_| catalog.translator("en"));
        install(cx, port.clone(), translator.clone());
        let port: Arc<dyn PluginUiPort> = port.clone();
        let host = cx.new(|cx| {
            PluginViewHost::new(
                "t@p".to_owned(),
                "main".to_owned(),
                None,
                port,
                translator,
                cx,
            )
        });
        host.update(cx, |host, cx| host.activate(cx));
        host
    });
    app.run_until_parked();
    (app, port, host)
}

#[test]
fn frames_apply_in_order_and_a_gap_fetches_a_snapshot() {
    let (mut app, port, host) = setup();
    app.update(|cx| {
        let host = host.read(cx);
        assert_eq!(host.session(), Some("s1"));
        assert_eq!(host.seq(), 0);
        assert_eq!(text_of(host), "0");
    });

    app.update(|cx| dispatch_frame(frame(1, "1"), cx));
    app.update(|cx| dispatch_frame(frame(2, "2"), cx));
    app.run_until_parked();
    app.update(|cx| {
        assert_eq!(host.read(cx).seq(), 2);
        assert_eq!(text_of(host.read(cx)), "2");
    });
    assert_eq!(port.count("daemon.plugin.view.snapshot"), 0);

    // 重复帧被忽略。
    app.update(|cx| dispatch_frame(frame(2, "dup"), cx));
    app.update(|cx| assert_eq!(text_of(host.read(cx)), "2"));

    // seq 跳到 4：重取 snapshot，树与序号追平。
    app.update(|cx| dispatch_frame(frame(4, "gap"), cx));
    app.run_until_parked();
    assert_eq!(port.count("daemon.plugin.view.snapshot"), 1);
    app.update(|cx| {
        assert_eq!(host.read(cx).seq(), 4);
        assert_eq!(text_of(host.read(cx)), "4");
    });
}

#[test]
fn frames_for_other_sessions_are_ignored() {
    let (mut app, _port, host) = setup();
    let mut other = frame(1, "x");
    other.session = "other".to_owned();
    app.update(|cx| dispatch_frame(other, cx));
    app.update(|cx| assert_eq!(host.read(cx).seq(), 0));
}

#[test]
fn closed_session_is_marked_and_reopen_opens_a_new_one() {
    let (mut app, port, host) = setup();
    app.update(|cx| dispatch_closed("s1", "pluginUnloaded", cx));
    app.update(|cx| {
        assert!(host.read(cx).is_closed());
        assert_eq!(host.read(cx).session(), None);
    });
    app.update(|cx| host.update(cx, |host, cx| host.activate(cx)));
    app.run_until_parked();
    assert_eq!(port.count("daemon.plugin.view.open"), 2);
    app.update(|cx| assert!(!host.read(cx).is_closed()));
}

#[test]
fn resync_all_reopens_sessions_the_daemon_no_longer_has() {
    let (mut app, port, host) = setup();
    app.update(resync_all);
    app.run_until_parked();
    assert_eq!(port.count("daemon.plugin.view.snapshot"), 1);
    app.update(|cx| assert_eq!(host.read(cx).seq(), 4));

    if let Ok(mut exists) = port.session_exists.lock() {
        *exists = false;
    }
    app.update(resync_all);
    app.run_until_parked();
    assert_eq!(port.count("daemon.plugin.view.open"), 2);
    app.update(|cx| {
        assert_eq!(host.read(cx).seq(), 0);
        assert_eq!(text_of(host.read(cx)), "0");
    });
}

#[test]
fn releasing_the_host_closes_the_session() {
    let (mut app, port, host) = setup();
    drop(host);
    // 释放在下一次 effect flush 时执行。
    app.update(|_| ());
    app.run_until_parked();
    assert_eq!(port.count("daemon.plugin.view.close"), 1);
}

#[test]
fn tone_table_matches_the_shared_fixture() {
    let fixture: BTreeMap<String, String> = serde_json::from_str(include_str!(
        "../../../../native/protocol/src/plugin_view_tones.json"
    ))
    .expect("tones fixture");
    let table: BTreeMap<String, String> = render::TONES
        .iter()
        .map(|(tone, path)| ((*tone).to_owned(), (*path).to_owned()))
        .collect();
    assert_eq!(table, fixture);
    assert_eq!(render::tone_path("danger"), Some("colors.destructive"));
    assert_eq!(render::tone_path("nope"), None);
}
