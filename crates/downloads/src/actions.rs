//! 下载能力的键盘 / 菜单动作。键位与菜单装配由 app 完成；本 crate 只定义动作并
//! 在 [`crate::DownloadView`] 根元素上注册处理器。

use gpui::actions;

actions!(
    downloads,
    [
        NewDownload,
        OpenTorrentFile,
        PauseSelected,
        ResumeSelected,
        TogglePauseSelected,
        DeleteSelected,
        DeleteSelectedWithFiles,
        SelectAllTasks,
        /// 以当前选区进入显式多选模式（右键「选择」）。
        EnterMultiSelect,
        ClearSelection,
        PauseAll,
        ResumeAll,
        OpenSelected,
        RevealSelected,
        OpenSelectedInWindow,
        /// 在停靠详情面板中查看选中任务（右键「详情」）。
        ShowSelectedDetail,
        CopySelectedUrl,
        RenameSelected,
        /// 更换选中任务的下载链接（右键「更换链接」）。
        ChangeSelectedUrl,
        /// 修改选中任务的线程数（右键「修改线程数」）。
        ChangeSelectedSegments,
        RedownloadSelected,
        ToggleBoostSelected,
        FocusSearch,
        CycleDensity,
        CycleGroupBy,
        CycleSort,
        ToggleDetailPanel,
        OpenQueueManager,
        ClearFinished,
    ]
);

/// 下载页根元素的键位上下文名。
pub const KEY_CONTEXT: &str = "DownloadView";
