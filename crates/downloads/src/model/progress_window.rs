//! 独立下载进度 / 完成窗口的开关决策（纯状态机，不碰窗口与会话）。
//!
//! 只为**用户在场时亲手开始**的任务开窗：新建下载表单提交的单个任务、单任务「继续」、
//! 「重新下载」，以及外部入口（浏览器静默捕获、插件页面脚本、命令行）开始的单个任务。
//! 进度窗口只给新建任务和外部入口的开始；应用内「继续」「重新下载」只保留完成提醒。
//! 新任务成功后登记，已有任务须在命令前捕获状态、成功后登记；
//! 队列调度、启动自动恢复、RSS、批量操作都不登记，因此不开窗。
//!
//! 按需显示：用户正看着主窗口时任务就在列表里，不再弹窗。应用内入口以主窗口可见为在场，
//! 外部入口以主窗口在前台为在场（见 [`StartOrigin`]）；进度窗口在任务进入活跃态时判定一次，
//! 完成窗口在完成时再判定一次。任务有待回答的选择请求时先让选择窗口，答复后再判定进度窗口。
//!
//! 窗口生命周期：任务进入活跃态开进度窗口；完成时已开的窗口按当前全局设置切换为完成视图
//! 或关闭，没有进度窗口（被用户关掉或因主窗口在场未弹）的任务完成时按该设置弹一次不抢焦点的
//! 完成窗口。

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

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

/// 用户开始的入口，决定「用户正看着主窗口」按什么判定。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartOrigin {
    /// 主窗口、新建下载表单、任务详情等应用内入口：主窗口可见即在场。
    App,
    /// 浏览器静默捕获、插件页面脚本、命令行 / 单实例唤起：主窗口在前台才算在场。此时前台
    /// 通常是浏览器；而 Windows 对被完全遮挡的窗口仍报可见，只有前台判定可靠。
    External,
}

/// 主窗口此刻的状态；没有主窗口时两项都为 false。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MainWindowState {
    /// 平台正在呈现主窗口（未最小化、未被隐藏；macOS 另含未被完全遮挡）。
    pub visible: bool,
    /// 主窗口是前台窗口。
    pub active: bool,
}

impl MainWindowState {
    /// 该入口开始的任务此刻是否已在用户眼前（主窗口列表里），无需再弹窗。
    #[must_use]
    pub fn attends(self, origin: StartOrigin) -> bool {
        match origin {
            StartOrigin::App => self.visible || self.active,
            StartOrigin::External => self.active,
        }
    }
}

/// [`ProgressWindowTracker::observe`] 的窗口环境。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WindowContext {
    /// 该任务当前是否有进度 / 完成窗口。
    pub window_open: bool,
    pub main: MainWindowState,
    /// 该任务有待用户回答的选择请求（画质 / BT 文件 / 插件 / 变体 / 文件已存在）。
    pub selection_pending: bool,
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

/// 一条开始意图：入口与本轮是否允许弹进度窗口。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Intent {
    origin: StartOrigin,
    /// 应用内的继续 / 重新下载由用户在主窗口或详情窗口里操作，任务就在眼前：不弹进度窗口，
    /// 只保留完成提醒。新建任务与外部入口的开始才弹。
    opens_progress: bool,
}

#[derive(Debug, Default)]
pub struct ProgressWindowTracker {
    armed: HashMap<String, Intent>,
    user_started: HashMap<String, Intent>,
    consumed: HashSet<String>,
    consumed_order: VecDeque<String>,
}

impl ProgressWindowTracker {
    /// 新建 / agent 重放使用幂等登记。返回 false 表示已登记、已消费或到达跟踪上限。
    pub fn arm_created(&mut self, task_id: &str, origin: StartOrigin) -> bool {
        self.arm(
            task_id,
            Intent {
                origin,
                opens_progress: true,
            },
        )
    }

    /// 真正的再次开始不受新建事件去重约束；宿主只可用命令后的新状态补判。应用内的继续 /
    /// 重新下载不弹进度窗口（见 [`Intent::opens_progress`]），外部入口的恢复照常弹。
    pub fn arm_restart(&mut self, task_id: &str, origin: StartOrigin) -> bool {
        self.armed.remove(task_id);
        self.user_started.remove(task_id);
        if self.consumed.remove(task_id) {
            self.consumed_order.retain(|id| id != task_id);
        }
        self.arm(
            task_id,
            Intent {
                origin,
                opens_progress: origin == StartOrigin::External,
            },
        )
    }

    fn arm(&mut self, task_id: &str, intent: Intent) -> bool {
        if self.armed.contains_key(task_id)
            || self.user_started.contains_key(task_id)
            || self.consumed.contains(task_id)
            || self.tracked_count() >= MAX_TRACKED_WINDOW_TASKS
        {
            return false;
        }
        self.armed.insert(task_id.to_owned(), intent);
        true
    }

    #[must_use]
    pub fn tracked_count(&self) -> usize {
        self.armed.len() + self.user_started.len()
    }

    #[must_use]
    pub fn is_armed(&self, task_id: &str) -> bool {
        self.armed.contains_key(task_id)
    }

    /// 尚未进入活跃态（或在等选择答复）的意图；选择请求结束后宿主据此重新判定。
    pub fn armed_ids(&self) -> impl Iterator<Item = &str> {
        self.armed.keys().map(String::as_str)
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
            .remove_entry(task_id)
            .or_else(|| self.user_started.remove_entry(task_id))
            .map_or_else(|| task_id.to_owned(), |(id, _)| id);
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
            .keys()
            .chain(self.user_started.keys())
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
                .keys()
                .chain(self.user_started.keys())
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

    /// 任务状态更新。
    pub fn observe(
        &mut self,
        task_id: &str,
        status: i32,
        context: WindowContext,
        prefs: ProgressWindowPrefs,
    ) -> Option<ProgressWindowEffect> {
        self.update_prefs(prefs);
        match status {
            STATUS_DOWNLOADING | STATUS_PREPARING => {
                // 选择优先：意图留在 armed，选择答复后宿主用当前状态重新判定。
                if context.selection_pending {
                    return None;
                }
                let intent = self.armed.remove(task_id)?;
                self.user_started.insert(task_id.to_owned(), intent);
                (prefs.progress
                    && intent.opens_progress
                    && !context.window_open
                    && !context.main.attends(intent.origin))
                .then_some(ProgressWindowEffect::OpenProgress)
            }
            STATUS_COMPLETED => {
                let origin = self
                    .armed
                    .get(task_id)
                    .or_else(|| self.user_started.get(task_id))
                    .map(|intent| intent.origin);
                if origin.is_some() {
                    self.consume(task_id);
                }
                if context.window_open {
                    (!prefs.completion).then_some(ProgressWindowEffect::Close)
                } else {
                    origin
                        .filter(|origin| prefs.completion && !context.main.attends(*origin))
                        .map(|_| ProgressWindowEffect::OpenCompletion)
                }
            }
            STATUS_PAUSED | STATUS_ERROR => {
                if self.armed.contains_key(task_id) || self.user_started.contains_key(task_id) {
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
    const APP: StartOrigin = StartOrigin::App;
    const EXTERNAL: StartOrigin = StartOrigin::External;

    /// 该任务没有窗口、主窗口不在、没有待答复的选择。
    const NONE: WindowContext = WindowContext {
        window_open: false,
        main: MainWindowState {
            visible: false,
            active: false,
        },
        selection_pending: false,
    };
    /// 该任务的进度 / 完成窗口已开。
    const OPEN: WindowContext = WindowContext {
        window_open: true,
        ..NONE
    };

    fn with_main(visible: bool, active: bool) -> WindowContext {
        WindowContext {
            main: MainWindowState { visible, active },
            ..NONE
        }
    }

    #[test]
    fn unarmed_tasks_never_open_windows() {
        let mut tracker = ProgressWindowTracker::default();
        assert_eq!(tracker.observe("t", 1, NONE, ON), None);
        assert_eq!(tracker.observe("t", 3, NONE, ON), None);
    }

    #[test]
    fn armed_task_opens_once_when_it_becomes_active() {
        let mut tracker = ProgressWindowTracker::default();
        tracker.arm_created("t", APP);
        assert_eq!(tracker.observe("t", 0, NONE, ON), None, "排队中不开窗");
        assert!(tracker.is_armed("t"), "排队期间意图保留，宿主据此保持界面");
        assert_eq!(
            tracker.observe("t", 5, NONE, ON),
            Some(ProgressWindowEffect::OpenProgress)
        );
        assert!(!tracker.is_armed("t"));
        assert_eq!(tracker.observe("t", 1, OPEN, ON), None);
    }

    #[test]
    fn open_window_turns_into_completion_view_or_closes() {
        let mut tracker = ProgressWindowTracker::default();
        tracker.arm_created("a", APP);
        tracker.observe("a", 1, NONE, ON);
        assert_eq!(
            tracker.observe("a", 3, OPEN, ON),
            None,
            "保留窗口切完成视图"
        );

        tracker.arm_created("b", APP);
        tracker.observe("b", 1, NONE, ON);
        let completion_off = ProgressWindowPrefs {
            completion: false,
            ..ON
        };
        assert_eq!(
            tracker.observe("b", 3, OPEN, completion_off),
            Some(ProgressWindowEffect::Close)
        );
    }

    #[test]
    fn closed_progress_window_still_gets_one_completion_window() {
        let mut tracker = ProgressWindowTracker::default();
        tracker.arm_created("t", APP);
        tracker.observe("t", 1, NONE, ON);
        assert_eq!(
            tracker.observe("t", 3, NONE, ON),
            Some(ProgressWindowEffect::OpenCompletion)
        );
        assert_eq!(tracker.observe("t", 3, NONE, ON), None, "只弹一次");
    }

    #[test]
    fn prefs_gate_each_window_independently() {
        let progress_off = ProgressWindowPrefs {
            progress: false,
            completion: true,
        };
        let mut tracker = ProgressWindowTracker::default();
        tracker.arm_created("t", APP);
        assert_eq!(tracker.observe("t", 1, NONE, progress_off), None);
        assert_eq!(
            tracker.observe("t", 3, NONE, progress_off),
            Some(ProgressWindowEffect::OpenCompletion)
        );

        let completion_off = ProgressWindowPrefs {
            progress: true,
            completion: false,
        };
        tracker.arm_created("u", APP);
        tracker.observe("u", 1, NONE, completion_off);
        assert_eq!(tracker.observe("u", 3, NONE, completion_off), None);
    }

    #[test]
    fn instant_completion_counts_as_user_started() {
        let mut tracker = ProgressWindowTracker::default();
        tracker.arm_created("t", APP);
        assert_eq!(
            tracker.observe("t", 3, NONE, ON),
            Some(ProgressWindowEffect::OpenCompletion)
        );
    }

    #[test]
    fn failure_before_start_drops_the_intent() {
        let mut tracker = ProgressWindowTracker::default();
        tracker.arm_created("t", APP);
        tracker.observe("t", 4, NONE, ON);
        assert_eq!(tracker.observe("t", 1, NONE, ON), None);
    }

    #[test]
    fn app_starts_stay_quiet_while_the_main_window_is_visible() {
        let mut tracker = ProgressWindowTracker::default();
        tracker.arm_created("t", APP);
        assert_eq!(
            tracker.observe("t", 1, with_main(true, false), ON),
            None,
            "主窗口可见（即使不在前台）：任务就在列表里"
        );
        assert_eq!(
            tracker.observe("t", 3, with_main(true, true), ON),
            None,
            "完成时主窗口仍在场：不弹完成窗口"
        );

        tracker.arm_created("minimized", APP);
        assert_eq!(
            tracker.observe("minimized", 1, with_main(false, false), ON),
            Some(ProgressWindowEffect::OpenProgress),
            "主窗口最小化 / 没开：照常弹"
        );
    }

    #[test]
    fn external_starts_need_the_main_window_in_front_to_stay_quiet() {
        let mut tracker = ProgressWindowTracker::default();
        tracker.arm_created("behind", EXTERNAL);
        assert_eq!(
            tracker.observe("behind", 1, with_main(true, false), ON),
            Some(ProgressWindowEffect::OpenProgress),
            "浏览器在前台、主窗口在后面：照常弹"
        );

        tracker.arm_created("front", EXTERNAL);
        assert_eq!(tracker.observe("front", 1, with_main(true, true), ON), None);
        assert_eq!(
            tracker.observe("front", 3, with_main(true, false), ON),
            Some(ProgressWindowEffect::OpenCompletion),
            "完成时主窗口已不在前台：弹完成窗口"
        );
    }

    #[test]
    fn app_restarts_never_open_progress_but_keep_completion() {
        let mut tracker = ProgressWindowTracker::default();
        assert!(tracker.arm_restart("resumed", APP));
        assert_eq!(
            tracker.observe("resumed", 1, NONE, ON),
            None,
            "应用内继续：即使主窗口不在也不弹进度窗口"
        );
        assert_eq!(
            tracker.observe("resumed", 3, NONE, ON),
            Some(ProgressWindowEffect::OpenCompletion)
        );

        assert!(tracker.arm_restart("page", EXTERNAL));
        assert_eq!(
            tracker.observe("page", 1, NONE, ON),
            Some(ProgressWindowEffect::OpenProgress),
            "插件页面脚本恢复仍按外部入口弹"
        );
    }

    #[test]
    fn suppressed_progress_keeps_the_completion_intent() {
        let mut tracker = ProgressWindowTracker::default();
        tracker.arm_created("t", APP);
        assert_eq!(tracker.observe("t", 1, with_main(true, true), ON), None);
        assert!(tracker.needs_keep_alive(ON));
        assert_eq!(
            tracker.observe("t", 3, NONE, ON),
            Some(ProgressWindowEffect::OpenCompletion),
            "下载期间关掉了主窗口：完成时提醒"
        );
    }

    #[test]
    fn pending_selection_defers_progress_until_answered() {
        let mut tracker = ProgressWindowTracker::default();
        tracker.arm_created("t", EXTERNAL);
        let asking = WindowContext {
            selection_pending: true,
            ..NONE
        };
        assert_eq!(tracker.observe("t", 5, asking, ON), None);
        assert!(tracker.is_armed("t"), "等选择答复期间意图保留");
        assert_eq!(tracker.armed_ids().collect::<Vec<_>>(), ["t"]);
        assert_eq!(
            tracker.observe("t", 5, NONE, ON),
            Some(ProgressWindowEffect::OpenProgress),
            "答复后按当前状态补开"
        );

        tracker.arm_created("cancelled", EXTERNAL);
        tracker.observe("cancelled", 5, asking, ON);
        tracker.observe("cancelled", STATUS_PAUSED, NONE, ON);
        assert_eq!(
            tracker.observe("cancelled", 1, NONE, ON),
            None,
            "取消选择即暂停：不再开窗"
        );
    }

    #[test]
    fn changing_completion_preference_applies_to_all_started_and_future_tasks() {
        let mut tracker = ProgressWindowTracker::default();
        for task in ["open", "closed"] {
            tracker.arm_created(task, APP);
            tracker.observe(task, STATUS_DOWNLOADING, NONE, ON);
        }
        let off = ProgressWindowPrefs {
            completion: false,
            ..ON
        };
        assert_eq!(
            tracker.observe("open", STATUS_COMPLETED, OPEN, off),
            Some(ProgressWindowEffect::Close),
        );
        assert_eq!(tracker.observe("closed", STATUS_COMPLETED, NONE, off), None);
        tracker.arm_created("future", APP);
        tracker.observe("future", STATUS_DOWNLOADING, NONE, off);
        assert_eq!(tracker.observe("future", STATUS_COMPLETED, NONE, off), None);
        tracker.arm_created("reenabled", APP);
        tracker.observe("reenabled", STATUS_DOWNLOADING, NONE, off);
        assert_eq!(
            tracker.observe("reenabled", STATUS_COMPLETED, NONE, ON),
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
        tracker.arm_created("fast", APP);
        assert!(tracker.needs_keep_alive(only_completion));
        assert_eq!(
            tracker.observe("fast", STATUS_DOWNLOADING, NONE, only_completion),
            None
        );
        assert!(!tracker.is_armed("fast"));
        assert!(tracker.needs_keep_alive(only_completion));
        assert_eq!(
            tracker.observe("fast", STATUS_COMPLETED, NONE, only_completion),
            Some(ProgressWindowEffect::OpenCompletion)
        );
        assert!(!tracker.needs_keep_alive(only_completion));
    }

    #[test]
    fn launch_and_capture_replay_are_idempotent_but_real_restart_is_not() {
        let mut tracker = ProgressWindowTracker::default();
        assert!(tracker.arm_created("t", EXTERNAL));
        assert!(!tracker.arm_created("t", EXTERNAL));
        assert_eq!(
            tracker.observe("t", STATUS_COMPLETED, NONE, ON),
            Some(ProgressWindowEffect::OpenCompletion)
        );
        assert!(
            !tracker.arm_created("t", EXTERNAL),
            "完成后的重放不能复活意图"
        );
        assert_eq!(tracker.observe("t", STATUS_COMPLETED, NONE, ON), None);
        assert!(tracker.arm_restart("t", EXTERNAL));
        assert_eq!(
            tracker.observe("t", STATUS_COMPLETED, NONE, ON),
            Some(ProgressWindowEffect::OpenCompletion)
        );
    }

    #[test]
    fn pause_failure_deletion_and_disabling_both_release_completion_wait() {
        for status in [STATUS_PAUSED, STATUS_ERROR] {
            let mut tracker = ProgressWindowTracker::default();
            tracker.arm_created("t", APP);
            tracker.observe("t", STATUS_DOWNLOADING, NONE, ON);
            tracker.observe("t", status, NONE, ON);
            assert!(!tracker.needs_keep_alive(ON));
            assert_eq!(tracker.observe("t", STATUS_COMPLETED, NONE, ON), None);
        }
        let mut tracker = ProgressWindowTracker::default();
        tracker.arm_created("deleted", APP);
        tracker.forget("deleted");
        assert!(!tracker.needs_keep_alive(ON));
        assert!(!tracker.arm_created("deleted", APP));
        tracker.forget("deleted-before-start-event");
        assert!(!tracker.arm_created("deleted-before-start-event", APP));
        tracker.arm_created("disabled", APP);
        tracker.update_prefs(ProgressWindowPrefs {
            progress: false,
            completion: false,
        });
        assert!(!tracker.needs_keep_alive(ON));
        assert_eq!(
            tracker.observe("disabled", STATUS_COMPLETED, NONE, ON),
            None
        );
        tracker.arm_created("snapshot-missing", APP);
        tracker.retain_existing(|_| false);
        assert!(!tracker.needs_keep_alive(ON));
    }

    #[test]
    fn burst_tracking_and_replay_memory_are_bounded() {
        let mut tracker = ProgressWindowTracker::default();
        for index in 0..MAX_TRACKED_WINDOW_TASKS {
            assert!(tracker.arm_created(&index.to_string(), APP));
        }
        assert!(!tracker.arm_created("overflow", APP));
        assert_eq!(tracker.tracked_count(), MAX_TRACKED_WINDOW_TASKS);
        for index in 0..MAX_TRACKED_WINDOW_TASKS {
            assert_eq!(
                tracker.observe(&index.to_string(), STATUS_COMPLETED, NONE, ON),
                Some(ProgressWindowEffect::OpenCompletion)
            );
        }
        assert!(!tracker.needs_keep_alive(ON));
        assert!(tracker.arm_created("next", APP));
        tracker.observe("next", STATUS_COMPLETED, NONE, ON);
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
