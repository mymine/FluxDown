//! 独立下载进度 / 完成窗口的开关决策（纯状态机，不碰窗口与会话）。
//!
//! 只为**用户在场时亲手开始**的任务开窗：新建下载表单提交的单个任务、单任务「继续」、
//! 「重新下载」。新任务成功后登记，已有任务须在命令前捕获状态、成功后登记；
//! 队列调度、启动自动恢复、RSS、批量操作都不登记，因此不开窗。
//!
//! 窗口生命周期：任务进入活跃态开进度窗口；完成时已开的窗口按当前全局设置切换为完成视图
//! 或关闭，用户中途关掉进度窗口的任务完成时仍按该设置弹一次不抢焦点的完成窗口。

use std::collections::{BTreeMap, HashSet, VecDeque};

use serde_json::Value;

/// 设备本地偏好：用户开始下载时弹出进度窗口（缺省开启）。
pub const PROGRESS_WINDOW_PREF: &str = "desktop.progress_window";
/// 设备本地偏好：用户开始的下载完成时弹出完成窗口（缺省开启）。
pub const COMPLETION_WINDOW_PREF: &str = "desktop.completion_window";

/// daemon 任务状态码（`TaskDto::status`）。
const STATUS_DOWNLOADING: i32 = 1;
const STATUS_PAUSED: i32 = 2;
const STATUS_COMPLETED: i32 = 3;
const STATUS_ERROR: i32 = 4;
const STATUS_PREPARING: i32 = 5;

/// 两个窗口开关的当前值。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProgressWindowPrefs {
    pub progress: bool,
    pub completion: bool,
}

impl Default for ProgressWindowPrefs {
    fn default() -> Self {
        Self {
            progress: true,
            completion: true,
        }
    }
}

impl ProgressWindowPrefs {
    /// 从 agent 偏好读取；缺省或类型不符按开启处理。
    #[must_use]
    pub fn from_preferences(values: &BTreeMap<String, Value>) -> Self {
        let flag = |key: &str| values.get(key).and_then(Value::as_bool).unwrap_or(true);
        Self {
            progress: flag(PROGRESS_WINDOW_PREF),
            completion: flag(COMPLETION_WINDOW_PREF),
        }
    }
}

/// 交互式开始的生命周期。已有任务必须先登记请求，不能把命令前的完成快照当成新完成。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UserStartIntent {
    Created(String),
    RestartRequested {
        task_id: String,
        replaces_task: bool,
    },
    RestartSucceeded(String),
    RestartFailed(String),
}

/// 自动提醒最多同时跟踪的有效开始意图；已消费 ID 另有同样大小的重放去重集合。
pub const MAX_TRACKED_WINDOW_TASKS: usize = 4096;

/// [`ProgressWindowTracker::observe`] 要求宿主执行的窗口操作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgressWindowEffect {
    /// 打开进度窗口并激活（用户刚发起的动作）。
    OpenProgress,
    /// 打开完成窗口，不抢焦点（后台完成）。
    OpenCompletion,
    /// 关闭该任务已打开的窗口。
    Close,
}

#[derive(Debug, Default)]
pub struct ProgressWindowTracker {
    armed: HashSet<String>,
    user_started: HashSet<String>,
    consumed: HashSet<String>,
    consumed_order: VecDeque<String>,
}

impl ProgressWindowTracker {
    /// 新建 / agent 重放使用幂等登记。返回 false 表示已登记、已消费或到达跟踪上限。
    pub fn arm_created(&mut self, task_id: &str) -> bool {
        if self.armed.contains(task_id)
            || self.user_started.contains(task_id)
            || self.consumed.contains(task_id)
            || self.tracked_count() >= MAX_TRACKED_WINDOW_TASKS
        {
            return false;
        }
        self.armed.insert(task_id.to_owned())
    }

    /// 真正的再次开始不受新建事件去重约束；宿主只可用命令后的新状态补判。
    pub fn arm_restart(&mut self, task_id: &str) -> bool {
        self.armed.remove(task_id);
        self.user_started.remove(task_id);
        if self.consumed.remove(task_id) {
            self.consumed_order.retain(|id| id != task_id);
        }
        self.arm_created(task_id)
    }

    #[must_use]
    pub fn tracked_count(&self) -> usize {
        self.armed.len() + self.user_started.len()
    }

    #[must_use]
    pub fn is_armed(&self, task_id: &str) -> bool {
        self.armed.contains(task_id)
    }

    /// 活跃但未开进度窗的完成意图仍须保活，不能仅检查 armed。
    #[must_use]
    pub fn needs_keep_alive(&self, prefs: ProgressWindowPrefs) -> bool {
        ((prefs.progress || prefs.completion) && !self.armed.is_empty())
            || (prefs.completion && !self.user_started.is_empty())
    }

    fn consume(&mut self, task_id: &str) {
        if self.consumed.contains(task_id) {
            return;
        }
        let task_id = self
            .armed
            .take(task_id)
            .or_else(|| self.user_started.take(task_id))
            .unwrap_or_else(|| task_id.to_owned());
        if self.consumed.insert(task_id.clone()) {
            self.consumed_order.push_back(task_id);
            if self.consumed_order.len() > MAX_TRACKED_WINDOW_TASKS
                && let Some(oldest) = self.consumed_order.pop_front()
            {
                self.consumed.remove(&oldest);
            }
        }
    }

    /// 删除 / 取消后保留有界去重记录，晚到的启动重放不能复活意图。
    pub fn forget(&mut self, task_id: &str) {
        self.consume(task_id);
    }

    /// 快照替换时丢弃已不存在的任务；增量进度不调用此全量核对。
    pub fn retain_existing(&mut self, mut exists: impl FnMut(&str) -> bool) {
        let missing: Vec<String> = self
            .armed
            .iter()
            .chain(&self.user_started)
            .filter(|task_id| !exists(task_id))
            .cloned()
            .collect();
        for task_id in missing {
            self.consume(&task_id);
        }
    }

    /// 两个开关全关时清理有效意图；重新开启不补弹旧的完成。
    pub fn update_prefs(&mut self, prefs: ProgressWindowPrefs) {
        if !prefs.progress && !prefs.completion {
            let pending: Vec<String> = self
                .armed
                .iter()
                .chain(&self.user_started)
                .cloned()
                .collect();
            for task_id in pending {
                self.consume(&task_id);
            }
        }
    }

    /// 服务停止 / 应用退出时释放所有记录。
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// 任务状态更新。`window_open` = 该任务当前是否有进度 / 完成窗口。
    pub fn observe(
        &mut self,
        task_id: &str,
        status: i32,
        window_open: bool,
        prefs: ProgressWindowPrefs,
    ) -> Option<ProgressWindowEffect> {
        self.update_prefs(prefs);
        match status {
            STATUS_DOWNLOADING | STATUS_PREPARING => {
                if !self.armed.remove(task_id) {
                    return None;
                }
                self.user_started.insert(task_id.to_owned());
                (prefs.progress && !window_open).then_some(ProgressWindowEffect::OpenProgress)
            }
            STATUS_COMPLETED => {
                let started = self.armed.contains(task_id) || self.user_started.contains(task_id);
                if started {
                    self.consume(task_id);
                }
                if window_open {
                    (!prefs.completion).then_some(ProgressWindowEffect::Close)
                } else {
                    (started && prefs.completion).then_some(ProgressWindowEffect::OpenCompletion)
                }
            }
            STATUS_PAUSED | STATUS_ERROR => {
                if self.armed.contains(task_id) || self.user_started.contains(task_id) {
                    self.consume(task_id);
                }
                None
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ON: ProgressWindowPrefs = ProgressWindowPrefs {
        progress: true,
        completion: true,
    };

    #[test]
    fn unarmed_tasks_never_open_windows() {
        let mut tracker = ProgressWindowTracker::default();
        assert_eq!(tracker.observe("t", 1, false, ON), None);
        assert_eq!(tracker.observe("t", 3, false, ON), None);
    }

    #[test]
    fn armed_task_opens_once_when_it_becomes_active() {
        let mut tracker = ProgressWindowTracker::default();
        tracker.arm_created("t");
        assert_eq!(tracker.observe("t", 0, false, ON), None, "排队中不开窗");
        assert!(tracker.is_armed("t"), "排队期间意图保留，宿主据此保持界面");
        assert_eq!(
            tracker.observe("t", 5, false, ON),
            Some(ProgressWindowEffect::OpenProgress)
        );
        assert!(!tracker.is_armed("t"));
        assert_eq!(tracker.observe("t", 1, true, ON), None);
    }

    #[test]
    fn open_window_turns_into_completion_view_or_closes() {
        let mut tracker = ProgressWindowTracker::default();
        tracker.arm_created("a");
        tracker.observe("a", 1, false, ON);
        assert_eq!(
            tracker.observe("a", 3, true, ON),
            None,
            "保留窗口切完成视图"
        );

        tracker.arm_created("b");
        tracker.observe("b", 1, false, ON);
        let completion_off = ProgressWindowPrefs {
            completion: false,
            ..ON
        };
        assert_eq!(
            tracker.observe("b", 3, true, completion_off),
            Some(ProgressWindowEffect::Close)
        );
    }

    #[test]
    fn closed_progress_window_still_gets_one_completion_window() {
        let mut tracker = ProgressWindowTracker::default();
        tracker.arm_created("t");
        tracker.observe("t", 1, false, ON);
        assert_eq!(
            tracker.observe("t", 3, false, ON),
            Some(ProgressWindowEffect::OpenCompletion)
        );
        assert_eq!(tracker.observe("t", 3, false, ON), None, "只弹一次");
    }

    #[test]
    fn prefs_gate_each_window_independently() {
        let progress_off = ProgressWindowPrefs {
            progress: false,
            completion: true,
        };
        let mut tracker = ProgressWindowTracker::default();
        tracker.arm_created("t");
        assert_eq!(tracker.observe("t", 1, false, progress_off), None);
        assert_eq!(
            tracker.observe("t", 3, false, progress_off),
            Some(ProgressWindowEffect::OpenCompletion)
        );

        let completion_off = ProgressWindowPrefs {
            progress: true,
            completion: false,
        };
        tracker.arm_created("u");
        tracker.observe("u", 1, false, completion_off);
        assert_eq!(tracker.observe("u", 3, false, completion_off), None);
    }

    #[test]
    fn instant_completion_counts_as_user_started() {
        let mut tracker = ProgressWindowTracker::default();
        tracker.arm_created("t");
        assert_eq!(
            tracker.observe("t", 3, false, ON),
            Some(ProgressWindowEffect::OpenCompletion)
        );
    }

    #[test]
    fn failure_before_start_drops_the_intent() {
        let mut tracker = ProgressWindowTracker::default();
        tracker.arm_created("t");
        tracker.observe("t", 4, false, ON);
        assert_eq!(tracker.observe("t", 1, false, ON), None);
    }

    #[test]
    fn changing_completion_preference_applies_to_all_started_and_future_tasks() {
        let mut tracker = ProgressWindowTracker::default();
        for task in ["open", "closed"] {
            tracker.arm_created(task);
            tracker.observe(task, STATUS_DOWNLOADING, false, ON);
        }
        let off = ProgressWindowPrefs {
            completion: false,
            ..ON
        };
        assert_eq!(
            tracker.observe("open", STATUS_COMPLETED, true, off),
            Some(ProgressWindowEffect::Close),
        );
        assert_eq!(
            tracker.observe("closed", STATUS_COMPLETED, false, off),
            None
        );
        tracker.arm_created("future");
        tracker.observe("future", STATUS_DOWNLOADING, false, off);
        assert_eq!(
            tracker.observe("future", STATUS_COMPLETED, false, off),
            None
        );
        tracker.arm_created("reenabled");
        tracker.observe("reenabled", STATUS_DOWNLOADING, false, off);
        assert_eq!(
            tracker.observe("reenabled", STATUS_COMPLETED, false, ON),
            Some(ProgressWindowEffect::OpenCompletion),
        );
    }

    #[test]
    fn completion_only_keeps_the_ui_until_a_terminal_state() {
        let only_completion = ProgressWindowPrefs {
            progress: false,
            completion: true,
        };
        let mut tracker = ProgressWindowTracker::default();
        tracker.arm_created("fast");
        assert!(tracker.needs_keep_alive(only_completion));
        assert_eq!(
            tracker.observe("fast", STATUS_DOWNLOADING, false, only_completion),
            None
        );
        assert!(!tracker.is_armed("fast"));
        assert!(tracker.needs_keep_alive(only_completion));
        assert_eq!(
            tracker.observe("fast", STATUS_COMPLETED, false, only_completion),
            Some(ProgressWindowEffect::OpenCompletion)
        );
        assert!(!tracker.needs_keep_alive(only_completion));
    }

    #[test]
    fn launch_and_capture_replay_are_idempotent_but_real_restart_is_not() {
        let mut tracker = ProgressWindowTracker::default();
        assert!(tracker.arm_created("t"));
        assert!(!tracker.arm_created("t"));
        assert_eq!(
            tracker.observe("t", STATUS_COMPLETED, false, ON),
            Some(ProgressWindowEffect::OpenCompletion)
        );
        assert!(!tracker.arm_created("t"), "完成后的重放不能复活意图");
        assert_eq!(tracker.observe("t", STATUS_COMPLETED, false, ON), None);
        assert!(tracker.arm_restart("t"));
        assert_eq!(
            tracker.observe("t", STATUS_COMPLETED, false, ON),
            Some(ProgressWindowEffect::OpenCompletion)
        );
    }

    #[test]
    fn pause_failure_deletion_and_disabling_both_release_completion_wait() {
        for status in [STATUS_PAUSED, STATUS_ERROR] {
            let mut tracker = ProgressWindowTracker::default();
            tracker.arm_created("t");
            tracker.observe("t", STATUS_DOWNLOADING, false, ON);
            tracker.observe("t", status, false, ON);
            assert!(!tracker.needs_keep_alive(ON));
            assert_eq!(tracker.observe("t", STATUS_COMPLETED, false, ON), None);
        }
        let mut tracker = ProgressWindowTracker::default();
        tracker.arm_created("deleted");
        tracker.forget("deleted");
        assert!(!tracker.needs_keep_alive(ON));
        assert!(!tracker.arm_created("deleted"));
        tracker.forget("deleted-before-start-event");
        assert!(!tracker.arm_created("deleted-before-start-event"));
        tracker.arm_created("disabled");
        tracker.update_prefs(ProgressWindowPrefs {
            progress: false,
            completion: false,
        });
        assert!(!tracker.needs_keep_alive(ON));
        assert_eq!(
            tracker.observe("disabled", STATUS_COMPLETED, false, ON),
            None
        );
        tracker.arm_created("snapshot-missing");
        tracker.retain_existing(|_| false);
        assert!(!tracker.needs_keep_alive(ON));
    }

    #[test]
    fn burst_tracking_and_replay_memory_are_bounded() {
        let mut tracker = ProgressWindowTracker::default();
        for index in 0..MAX_TRACKED_WINDOW_TASKS {
            assert!(tracker.arm_created(&index.to_string()));
        }
        assert!(!tracker.arm_created("overflow"));
        assert_eq!(tracker.tracked_count(), MAX_TRACKED_WINDOW_TASKS);
        for index in 0..MAX_TRACKED_WINDOW_TASKS {
            assert_eq!(
                tracker.observe(&index.to_string(), STATUS_COMPLETED, false, ON),
                Some(ProgressWindowEffect::OpenCompletion)
            );
        }
        assert!(!tracker.needs_keep_alive(ON));
        assert!(tracker.arm_created("next"));
        tracker.observe("next", STATUS_COMPLETED, false, ON);
        assert_eq!(tracker.consumed.len(), MAX_TRACKED_WINDOW_TASKS);
        assert_eq!(tracker.consumed_order.len(), MAX_TRACKED_WINDOW_TASKS);
        tracker.clear();
        assert_eq!(tracker.tracked_count(), 0);
        assert!(tracker.consumed.is_empty());
        assert!(tracker.consumed_order.is_empty());
    }

    #[test]
    fn missing_or_invalid_prefs_default_to_enabled() {
        let mut values = BTreeMap::new();
        values.insert(PROGRESS_WINDOW_PREF.to_owned(), Value::Bool(false));
        values.insert(COMPLETION_WINDOW_PREF.to_owned(), Value::String("x".into()));
        let prefs = ProgressWindowPrefs::from_preferences(&values);
        assert!(!prefs.progress);
        assert!(prefs.completion);
    }
}
