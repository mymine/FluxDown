use std::{
    cell::Cell,
    collections::HashMap,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use crate::{
    UserStartIntent,
    actions::{
        ClearFinished, ClearSelection, CopySelectedUrl, CycleDensity, CycleGroupBy, CycleSort,
        DeleteSelected, DeleteSelectedWithFiles, FocusSearch, KEY_CONTEXT, NewDownload,
        OpenQueueManager, OpenSelected, OpenSelectedInWindow, OpenTorrentFile, PauseAll,
        PauseSelected, RedownloadSelected, RenameSelected, ResumeAll, ResumeSelected,
        RevealSelected, SelectAllTasks, ShowSelectedDetail, ToggleBoostSelected, ToggleDetailPanel,
        TogglePauseSelected,
    },
    components::{
        task_table::{
            DownloadTableDelegate, SelectionSummary, TableFilter, ToolbarCommand,
            spawn_midnight_refresh,
        },
        title_bar::{DownloadTitleBar, left_edge_probe},
    },
    controller::{DownloadsCommand, DownloadsController, DownloadsPort},
    model::{
        DownloadFilter, DownloadStatusFilter, RowKey, SidebarSection, SidebarSelection,
        StatusFolderMotion, TaskState,
        file_rescan::{RescanDecision, RescanThrottle},
        refresh_gate::{RefreshGate, RefreshPlan, RefreshTrigger},
        view_prefs::{DetailPlacement, VIEW_PREFS_KEY, ViewGroupBy, ViewPrefs},
    },
    pages::new_download::{NewDownloadContext, build_new_download_context},
    pages::task_detail::TaskDetailView,
    strings::{DownloadStrings, error_text},
    submission::{NewDownloadSubmission, SubmitNotice, run_submission},
};
use fluxdown_ui_components::{ControlExt as _, FluxIcon, SidebarChange, SidebarState};
use fluxdown_ui_i18n::Translator;
use gpui::{
    App, AppContext as _, ClipboardItem, Context, Entity, ExternalPaths, FocusHandle, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement, PathPromptOptions, Pixels, Render,
    SharedString, Styled, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    Icon, ResizableState, WindowExt as _, h_flex, h_resizable,
    input::{Input, InputEvent, InputState},
    notification::Notification,
    resizable_panel,
    table::{TableEvent, TableState},
    v_flex, v_resizable,
};

pub(crate) const SIDEBAR_MOTION_DURATION: Duration = Duration::from_millis(200);
const PREFS_DEBOUNCE: Duration = Duration::from_millis(300);
/// 搜索框输入防抖。
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(150);
/// 「在独立窗口打开」一次最多开的窗口数。
pub const MAX_TASK_WINDOWS_PER_ACTION: usize = 8;

/// 宿主可直接触发的下载页命令（见 [`DownloadView::run_page_command`]）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageCommand {
    OpenTorrentFile,
    PauseAll,
    ResumeAll,
    ClearFinished,
    SelectAll,
    FocusSearch,
    CycleDensity,
    CycleGroupBy,
    CycleSort,
    ToggleDetailPanel,
}

/// 宿主注入的「新建下载」入口：由 app 打开独立对话框窗口。
/// 表单初值由下载页在点击瞬间算好传入，打开方不得再回读 `DownloadView`
///（此时实体正处于 update 中）。
pub type NewDownloadOpener = Rc<dyn Fn(NewDownloadContext, &mut Window, &mut App)>;
/// 以 id 打开一个窗口 / 编辑器（任务窗口、组窗口、分类编辑）。
pub type IdOpener = Rc<dyn Fn(String, &mut Window, &mut App)>;
/// 无参数窗口入口（队列管理）。
pub type PlainOpener = Rc<dyn Fn(&mut Window, &mut App)>;
/// 分类编辑入口：`Some(id)` 编辑现有分类，`None` 新建。
pub type CategoryEditorOpener = Rc<dyn Fn(Option<String>, &mut Window, &mut App)>;
/// 单任务交互式开始的生命周期。新建成功直接通知；已有任务须在执行命令前通知 Requested。
pub type UserStartHook = Rc<dyn Fn(UserStartIntent, &mut App)>;

/// app 注入的跨窗口 / 跨能力入口；未注入的入口对应按钮无动作。
#[derive(Clone, Default)]
pub struct DownloadHostActions {
    pub open_new_download: Option<NewDownloadOpener>,
    pub open_task_window: Option<IdOpener>,
    pub open_group_window: Option<IdOpener>,
    pub open_queue_manager: Option<PlainOpener>,
    /// 侧栏「设备」区标题上的「添加设备」入口；`None` 时不显示按钮。
    pub open_add_device: Option<PlainOpener>,
    /// 任务表「待确认」角标点击：打开 / 置前聚合的「文件已存在」窗口。
    pub open_file_conflicts: Option<PlainOpener>,
    /// `Some(id)` 编辑现有分类，`None` 新建。
    pub open_category_editor: Option<CategoryEditorOpener>,
    /// 完成后关机的只读状态投影（`None` = Resident 未装配）。
    pub shutdown_status: Option<crate::model::shutdown::SharedShutdownStatus>,
    /// 状态栏发起关机请求的端口。
    pub shutdown: Option<crate::model::shutdown::ShutdownPort>,
    /// 单任务交互式开始回调（批量操作不回调）。
    pub on_user_started: Option<UserStartHook>,
}

/// 下载能力的顶层页面。
pub struct DownloadView {
    pub(crate) controller: DownloadsController,
    /// 详情面板 / 弹出任务窗口按需构造 `TaskDetailView` 用（`DownloadsController`
    /// 不暴露内部 port）。
    pub(crate) port: Arc<dyn DownloadsPort>,
    /// 状态栏新增文案的实时查表源（现有渲染热路径用 `strings` 缓存字段）。
    pub(crate) translator: Entity<Translator>,
    pub(crate) strings: DownloadStrings,
    pub(crate) selected_item: SidebarSelection,
    pub(crate) expanded_status: Option<DownloadStatusFilter>,
    pub(crate) folder_motion: StatusFolderMotion,
    pub(crate) folder_motion_started_at: Option<Instant>,
    pub(crate) section_expanded: HashMap<SidebarSection, bool>,
    pub(crate) section_motion_from: HashMap<SidebarSection, f32>,
    pub(crate) section_motion_started_at: HashMap<SidebarSection, Instant>,
    pub(crate) sidebar: Entity<SidebarState>,
    pub(crate) table_state: Entity<TableState<DownloadTableDelegate>>,
    pub(crate) host: DownloadHostActions,
    pub(crate) last_error: Option<SharedString>,
    /// 根元素 focus handle：右键菜单 action_context 分派目标，`escape`
    /// 清空搜索框后也交回给它。
    pub(crate) focus_handle: FocusHandle,
    /// 顶栏搜索框状态（输入框渲染在 [`DownloadTitleBar`] 插槽里）。
    pub(crate) search_input: Entity<InputState>,
    /// 已下发给搜索框的 placeholder（语言切换后需重下发）。
    search_placeholder: SharedString,
    /// 搜索防抖代数。
    search_generation: Rc<Cell<u64>>,
    prefs_generation: Rc<Cell<u64>>,
    /// 已从偏好加载过视图设置（缺省分组维度只在首次决定）。
    prefs_loaded: bool,
    /// 最近一次应用 / 写出的视图偏好持久化值：未变化的偏好回流不重载。
    applied_view_prefs: Option<serde_json::Value>,
    /// 停靠详情面板（`None` = 尚未选中过任何任务）。
    pub(crate) detail: Option<Entity<TaskDetailView>>,
    /// 详情面板 / 主内容拆分的独立 resizable 状态。
    pub(crate) detail_resizable_state: Entity<ResizableState>,
    /// 上次渲染时的选中投影；表格选中变化时与之比较，变了才重绘本页（浮动选择条）。
    pub(crate) selection_summary: Cell<SelectionSummary>,
    /// 内容区（侧栏右侧）左缘的窗口横坐标；顶栏插槽据此把「新建」主按钮与内容区左对齐。
    pub(crate) content_left: Pixels,
    /// 文件跟踪重扫节流（主窗口获焦触发）。
    file_rescan: RescanThrottle,
    /// 事件驱动刷新的合并闸门（≤30Hz、同批事件一次刷新）。
    refresh_gate: RefreshGate,
    /// 上次刷新时存储的行布局计数；与当前不同说明行 ID 可能已指向别的任务。
    refreshed_structure: u64,
}

impl DownloadView {
    /// 创建下载页面，并订阅共享翻译状态。
    pub fn new(
        translator: Entity<Translator>,
        port: Arc<dyn DownloadsPort>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let strings = DownloadStrings::from_translator(translator.read(cx));
        let controller = DownloadsController::new(Arc::clone(&port));
        crate::components::file_icon::install_port(&port, cx);
        let store = Rc::clone(controller.store());
        let focus_handle = cx.focus_handle();
        let table_state = cx.new(|cx| {
            TableState::new(
                DownloadTableDelegate::new(strings.clone(), store),
                window,
                cx,
            )
            .row_selectable(false)
            .col_selectable(false)
        });
        spawn_midnight_refresh(&table_state, cx);
        let weak_self = cx.weak_entity();
        table_state.update(cx, |table, _| {
            table.delegate_mut().set_host(weak_self);
            table
                .delegate_mut()
                .set_action_context(focus_handle.clone());
        });
        let strings_placeholder = strings.search_tasks_placeholder.clone();
        let search_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(strings.search_tasks_placeholder.clone())
        });
        cx.subscribe(&search_input, Self::handle_search_changed)
            .detach();

        cx.observe(&translator, |this, translator, cx| {
            this.set_strings(DownloadStrings::from_translator(translator.read(cx)), cx);
        })
        .detach();
        cx.subscribe_in(&table_state, window, Self::handle_table_event)
            .detach();
        // 选中变化只通知表格实体；浮动选择条依赖它，按投影差异重绘本页，
        // 避免表格滚动 / 悬停等高频 notify 带着整页重绘。停靠详情面板打开时跟随单选任务。
        cx.observe_in(&table_state, window, |this, table_state, window, cx| {
            let (selection, follow) = {
                let delegate = table_state.read(cx).delegate();
                let follow = delegate
                    .prefs()
                    .detail_open
                    .then(|| delegate.single_selected_key().filter(|key| key.is_local()))
                    .flatten()
                    .cloned();
                (delegate.selection_summary(), follow)
            };
            if this.selection_summary.get() != selection {
                this.selection_summary.set(selection);
                cx.notify();
            }
            if let Some(key) = follow {
                this.bind_detail(key, window, cx);
            }
        })
        .detach();
        // 文件跟踪：主窗口获焦时用户可能刚在文件管理器里删除 / 移走了已完成任务的文件。
        cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.request_file_rescan(cx);
            }
        })
        .detach();
        let sidebar = cx.new(|cx| SidebarState::new(px(200.), px(176.)..px(300.), cx));
        cx.observe(&sidebar, |_, _, cx| cx.notify()).detach();
        cx.subscribe(&sidebar, |this, _, change: &SidebarChange, cx| {
            this.table_state.update(cx, |table, _| {
                let prefs = table.delegate_mut().prefs_mut();
                prefs.sidebar_width = f32::from(change.width);
                prefs.sidebar_collapsed = change.collapsed;
            });
            this.schedule_persist_prefs(cx);
            cx.notify();
        })
        .detach();

        Self {
            controller,
            port,
            translator: translator.clone(),
            strings,
            selected_item: SidebarSelection::Download(DownloadFilter::ALL),
            expanded_status: Some(DownloadStatusFilter::All),
            folder_motion: StatusFolderMotion::settled(Some(DownloadStatusFilter::All)),
            folder_motion_started_at: None,
            section_expanded: SidebarSection::ALL
                .into_iter()
                .map(|section| (section, true))
                .collect(),
            section_motion_from: HashMap::new(),
            section_motion_started_at: HashMap::new(),
            sidebar,
            table_state,
            host: DownloadHostActions::default(),
            last_error: None,
            focus_handle,
            search_input,
            search_placeholder: strings_placeholder,
            search_generation: Rc::new(Cell::new(0)),
            prefs_generation: Rc::new(Cell::new(0)),
            prefs_loaded: false,
            applied_view_prefs: None,
            detail: None,
            detail_resizable_state: cx.new(|_| ResizableState::default()),
            selection_summary: Cell::new(SelectionSummary::default()),
            content_left: px(0.),
            file_rescan: RescanThrottle::default(),
            refresh_gate: RefreshGate::default(),
            refreshed_structure: u64::MAX,
        }
    }

    /// 供 shell 顶栏与页面共享的侧栏状态；布局修改沿用下载页偏好持久化。
    pub fn sidebar_state(&self) -> Entity<SidebarState> {
        self.sidebar.clone()
    }

    /// 注入宿主入口（新建下载 / 任务窗口 / 队列管理…）。
    pub fn set_host_actions(&mut self, host: DownloadHostActions) {
        self.host = host;
    }

    /// 创建挂到 shell 统一顶栏的下载页插槽（搜索、视图选项、新建）。
    pub fn new_title_bar(&self, cx: &mut Context<Self>) -> Entity<DownloadTitleBar> {
        let view = cx.entity();
        let translator = self.translator.clone();
        let search_input = self.search_input.clone();
        let table_state = self.table_state.clone();
        cx.new(|cx| DownloadTitleBar::new(&view, translator, search_input, table_state, cx))
    }

    /// 「新建下载」表单的环境快照：保存目录 / 默认队列 / 线程数初值与队列候选；
    /// 队列优先侧栏当前筛选（规则见 [`build_new_download_context`]）。
    #[must_use]
    pub fn new_download_context(&self) -> NewDownloadContext {
        let controller = &self.controller;
        let selected_queue = match &self.selected_item {
            SidebarSelection::Queue(queue_id) => Some(queue_id.as_str()),
            _ => None,
        };
        build_new_download_context(
            controller.config(),
            &controller.runtime_stats().save_dir,
            controller.preferences(),
            controller.queues(),
            selected_queue,
            controller.other_devices(),
        )
    }

    /// 打开「新建下载」窗口（可预填链接）。
    pub(crate) fn open_new_download_with(
        &self,
        initial_urls: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(opener) = self.host.open_new_download.clone() else {
            return;
        };
        let mut context = self.new_download_context();
        context.initial_urls = initial_urls;
        opener(context, window, cx);
    }

    pub(crate) fn open_new_download(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_new_download_with(Vec::new(), window, cx);
    }

    /// 按表单提交创建任务 / 确认外部捕获 / 下发到其他设备；对话框确认后由宿主调用。
    ///
    /// 命令逐条执行，任一失败即在页面横幅提示（按错误 `reason` 给出原因）；同时把本次
    /// 保存目录 / 下载目标记入本机偏好（尽力而为）。返回的 task 在全部完成后给出汇总提示。
    pub fn create_download(
        &mut self,
        submission: NewDownloadSubmission,
        cx: &mut Context<Self>,
    ) -> gpui::Task<SubmitNotice> {
        let starts_immediately = submission.starts_immediately();
        let port = Arc::clone(&self.port);
        let on_user_started = self.host.on_user_started.clone();
        cx.spawn(async move |this, cx| {
            let report = run_submission(submission, move |command| port.execute(command)).await;
            if starts_immediately
                && let ([task_id], Some(hook)) = (report.created_task_ids(), on_user_started)
            {
                cx.update(|cx| hook(UserStartIntent::Created(task_id.clone()), cx));
            }
            let fallback = SubmitNotice {
                ok: !report.failed(),
                message: String::new(),
            };
            this.update(cx, |this, cx| {
                let notice = report.notice(this.translator.read(cx));
                this.last_error = report
                    .failed()
                    .then(|| SharedString::from(notice.message.clone()));
                cx.notify();
                notice
            })
            .unwrap_or(fallback)
        })
    }

    /// 角标点击入口：交给宿主打开 / 置前「文件已存在」窗口。
    pub(crate) fn open_file_conflicts(&self, window: &mut Window, cx: &mut App) {
        if let Some(open) = self.host.open_file_conflicts.as_ref() {
            open(window, cx);
        }
    }

    pub fn replace_snapshot(
        &mut self,
        snapshot: &fluxdown_protocol::AgentSnapshot,
        cx: &mut Context<Self>,
    ) {
        self.controller.replace_snapshot(snapshot);
        if let Some(detail) = self.detail.clone() {
            detail.update(cx, |detail, cx| detail.replace_snapshot(snapshot, cx));
        }
        self.reconcile_sidebar_selection();
        self.last_error = (!snapshot.daemon_connected).then(|| self.strings.disconnected.clone());
        self.load_view_prefs(cx);
        self.refresh_from_store(cx);
    }

    pub fn apply_event(&mut self, event: &fluxdown_protocol::ServiceEvent, cx: &mut Context<Self>) {
        let table_changed = self.controller.apply_event(event);
        if let Some(detail) = self.detail.clone() {
            detail.update(cx, |detail, cx| detail.apply_event(event, cx));
        }
        match event {
            fluxdown_protocol::ServiceEvent::Agent(
                fluxdown_protocol::AgentEvent::PreferencesChanged(_),
            ) => self.load_view_prefs(cx),
            // agent 先于 daemon 就绪时首个快照即为未连接；连接态以事件为准，横幅随之出现 / 消失。
            fluxdown_protocol::ServiceEvent::Agent(
                fluxdown_protocol::AgentEvent::DaemonConnectionChanged(connected),
            ) => {
                self.last_error = (!connected).then(|| self.strings.disconnected.clone());
            }
            _ => {}
        }
        if table_changed {
            if matches!(
                event,
                fluxdown_protocol::ServiceEvent::Agent(
                    fluxdown_protocol::AgentEvent::Daemon(
                        fluxdown_protocol::DaemonEvent::QueuesChanged(_)
                            | fluxdown_protocol::DaemonEvent::SnapshotReplaced(_)
                    ) | fluxdown_protocol::AgentEvent::DaemonSnapshotReplaced(_)
                        | fluxdown_protocol::AgentEvent::CloudDevicesChanged(_)
                        | fluxdown_protocol::AgentEvent::LinkedDevicesChanged(_)
                        | fluxdown_protocol::AgentEvent::SessionChanged(_)
                        | fluxdown_protocol::AgentEvent::PreferencesChanged(_)
                )
            ) {
                self.reconcile_sidebar_selection();
            }
            self.schedule_table_refresh(cx);
        } else {
            cx.notify();
        }
    }

    /// 队列删除 / 设备消失（登出、被移除、解除配对）/ daemon 快照更替后，不能继续筛选
    /// 已不存在的队列或设备。
    fn reconcile_sidebar_selection(&mut self) {
        match &self.selected_item {
            SidebarSelection::Queue(queue_id)
                if !self
                    .controller
                    .queues()
                    .iter()
                    .any(|queue| &queue.queue_id == queue_id) =>
            {
                self.selected_item = SidebarSelection::Download(DownloadFilter::ALL);
            }
            SidebarSelection::Download(DownloadFilter {
                status,
                category: Some(category),
            }) if !self.controller.categories().rules().is_empty()
                && !self
                    .controller
                    .categories()
                    .visible()
                    .any(|rule| rule.dto.id == *category) =>
            {
                self.selected_item = SidebarSelection::Download(DownloadFilter::status(*status));
            }
            SidebarSelection::Device(id)
                if id != SidebarSelection::LOCAL_DEVICE
                    && id != SidebarSelection::ALL_DEVICES
                    && !self
                        .controller
                        .other_devices()
                        .iter()
                        .any(|device| device.id == *id) =>
            {
                self.selected_item = SidebarSelection::Download(DownloadFilter::ALL);
            }
            _ => {}
        }
    }

    pub fn mark_stale(&mut self, cx: &mut Context<Self>) {
        self.last_error = Some(self.strings.disconnected.clone());
        self.controller.mark_stale();
        if let Some(detail) = self.detail.clone() {
            detail.update(cx, |detail, cx| detail.mark_stale(cx));
        }
        cx.notify();
    }

    /// 偏好 → 表格视图设置；无偏好时首次按「存在组任务」决定默认分组维度。
    ///
    /// 任何偏好键变化都会触发这里（`PreferencesChanged` 不区分键），只有视图偏好本身的持久化值
    /// 变了才重载：否则别的键（如命令面板使用记录）回流时，会用旧持久化值覆盖 300ms 防抖
    /// 窗口内尚未写回的本地改动，表现为切换密度 / 分组 / 详情面板「无效」。
    fn load_view_prefs(&mut self, cx: &mut Context<Self>) {
        let prefs = match self.controller.preference(VIEW_PREFS_KEY) {
            Some(value) if self.applied_view_prefs.as_ref() == Some(value) => return,
            Some(value) => {
                self.applied_view_prefs = Some(value.clone());
                ViewPrefs::from_value(value)
            }
            None if self.prefs_loaded => return,
            None => {
                let mut prefs = ViewPrefs::default();
                let has_groups = self
                    .controller
                    .store()
                    .local()
                    .iter()
                    .any(|task| !task.group_id.is_empty());
                if has_groups {
                    prefs.group_by = ViewGroupBy::Group;
                }
                prefs
            }
        };
        self.prefs_loaded = true;
        self.table_state.update(cx, |table, cx| {
            table.delegate_mut().set_prefs(prefs);
            cx.notify();
        });
    }

    /// 队列名 / 组名 / 分类 / 设备名同步进表格代理（只在变化时触发重算）。
    fn sync_delegate_context(&mut self, cx: &mut Context<Self>) {
        let queues: Vec<(String, String)> = self
            .controller
            .queues()
            .iter()
            .map(|queue| (queue.queue_id.clone(), queue.name.clone()))
            .collect();
        let groups: HashMap<String, String> = self
            .controller
            .groups()
            .iter()
            .map(|group| (group.group_id.clone(), group.name.clone()))
            .collect();
        let devices: HashMap<String, String> = self
            .controller
            .other_devices()
            .into_iter()
            .map(|device| (device.id, device.label))
            .collect();
        let categories = Rc::clone(self.controller.categories());
        if let Some(detail) = self.detail.clone() {
            detail.update(cx, |detail, cx| detail.set_queue_names(queues.clone(), cx));
        }
        self.table_state.update(cx, |table, _| {
            let delegate = table.delegate_mut();
            delegate.set_queue_names(queues);
            delegate.set_group_names(groups);
            delegate.set_device_names(devices);
            delegate.set_categories(categories);
        });
    }

    /// 停靠详情面板：把当前任务的最新 DTO 推给面板刷新（事件 / 快照后调用）。
    fn sync_detail_panel(&mut self, cx: &mut Context<Self>) {
        if self.controller.is_stale() {
            return;
        }
        let Some(detail) = self.detail.clone() else {
            return;
        };
        let task_id = detail.read(cx).task_id().to_owned();
        let dto = self.controller.task_dto(&task_id).cloned();
        let runtime = self.controller.task_runtime(&task_id).cloned();
        detail.update(cx, |detail, cx| {
            detail.sync(dto, cx);
            detail.sync_runtime(runtime, cx);
        });
    }

    pub(crate) fn refresh_tasks(&mut self, cx: &mut Context<Self>) {
        let filter = TableFilter::from(&self.selected_item);
        self.table_state.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            delegate.set_filter(filter);
            delegate.refresh_view();
            // 行变化只需重绘（行数每帧从代理读取）；只有列配置变了才重建
            // `col_groups`，否则进度节拍会覆盖拖拽中的列宽。
            if delegate.take_columns_dirty() {
                table.refresh(cx);
            }
            // 行从共享的 `TaskStore` 读取：表格必须显式 notify，retained 渲染才会重画行。
            cx.notify();
        });
        cx.notify();
    }

    /// 任务 / 上下文变化后立即把存储同步进表格、侧栏与详情面板。
    fn refresh_from_store(&mut self, cx: &mut Context<Self>) {
        self.sync_delegate_context(cx);
        self.refresh_tasks(cx);
        self.sync_detail_panel(cx);
        self.refreshed_structure = self.controller.store().structure_generation();
        self.refresh_gate.flushed(Instant::now());
    }

    /// 事件使表格数据过期：按 [`RefreshGate`] 合并成一次刷新（同批事件之后、≤30Hz），
    /// 而不是每条事件都全量重算可见行。
    fn schedule_table_refresh(&mut self, cx: &mut Context<Self>) {
        let structural = self.controller.store().structure_generation() != self.refreshed_structure;
        match self.refresh_gate.mark(structural, Instant::now()) {
            RefreshPlan::Nothing => {}
            RefreshPlan::Deferred => {
                let this = cx.weak_entity();
                cx.defer(move |cx| {
                    let Ok(()) = this.update(cx, |this, cx| {
                        this.run_scheduled_refresh(RefreshTrigger::Deferred, cx);
                    }) else {
                        // 下载页已释放，结束回调，不再提交请求或刷新状态。
                        return;
                    };
                });
            }
            RefreshPlan::After(delay) => {
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(delay).await;

                    let Ok(()) = this.update(cx, |this, cx| {
                        this.run_scheduled_refresh(RefreshTrigger::Timer, cx);
                    }) else {
                        // 下载页已释放，结束回调，不再提交请求或刷新状态。
                        return;
                    };
                })
                .detach();
            }
        }
    }

    fn run_scheduled_refresh(&mut self, trigger: RefreshTrigger, cx: &mut Context<Self>) {
        if self.refresh_gate.take(trigger, Instant::now()) {
            self.refresh_from_store(cx);
        }
    }

    pub(crate) fn select_sidebar_item(
        &mut self,
        selection: SidebarSelection,
        cx: &mut Context<Self>,
    ) {
        if self.selected_item == selection {
            return;
        }
        self.selected_item = selection;
        self.refresh_tasks(cx);
    }

    fn set_strings(&mut self, strings: DownloadStrings, cx: &mut Context<Self>) {
        self.strings = strings.clone();
        self.table_state.update(cx, |table, cx| {
            table.delegate_mut().set_strings(strings);
            table.delegate_mut().refresh_view();
            table.refresh(cx);
            cx.notify();
        });
        cx.notify();
    }

    fn handle_table_event(
        &mut self,
        table_state: &Entity<TableState<DownloadTableDelegate>>,
        event: &TableEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            TableEvent::ColumnWidthsChanged(widths) => {
                table_state.update(cx, |table, _| {
                    table.delegate_mut().sync_column_widths(widths);
                });
                self.schedule_persist_prefs(cx);
            }
            TableEvent::MoveColumn(..) => {
                self.schedule_persist_prefs(cx);
            }
            _ => {}
        }
    }

    /// 打开停靠详情面板并切换到该任务（双击无法直接打开文件的行、右键「详情」）。
    pub(crate) fn open_detail_for(
        &mut self,
        key: RowKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !key.is_local() {
            return;
        }
        self.bind_detail(key, window, cx);
        if !self.table_state.read(cx).delegate().prefs().detail_open {
            self.mutate_prefs(|prefs| prefs.detail_open = true, cx);
        }
    }

    /// 行尾「详情」按钮：像普通单击一样只选中该行（停靠面板跟随单选，先选中才不会被
    /// 跟随逻辑切回旧选中项），再打开详情面板。
    pub(crate) fn show_row_detail(
        &mut self,
        key: RowKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.table_state.update(cx, |table, cx| {
            table
                .delegate_mut()
                .select_task(key.clone(), gpui::Modifiers::default());
            cx.notify();
        });
        self.open_detail_for(key, window, cx);
    }

    /// 双击任务行：已完成且文件仍在下载目录 → 用系统默认程序打开；其余（含文件已被删除
    /// / 移走的已完成任务）→ 停靠详情面板查看。文件已被标记丢失时顺带重扫，文件移回后
    /// 标记自愈。远程任务没有本机文件与详情，双击无动作。
    pub(crate) fn activate_row(
        &mut self,
        key: RowKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !key.is_local() {
            return;
        }
        let Some((openable, missing)) = self
            .controller
            .store()
            .get(&key)
            .map(|row| (row.has_local_file(), row.is_file_missing()))
        else {
            return;
        };
        if openable {
            self.execute_commands(
                vec![DownloadsCommand::OpenTask {
                    task_id: key.task_id().to_owned(),
                }],
                cx,
            );
            return;
        }
        if missing {
            self.rescan_files_now(cx);
        }
        self.open_detail_for(key, window, cx);
    }

    /// 可合并的文件跟踪重扫（获焦触发），见 [`RescanThrottle`]。
    fn request_file_rescan(&mut self, cx: &mut Context<Self>) {
        match self.file_rescan.request(Instant::now()) {
            RescanDecision::Now => self.send_file_rescan(cx),
            RescanDecision::After(delay) => {
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(delay).await;

                    let Ok(()) = this.update(cx, |this, cx| {
                        this.file_rescan.trailing_fired(Instant::now());
                        this.send_file_rescan(cx);
                    }) else {
                        // 下载页已释放，结束回调，不再提交请求或刷新状态。
                        return;
                    };
                })
                .detach();
            }
            RescanDecision::Coalesced => {}
        }
    }

    /// 立即重扫：打开 / 拖出时发现文件已不在，行上的丢失标记要尽快跟上磁盘现状。
    pub(crate) fn rescan_files_now(&mut self, cx: &mut Context<Self>) {
        self.file_rescan.record_immediate(Instant::now());
        self.send_file_rescan(cx);
    }

    /// 结果经 `fileMissingChanged` 事件回流；失败（daemon 断开）不打扰用户，daemon 自身
    /// 的定时扫描兜底。不走 `execute_commands`，以免清掉页面横幅上的真实错误。
    fn send_file_rescan(&self, cx: &mut Context<Self>) {
        let future = self.controller.execute(DownloadsCommand::RescanFiles);
        cx.background_executor()
            .spawn(async move {
                if let Err(error) = future.await {
                    // 后台定时扫描仍会兜底，不覆盖页面现有业务错误。
                    eprintln!(
                        "download file rescan request failed: {:?} ({:?})",
                        error.code, error.reason
                    );
                }
            })
            .detach();
    }

    /// 让停靠详情面板承载该任务（面板视图按需创建）；已是该任务时不做事。
    fn bind_detail(&mut self, key: RowKey, window: &mut Window, cx: &mut Context<Self>) {
        if !key.is_local() {
            return;
        }
        let task_id = key.task_id().to_owned();
        match self.detail.clone() {
            Some(detail) if detail.read(cx).task_id() == task_id => return,
            Some(detail) => {
                detail.update(cx, |detail, cx| detail.set_task(task_id, cx));
            }
            None => {
                let store = Rc::clone(self.controller.store());
                let host = self.host.clone();
                let translator = self.translator.clone();
                let port = Arc::clone(&self.port);
                let detail = cx.new(|cx| {
                    TaskDetailView::new_docked(translator, task_id, store, port, host, window, cx)
                });
                if self.controller.is_stale() {
                    detail.update(cx, |detail, cx| detail.mark_stale(cx));
                }
                self.detail = Some(detail);
            }
        }
        self.sync_detail_panel(cx);
        cx.notify();
    }

    /// 右键「详情」：在停靠面板中查看选区锚点（或第一个本地选中）任务。
    fn on_show_selected_detail(
        &mut self,
        _: &ShowSelectedDetail,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let candidate = self.table_state.read(cx).delegate().detail_candidate();
        if let Some(key) = candidate {
            self.open_detail_for(key, window, cx);
        }
    }

    fn on_toggle_detail_panel(
        &mut self,
        _: &ToggleDetailPanel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if Self::guard_letter_key(window, cx) {
            self.run_page_command(PageCommand::ToggleDetailPanel, window, cx);
        }
    }

    fn on_close_detail_panel(&mut self, cx: &mut Context<Self>) {
        self.detail = None;
        self.mutate_prefs(|prefs| prefs.detail_open = false, cx);
    }

    fn on_toggle_detail_placement(&mut self, cx: &mut Context<Self>) {
        self.mutate_prefs(
            |prefs| prefs.detail_placement = prefs.detail_placement.toggled(),
            cx,
        );
    }

    /// 面板「弹出为窗口」：交给宿主开独立任务窗口后关闭面板。
    fn on_pop_out_detail(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(detail) = self.detail.clone() else {
            return;
        };
        let task_id = detail.read(cx).task_id().to_owned();
        if let Some(opener) = self.host.open_task_window.clone() {
            opener(task_id, window, cx);
        }
        self.on_close_detail_panel(cx);
    }

    /// 视图偏好写回：300ms 防抖后合并成一次 `SetLocalPreference`。
    pub(crate) fn schedule_persist_prefs(&mut self, cx: &mut Context<Self>) {
        let generation = self.prefs_generation.get().wrapping_add(1);
        self.prefs_generation.set(generation);
        let marker = Rc::clone(&self.prefs_generation);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PREFS_DEBOUNCE).await;
            if marker.get() != generation {
                return;
            }

            let Ok(()) = this.update(cx, |this, cx| {
                let value = this.table_state.update(cx, |table, _| {
                    let delegate = table.delegate_mut();
                    let columns = delegate.column_prefs();
                    delegate.prefs_mut().columns = columns;
                    delegate.prefs().to_value()
                });
                // 自己写出的值回流时不再重载（见 `load_view_prefs`）。
                this.applied_view_prefs = Some(value.clone());
                let future = this
                    .controller
                    .execute(DownloadsCommand::SetLocalPreference {
                        key: VIEW_PREFS_KEY,
                        value,
                    });
                cx.spawn(async move |this, cx| {
                    if let Err(error) = future.await {
                        let Ok(()) = this.update(cx, |this, cx| {
                            if this.prefs_generation.get() == generation {
                                this.applied_view_prefs = None;
                            }
                            this.last_error = Some(SharedString::from(error_text(
                                this.translator.read(cx),
                                &error,
                            )));
                            cx.notify();
                        }) else {
                            // 下载页已关闭，停止回写偏好保存结果。
                            return;
                        };
                    }
                })
                .detach();
            }) else {
                // 下载页已释放，结束回调，不再提交请求或刷新状态。
                return;
            };
        })
        .detach();
    }

    pub(crate) fn mutate_prefs(
        &mut self,
        mutate: impl FnOnce(&mut ViewPrefs),
        cx: &mut Context<Self>,
    ) {
        self.table_state.update(cx, |table, cx| {
            mutate(table.delegate_mut().prefs_mut());
            table.delegate_mut().refresh_view();
            table.refresh(cx);
            cx.notify();
        });
        self.schedule_persist_prefs(cx);
        cx.notify();
    }

    /// 搜索框内容变化：150ms 防抖后写入表格代理并重算可见行。
    fn handle_search_changed(
        &mut self,
        _input: Entity<InputState>,
        event: &InputEvent,
        cx: &mut Context<Self>,
    ) {
        if !matches!(event, InputEvent::Change) {
            return;
        }
        let generation = self.search_generation.get().wrapping_add(1);
        self.search_generation.set(generation);
        let marker = Rc::clone(&self.search_generation);
        let input = self.search_input.clone();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            if marker.get() != generation {
                return;
            }

            let Ok(()) = this.update(cx, |this, cx| {
                let query = input.read(cx).value().to_string();
                this.table_state.update(cx, |table, cx| {
                    table.delegate_mut().set_query(&query);
                    if table.delegate_mut().refresh_view() {
                        table.refresh(cx);
                        // 空闲时没有别的重绘来源：不 notify 则 retained 表格停在旧结果。
                        cx.notify();
                    }
                });
            }) else {
                // 下载页已释放，结束回调，不再提交请求或刷新状态。
                return;
            };
        })
        .detach();
    }

    fn on_focus_search(&mut self, _: &FocusSearch, window: &mut Window, cx: &mut Context<Self>) {
        self.run_page_command(PageCommand::FocusSearch, window, cx);
    }

    /// 搜索框内 `escape`：清空查询并把焦点交回下载页根元素。
    pub(crate) fn on_search_escape(
        &mut self,
        _: &gpui_component::input::Escape,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.search_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.table_state.update(cx, |table, cx| {
            table.delegate_mut().set_query("");
            if table.delegate_mut().refresh_view() {
                table.refresh(cx);
                cx.notify();
            }
        });
        self.focus_handle.focus(window, cx);
    }

    /// 内联重命名对话框（单选本地任务）；确认后写 `DownloadsCommand::Rename`。
    fn on_rename_selected(
        &mut self,
        _: &RenameSelected,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selected = self.table_state.read(cx).delegate().selected_keys();
        let [key] = selected.as_slice() else {
            return;
        };
        if !key.is_local() {
            return;
        }
        let task_id = key.task_id().to_owned();
        let current_name = self
            .controller
            .store()
            .get(key)
            .map(|row| row.name.clone())
            .unwrap_or_default();
        let title = self.strings.rename_task_title.clone();
        let field_label = self.strings.col_file_name.clone();
        let placeholder = self.strings.rename_task_placeholder.clone();
        let ok_label = self.strings.confirm.clone();
        let cancel_label = self.strings.cancel.clone();
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(current_name)
                .placeholder(placeholder)
        });
        let dialog_input = input.clone();
        let this = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, cx| {
            let content_input = dialog_input.clone();
            let ok_input = dialog_input.clone();
            let this = this.clone();
            let task_id = task_id.clone();
            let field_label = field_label.clone();
            dialog
                .title(fluxdown_ui_components::dialog_title(title.clone(), cx))
                .w(px(520.))
                .content(move |content, _, cx| {
                    content.child(fluxdown_ui_components::form(cx).child(
                        fluxdown_ui_components::form_field(
                            field_label.clone(),
                            Input::new(&content_input).control(cx).w_full(),
                            None,
                            cx,
                        ),
                    ))
                })
                .footer(fluxdown_ui_components::dialog_footer(
                    Some(cancel_label.clone()),
                    ok_label.clone(),
                    fluxdown_ui_components::DialogIntent::Confirm,
                    cx,
                ))
                .on_ok(move |_, _, cx| {
                    let file_name = ok_input.read(cx).value().trim().to_owned();
                    if file_name.is_empty() {
                        return false;
                    }
                    let task_id = task_id.clone();
                    // 页面释放后没有提交命令，不能让确认框报告成功。
                    this.update(cx, |this, cx| {
                        this.execute_commands(
                            vec![DownloadsCommand::Rename { task_id, file_name }],
                            cx,
                        );
                    })
                    .is_ok()
                })
        });
        input.update(cx, |state, cx| state.focus(window, cx));
    }

    /// 插件重试挂起确认（右键菜单经 [`DownloadTableDelegate`] 回调）。
    pub(crate) fn confirm_ignore_plugin_retry(
        &mut self,
        task_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = self.strings.ignore_plugin_retry_title.clone();
        let description = self.strings.ignore_plugin_retry_msg.clone();
        let ok_label = self.strings.ignore_plugin_retry.clone();
        let cancel_label = self.strings.cancel.clone();
        let this = cx.weak_entity();
        window.open_alert_dialog(cx, move |dialog, _, cx| {
            let this = this.clone();
            let task_id = task_id.clone();
            dialog
                .title(fluxdown_ui_components::dialog_title(title.clone(), cx))
                .description(description.clone())
                .footer(fluxdown_ui_components::dialog_footer(
                    Some(cancel_label.clone()),
                    ok_label.clone(),
                    fluxdown_ui_components::DialogIntent::Confirm,
                    cx,
                ))
                .on_ok(move |_, _, cx| {
                    let task_id = task_id.clone();
                    // 页面释放后没有提交命令，不能让确认框报告成功。
                    this.update(cx, |this, cx| {
                        this.execute_commands(
                            vec![DownloadsCommand::IgnorePluginRetry { task_id }],
                            cx,
                        );
                    })
                    .is_ok()
                })
        });
    }

    /// 将当前选中的本地任务批量移动到目标队列（右键菜单「移动到队列」）。
    pub(crate) fn move_selected_to_queue(&mut self, queue_id: String, cx: &mut Context<Self>) {
        let commands: Vec<DownloadsCommand> = self
            .table_state
            .read(cx)
            .delegate()
            .selected_keys()
            .into_iter()
            .filter(RowKey::is_local)
            .map(|key| DownloadsCommand::MoveToQueue {
                task_id: key.task_id().to_owned(),
                queue_id: queue_id.clone(),
            })
            .collect();
        self.execute_commands(commands, cx);
    }

    pub(crate) fn group_pause_all(&mut self, group_id: String, cx: &mut Context<Self>) {
        self.execute_commands(vec![DownloadsCommand::GroupPause { group_id }], cx);
    }

    pub(crate) fn group_resume_all(&mut self, group_id: String, cx: &mut Context<Self>) {
        self.execute_commands(vec![DownloadsCommand::GroupResume { group_id }], cx);
    }

    /// 组内失败成员逐个恢复。
    pub(crate) fn group_retry_failed(&mut self, group_id: String, cx: &mut Context<Self>) {
        let commands: Vec<DownloadsCommand> = self
            .controller
            .store()
            .local()
            .iter()
            .filter(|task| task.group_id == group_id && task.state == TaskState::Failed)
            .map(|task| DownloadsCommand::Resume {
                task_id: task.key.task_id().to_owned(),
            })
            .collect();
        self.execute_commands(commands, cx);
    }

    /// 打开组内首个成员所在目录。
    pub(crate) fn group_open_folder(&mut self, group_id: String, cx: &mut Context<Self>) {
        let Some(task_id) = self
            .controller
            .store()
            .local()
            .iter()
            .find(|task| task.group_id == group_id)
            .map(|task| task.key.task_id().to_owned())
        else {
            return;
        };
        self.execute_commands(vec![DownloadsCommand::RevealTask { task_id }], cx);
    }

    pub(crate) fn group_copy_source_link(
        &mut self,
        group_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(url) = self
            .controller
            .group_summaries()
            .iter()
            .find(|group| group.id == group_id)
            .map(|group| group.origin_url.clone())
            .filter(|url| !url.is_empty())
        else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(url));
        window.push_notification(Notification::success(self.strings.url_copied.clone()), cx);
    }

    pub(crate) fn group_delete(&mut self, group_id: String, cx: &mut Context<Self>) {
        self.execute_commands(
            vec![DownloadsCommand::GroupDelete {
                group_id,
                delete_files: false,
            }],
            cx,
        );
    }

    /// 「删除组及文件」二次确认。
    pub(crate) fn confirm_group_delete_with_files(
        &mut self,
        group_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = self
            .controller
            .group_summaries()
            .iter()
            .find(|group| group.id == group_id)
            .map_or_else(|| group_id.clone(), |group| group.name.clone());
        let title = self.strings.group_delete_with_files.clone();
        let description = self.strings.group_delete_with_files_description(&name);
        let ok_label = self.strings.delete.clone();
        let cancel_label = self.strings.cancel.clone();
        let this = cx.weak_entity();
        window.open_alert_dialog(cx, move |dialog, _, cx| {
            let this = this.clone();
            let group_id = group_id.clone();
            dialog
                .title(fluxdown_ui_components::dialog_title(title.clone(), cx))
                .description(description.clone())
                .footer(fluxdown_ui_components::dialog_footer(
                    Some(cancel_label.clone()),
                    ok_label.clone(),
                    fluxdown_ui_components::DialogIntent::Destructive,
                    cx,
                ))
                .on_ok(move |_, _, cx| {
                    let group_id = group_id.clone();
                    // 页面释放后没有提交命令，不能让确认框报告成功。
                    this.update(cx, |this, cx| {
                        this.execute_commands(
                            vec![DownloadsCommand::GroupDelete {
                                group_id,
                                delete_files: true,
                            }],
                            cx,
                        );
                    })
                    .is_ok()
                })
        });
    }

    pub(crate) fn open_group_in_window(
        &mut self,
        group_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(opener) = self.host.open_group_window.clone() {
            opener(group_id, window, cx);
        }
    }

    /// 拖放导入：`.torrent` 按用户主动打开处理（走 BT 文件选择后建任务）；
    /// `.txt`/`.url`/`.list`（≤1MB）按行提取
    /// 受支持的链接后打开新建下载窗口预填（窗口已开则追加进表单）；其他文件类型提示不支持。
    fn on_paths_dropped(
        &mut self,
        paths: &ExternalPaths,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut torrent_commands = Vec::new();
        let mut urls = Vec::new();
        let mut unsupported = false;
        for path in paths.paths() {
            let ext = path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(str::to_ascii_lowercase);
            match ext.as_deref() {
                Some("torrent") => torrent_commands.push(DownloadsCommand::open_torrent_file(path)),
                Some("txt" | "url" | "list") => match read_drop_text_file(path) {
                    Some(text) => urls.extend(parse_drop_urls(&text)),
                    None => unsupported = true,
                },
                _ => unsupported = true,
            }
        }
        if !torrent_commands.is_empty() {
            self.execute_commands(torrent_commands, cx);
        }
        if !urls.is_empty() {
            self.open_new_download_with(urls, window, cx);
        }
        if unsupported {
            window.push_notification(self.strings.unsupported_drop_hint.clone(), cx);
        }
    }

    fn guard_letter_key(window: &mut Window, cx: &mut Context<Self>) -> bool {
        if window.has_focused_input(cx) {
            cx.propagate();
            return false;
        }
        true
    }

    fn on_select_all(&mut self, _: &SelectAllTasks, window: &mut Window, cx: &mut Context<Self>) {
        self.run_page_command(PageCommand::SelectAll, window, cx);
    }

    fn on_clear_selection(&mut self, _: &ClearSelection, _: &mut Window, cx: &mut Context<Self>) {
        self.clear_table_selection(cx);
    }

    /// 清空表格选中（Esc 快捷键与浮动选择条「取消选择」共用）。
    pub(crate) fn clear_table_selection(&mut self, cx: &mut Context<Self>) {
        self.table_state.update(cx, |table, cx| {
            table.delegate_mut().clear_selection();
            cx.notify();
        });
    }

    fn on_pause_selected(&mut self, _: &PauseSelected, _: &mut Window, cx: &mut Context<Self>) {
        self.execute_toolbar(ToolbarCommand::Pause, cx);
    }

    fn on_resume_selected(&mut self, _: &ResumeSelected, _: &mut Window, cx: &mut Context<Self>) {
        self.execute_toolbar(ToolbarCommand::Resume, cx);
    }

    fn on_toggle_pause_selected(
        &mut self,
        _: &TogglePauseSelected,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !Self::guard_letter_key(window, cx) {
            return;
        }
        let selected = self.table_state.read(cx).delegate().selected_keys();
        let store = self.controller.store();
        let any_active = selected.iter().any(|key| {
            store
                .get(key)
                .is_some_and(|row| matches!(row.state, TaskState::Downloading | TaskState::Pending))
        });
        let action = if any_active {
            ToolbarCommand::Pause
        } else {
            ToolbarCommand::Resume
        };
        self.execute_toolbar(action, cx);
    }

    fn on_delete_selected(&mut self, _: &DeleteSelected, _: &mut Window, cx: &mut Context<Self>) {
        self.execute_toolbar(ToolbarCommand::Delete, cx);
    }

    fn on_delete_selected_with_files(
        &mut self,
        _: &DeleteSelectedWithFiles,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.delete_selected_with_files(window, cx);
    }

    /// 对当前选中集合发起「删除任务和文件」（带二次确认）；空选择为 no-op。
    pub(crate) fn delete_selected_with_files(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selected = self.table_state.read(cx).delegate().selected_keys();
        if selected.is_empty() {
            return;
        }
        self.confirm_delete_with_files(selected, window, cx);
    }

    /// 「删除任务及文件」二次确认。
    pub(crate) fn confirm_delete_with_files(
        &mut self,
        keys: Vec<RowKey>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = self.strings.delete_task_and_file.clone();
        let description = self
            .strings
            .delete_with_files_description(&keys, self.controller.store());
        let ok_label = self.strings.delete.clone();
        let cancel_label = self.strings.cancel.clone();
        let this = cx.weak_entity();
        window.open_alert_dialog(cx, move |dialog, _, cx| {
            let this = this.clone();
            let keys = keys.clone();
            dialog
                .title(fluxdown_ui_components::dialog_title(title.clone(), cx))
                .description(description.clone())
                .footer(fluxdown_ui_components::dialog_footer(
                    Some(cancel_label.clone()),
                    ok_label.clone(),
                    fluxdown_ui_components::DialogIntent::Destructive,
                    cx,
                ))
                .on_ok(move |_, _, cx| {
                    // 页面释放后没有提交命令，不能让确认框报告成功。
                    this.update(cx, |this, cx| {
                        let commands = this.delete_commands(&keys, true);
                        this.execute_commands(commands, cx);
                    })
                    .is_ok()
                })
        });
    }

    fn on_pause_all(&mut self, _: &PauseAll, window: &mut Window, cx: &mut Context<Self>) {
        self.run_page_command(PageCommand::PauseAll, window, cx);
    }

    fn on_resume_all(&mut self, _: &ResumeAll, window: &mut Window, cx: &mut Context<Self>) {
        self.run_page_command(PageCommand::ResumeAll, window, cx);
    }

    fn on_open_selected(&mut self, _: &OpenSelected, _: &mut Window, cx: &mut Context<Self>) {
        self.execute_toolbar(ToolbarCommand::Open, cx);
    }

    fn on_reveal_selected(&mut self, _: &RevealSelected, _: &mut Window, cx: &mut Context<Self>) {
        self.execute_toolbar(ToolbarCommand::Reveal, cx);
    }

    fn on_open_selected_in_window(
        &mut self,
        _: &OpenSelectedInWindow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(opener) = self.host.open_task_window.clone() else {
            return;
        };
        let selected: Vec<RowKey> = self
            .table_state
            .read(cx)
            .delegate()
            .selected_keys()
            .into_iter()
            .filter(RowKey::is_local)
            .collect();
        if selected.len() > MAX_TASK_WINDOWS_PER_ACTION {
            window.push_notification(self.strings.too_many_windows_hint.clone(), cx);
        }
        for key in selected.into_iter().take(MAX_TASK_WINDOWS_PER_ACTION) {
            opener(key.task_id().to_owned(), window, cx);
        }
    }

    fn on_copy_selected_url(
        &mut self,
        _: &CopySelectedUrl,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selected = self.table_state.read(cx).delegate().selected_keys();
        let store = self.controller.store();
        let urls: Vec<String> = selected
            .iter()
            .filter_map(|key| store.get(key).map(|row| row.share_url().to_owned()))
            .collect();
        if !urls.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(urls.join("\n")));
            window.push_notification(Notification::success(self.strings.url_copied.clone()), cx);
        }
    }

    fn on_redownload_selected(
        &mut self,
        _: &RedownloadSelected,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let commands = self.redownload_commands(cx);
        self.execute_commands(commands, cx);
    }

    /// 选中集合中可重新下载的本地任务（完成 / 失败且非 `torrent-file://` 哨兵）。
    pub(crate) fn redownload_commands(&self, cx: &Context<Self>) -> Vec<DownloadsCommand> {
        let selected = self.table_state.read(cx).delegate().selected_keys();
        selected
            .iter()
            .filter(|key| key.is_local())
            .filter_map(|key| {
                let task_id = key.task_id();
                let row = self.controller.store().get(key)?;
                if !matches!(row.state, TaskState::Completed | TaskState::Failed)
                    || row.url.starts_with("torrent-file://")
                {
                    return None;
                }
                let dto = self.controller.task_dto(task_id)?;
                let request = fluxdown_protocol::CreateTaskRequest {
                    url: if dto.origin_url.is_empty() {
                        dto.url.clone()
                    } else {
                        dto.origin_url.clone()
                    },
                    file_name: dto.file_name.clone(),
                    save_dir: dto.save_dir.clone(),
                    segments: 0,
                    cookies: String::new(),
                    referrer: dto.referrer.clone(),
                    proxy_url: dto.proxy_url.clone(),
                    user_agent: String::new(),
                    queue_id: dto.queue_id.clone(),
                    checksum: dto.checksum.clone(),
                    ignore_tls_errors: dto.ignore_tls_errors,
                    headers: None,
                    torrent_b64: None,
                    method: None,
                    body: None,
                    audio_url: None,
                    start_paused: false,
                    http_user: String::new(),
                    http_password: String::new(),
                    save_site_auth: false,
                };
                Some(DownloadsCommand::Redownload(
                    Box::new(request),
                    task_id.to_owned(),
                ))
            })
            .collect()
    }

    fn on_toggle_boost_selected(
        &mut self,
        _: &ToggleBoostSelected,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selected = self.table_state.read(cx).delegate().selected_keys();
        let Some(key) = selected.iter().find(|key| key.is_local()) else {
            return;
        };
        self.execute_commands(
            vec![DownloadsCommand::ToggleBoost {
                task_id: key.task_id().to_owned(),
            }],
            cx,
        );
    }

    fn on_cycle_density(&mut self, _: &CycleDensity, window: &mut Window, cx: &mut Context<Self>) {
        if Self::guard_letter_key(window, cx) {
            self.run_page_command(PageCommand::CycleDensity, window, cx);
        }
    }

    fn on_cycle_group_by(&mut self, _: &CycleGroupBy, window: &mut Window, cx: &mut Context<Self>) {
        if Self::guard_letter_key(window, cx) {
            self.run_page_command(PageCommand::CycleGroupBy, window, cx);
        }
    }

    fn on_cycle_sort(&mut self, _: &CycleSort, window: &mut Window, cx: &mut Context<Self>) {
        if Self::guard_letter_key(window, cx) {
            self.run_page_command(PageCommand::CycleSort, window, cx);
        }
    }

    fn on_new_download(&mut self, _: &NewDownload, window: &mut Window, cx: &mut Context<Self>) {
        self.open_new_download(window, cx);
    }

    fn on_open_torrent_file(
        &mut self,
        _: &OpenTorrentFile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_page_command(PageCommand::OpenTorrentFile, window, cx);
    }

    fn open_torrent_file(&mut self, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = receiver.await else {
                return;
            };
            let commands: Vec<DownloadsCommand> = paths
                .into_iter()
                .filter(|path| {
                    path.extension()
                        .and_then(|ext| ext.to_str())
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("torrent"))
                })
                .map(|path| DownloadsCommand::open_torrent_file(&path))
                .collect();

            let Ok(()) = this.update(cx, |this, cx| this.execute_commands(commands, cx)) else {
                // 下载页已释放，结束回调，不再提交请求或刷新状态。
                return;
            };
        })
        .detach();
    }

    fn on_open_queue_manager(
        &mut self,
        _: &OpenQueueManager,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(opener) = self.host.open_queue_manager.clone() {
            opener(window, cx);
        }
    }

    fn on_clear_finished(
        &mut self,
        _: &ClearFinished,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_page_command(PageCommand::ClearFinished, window, cx);
    }

    /// 宿主直接触发的页面命令（命令面板）。键盘 / 菜单动作与这里共用同一份实现，但宿主调用
    /// 不经焦点派发，也不受「输入框聚焦时字母键让给输入框」的限制——这些命令不是按键。
    pub fn run_page_command(
        &mut self,
        command: PageCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match command {
            PageCommand::OpenTorrentFile => self.open_torrent_file(cx),
            PageCommand::PauseAll => self.execute_toolbar(ToolbarCommand::PauseAll, cx),
            PageCommand::ResumeAll => self.execute_toolbar(ToolbarCommand::ResumeAll, cx),
            PageCommand::ClearFinished => {
                let commands: Vec<DownloadsCommand> = self
                    .controller
                    .store()
                    .local()
                    .iter()
                    .filter(|row| row.state == TaskState::Completed)
                    .map(|row| DownloadsCommand::Delete {
                        task_id: row.key.task_id().to_owned(),
                        delete_files: false,
                    })
                    .collect();
                self.execute_commands(commands, cx);
            }
            PageCommand::SelectAll => self.table_state.update(cx, |table, cx| {
                table.delegate_mut().select_all_tasks();
                cx.notify();
            }),
            PageCommand::FocusSearch => self
                .search_input
                .update(cx, |input, cx| input.focus(window, cx)),
            PageCommand::CycleDensity => self.mutate_prefs(ViewPrefs::cycle_density, cx),
            PageCommand::CycleGroupBy => self.mutate_prefs(ViewPrefs::cycle_group_by, cx),
            PageCommand::CycleSort => self.mutate_prefs(ViewPrefs::cycle_sort, cx),
            PageCommand::ToggleDetailPanel => {
                self.mutate_prefs(|prefs| prefs.detail_open = !prefs.detail_open, cx);
            }
        }
    }

    /// 错误 / 断连提示：紧凑的 destructive 文字条，可关闭。
    fn render_error_strip(&self, error: SharedString, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = fluxdown_ui_theme::active_theme(cx);
        let tokens = theme.tokens();
        let spacing = tokens.spacing;
        let text_size = tokens.typography.xs.size;
        let line_height = tokens.typography.xs.line_height;
        let destructive = tokens.colors.destructive;
        let icon_size = theme.extended().icon.sm;
        let close_label = SharedString::from(self.translator.read(cx).text("close").to_owned());
        let dismiss = self.icon_action(
            "download-error-dismiss",
            close_label,
            Icon::new(FluxIcon::X).size(icon_size),
            false,
            |this, _, cx| {
                this.last_error = None;
                cx.notify();
            },
            cx,
        );
        h_flex()
            .w_full()
            .flex_none()
            .items_center()
            .gap(spacing.sm)
            .pl(spacing.md)
            .pr(spacing.xs)
            .text_size(text_size)
            .line_height(line_height)
            .text_color(destructive)
            .child(div().flex_1().min_w_0().truncate().child(error))
            .child(dismiss)
            .into_any_element()
    }

    pub(crate) fn render_main(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let surface = fluxdown_ui_theme::active_theme(cx).tokens().colors.surface;
        let prefs = self.table_state.read(cx).delegate().prefs().clone();
        let error_strip = self
            .last_error
            .clone()
            .map(|error| self.render_error_strip(error, cx));
        // 选择条覆盖在表格容器顶部的表头上，不遮挡任务行。
        let table = self
            .render_table(cx)
            .children(self.render_selection_bar(cx));
        let content = v_flex()
            .size_full()
            .min_w_0()
            .min_h_0()
            .children(error_strip)
            .child(v_flex().flex_1().min_w_0().min_h_0().child(table));

        let body: gpui::AnyElement = if prefs.detail_open {
            let panel = self.render_detail_panel(cx);
            let on_resize = cx.listener(|this, state: &Entity<ResizableState>, _, cx| {
                if let Some(size) = state.read(cx).sizes().get(1).copied() {
                    let size = f32::from(size);
                    if size > 0. {
                        this.mutate_prefs(|prefs| prefs.detail_size = size, cx);
                    }
                }
            });
            match prefs.detail_placement {
                DetailPlacement::Bottom => v_resizable("downloads-detail-split")
                    .with_state(&self.detail_resizable_state)
                    .on_resize(on_resize)
                    .child(resizable_panel().child(content))
                    .child(
                        resizable_panel()
                            .size(px(prefs.detail_size))
                            .flex_none()
                            .size_range(px(160.)..px(480.))
                            .child(panel),
                    )
                    .into_any_element(),
                DetailPlacement::Right => h_resizable("downloads-detail-split")
                    .with_state(&self.detail_resizable_state)
                    .on_resize(on_resize)
                    .child(resizable_panel().child(content))
                    .child(
                        resizable_panel()
                            .size(px(prefs.detail_size))
                            .flex_none()
                            .size_range(px(220.)..px(560.))
                            .child(panel),
                    )
                    .into_any_element(),
            }
        } else {
            content.into_any_element()
        };

        // 侧栏 | 内容的结构线由 resizable 把手绘制（主题已映射为 hairline），这里不再画边框。
        div()
            .relative()
            .size_full()
            .min_w_0()
            .min_h_0()
            .bg(surface)
            .child(body)
            .child(left_edge_probe(
                cx.weak_entity(),
                |view| view.content_left,
                |view, left| view.content_left = left,
            ))
            .into_any_element()
    }

    fn render_detail_panel(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = fluxdown_ui_theme::active_theme(cx);
        let tokens = theme.tokens().clone();
        let icon_size = theme.extended().icon.md;
        let hairline = theme.extended().colors.hairline;
        let stroke = theme.extended().stroke.thin;
        let toolbar_button = theme.density().toolbar_button;
        let tertiary = theme.extended().colors.text_tertiary;
        let placement = self
            .table_state
            .read(cx)
            .delegate()
            .prefs()
            .detail_placement;
        let translator = self.translator.read(cx);
        let title = translator.text("detail").to_owned();
        let hint = translator.text("selectTaskHint").to_owned();
        // 切换按钮指向「切换后」的位置。
        let (placement_icon, placement_label) = if placement == DetailPlacement::Bottom {
            (
                FluxIcon::PanelRight,
                translator.text("viewDetailRight").to_owned(),
            )
        } else {
            (
                FluxIcon::PanelBottom,
                translator.text("viewDetailBottom").to_owned(),
            )
        };
        let close_label = SharedString::from(translator.text("close").to_owned());
        let pop_out_label = self.strings.open_in_window.clone();
        let toggle_position = self.icon_action(
            "detail-panel-toggle-position",
            SharedString::from(placement_label),
            Icon::new(placement_icon).size(icon_size),
            false,
            |this, _, cx| this.on_toggle_detail_placement(cx),
            cx,
        );
        let pop_out = self.icon_action(
            "detail-panel-pop-out",
            pop_out_label,
            Icon::new(FluxIcon::AppWindow).size(icon_size),
            false,
            |this, window, cx| this.on_pop_out_detail(window, cx),
            cx,
        );
        let close = self.icon_action(
            "detail-panel-close",
            close_label,
            Icon::new(FluxIcon::X).size(icon_size),
            false,
            |this, _, cx| this.on_close_detail_panel(cx),
            cx,
        );

        v_flex()
            .size_full()
            .min_h_0()
            .min_w_0()
            .bg(tokens.colors.surface)
            .child(
                h_flex()
                    // 面板头：`density.toolbarButton` 的图标按钮上下各留 spacing.xxs。
                    .min_h(toolbar_button + tokens.spacing.xs)
                    .flex_none()
                    .items_center()
                    .justify_between()
                    .pl(tokens.spacing.md)
                    .pr(tokens.spacing.xxs)
                    .border_b(stroke)
                    .border_color(hairline)
                    .child(
                        div()
                            .text_size(tokens.typography.sm.size)
                            .line_height(tokens.typography.sm.line_height)
                            .font_weight(FontWeight::MEDIUM)
                            .child(title),
                    )
                    .child(
                        h_flex()
                            .gap(tokens.spacing.xxs)
                            .child(toggle_position)
                            .child(pop_out)
                            .child(close),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .when_some(self.detail.clone(), |this, detail| this.child(detail))
                    .when(self.detail.is_none(), |this| {
                        this.child(
                            v_flex()
                                .size_full()
                                .items_center()
                                .justify_center()
                                .gap(tokens.spacing.sm)
                                .child(Icon::new(FluxIcon::File).size(px(32.)).text_color(tertiary))
                                .child(
                                    div()
                                        .text_size(tokens.typography.xs.size)
                                        .line_height(tokens.typography.xs.line_height)
                                        .text_color(tokens.colors.muted_foreground)
                                        .child(hint),
                                ),
                        )
                    }),
            )
            .into_any_element()
    }
}

impl Render for DownloadView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 语言切换后同步搜索框 placeholder（InputState 只在构造时取一次）。
        if self.search_placeholder != self.strings.search_tasks_placeholder {
            self.search_placeholder = self.strings.search_tasks_placeholder.clone();
            let placeholder = self.search_placeholder.clone();
            self.search_input.update(cx, |input, cx| {
                input.set_placeholder(placeholder, window, cx);
            });
        }
        v_flex()
            .key_context(KEY_CONTEXT)
            .size_full()
            .min_w_0()
            .min_h_0()
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::on_select_all))
            .on_action(cx.listener(Self::on_clear_selection))
            .on_action(cx.listener(Self::on_pause_selected))
            .on_action(cx.listener(Self::on_resume_selected))
            .on_action(cx.listener(Self::on_toggle_pause_selected))
            .on_action(cx.listener(Self::on_delete_selected))
            .on_action(cx.listener(Self::on_delete_selected_with_files))
            .on_action(cx.listener(Self::on_pause_all))
            .on_action(cx.listener(Self::on_resume_all))
            .on_action(cx.listener(Self::on_open_selected))
            .on_action(cx.listener(Self::on_reveal_selected))
            .on_action(cx.listener(Self::on_open_selected_in_window))
            .on_action(cx.listener(Self::on_show_selected_detail))
            .on_action(cx.listener(Self::on_copy_selected_url))
            .on_action(cx.listener(Self::on_redownload_selected))
            .on_action(cx.listener(Self::on_toggle_boost_selected))
            .on_action(cx.listener(Self::on_cycle_density))
            .on_action(cx.listener(Self::on_cycle_group_by))
            .on_action(cx.listener(Self::on_cycle_sort))
            .on_action(cx.listener(Self::on_new_download))
            .on_action(cx.listener(Self::on_open_torrent_file))
            .on_action(cx.listener(Self::on_open_queue_manager))
            .on_action(cx.listener(Self::on_clear_finished))
            .on_action(cx.listener(Self::on_focus_search))
            .on_action(cx.listener(Self::on_rename_selected))
            .on_action(cx.listener(Self::on_toggle_detail_panel))
            .on_drop(cx.listener(Self::on_paths_dropped))
            .drag_over::<ExternalPaths>(|style, _, _, cx| {
                let tokens = fluxdown_ui_theme::active_theme(cx).tokens();
                style.bg(tokens.colors.accent.opacity(0.2))
            })
            .child(self.render_sidebar_layout(window, cx))
            .child(self.render_status_bar(cx))
    }
}

/// 拖入 `.txt`/`.url`/`.list` 文件（≤1MB）；超限或读取失败返回 `None`。
fn read_drop_text_file(path: &std::path::Path) -> Option<String> {
    const MAX_BYTES: u64 = 1_048_576;
    let metadata = std::fs::metadata(path).ok()?;
    if metadata.len() > MAX_BYTES {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

/// 纯函数：从拖入文本按行提取受支持协议的下载链接（`http(s)://`、
/// `ftp(s)://`、`magnet:`、`ed2k://`）。
pub(crate) fn parse_drop_urls(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| is_supported_drop_url(line))
        .map(str::to_owned)
        .collect()
}

fn is_supported_drop_url(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("ftp://")
        || lower.starts_with("ftps://")
        || lower.starts_with("magnet:")
        || lower.starts_with("ed2k://")
}

#[cfg(test)]
mod drop_tests {
    use super::parse_drop_urls;

    #[test]
    fn extracts_only_supported_url_lines() {
        let text = "  https://example.com/a.zip  \n\
             not a url\n\
             \n\
             magnet:?xt=urn:btih:abc\n\
             ftp://files.example.com/b.iso\n\
             ed2k://|file|c.bin|123|abcdef|/\n\
             javascript:alert(1)\n";
        assert_eq!(
            parse_drop_urls(text),
            vec![
                "https://example.com/a.zip".to_owned(),
                "magnet:?xt=urn:btih:abc".to_owned(),
                "ftp://files.example.com/b.iso".to_owned(),
                "ed2k://|file|c.bin|123|abcdef|/".to_owned(),
            ]
        );
    }

    #[test]
    fn empty_or_unsupported_text_yields_no_urls() {
        assert!(parse_drop_urls("").is_empty());
        assert!(parse_drop_urls("just some notes\nno links here").is_empty());
    }

    #[test]
    fn is_case_insensitive_for_scheme() {
        assert_eq!(
            parse_drop_urls("HTTPS://EXAMPLE.COM/file"),
            vec!["HTTPS://EXAMPLE.COM/file".to_owned()]
        );
    }
}
