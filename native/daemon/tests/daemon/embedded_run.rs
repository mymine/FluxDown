//! `run_with` 的嵌入式契约：真实监听地址 / token 的上报、进程租约冲突的错误语义。

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use fluxdown_daemon::config::DaemonConfig;
use fluxdown_daemon::runtime::run_with;
use tokio_util::sync::CancellationToken;

const STEP: Duration = Duration::from_secs(30);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_with_reports_the_bound_address_and_rejects_a_second_embedder() {
    let root = std::env::temp_dir().join(format!(
        "fluxdown_daemon_embedded_{}_{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    let cancel = CancellationToken::new();
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let config = DaemonConfig::embedded(
        root.join("engine"),
        Some(root.join("dl").display().to_string()),
    );
    let first = tokio::spawn(run_with(config.clone(), cancel.clone(), Some(ready_tx)));
    let ready = tokio::time::timeout(STEP, ready_rx)
        .await
        .expect("daemon ready timeout")
        .expect("daemon exited before reporting ready");
    assert!(ready.addr.ip().is_loopback());
    assert_ne!(ready.addr.port(), 0, "the real ephemeral port is reported");
    assert!(!ready.token.is_empty());

    // 租约已被第一个 daemon 持有：带 ready 的嵌入方必须得到错误，而不是 `Ok(())`。
    let (second_tx, second_rx) = tokio::sync::oneshot::channel();
    let error = tokio::time::timeout(
        STEP,
        run_with(config.clone(), CancellationToken::new(), Some(second_tx)),
    )
    .await
    .expect("second embedder must fail promptly")
    .expect_err("lease held with ready requested is an error");
    assert!(
        error.to_string().contains("process lease"),
        "unexpected error: {error}"
    );
    assert!(
        second_rx.await.is_err(),
        "no ready report for a failed start"
    );

    // 独立进程语义不变：没有 ready 时静默 `Ok(())`。
    tokio::time::timeout(STEP, run_with(config, CancellationToken::new(), None))
        .await
        .expect("standalone duplicate must return promptly")
        .expect("standalone duplicate keeps returning Ok");

    cancel.cancel();
    tokio::time::timeout(STEP, first)
        .await
        .expect("daemon shutdown timeout")
        .expect("daemon task join")
        .expect("daemon shutdown result");
    if let Err(error) = std::fs::remove_dir_all(&root) {
        eprintln!("test cleanup failed for {}: {error}", root.display());
    }
}
