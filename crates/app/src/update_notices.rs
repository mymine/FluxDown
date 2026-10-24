//! 应用内更新提示：「发现新版本」提醒窗口的开窗时机、菜单「检查更新…」的结果，以及安装失败 /
//! 已更新的一次性通知。
//!
//! 状态唯一来源是 agent（`AgentSnapshot.update` / `UpdateChanged`）。提醒窗口电平触发：每次
//! 更新状态 / 偏好变化都按 [`update_prompt::is_due`] 重新判定，到期且窗口未开就开窗；「稍后提示」
//! 到期由本地定时器补一次判定。界面不在运行时由 agent 按同一判定拉起界面（`--capture`）。
//! 用户关闭提醒窗口（「稍后提示」或关闭按钮）而提醒仍到期时，记为稍后提示。

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fluxdown_protocol::{
    AgentEvent, ServiceEvent, UpdatePhase, UpdateStatusDto, method, update_prompt,
};
use fluxdown_ui_settings::{update_prompt::install_in_flight, update_view};
use gpui::{App, Global, Task};
use gpui_component::{WindowExt as _, notification::Notification};
use serde_json::Value;

use crate::{
    app::Desktop,
    session::SessionSignal,
    windows::{WindowKey, WindowRegistry},
};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Notice {
    message: String,
    error: bool,
}

#[derive(Default)]
struct UpdateNotices {
    /// 上一份状态里「用户已要求安装」（正在安装，或下载完成后立即安装）。
    install_in_flight: bool,
    /// 「稍后提示」到期时重新判定的定时器；替换即取消上一个。
    remind_timer: Option<Task<()>>,
    /// 承载窗口尚不存在时暂存的提示。
    pending: Vec<Notice>,
}

impl Global for UpdateNotices {}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

/// 安装请求失败：上一份状态里用户已要求安装，这一份落到 `Failed`。
fn install_failed(previous_in_flight: bool, status: &UpdateStatusDto) -> bool {
    previous_in_flight && status.phase == UpdatePhase::Failed
}

/// 订阅会话：快照与更新 / 偏好事件都走同一判定。须在 `SettingsStore` 接入会话之后安装，
/// 读到的状态与偏好才是本帧的。
pub(crate) fn install(cx: &mut App) {
    cx.set_global(UpdateNotices::default());
    reconcile(cx);
    let session = Desktop::global(cx).session.clone();
    cx.subscribe(&session, |_, signal, cx| match signal {
        SessionSignal::Snapshot(_) => reconcile(cx),
        SessionSignal::Event(frame) => {
            if let ServiceEvent::Agent(
                AgentEvent::UpdateChanged(_) | AgentEvent::PreferencesChanged(_),
            ) = &frame.event
            {
                reconcile(cx);
            }
        }
        SessionSignal::Stale
        | SessionSignal::Fatal(_)
        | SessionSignal::ServiceStopped
        | SessionSignal::ServiceFailed(_) => {}
    })
    .detach();
}

fn reconcile(cx: &mut App) {
    let store = Desktop::global(cx).settings_store.clone();
    let (status, skipped, remind_after) = {
        let store = store.read(cx);
        let preferences = store.preferences();
        (
            store.update_status().clone(),
            update_prompt::skipped_version(preferences).map(str::to_owned),
            update_prompt::remind_after_ms(preferences),
        )
    };
    let state = cx.global_mut::<UpdateNotices>();
    let previous = std::mem::replace(&mut state.install_in_flight, install_in_flight(&status));
    state.remind_timer = None;
    if install_failed(previous, &status) && !WindowRegistry::is_open(cx, &WindowKey::UpdatePrompt) {
        // 提醒窗口开着时失败原因就显示在窗口里。
        let message = Desktop::global(cx)
            .translator
            .read(cx)
            .text(update_view::failure_key(status.failure))
            .to_owned();
        show(
            Notice {
                message,
                error: true,
            },
            cx,
        );
    }
    let Some(version) = update_prompt::prompt_version(&status) else {
        return;
    };
    if skipped.as_deref() == Some(version) {
        return;
    }
    let now = now_ms();
    match remind_after.filter(|after| *after > now) {
        Some(after) => schedule_reminder(after - now, cx),
        None => {
            if !WindowRegistry::is_open(cx, &WindowKey::UpdatePrompt) {
                crate::windows::update_prompt::open(cx);
            }
        }
    }
}

fn schedule_reminder(delay_ms: u64, cx: &mut App) {
    let timer = cx.spawn(async move |cx| {
        cx.background_executor()
            .timer(Duration::from_millis(delay_ms))
            .await;
        cx.update(reconcile);
    });
    cx.global_mut::<UpdateNotices>().remind_timer = Some(timer);
}

/// 提醒窗口已关闭：未跳过、未在安装且提醒仍到期 → 记为「稍后提示」。
pub(crate) fn prompt_closed(cx: &mut App) {
    let store = Desktop::global(cx).settings_store.clone();
    let snooze = {
        let store = store.read(cx);
        let status = store.update_status();
        !install_in_flight(status)
            && !store.is_busy("updateInstall")
            && update_prompt::is_due(status, store.preferences(), now_ms())
    };
    if !snooze {
        return;
    }
    let after = now_ms().saturating_add(update_prompt::REMIND_LATER_MS);
    store.update(cx, |store, cx| {
        store.set_pref(update_prompt::REMIND_AFTER_KEY, Value::from(after), cx);
    });
    schedule_reminder(update_prompt::REMIND_LATER_MS, cx);
}

/// 菜单「检查更新…」：有新版本即打开提醒窗口（不受跳过 / 稍后提示限制）；否则通知结果。
pub(crate) fn check_now(cx: &mut App) {
    let future = Desktop::global(cx)
        .client
        .call::<serde_json::Value, UpdateStatusDto>(
            method::AGENT_UPDATE_CHECK,
            Some(serde_json::json!({})),
        );
    cx.spawn(async move |cx| {
        let result = future.await;
        cx.update(|cx| {
            let translator = Desktop::global(cx).translator.read(cx).clone();
            let notice = match result {
                Ok(status) if status.has_update => {
                    crate::windows::update_prompt::open(cx);
                    return;
                }
                Ok(status) if status.phase == UpdatePhase::Failed => Notice {
                    message: translator
                        .text(update_view::failure_key(status.failure))
                        .to_owned(),
                    error: true,
                },
                Ok(_) => Notice {
                    message: translator.text("upToDate").to_owned(),
                    error: false,
                },
                Err(_) => Notice {
                    message: translator.text("localServiceActionFailed").to_owned(),
                    error: true,
                },
            };
            show(notice, cx);
        });
    })
    .detach();
}

/// `--after-update` 启动完成：提示已更新到当前版本。
pub(crate) fn show_installed(cx: &mut App) {
    let message = Desktop::global(cx).translator.read(cx).text_with(
        "updateInstalledToast",
        &[("v", fluxdown_protocol::APP_VERSION)],
    );
    show(
        Notice {
            message,
            error: false,
        },
        cx,
    );
}

/// 主窗口就绪后补弹窗口尚不存在时暂存的提示。
pub(crate) fn replay_pending(cx: &mut App) {
    let pending = std::mem::take(&mut cx.global_mut::<UpdateNotices>().pending);
    for notice in pending {
        show(notice, cx);
    }
}

fn show(notice: Notice, cx: &mut App) {
    let Some(handle) = WindowRegistry::overlay_window(cx) else {
        cx.global_mut::<UpdateNotices>().pending.push(notice);
        return;
    };
    let built = notice.clone();
    let shown = handle
        .update(cx, |_, window, cx| {
            let note = if built.error {
                Notification::error(built.message)
            } else {
                Notification::info(built.message)
            };
            window.push_notification(note, cx);
        })
        .is_ok();
    if !shown {
        cx.global_mut::<UpdateNotices>().pending.push(notice);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(phase: UpdatePhase) -> UpdateStatusDto {
        UpdateStatusDto {
            phase,
            latest_version: "2.0.0".to_owned(),
            has_update: true,
            ..UpdateStatusDto::default()
        }
    }

    #[test]
    fn failure_only_notifies_after_an_install_request() {
        let failed = status(UpdatePhase::Failed);
        assert!(!install_failed(false, &failed));
        assert!(install_failed(true, &failed));
        let mut pending = status(UpdatePhase::Downloading);
        assert!(!install_in_flight(&pending));
        pending.install_pending = true;
        assert!(install_in_flight(&pending));
        assert!(install_in_flight(&status(UpdatePhase::Installing)));
    }
}
