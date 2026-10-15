//! 共享任务行存储：controller 增量写、表格 / 详情按下标读，不逐事件全量克隆。

use std::{
    cell::{Cell, Ref, RefCell},
    collections::HashMap,
};

use super::{DownloadTaskView, RowKey, view_prefs::ViewSortKey};

/// 行在存储中的位置（随删除会变；跨帧身份用 [`RowKey`]）。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum RowId {
    Local(usize),
    Remote(usize),
}

#[derive(Default)]
pub(crate) struct TaskStore {
    local: RefCell<Vec<DownloadTaskView>>,
    remote: RefCell<Vec<DownloadTaskView>>,
    /// 本地 task_id → `local` 下标。
    index: RefCell<HashMap<String, usize>>,
    generation: Cell<u64>,
    /// 行集合 / 下标布局变化计数（增删、整表换成不同的行）。只改行内容时不变，
    /// 表格据此判断 [`RowId`] 是否仍指向同一任务、能否沿用上次的行顺序。
    structure: Cell<u64>,
    /// 可能改变筛选 / 搜索 / 分组的行字段；整表替换也使缓存失效。
    view_fields: Cell<u64>,
    /// 各排序键实际读取的值的变化计数，不保留任务或字符串副本。
    sort_values: [Cell<u64>; ViewSortKey::CYCLE.len()],
}

impl TaskStore {
    pub(crate) fn local(&self) -> Ref<'_, [DownloadTaskView]> {
        Ref::map(self.local.borrow(), Vec::as_slice)
    }

    pub(crate) fn remote(&self) -> Ref<'_, [DownloadTaskView]> {
        Ref::map(self.remote.borrow(), Vec::as_slice)
    }

    pub(crate) fn row(&self, id: RowId) -> Option<Ref<'_, DownloadTaskView>> {
        match id {
            RowId::Local(ix) => Ref::filter_map(self.local.borrow(), |rows| rows.get(ix)).ok(),
            RowId::Remote(ix) => Ref::filter_map(self.remote.borrow(), |rows| rows.get(ix)).ok(),
        }
    }

    pub(crate) fn row_id(&self, key: &RowKey) -> Option<RowId> {
        match key {
            RowKey::Local(id) => self.find_local(id).map(RowId::Local),
            RowKey::Remote(id) => self
                .remote
                .borrow()
                .iter()
                .position(|row| row.key.task_id() == id)
                .map(RowId::Remote),
        }
    }

    pub(crate) fn get(&self, key: &RowKey) -> Option<Ref<'_, DownloadTaskView>> {
        self.row_id(key).and_then(|id| self.row(id))
    }

    pub(crate) fn find_local(&self, task_id: &str) -> Option<usize> {
        self.index.borrow().get(task_id).copied()
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation.get()
    }

    pub(crate) fn structure_generation(&self) -> u64 {
        self.structure.get()
    }

    pub(crate) fn view_fields_generation(&self) -> u64 {
        self.view_fields.get()
    }

    pub(crate) fn sort_generation(&self, key: ViewSortKey) -> u64 {
        ViewSortKey::CYCLE
            .iter()
            .position(|candidate| *candidate == key)
            .map_or_else(|| self.generation(), |ix| self.sort_values[ix].get())
    }

    fn invalidate_view_fields(&self) {
        self.view_fields.set(self.view_fields.get().wrapping_add(1));
    }

    fn bump(&self) {
        self.generation.set(self.generation.get().wrapping_add(1));
    }

    fn bump_structure(&self) {
        self.structure.set(self.structure.get().wrapping_add(1));
        self.bump();
    }

    /// 整表替换；行 key 与顺序不变时（快照 / 重连重建）只算内容变化。
    pub(crate) fn replace_local(&self, rows: Vec<DownloadTaskView>) {
        self.invalidate_view_fields();
        let same_rows = same_row_keys(&self.local.borrow(), &rows);
        if !same_rows {
            let index = rows
                .iter()
                .enumerate()
                .map(|(ix, row)| (row.key.task_id().to_owned(), ix))
                .collect();
            *self.index.borrow_mut() = index;
        }
        *self.local.borrow_mut() = rows;
        if same_rows {
            self.bump();
        } else {
            self.bump_structure();
        }
    }

    pub(crate) fn replace_remote(&self, rows: Vec<DownloadTaskView>) {
        self.invalidate_view_fields();
        let same_rows = same_row_keys(&self.remote.borrow(), &rows);
        *self.remote.borrow_mut() = rows;
        if same_rows {
            self.bump();
        } else {
            self.bump_structure();
        }
    }

    /// 覆盖单行（下标必须已存在）。
    pub(crate) fn set_local(&self, ix: usize, row: DownloadTaskView) {
        if let Some(slot) = self.local.borrow_mut().get_mut(ix) {
            if !same_view_fields(slot, &row) {
                self.invalidate_view_fields();
            }
            for (key, generation) in ViewSortKey::CYCLE.iter().zip(&self.sort_values) {
                if !key.same_value(slot, &row) {
                    generation.set(generation.get().wrapping_add(1));
                }
            }
            *slot = row;
            self.bump();
        }
    }

    pub(crate) fn push_local(&self, row: DownloadTaskView) -> usize {
        let ix = self.append_local(row);
        self.bump_structure();
        ix
    }

    /// 追加一行但不记结构变化：批量追加后由调用方 [`Self::commit_appends`] 一次登记。
    pub(crate) fn append_local(&self, row: DownloadTaskView) -> usize {
        let mut rows = self.local.borrow_mut();
        let ix = rows.len();
        self.index
            .borrow_mut()
            .insert(row.key.task_id().to_owned(), ix);
        rows.push(row);
        ix
    }

    /// 登记一批 [`Self::append_local`] 的结构变化（整批一次）。
    pub(crate) fn commit_appends(&self) {
        self.bump_structure();
    }

    /// Preserve authoritative snapshot order without replacing unchanged views.
    pub(crate) fn swap_local(&self, left: usize, right: usize) {
        let mut rows = self.local.borrow_mut();
        rows.swap(left, right);
        let mut index = self.index.borrow_mut();
        for position in [left, right] {
            let task_id = rows[position].key.task_id();
            if let Some(indexed) = index.get_mut(task_id) {
                *indexed = position;
            } else {
                eprintln!("repairing missing local task index during snapshot reorder");
                index.insert(task_id.to_owned(), position);
            }
        }
        self.bump_structure();
    }

    /// `swap_remove` 并修正被换入行的索引。
    pub(crate) fn swap_remove_local(&self, ix: usize) {
        if self.swap_remove_local_quiet(ix) {
            self.bump_structure();
        }
    }

    /// 同 [`Self::swap_remove_local`] 但不记结构变化：批量删除后由调用方
    /// [`Self::commit_removals`] 一次登记。返回是否真的移除了一行。
    pub(crate) fn swap_remove_local_quiet(&self, ix: usize) -> bool {
        let mut rows = self.local.borrow_mut();
        if ix >= rows.len() {
            return false;
        }
        let removed = rows.swap_remove(ix);
        let mut index = self.index.borrow_mut();
        index.remove(removed.key.task_id());
        if let Some(moved) = rows.get(ix) {
            index.insert(moved.key.task_id().to_owned(), ix);
        }
        true
    }

    /// 登记一批 [`Self::swap_remove_local_quiet`] 的结构变化（整批一次）。
    pub(crate) fn commit_removals(&self) {
        self.bump_structure();
    }
}

fn same_row_keys(current: &[DownloadTaskView], next: &[DownloadTaskView]) -> bool {
    current.len() == next.len() && current.iter().zip(next).all(|(a, b)| a.key == b.key)
}

/// 覆盖当前所有筛选、搜索与分组读取的字段；不确定当前视图是否受影响时保守地失效。
fn same_view_fields(left: &DownloadTaskView, right: &DownloadTaskView) -> bool {
    left.key == right.key
        && left.state == right.state
        && left.source == right.source
        && left.queue_id == right.queue_id
        && left.name == right.name
        && left.name_fold == right.name_fold
        && left.file_extension == right.file_extension
        && left.url_fold == right.url_fold
        && left.site_fold == right.site_fold
        && left.site == right.site
        && left.referrer == right.referrer
        && left.protocol == right.protocol
        && left.created_at_secs == right.created_at_secs
        && left.group_id == right.group_id
        && left.to_device == right.to_device
}
