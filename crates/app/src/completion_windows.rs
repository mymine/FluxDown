//! One automatic completion window, a leading-edge batch timer and bounded memory.

use std::{
    collections::{HashSet, VecDeque},
    time::{Duration, Instant},
};

use fluxdown_ui_downloads::{COMPLETION_WINDOW_LIMIT, ProgressWindowPrefs, ProgressWindowView};
use futures_util::future::{Either, select};
use gpui::{App, Entity, Global};
use tokio::sync::oneshot;

use crate::{
    app::Desktop,
    session::SessionSignal,
    windows::{WindowKey, WindowRegistry},
};

const BATCH_DELAY: Duration = Duration::from_millis(500);
const CLOSE_COOLDOWN: Duration = Duration::from_secs(5);
const RECENT_LIMIT: usize = 4096;

#[derive(Default)]
struct BatchModel {
    enabled: bool,
    pending: VecDeque<String>,
    count: usize,
    recent: HashSet<String>,
    recent_order: VecDeque<String>,
    deadline: Option<Instant>,
    cooldown: Option<Instant>,
    /// The open window is handing a file to the system and closes once that settles.
    closing: bool,
}

impl BatchModel {
    fn enqueue(&mut self, task_id: String, now: Instant) -> bool {
        if !self.enabled || self.recent.contains(&task_id) {
            return false;
        }
        if self.recent_order.len() == RECENT_LIMIT
            && let Some(old) = self.recent_order.pop_front()
        {
            self.recent.remove(&old);
        }
        self.recent.insert(task_id.clone());
        self.recent_order.push_back(task_id.clone());
        if self.pending.len() == COMPLETION_WINDOW_LIMIT {
            self.pending.pop_front();
        }
        self.pending.push_back(task_id);
        self.count = self.count.saturating_add(1);
        if self.deadline.is_none() {
            self.deadline = Some(
                self.cooldown
                    .map_or(now + BATCH_DELAY, |until| until.max(now + BATCH_DELAY)),
            );
        }
        true
    }

    fn take_due(&mut self, now: Instant) -> Option<(VecDeque<String>, usize)> {
        if !self.enabled || self.deadline.is_none_or(|due| due > now) {
            return None;
        }
        if self.closing {
            // Never append to a view that is about to close: the batch waits for the close (and
            // its cooldown) or for the handoff to end with the window kept open.
            self.deadline = Some(now + CLOSE_COOLDOWN);
            return None;
        }
        self.deadline = None;
        Some((
            std::mem::take(&mut self.pending),
            std::mem::take(&mut self.count),
        ))
    }

    /// A file handoff started or succeeded: the window closes once the opened program takes over.
    /// Due batches are held back meanwhile; the cooldown starts at the actual close.
    fn closing(&mut self) {
        self.closing = true;
    }

    /// The window was removed.
    fn closed(&mut self, now: Instant) {
        self.closing = false;
        let until = now + CLOSE_COOLDOWN;
        self.cooldown = Some(until);
        if let Some(due) = self.deadline.as_mut() {
            *due = (*due).max(until);
        }
    }

    /// The handoff failed or more completions remain: the window stays, and batches held back
    /// while it was closing become due as if they had just been enqueued.
    fn kept(&mut self, now: Instant) {
        self.closing = false;
        let earliest = self
            .cooldown
            .map_or(now + BATCH_DELAY, |until| until.max(now + BATCH_DELAY));
        if let Some(due) = self.deadline.as_mut() {
            *due = (*due).min(earliest);
        }
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.pending.clear();
            self.count = 0;
            self.deadline = None;
            self.recent.clear();
            self.recent_order.clear();
        }
    }

    fn forget(&mut self, task_id: &str) {
        self.recent.remove(task_id);
        self.recent_order.retain(|id| id != task_id);
        let before = self.pending.len();
        self.pending.retain(|id| id != task_id);
        self.count = self.count.saturating_sub(before - self.pending.len());
        if self.pending.is_empty() {
            self.count = 0;
            self.deadline = None;
        }
    }
}

#[derive(Default)]
struct CompletionWindows {
    model: BatchModel,
    view: Option<Entity<ProgressWindowView>>,
    cancel: Option<oneshot::Sender<()>>,
    generation: u64,
}

impl Global for CompletionWindows {}

pub(crate) fn install(cx: &mut App) {
    let enabled = enabled(cx);
    let mut state = CompletionWindows::default();
    state.model.set_enabled(enabled);
    cx.set_global(state);
    let desktop = Desktop::global(cx);
    let settings = desktop.settings_store.clone();
    let session = desktop.session.clone();
    cx.observe(&settings, |settings, cx| {
        let enabled =
            ProgressWindowPrefs::from_preferences(settings.read(cx).preferences()).completion;
        cx.global_mut::<CompletionWindows>()
            .model
            .set_enabled(enabled);
        if !enabled {
            cancel_pending(cx);
            WindowRegistry::close(cx, &WindowKey::Completion);
        }
    })
    .detach();
    cx.subscribe(&session, |_, signal, cx| {
        if matches!(signal, SessionSignal::ServiceStopped) {
            shutdown(cx);
        }
    })
    .detach();
    cx.on_app_quit(|cx| {
        shutdown(cx);
        async {}
    })
    .detach();
}

fn enabled(cx: &App) -> bool {
    !Desktop::global(cx).quitting
        && ProgressWindowPrefs::from_preferences(Desktop::preferences(cx)).completion
}

pub(crate) fn enqueue(task_id: String, cx: &mut App) {
    if !enabled(cx) || already_presented(&task_id, cx) {
        return;
    }
    if cx
        .global_mut::<CompletionWindows>()
        .model
        .enqueue(task_id, Instant::now())
    {
        schedule(cx);
    }
}

fn already_presented(task_id: &str, cx: &App) -> bool {
    WindowRegistry::is_open(cx, &WindowKey::Progress(task_id.to_owned()))
        || cx
            .global::<CompletionWindows>()
            .view
            .as_ref()
            .is_some_and(|view| view.read(cx).contains_completed(task_id))
}

/// Explicitly beginning another run can notify again; replayed start events must not call this.
pub(crate) fn forget(task_id: &str, cx: &mut App) {
    cx.global_mut::<CompletionWindows>().model.forget(task_id);
    if cx.global::<CompletionWindows>().model.deadline.is_none() {
        cancel_pending(cx);
    }
}

/// A file handoff started (or succeeded): stop appending to the view until it closes or
/// [`window_kept`] reports that it stays.
pub(crate) fn window_closing(cx: &mut App) {
    cx.global_mut::<CompletionWindows>().model.closing();
}

/// The handoff failed or the view moved on to another completion: it keeps receiving batches.
pub(crate) fn window_kept(cx: &mut App) {
    cx.global_mut::<CompletionWindows>()
        .model
        .kept(Instant::now());
    // The deadline may have moved earlier than the running timer sleeps: restart it.
    cancel_pending(cx);
    schedule(cx);
}

pub(crate) fn window_closed(cx: &mut App) {
    let state = cx.global_mut::<CompletionWindows>();
    state.view = None;
    state.model.closed(Instant::now());
    schedule(cx);
}

fn cancel_pending(cx: &mut App) {
    let state = cx.global_mut::<CompletionWindows>();
    state.generation = state.generation.wrapping_add(1);
    if let Some(cancel) = state.cancel.take()
        && cancel.send(()).is_err()
    {
        log::trace!("completion batch timer already finished");
    }
}

fn shutdown(cx: &mut App) {
    let state = cx.global_mut::<CompletionWindows>();
    state.model.set_enabled(false);
    state.model.kept(Instant::now());
    cancel_pending(cx);
    cx.global_mut::<CompletionWindows>().view = None;
}

fn schedule(cx: &mut App) {
    let state = cx.global_mut::<CompletionWindows>();
    if state.cancel.is_some() || state.model.deadline.is_none() {
        return;
    }
    let (cancel, cancelled) = oneshot::channel();
    state.cancel = Some(cancel);
    let generation = state.generation;
    // Exactly one in-flight task and keepalive for the whole batch/cooldown, never per row.
    let work = cx.spawn(async move |cx| {
        let mut cancelled = Box::pin(cancelled);
        loop {
            let deadline = cx.update(|cx| {
                let state = cx.global::<CompletionWindows>();
                (state.generation == generation)
                    .then_some(state.model.deadline)
                    .flatten()
            });
            let Some(deadline) = deadline else {
                break;
            };
            let timer = Box::pin(
                cx.background_executor()
                    .timer(deadline.saturating_duration_since(Instant::now())),
            );
            match select(cancelled.as_mut(), timer).await {
                Either::Left((result, _)) => {
                    if result.is_err() {
                        log::trace!("completion timer owner released");
                    }
                    break;
                }
                Either::Right(((), _)) => cx.update(|cx| flush(generation, cx)),
            }
        }
        cx.update(|cx| {
            let state = cx.global_mut::<CompletionWindows>();
            if state.generation == generation {
                state.cancel = None;
            }
        });
    });
    crate::lifecycle::keep_alive(cx, work).detach();
}

fn flush(generation: u64, cx: &mut App) {
    if !enabled(cx) {
        shutdown(cx);
        return;
    }
    let state = cx.global_mut::<CompletionWindows>();
    if state.generation != generation {
        return;
    }
    let Some((ids, count)) = state.model.take_due(Instant::now()) else {
        return;
    };
    let session = Desktop::global(cx).session.clone();
    let wanted: HashSet<&str> = ids.iter().map(String::as_str).collect();
    // Borrow only the current status/ID: all names, paths and actions come from the existing view.
    let completed: HashSet<&str> = session
        .read(cx)
        .agent_snapshot()
        .filter(|body| body.daemon_connected)
        .map(|body| {
            body.daemon
                .tasks
                .iter()
                .filter(|task| task.status == 3 && wanted.contains(task.task_id.as_str()))
                .map(|task| task.task_id.as_str())
                .collect()
        })
        .unwrap_or_default();
    let (ids, count) = ready_batch(ids, count, |id| {
        completed.contains(id) && !already_presented(id, cx)
    });
    if ids.is_empty() {
        return;
    }
    if let Some(view) = cx.global::<CompletionWindows>().view.clone() {
        view.update(cx, |view, cx| view.append_completed(ids, count, cx));
    } else if let Some(view) = crate::windows::progress::open_completion(cx, ids, count) {
        cx.global_mut::<CompletionWindows>().view = Some(view);
    }
}

fn ready_batch(
    mut ids: VecDeque<String>,
    count: usize,
    mut eligible: impl FnMut(&str) -> bool,
) -> (Vec<String>, usize) {
    let before = ids.len();
    ids.retain(|id| eligible(id));
    let count = count.saturating_sub(before - ids.len());
    (ids.into(), count)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{
        BATCH_DELAY, BatchModel, CLOSE_COOLDOWN, COMPLETION_WINDOW_LIMIT, RECENT_LIMIT, ready_batch,
    };

    fn enabled_model() -> BatchModel {
        BatchModel {
            enabled: true,
            ..BatchModel::default()
        }
    }

    #[test]
    fn thousand_completions_share_one_leading_edge_batch_and_one_hundred_rows() {
        let now = Instant::now();
        let mut model = enabled_model();
        for index in 0..1000 {
            assert!(model.enqueue(format!("task-{index}"), now + Duration::from_micros(index)));
        }
        assert_eq!(model.deadline, Some(now + BATCH_DELAY));
        assert!(
            model
                .take_due(now + BATCH_DELAY - Duration::from_millis(1))
                .is_none()
        );
        let (rows, count) = model.take_due(now + BATCH_DELAY).expect("batch due");
        assert_eq!(count, 1000);
        assert_eq!(rows.len(), COMPLETION_WINDOW_LIMIT);
        assert_eq!(rows.front().map(String::as_str), Some("task-900"));
        assert!(model.deadline.is_none());
    }

    #[test]
    fn duplicates_do_not_extend_deadline_or_inflate_count_even_after_flush() {
        let now = Instant::now();
        let mut model = enabled_model();
        assert!(model.enqueue("a".into(), now));
        assert!(!model.enqueue("a".into(), now + BATCH_DELAY));
        assert_eq!(model.take_due(now + BATCH_DELAY).expect("due").1, 1);
        assert!(!model.enqueue("a".into(), now + CLOSE_COOLDOWN));
        model.forget("a");
        assert!(model.enqueue("a".into(), now + CLOSE_COOLDOWN));
    }

    #[test]
    fn closing_extends_pending_deadline_without_discarding_completions() {
        let now = Instant::now();
        let mut model = enabled_model();
        model.enqueue("a".into(), now);
        model.closed(now + Duration::from_millis(100));
        let due = now + Duration::from_millis(100) + CLOSE_COOLDOWN;
        model.enqueue("b".into(), now + Duration::from_secs(1));
        assert_eq!(model.deadline, Some(due));
        assert!(model.take_due(due - Duration::from_millis(1)).is_none());
        assert_eq!(model.take_due(due).expect("cooldown over").1, 2);
    }

    #[test]
    fn closing_empty_window_applies_cooldown_to_the_next_batch() {
        let now = Instant::now();
        let mut model = enabled_model();
        model.closed(now);
        model.enqueue("a".into(), now + Duration::from_millis(1));
        assert_eq!(model.deadline, Some(now + CLOSE_COOLDOWN));
    }

    #[test]
    fn disabling_cancels_queued_completions_and_does_not_replay_them() {
        let now = Instant::now();
        let mut model = enabled_model();
        model.enqueue("a".into(), now);
        model.set_enabled(false);
        assert!(model.pending.is_empty());
        assert_eq!(model.count, 0);
        assert!(!model.enqueue("b".into(), now));
        model.set_enabled(true);
        assert!(model.take_due(now + CLOSE_COOLDOWN).is_none());
    }

    #[test]
    fn deduplication_storage_is_bounded() {
        let now = Instant::now();
        let mut model = enabled_model();
        for index in 0..RECENT_LIMIT * 2 {
            model.enqueue(index.to_string(), now);
        }
        assert_eq!(model.pending.len(), COMPLETION_WINDOW_LIMIT);
        assert_eq!(model.recent.len(), RECENT_LIMIT);
        assert_eq!(model.recent_order.len(), RECENT_LIMIT);
    }

    #[test]
    fn file_handoff_holds_the_next_batch_until_after_actual_close() {
        let now = Instant::now();
        let mut model = enabled_model();
        model.closing();
        model.enqueue("next".into(), now + Duration::from_millis(100));
        let first_due = now + Duration::from_millis(100) + BATCH_DELAY;
        assert_eq!(model.deadline, Some(first_due));
        assert!(model.take_due(first_due).is_none());
        let actual_close = now + Duration::from_millis(1500);
        model.closed(actual_close);
        let due = actual_close + CLOSE_COOLDOWN;
        assert_eq!(model.deadline, Some(due));
        assert!(model.take_due(due - Duration::from_millis(1)).is_none());
        assert_eq!(model.take_due(due).expect("handoff cooldown ended").1, 1);
    }

    #[test]
    fn a_long_handoff_never_feeds_the_closing_window() {
        let now = Instant::now();
        let mut model = enabled_model();
        model.closing();
        model.enqueue("late".into(), now);
        let mut due = now + BATCH_DELAY;
        // Still handing off (e.g. waiting on an elevation prompt): keep deferring, never append.
        for _ in 0..3 {
            assert!(model.take_due(due).is_none());
            due += CLOSE_COOLDOWN;
            assert_eq!(model.deadline, Some(due));
        }
        let actual_close = due + Duration::from_secs(1);
        model.closed(actual_close);
        let due = actual_close + CLOSE_COOLDOWN;
        assert_eq!(model.deadline, Some(due));
        assert_eq!(model.take_due(due).expect("next window").1, 1);
    }

    #[test]
    fn a_window_kept_after_a_failed_handoff_gets_held_batches_promptly() {
        let now = Instant::now();
        let mut model = enabled_model();
        model.closing();
        model.enqueue("late".into(), now);
        assert!(model.take_due(now + BATCH_DELAY).is_none());
        let failed = now + Duration::from_secs(1);
        model.kept(failed);
        let due = failed + BATCH_DELAY;
        assert_eq!(model.deadline, Some(due));
        let (ids, count) = model.take_due(due).expect("appended to the kept window");
        assert_eq!(ids, ["late"]);
        assert_eq!(count, 1);
        // A batch that was not held back keeps its own earlier deadline.
        model.closing();
        model.enqueue("fresh".into(), failed + Duration::from_secs(2));
        model.kept(failed + Duration::from_secs(2) + Duration::from_millis(100));
        assert_eq!(
            model.deadline,
            Some(failed + Duration::from_secs(2) + BATCH_DELAY)
        );
    }

    #[test]
    fn restart_removes_old_pending_completion_without_counting_it_twice() {
        let now = Instant::now();
        let mut model = enabled_model();
        model.enqueue("restarted".into(), now);
        model.enqueue("other".into(), now);
        model.closed(now);
        model.forget("restarted");
        assert_eq!(model.count, 1);
        assert_eq!(
            model.pending.iter().map(String::as_str).collect::<Vec<_>>(),
            ["other"]
        );
        assert_eq!(model.deadline, Some(now + CLOSE_COOLDOWN));
        assert!(model.enqueue("restarted".into(), now + Duration::from_secs(1)));
        let (ids, count) = model
            .take_due(now + CLOSE_COOLDOWN)
            .expect("new run completed");
        assert_eq!(
            ids.iter().map(String::as_str).collect::<Vec<_>>(),
            ["other", "restarted"]
        );
        assert_eq!(count, 2);
    }

    #[test]
    fn restarting_last_pending_task_cancels_old_deadline_but_keeps_close_cooldown() {
        let now = Instant::now();
        let mut model = enabled_model();
        model.closed(now);
        model.enqueue("task".into(), now);
        model.forget("task");
        assert!(model.pending.is_empty());
        assert_eq!(model.count, 0);
        assert!(model.deadline.is_none());
        assert!(model.take_due(now + CLOSE_COOLDOWN).is_none());
        model.enqueue("task".into(), now + Duration::from_secs(1));
        assert_eq!(model.deadline, Some(now + CLOSE_COOLDOWN));
        assert_eq!(
            model
                .take_due(now + CLOSE_COOLDOWN)
                .expect("fresh completion")
                .1,
            1
        );
    }

    #[test]
    fn restarting_retained_task_corrects_overflow_count() {
        let now = Instant::now();
        let mut model = enabled_model();
        for index in 0..1000 {
            model.enqueue(format!("task-{index}"), now);
        }
        model.forget("task-999");
        let (ids, count) = model.take_due(now + BATCH_DELAY).expect("batch due");
        assert_eq!(ids.len(), COMPLETION_WINDOW_LIMIT - 1);
        assert_eq!(count, 999);
        assert!(!ids.iter().any(|id| id == "task-999"));
    }

    #[test]
    fn flush_rechecks_status_progress_windows_and_already_shown_ids() {
        let ids = ["ready", "deleted", "restarted", "progress-open", "shown"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        let current = [
            ("ready", 3),
            ("restarted", 1),
            ("progress-open", 3),
            ("shown", 3),
        ];
        let (ids, count) = ready_batch(ids, 5, |id| {
            current
                .iter()
                .any(|(task, status)| *task == id && *status == 3)
                && id != "progress-open"
                && id != "shown"
        });
        assert_eq!(ids, ["ready"]);
        assert_eq!(count, 1);
    }

    #[test]
    fn filtered_batches_preserve_overflow_count_but_cannot_open_an_empty_window() {
        let (ids, count) = ready_batch(
            ["ready", "progress-open"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            1000,
            |id| id == "ready",
        );
        assert_eq!(ids, ["ready"]);
        assert_eq!(count, 999);
        let (ids, _) = ready_batch(ids.into(), count, |_| false);
        assert!(ids.is_empty());
    }
}
