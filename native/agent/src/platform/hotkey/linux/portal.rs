//! Wayland 后端：XDG Desktop Portal `org.freedesktop.portal.GlobalShortcuts`。
//!
//! portal 只接受「组合键」，最终按键由桌面环境（首次绑定时的对话框 / 系统设置）确认；双击修饰键
//! 无法表达，恒为 `Unsupported`。portal 的绑定只增不减，所以每次 `apply` 都关闭旧会话、新建会话
//! 再整表 `BindShortcuts`。portal 侧的快捷键 id 取触发器规范文本（如 `ctrl-shift-d`）：相同触发器
//! 跨次 `apply` 保持同一 id，桌面环境记住的用户改键不会错配到别的动作。

use std::collections::{HashMap, HashSet};
use std::pin::Pin;
use std::sync::Arc;

use ashpd::desktop::Session;
use ashpd::desktop::global_shortcuts::{Activated, GlobalShortcuts, NewShortcut};
use fluxdown_protocol::shortcuts::{
    GlobalShortcutBackend, GlobalShortcutState, ShortcutChord, ShortcutTrigger,
};
use futures_util::{Stream, StreamExt};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use super::keys;
use crate::hotkey::{HotkeyBinding, HotkeyOutcome, HotkeyPort, HotkeySink};

pub(super) fn start(sink: Arc<dyn HotkeySink>) -> Result<Box<dyn HotkeyPort>, String> {
    let handle = tokio::runtime::Handle::try_current()
        .map_err(|e| format!("the portal backend needs a Tokio runtime: {e}"))?;
    let (tx, rx) = mpsc::unbounded_channel();
    handle.spawn(supervise(rx, sink));
    Ok(Box::new(PortalPort { tx }))
}

struct PortalPort {
    tx: UnboundedSender<Vec<HotkeyBinding>>,
}

impl HotkeyPort for PortalPort {
    fn apply(&self, bindings: Vec<HotkeyBinding>) {
        if let Err(e) = self.tx.send(bindings) {
            tracing::error!("portal hotkey task is gone: {e}");
        }
    }
}

/// 后台任务异常退出（含 panic）时回报 `Unavailable`，避免设置页一直显示旧状态。
async fn supervise(rx: UnboundedReceiver<Vec<HotkeyBinding>>, sink: Arc<dyn HotkeySink>) {
    let task = tokio::spawn(run(rx, Arc::clone(&sink)));
    if let Err(e) = task.await
        && !e.is_cancelled()
    {
        tracing::error!("portal hotkey task failed: {e}");
        sink.report(GlobalShortcutBackend::Unavailable, Vec::new());
    }
}

/// 与 portal 的连接：代理、全局 `Activated` 信号流、当前会话及其触发路由。
struct Connected {
    proxy: GlobalShortcuts,
    activated: Pin<Box<dyn Stream<Item = Activated> + Send>>,
    session: Option<Session<GlobalShortcuts>>,
    /// portal 快捷键 id → 绑定 id。
    routes: HashMap<String, Vec<u32>>,
}

impl Connected {
    async fn connect() -> Result<Self, ashpd::Error> {
        let proxy = GlobalShortcuts::new().await?;
        let activated = Box::pin(proxy.receive_activated().await?);
        Ok(Self {
            proxy,
            activated,
            session: None,
            routes: HashMap::new(),
        })
    }

    async fn close_session(&mut self) {
        self.routes.clear();
        if let Some(session) = self.session.take() {
            close(&session).await;
        }
    }

    fn dispatch(&self, activated: &Activated, sink: &dyn HotkeySink) {
        if let Some(ids) = self.routes.get(activated.shortcut_id()) {
            for id in ids {
                sink.fired(*id);
            }
        }
    }
}

async fn close(session: &Session<GlobalShortcuts>) {
    if let Err(e) = session.close().await {
        tracing::warn!("closing the GlobalShortcuts session failed: {e}");
    }
}

async fn run(mut rx: UnboundedReceiver<Vec<HotkeyBinding>>, sink: Arc<dyn HotkeySink>) {
    let mut connection: Option<Connected> = None;
    let mut queued: Option<Vec<HotkeyBinding>> = None;
    loop {
        let table = match queued.take() {
            Some(table) => table,
            None => match next_table(&mut rx, &mut connection, sink.as_ref()).await {
                Some(table) => table,
                None => break,
            },
        };
        let mut table = table;
        while let Ok(newer) = rx.try_recv() {
            table = newer;
        }
        queued = apply(&mut connection, &table, &mut rx, sink.as_ref()).await;
    }
    if let Some(mut connection) = connection {
        connection.close_session().await;
    }
}

/// 等待下一张绑定表，期间转发 portal 的 `Activated` 信号；通道关闭返回 `None`。
async fn next_table(
    rx: &mut UnboundedReceiver<Vec<HotkeyBinding>>,
    connection: &mut Option<Connected>,
    sink: &dyn HotkeySink,
) -> Option<Vec<HotkeyBinding>> {
    loop {
        let Some(live) = connection.as_mut() else {
            return rx.recv().await;
        };
        tokio::select! {
            table = rx.recv() => return table,
            activated = live.activated.next() => match activated {
                Some(activated) => live.dispatch(&activated, sink),
                None => {
                    tracing::warn!("the GlobalShortcuts Activated stream ended");
                    *connection = None;
                }
            },
        }
    }
}

enum Bound {
    Done(Result<HashSet<String>, String>),
    /// 绑定等待用户确认期间收到了新表（`None` = 通道已关闭）。
    Superseded(Option<Vec<HotkeyBinding>>),
}

/// 应用一张表；返回绑定过程中被新表取代时的新表。
async fn apply(
    connection: &mut Option<Connected>,
    table: &[HotkeyBinding],
    rx: &mut UnboundedReceiver<Vec<HotkeyBinding>>,
    sink: &dyn HotkeySink,
) -> Option<Vec<HotkeyBinding>> {
    let plan = Plan::new(table);

    if connection.is_none() {
        match Connected::connect().await {
            Ok(connected) => *connection = Some(connected),
            Err(e) => {
                let detail = format!("xdg-desktop-portal GlobalShortcuts is unavailable: {e}");
                tracing::warn!("{detail}");
                sink.report(
                    GlobalShortcutBackend::Unavailable,
                    plan.finish(|_| failed(&detail)),
                );
                return None;
            }
        }
    }
    let live = connection.as_mut()?;
    live.close_session().await;

    if plan.chords.is_empty() {
        sink.report(
            GlobalShortcutBackend::WaylandPortal,
            plan.finish(|_| failed("no shortcut to bind")),
        );
        return None;
    }

    let session = match live.proxy.create_session(Default::default()).await {
        Ok(session) => session,
        Err(e) => {
            let detail = format!("creating a GlobalShortcuts session failed: {e}");
            tracing::warn!("{detail}");
            sink.report(
                GlobalShortcutBackend::Unavailable,
                plan.finish(|_| failed(&detail)),
            );
            return None;
        }
    };

    let shortcuts: Vec<NewShortcut> = plan
        .chords
        .iter()
        .map(|chord| chord.shortcut.clone())
        .collect();
    let bound = tokio::select! {
        result = bind(&live.proxy, &session, &shortcuts) => Bound::Done(result),
        newer = rx.recv() => Bound::Superseded(newer),
    };

    match bound {
        Bound::Superseded(newer) => {
            close(&session).await;
            newer
        }
        Bound::Done(Err(detail)) => {
            tracing::warn!("{detail}");
            close(&session).await;
            sink.report(
                GlobalShortcutBackend::WaylandPortal,
                plan.finish(|_| failed(&detail)),
            );
            None
        }
        Bound::Done(Ok(bound)) => {
            live.routes = plan.routes();
            live.session = Some(session);
            sink.report(
                GlobalShortcutBackend::WaylandPortal,
                plan.finish(|chord| {
                    if bound.contains(&chord.portal_id) {
                        (GlobalShortcutState::Active, None)
                    } else {
                        failed("the desktop environment did not bind this shortcut")
                    }
                }),
            );
            None
        }
    }
}

/// `BindShortcuts`：返回桌面环境确认绑定的 portal 快捷键 id。
async fn bind(
    proxy: &GlobalShortcuts,
    session: &Session<GlobalShortcuts>,
    shortcuts: &[NewShortcut],
) -> Result<HashSet<String>, String> {
    let request = proxy
        .bind_shortcuts(session, shortcuts, None, Default::default())
        .await
        .map_err(|e| format!("BindShortcuts failed: {e}"))?;
    let response = request
        .response()
        .map_err(|e| format!("BindShortcuts was rejected: {e}"))?;
    Ok(response
        .shortcuts()
        .iter()
        .map(|shortcut| shortcut.id().to_owned())
        .collect())
}

struct PlannedChord {
    portal_id: String,
    shortcut: NewShortcut,
    ids: Vec<u32>,
}

/// 一张绑定表拆成：待绑定的组合键（相同触发器合并）+ 已确定结果的条目。
struct Plan {
    chords: Vec<PlannedChord>,
    settled: Vec<HotkeyOutcome>,
}

impl Plan {
    fn new(table: &[HotkeyBinding]) -> Self {
        let mut chords: Vec<PlannedChord> = Vec::new();
        let mut settled = Vec::new();
        for binding in table {
            match &binding.trigger {
                ShortcutTrigger::DoubleTap(_) => settled.push(HotkeyOutcome {
                    id: binding.id,
                    state: GlobalShortcutState::Unsupported,
                    detail: None,
                }),
                ShortcutTrigger::Chord(chord) => {
                    let portal_id = binding.trigger.to_string();
                    if let Some(existing) = chords.iter_mut().find(|c| c.portal_id == portal_id) {
                        existing.ids.push(binding.id);
                        continue;
                    }
                    match xdg_trigger(chord) {
                        Some(trigger) => chords.push(PlannedChord {
                            shortcut: NewShortcut::new(portal_id.clone(), &binding.description)
                                .preferred_trigger(trigger.as_str()),
                            portal_id,
                            ids: vec![binding.id],
                        }),
                        None => {
                            let (state, detail) =
                                failed(&format!("no XKB key name for {}", chord.key()));
                            settled.push(HotkeyOutcome {
                                id: binding.id,
                                state,
                                detail,
                            });
                        }
                    }
                }
            }
        }
        Self { chords, settled }
    }

    fn routes(&self) -> HashMap<String, Vec<u32>> {
        self.chords
            .iter()
            .map(|chord| (chord.portal_id.clone(), chord.ids.clone()))
            .collect()
    }

    /// 为每个组合键套用 `state`，与已确定的条目合并成完整结果。
    fn finish(
        self,
        mut state: impl FnMut(&PlannedChord) -> (GlobalShortcutState, Option<String>),
    ) -> Vec<HotkeyOutcome> {
        let mut outcomes = self.settled;
        for chord in &self.chords {
            let (result, detail) = state(chord);
            outcomes.extend(chord.ids.iter().map(|id| HotkeyOutcome {
                id: *id,
                state: result,
                detail: detail.clone(),
            }));
        }
        outcomes
    }
}

fn failed(detail: &str) -> (GlobalShortcutState, Option<String>) {
    (GlobalShortcutState::Failed, Some(detail.to_owned()))
}

/// freedesktop「shortcuts」规范的触发器文本：`CTRL+ALT+SHIFT+LOGO+<XKB 键名>`。
fn xdg_trigger(chord: &ShortcutChord) -> Option<String> {
    let mut parts: Vec<String> = Vec::with_capacity(5);
    if chord.ctrl {
        parts.push("CTRL".to_owned());
    }
    if chord.alt {
        parts.push("ALT".to_owned());
    }
    if chord.shift {
        parts.push("SHIFT".to_owned());
    }
    if chord.super_key {
        parts.push("LOGO".to_owned());
    }
    parts.push(keys::xkb_name(chord.key())?);
    Some(parts.join("+"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(text: &str) -> ShortcutTrigger {
        match text.parse() {
            Ok(trigger) => trigger,
            Err(e) => panic!("invalid trigger text {text}: {e:?}"),
        }
    }

    #[test]
    fn xdg_trigger_follows_the_shortcuts_spec() {
        let ShortcutTrigger::Chord(c) = chord("ctrl-alt-shift-super-d") else {
            panic!("expected a chord");
        };
        assert_eq!(xdg_trigger(&c).as_deref(), Some("CTRL+ALT+SHIFT+LOGO+d"));
        let ShortcutTrigger::Chord(c) = chord("super-enter") else {
            panic!("expected a chord");
        };
        assert_eq!(xdg_trigger(&c).as_deref(), Some("LOGO+Return"));
        let ShortcutTrigger::Chord(c) = chord("ctrl-f5") else {
            panic!("expected a chord");
        };
        assert_eq!(xdg_trigger(&c).as_deref(), Some("CTRL+F5"));
    }

    #[test]
    fn plan_merges_equal_triggers_and_settles_double_taps() {
        let binding = |id, text| HotkeyBinding {
            id,
            trigger: chord(text),
            description: format!("action {id}"),
        };
        let table = vec![
            binding(1, "ctrl-d"),
            binding(2, "double:ctrl"),
            binding(3, "ctrl-d"),
            binding(4, "alt-f1"),
        ];
        let plan = Plan::new(&table);
        assert_eq!(plan.chords.len(), 2);
        assert_eq!(plan.chords[0].portal_id, "ctrl-d");
        assert_eq!(plan.chords[0].ids, [1, 3]);
        assert_eq!(plan.routes().get("alt-f1"), Some(&vec![4]));

        let mut outcomes = plan.finish(|_| (GlobalShortcutState::Active, None));
        outcomes.sort_by_key(|o| o.id);
        let states: Vec<_> = outcomes.iter().map(|o| (o.id, o.state)).collect();
        assert_eq!(
            states,
            [
                (1, GlobalShortcutState::Active),
                (2, GlobalShortcutState::Unsupported),
                (3, GlobalShortcutState::Active),
                (4, GlobalShortcutState::Active),
            ]
        );
    }
}
