//! 可移植插件视图：daemon 里的插件 JS 产出数据树，这里用 GPUI 原生控件渲染。
//!
//! 数据流：`PluginViewHost.activate` → `daemon.plugin.view.open` 拿到整棵树；之后 daemon 推
//! `PluginViewFrame`（增量 ops），由 app 转给 [`dispatch_frame`]；用户操作经
//! `daemon.plugin.view.event` 回到插件。

mod host;
mod render;
#[cfg(test)]
mod tests;

use std::{collections::HashMap, sync::Arc, time::Duration};

use fluxdown_protocol::PluginViewFrameDto;
use fluxdown_ui_i18n::Translator;
use gpui::{App, Entity, Global, Task, WeakEntity};
use serde_json::json;

pub use host::PluginViewHost;
pub use render::{TONES, format_bytes, format_percent, format_speed, format_time_local, tone_path};

use crate::PluginUiPort;

/// 活跃会话保活间隔（daemon 90 s 无保活即关闭会话）。
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(30);

/// 渲染与会话共用的进程级依赖。
pub(crate) struct PortableEnv {
    pub port: Arc<dyn PluginUiPort>,
    pub translator: Entity<Translator>,
}

impl Global for PortableEnv {}

/// session id → 宿主实体。
#[derive(Default)]
struct PortableSessions {
    by_session: HashMap<String, WeakEntity<PluginViewHost>>,
    keepalive: Option<Task<()>>,
}

impl Global for PortableSessions {}

impl PortableSessions {
    fn register(session: String, host: WeakEntity<PluginViewHost>, cx: &mut App) {
        let sessions = cx.default_global::<PortableSessions>();
        sessions.by_session.insert(session, host);
        ensure_keepalive(cx);
    }

    fn unregister(session: &str, cx: &mut App) {
        if cx
            .try_global::<PortableSessions>()
            .is_some_and(|sessions| sessions.by_session.contains_key(session))
        {
            cx.global_mut::<PortableSessions>()
                .by_session
                .remove(session);
        }
    }

    fn host(session: &str, cx: &App) -> Option<WeakEntity<PluginViewHost>> {
        cx.try_global::<PortableSessions>()?
            .by_session
            .get(session)
            .cloned()
    }

    /// 清掉已释放的宿主，返回存活的 (session, host)。
    fn live(cx: &mut App) -> Vec<(String, Entity<PluginViewHost>)> {
        let Some(sessions) = cx.try_global::<PortableSessions>() else {
            return Vec::new();
        };
        let entries: Vec<(String, WeakEntity<PluginViewHost>)> = sessions
            .by_session
            .iter()
            .map(|(session, host)| (session.clone(), host.clone()))
            .collect();
        let mut live = Vec::with_capacity(entries.len());
        let mut dead = Vec::new();
        for (session, weak) in entries {
            match weak.upgrade() {
                Some(host) => live.push((session, host)),
                None => dead.push(session),
            }
        }
        if !dead.is_empty() {
            let sessions = cx.global_mut::<PortableSessions>();
            for session in dead {
                sessions.by_session.remove(&session);
            }
        }
        live
    }
}

/// 安装渲染器依赖。须在创建第一个 [`PluginViewHost`] 之前调用（app 启动期，`init` 之后）。
///
/// 同时挂上主题与语言变化的重绘：主题 token 变化后所有存活视图重新渲染。
pub fn install(cx: &mut App, port: Arc<dyn PluginUiPort>, translator: Entity<Translator>) {
    cx.observe_global::<fluxdown_ui_theme::FluxThemeState>(notify_all)
        .detach();
    cx.observe(&translator, |_, cx| notify_all(cx)).detach();
    cx.set_global(PortableEnv { port, translator });
    cx.set_global(PortableSessions::default());
}

/// 让所有存活的可移植视图重绘（主题 / 语言变化）。
pub fn notify_all(cx: &mut App) {
    for (_, host) in PortableSessions::live(cx) {
        host.update(cx, |_, cx| cx.notify());
    }
}

/// daemon 推来的增量帧。未知会话（别的窗口 / 已关闭）忽略。
pub fn dispatch_frame(frame: PluginViewFrameDto, cx: &mut App) {
    let Some(host) = PortableSessions::host(&frame.session, cx).and_then(|weak| weak.upgrade())
    else {
        return;
    };
    host.update(cx, |host, cx| host.apply_frame(frame, cx));
}

/// daemon 宣布会话结束（插件卸载 / 禁用等）。
pub fn dispatch_closed(session: &str, reason: &str, cx: &mut App) {
    let Some(host) = PortableSessions::host(session, cx).and_then(|weak| weak.upgrade()) else {
        return;
    };
    host.update(cx, |host, cx| host.mark_closed(reason, cx));
}

/// 连接恢复 / 快照被整体替换后：所有存活会话重取 snapshot，会话已不存在则重新 open。
pub fn resync_all(cx: &mut App) {
    for (_, host) in PortableSessions::live(cx) {
        host.update(cx, |host, cx| host.resync(true, cx));
    }
}

/// 有存活会话时每 30 s 向 daemon 发一次保活；没有会话后自行退出。
fn ensure_keepalive(cx: &mut App) {
    if cx
        .try_global::<PortableSessions>()
        .is_none_or(|sessions| sessions.keepalive.is_some())
    {
        return;
    }
    let task = cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(KEEPALIVE_INTERVAL).await;
            let Some(call) = cx.update(keepalive_tick) else {
                break;
            };
            if let Err(error) = call.await {
                log::warn!("plugin view keepalive failed: {error:?}");
            }
        }
    });
    cx.global_mut::<PortableSessions>().keepalive = Some(task);
}

/// 返回本轮保活调用；没有存活会话时返回 `None` 并复位，下一个会话注册时重新启动。
fn keepalive_tick(cx: &mut App) -> Option<crate::PortFuture<serde_json::Value>> {
    let live = PortableSessions::live(cx);
    if live.is_empty() {
        cx.global_mut::<PortableSessions>().keepalive = None;
        return None;
    }
    let port = cx.try_global::<PortableEnv>()?.port.clone();
    let sessions: Vec<String> = live.into_iter().map(|(session, _)| session).collect();
    Some(port.call(
        "daemon.plugin.view.keepalive".to_owned(),
        json!({ "sessions": sessions }),
    ))
}
