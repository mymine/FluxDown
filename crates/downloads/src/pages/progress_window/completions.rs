use super::ProgressWindowView;
use crate::controller::DownloadsController;
use fluxdown_ui_components::ControlExt as _;
use fluxdown_ui_theme::active_theme;
use gpui::{AnyElement, Context, IntoElement as _, ParentElement as _, Styled as _, div};
use gpui_component::{Disableable as _, button::Button, h_flex};

/// 自动完成窗口最多保留的任务数；宿主的合并批次使用同一上限。
pub const COMPLETION_WINDOW_LIMIT: usize = 100;

pub(super) struct CompletedTasks {
    ids: Vec<String>,
    selected: usize,
    total: usize,
}

impl Default for CompletedTasks {
    fn default() -> Self {
        Self {
            ids: Vec::with_capacity(COMPLETION_WINDOW_LIMIT),
            selected: 0,
            total: 0,
        }
    }
}

impl CompletedTasks {
    fn contains(&self, task_id: &str) -> bool {
        self.ids.iter().any(|id| id == task_id)
    }

    fn current(&self) -> Option<&str> {
        self.ids.get(self.selected).map(String::as_str)
    }

    fn append(&mut self, ids: Vec<String>, total: usize, controller: &DownloadsController) {
        self.total = self.total.saturating_add(total);
        for id in ids {
            if self.contains(&id) || !is_completed(controller, &id) {
                self.total = self.total.saturating_sub(1);
                continue;
            }
            if self.ids.len() == COMPLETION_WINDOW_LIMIT {
                // 追加不得夺走当前选择；淘汰最旧的其它项。
                let index = usize::from(self.selected == 0);
                self.remove_index(index);
            }
            self.ids.push(id);
        }
        self.total = self.total.max(self.ids.len());
    }

    fn remove_index(&mut self, index: usize) {
        self.ids.remove(index);
        if index < self.selected {
            self.selected -= 1;
        }
        self.selected = self.selected.min(self.ids.len().saturating_sub(1));
    }

    fn remove(&mut self, task_id: &str) {
        if let Some(index) = self.ids.iter().position(|id| id == task_id) {
            self.remove_index(index);
            self.total = self.total.saturating_sub(1);
        }
    }

    fn reconcile(&mut self, controller: &DownloadsController) {
        for index in (0..self.ids.len()).rev() {
            if !is_completed(controller, &self.ids[index]) {
                self.remove_index(index);
                self.total = self.total.saturating_sub(1);
            }
        }
    }

    fn navigate(&mut self, next: bool) {
        self.selected = if next {
            self.selected
                .saturating_add(1)
                .min(self.ids.len().saturating_sub(1))
        } else {
            self.selected.saturating_sub(1)
        };
    }
}

fn is_completed(controller: &DownloadsController, task_id: &str) -> bool {
    controller
        .task_dto(task_id)
        .is_some_and(|task| task.status == 3)
}

impl ProgressWindowView {
    /// 在 session attach 后进入完成集合模式；内容与单任务完成窗口共用。
    pub fn append_completed(
        &mut self,
        task_ids: Vec<String>,
        total: usize,
        cx: &mut Context<Self>,
    ) {
        self.completed
            .get_or_insert_with(CompletedTasks::default)
            .append(task_ids, total, &self.controller);
        self.refresh(cx);
    }

    #[must_use]
    pub fn contains_completed(&self, task_id: &str) -> bool {
        self.completed
            .as_ref()
            .is_some_and(|tasks| tasks.contains(task_id))
    }

    pub(super) fn refresh_completed(&mut self) -> bool {
        let Some(tasks) = &mut self.completed else {
            return true;
        };
        tasks.reconcile(&self.controller);
        let Some(task_id) = tasks.current() else {
            return false;
        };
        if self.task_id != task_id {
            self.task_id.clear();
            self.task_id.push_str(task_id);
            self.last_error = None;
        }
        true
    }

    /// 删除/打开的回调必须针对发起操作的 ID，不能针对回调时的新选择。
    pub(super) fn finish_completed_action(
        &mut self,
        task_id: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(tasks) = &mut self.completed else {
            return false;
        };
        tasks.remove(task_id);
        if tasks.current().is_none() {
            return false;
        }
        self.refresh(cx);
        true
    }

    fn navigate_completed(&mut self, next: bool, cx: &mut Context<Self>) {
        if let Some(tasks) = &mut self.completed {
            tasks.navigate(next);
            self.last_error = None;
            self.refresh(cx);
        }
    }

    pub(super) fn render_completion_navigation(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let tasks = self.completed.as_ref()?;
        if tasks.ids.len() <= 1 && tasks.total <= 1 {
            return None;
        }
        let theme = active_theme(cx);
        let tokens = theme.tokens();
        let position = self
            .translator
            .read(cx)
            .text("progressWindowCompletionPosition")
            .replace("{current}", &(tasks.selected + 1).to_string())
            .replace("{count}", &tasks.ids.len().to_string())
            .replace("{total}", &tasks.total.to_string());
        Some(
            h_flex()
                .flex_none()
                .flex_wrap()
                .justify_center()
                .gap(tokens.spacing.sm)
                .px(tokens.spacing.lg)
                .pt(tokens.spacing.md)
                .child(
                    Button::new("completion-previous")
                        .outline()
                        .control(cx)
                        .label(self.t(cx, "progressWindowPrevious"))
                        .disabled(tasks.selected == 0)
                        .on_click(cx.listener(|this, _, _, cx| this.navigate_completed(false, cx))),
                )
                .child(div().text_size(tokens.typography.xs.size).child(position))
                .child(
                    Button::new("completion-next")
                        .outline()
                        .control(cx)
                        .label(self.t(cx, "progressWindowNext"))
                        .disabled(tasks.selected + 1 >= tasks.ids.len())
                        .on_click(cx.listener(|this, _, _, cx| this.navigate_completed(true, cx))),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{COMPLETION_WINDOW_LIMIT, CompletedTasks};
    use crate::controller::{
        DownloadsCommand, DownloadsController, DownloadsPort, DownloadsResult, PortFuture,
    };
    use fluxdown_protocol::{AgentEvent, DaemonEvent, ServiceEvent, TaskDto, WsServerMsg};
    use serde_json::json;
    use std::sync::Arc;

    struct NullPort;

    impl DownloadsPort for NullPort {
        fn execute(&self, _: DownloadsCommand) -> PortFuture<DownloadsResult> {
            Box::pin(async { Ok(DownloadsResult::Unit) })
        }
    }

    fn task(id: &str, status: i32) -> TaskDto {
        serde_json::from_value(json!({
            "taskId": id, "url": "https://example.com/file", "fileName": id,
            "saveDir": "/tmp", "status": status, "downloadedBytes": 1, "totalBytes": 1,
            "errorMessage": "", "createdAt": "1", "proxyUrl": "", "queueId": "main", "checksum": ""
        }))
        .expect("task")
    }

    fn controller(ids: &[&str]) -> DownloadsController {
        let mut controller = DownloadsController::new(Arc::new(NullPort));
        snapshot(&mut controller, ids.iter().map(|id| task(id, 3)).collect());
        controller
    }

    fn snapshot(controller: &mut DownloadsController, tasks: Vec<TaskDto>) {
        controller.apply_event(&ServiceEvent::Agent(AgentEvent::Daemon(
            DaemonEvent::Engine(WsServerMsg::TasksSnapshot { tasks }),
        )));
    }

    fn append(tasks: &mut CompletedTasks, ids: &[&str], controller: &DownloadsController) {
        tasks.append(
            ids.iter().map(|id| (*id).to_owned()).collect(),
            ids.len(),
            controller,
        );
    }

    #[test]
    fn append_deduplicates_without_stealing_selection() {
        let controller = controller(&["a", "b", "c"]);
        let mut tasks = CompletedTasks::default();
        append(&mut tasks, &["a"], &controller);
        assert_eq!(tasks.current(), Some("a"));
        append(&mut tasks, &["a", "b", "b"], &controller);
        tasks.navigate(true);
        append(&mut tasks, &["c", "a"], &controller);
        assert_eq!(tasks.current(), Some("b"));
        assert_eq!(tasks.ids, ["a", "b", "c"]);
        assert_eq!(tasks.total, 3);
    }

    #[test]
    fn navigation_clamps_and_removal_selects_neighbor() {
        let controller = controller(&["a", "b", "c"]);
        let mut tasks = CompletedTasks::default();
        append(&mut tasks, &["a", "b", "c"], &controller);
        tasks.navigate(false);
        assert_eq!(tasks.current(), Some("a"));
        tasks.navigate(true);
        tasks.remove("b");
        assert_eq!(tasks.current(), Some("c"));
        tasks.navigate(true);
        assert_eq!(tasks.current(), Some("c"));
        tasks.remove("c");
        assert_eq!(tasks.current(), Some("a"));
        tasks.remove("a");
        assert_eq!(tasks.current(), None);
        assert_eq!(tasks.total, 0);
    }

    #[test]
    fn delayed_action_removes_original_not_new_selection() {
        let controller = controller(&["a", "b", "c"]);
        let mut tasks = CompletedTasks::default();
        append(&mut tasks, &["a", "b", "c"], &controller);
        let action_id = tasks.current().unwrap().to_owned();
        tasks.navigate(true);
        tasks.remove(&action_id);
        assert_eq!(tasks.current(), Some("b"));
        tasks.remove(&action_id); // TaskDeleted may arrive before command completion.
        assert_eq!(tasks.current(), Some("b"));
        assert_eq!(tasks.total, 2);
    }

    #[test]
    fn overflow_is_bounded_and_preserves_current_item() {
        let mut controller = controller(&["selected"]);
        let mut tasks = CompletedTasks::default();
        append(&mut tasks, &["selected"], &controller);
        let ids: Vec<_> = (0..200).map(|index| index.to_string()).collect();
        let mut rows: Vec<_> = ids.iter().map(|id| task(id, 3)).collect();
        rows.push(task("selected", 3));
        snapshot(&mut controller, rows);
        tasks.append(ids, 200, &controller);
        assert_eq!(tasks.ids.len(), COMPLETION_WINDOW_LIMIT);
        assert_eq!(tasks.current(), Some("selected"));
        assert_eq!(tasks.total, 201);
        assert_eq!(tasks.ids.last().map(String::as_str), Some("199"));
    }

    #[test]
    fn snapshots_filter_delayed_restarted_and_deleted_completions() {
        let mut controller = controller(&["a", "b", "c"]);
        let mut tasks = CompletedTasks::default();
        append(&mut tasks, &["a", "b", "c"], &controller);
        snapshot(&mut controller, vec![task("a", 1), task("c", 3)]);
        tasks.reconcile(&controller);
        assert_eq!(tasks.current(), Some("c"));
        assert_eq!(tasks.ids, ["c"]);
        append(&mut tasks, &["a", "b"], &controller);
        assert_eq!(tasks.ids, ["c"]);
        assert_eq!(tasks.total, 1);
        snapshot(&mut controller, Vec::new());
        tasks.reconcile(&controller);
        assert_eq!(tasks.current(), None);
    }

    #[test]
    fn deletion_event_and_late_action_keep_remaining_selection() {
        let mut controller = controller(&["a", "b", "c"]);
        let mut tasks = CompletedTasks::default();
        append(&mut tasks, &["a", "b", "c"], &controller);
        controller.apply_event(&ServiceEvent::Agent(AgentEvent::Daemon(
            DaemonEvent::TaskDeleted {
                task_id: "a".to_owned(),
            },
        )));
        tasks.reconcile(&controller);
        assert_eq!(tasks.current(), Some("b"));
        tasks.navigate(true);
        tasks.remove("a");
        assert_eq!(tasks.current(), Some("c"));
        assert_eq!(tasks.ids, ["b", "c"]);
        assert_eq!(tasks.total, 2);
    }
}
