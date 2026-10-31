//! 侧栏计数：一次扫描得出「状态 × 分类」「队列」「设备」全部桶的任务数。
//!
//! 侧栏每帧要为每个状态项、分类子项、队列、设备各显示一个数字；逐项扫描全部任务是
//! O(项数 × 任务数)。这里对全部任务只扫一遍，结果与逐项按 [`DownloadFilter::matches`] /
//! [`SidebarSelection::device_matches`] 计数逐一相同，调用方按存储 generation 缓存。

use std::collections::HashMap;

use super::{
    CategoryIndex, DownloadFilter, DownloadStatusFilter, DownloadTaskView, SidebarSelection,
    TaskSource,
};

#[derive(Default)]
pub(crate) struct SidebarCounts {
    /// 不限分类时各状态的任务数（下标见 `DownloadStatusFilter::slot`）。
    by_status: [usize; DownloadStatusFilter::COUNT],
    /// 分类 id → 各状态的任务数。只含至少命中过一个任务的分类。
    by_category: HashMap<String, [usize; DownloadStatusFilter::COUNT]>,
    /// 本地任务按队列。
    by_queue: HashMap<String, usize>,
    local: usize,
    remote: usize,
    /// 远程任务按目标设备。
    by_device: HashMap<String, usize>,
}

impl SidebarCounts {
    /// 扫描全部任务（本地 + 远程）。
    pub(crate) fn compute<'a>(
        tasks: impl IntoIterator<Item = &'a DownloadTaskView>,
        categories: &CategoryIndex,
    ) -> Self {
        let mut counts = Self::default();
        for task in tasks {
            counts.add(task, categories);
        }
        counts
    }

    /// 一个任务进入时计入各桶；与 [`Self::remove`] 互逆。
    pub(crate) fn add(&mut self, task: &DownloadTaskView, categories: &CategoryIndex) {
        let mut statuses = [None; DownloadStatusFilter::COUNT];
        for status in DownloadStatusFilter::ALL {
            if status.matches(task) {
                self.by_status[status.slot()] += 1;
                statuses[status.slot()] = Some(status);
            }
        }
        categories.for_each_match(task, |id| match self.by_category.get_mut(id) {
            Some(buckets) => bump(buckets, &statuses),
            None => {
                let mut buckets = [0; DownloadStatusFilter::COUNT];
                bump(&mut buckets, &statuses);
                self.by_category.insert(id.to_owned(), buckets);
            }
        });
        match task.source {
            TaskSource::Local => {
                self.local += 1;
                match self.by_queue.get_mut(&task.queue_id) {
                    Some(count) => *count += 1,
                    None => {
                        self.by_queue.insert(task.queue_id.clone(), 1);
                    }
                }
            }
            TaskSource::Remote => {
                self.remote += 1;
                match self.by_device.get_mut(&task.to_device) {
                    Some(count) => *count += 1,
                    None => {
                        self.by_device.insert(task.to_device.clone(), 1);
                    }
                }
            }
        }
    }

    /// 撤销 [`Self::add`] 对同一任务（同一状态 / 分类 / 队列 / 设备）的计入；清零的分类、队列、
    /// 设备桶一并移除，结果与对剩余任务重新 [`Self::compute`] 相同。
    pub(crate) fn remove(&mut self, task: &DownloadTaskView, categories: &CategoryIndex) {
        let mut statuses = [None; DownloadStatusFilter::COUNT];
        for status in DownloadStatusFilter::ALL {
            if status.matches(task) {
                let count = &mut self.by_status[status.slot()];
                *count = count.saturating_sub(1);
                statuses[status.slot()] = Some(status);
            }
        }
        categories.for_each_match(task, |id| {
            if let Some(buckets) = self.by_category.get_mut(id) {
                for status in statuses.iter().flatten() {
                    let count = &mut buckets[status.slot()];
                    *count = count.saturating_sub(1);
                }
                if buckets.iter().all(|count| *count == 0) {
                    self.by_category.remove(id);
                }
            }
        });
        match task.source {
            TaskSource::Local => {
                self.local = self.local.saturating_sub(1);
                decrement(&mut self.by_queue, &task.queue_id);
            }
            TaskSource::Remote => {
                self.remote = self.remote.saturating_sub(1);
                decrement(&mut self.by_device, &task.to_device);
            }
        }
    }

    /// 与 `tasks.filter(|task| filter.matches(task, categories)).count()` 相同。
    #[must_use]
    pub(crate) fn filter(&self, filter: &DownloadFilter) -> usize {
        match filter.category.as_deref() {
            None => self.by_status[filter.status.slot()],
            Some(category) => self
                .by_category
                .get(category)
                .map_or(0, |buckets| buckets[filter.status.slot()]),
        }
    }

    /// 本地任务中属于该队列的数量。
    #[must_use]
    pub(crate) fn queue(&self, queue_id: &str) -> usize {
        self.by_queue.get(queue_id).copied().unwrap_or(0)
    }

    /// 与 [`SidebarSelection::device_matches`] 逐任务计数相同。
    #[must_use]
    pub(crate) fn device(&self, device_id: &str) -> usize {
        match device_id {
            SidebarSelection::LOCAL_DEVICE => self.local,
            SidebarSelection::ALL_DEVICES => self.local + self.remote,
            id => self.by_device.get(id).copied().unwrap_or(0),
        }
    }
}

/// 任务命中的每个状态各记一次。
fn bump(
    buckets: &mut [usize; DownloadStatusFilter::COUNT],
    statuses: &[Option<DownloadStatusFilter>; DownloadStatusFilter::COUNT],
) {
    for status in statuses.iter().flatten() {
        buckets[status.slot()] += 1;
    }
}

/// 桶计数减一，减到零时移除桶。
fn decrement(buckets: &mut HashMap<String, usize>, key: &str) {
    if let Some(count) = buckets.get_mut(key) {
        *count = count.saturating_sub(1);
        if *count == 0 {
            buckets.remove(key);
        }
    }
}

#[cfg(test)]
mod tests {
    use fluxdown_protocol::CustomCategoryDto;

    use super::SidebarCounts;
    use crate::model::{
        CategoryIndex, DownloadFilter, DownloadStatusFilter, DownloadTaskView, SidebarSelection,
        TaskSource,
    };

    fn local(id: usize, name: &str, status: i32, queue: &str) -> DownloadTaskView {
        let dto = serde_json::from_value::<fluxdown_protocol::TaskDto>(serde_json::json!({
            "taskId": format!("t{id}"), "url": "https://example.com/x", "fileName": name,
            "saveDir": "/tmp", "status": status, "downloadedBytes": 1, "totalBytes": 2,
            "errorMessage": "", "createdAt": "1", "proxyUrl": "", "queueId": queue,
            "checksum": ""
        }))
        .expect("task");
        DownloadTaskView::local(&dto, None, false)
    }

    fn remote(id: usize, device: &str, status: i32) -> DownloadTaskView {
        let mut view = local(id, "r.bin", status, "main");
        view.source = TaskSource::Remote;
        view.to_device = device.to_owned();
        view
    }

    fn custom(id: &str, mode: &str, extensions: &[&str], pattern: &str) -> CustomCategoryDto {
        CustomCategoryDto {
            id: id.to_owned(),
            name: id.to_owned(),
            icon: "file".to_owned(),
            match_mode: mode.to_owned(),
            extensions: extensions.iter().map(|ext| (*ext).to_owned()).collect(),
            regex_pattern: pattern.to_owned(),
            url_pattern: String::new(),
            position: 0,
            visible: true,
            is_builtin: false,
            builtin_type: None,
            save_dir: String::new(),
        }
    }

    fn sample_tasks() -> Vec<DownloadTaskView> {
        let names = [
            "a.mp4",
            "b.zip",
            "c.mp3",
            "README",
            "d.iso",
            "e.mkv",
            "f.pdf",
            "g.torrent",
        ];
        let mut tasks = Vec::new();
        for (index, name) in names.iter().enumerate() {
            for status in 0..5 {
                let queue = if (index + status as usize).is_multiple_of(2) {
                    "main"
                } else {
                    "later"
                };
                tasks.push(local(tasks.len(), name, status, queue));
            }
        }
        tasks.push(remote(900, "dev-a", 1));
        tasks.push(remote(901, "dev-a", 3));
        tasks.push(remote(902, "dev-b", 2));
        tasks
    }

    /// 每个桶都与「逐任务调用 `filter.matches` / `device_matches` 再计数」的结果对照。
    fn assert_equivalent(categories: &CategoryIndex, tasks: &[DownloadTaskView]) {
        let counts = SidebarCounts::compute(tasks, categories);
        let mut category_ids: Vec<Option<String>> = vec![None, Some("missing".to_owned())];
        category_ids.extend(
            categories
                .rules()
                .iter()
                .map(|rule| Some(rule.dto.id.clone())),
        );
        for status in DownloadStatusFilter::ALL {
            for category in &category_ids {
                let filter = DownloadFilter {
                    status,
                    category: category.clone(),
                };
                let expected = tasks
                    .iter()
                    .filter(|task| filter.matches(task, categories))
                    .count();
                assert_eq!(counts.filter(&filter), expected, "{filter:?}");
            }
        }
        for queue in ["main", "later", "absent"] {
            let expected = tasks
                .iter()
                .filter(|task| task.source == TaskSource::Local && task.queue_id == queue)
                .count();
            assert_eq!(counts.queue(queue), expected, "queue {queue}");
        }
        for device in [
            SidebarSelection::LOCAL_DEVICE,
            SidebarSelection::ALL_DEVICES,
            "dev-a",
            "dev-b",
            "dev-none",
        ] {
            let expected = tasks
                .iter()
                .filter(|task| SidebarSelection::device_matches(device, task))
                .count();
            assert_eq!(counts.device(device), expected, "device {device}");
        }
    }

    #[test]
    fn one_scan_matches_per_filter_counting_for_builtin_categories() {
        let categories = CategoryIndex::from_dtos(CustomCategoryDto::builtin_defaults());
        assert_equivalent(&categories, &sample_tasks());
    }

    #[test]
    fn overlapping_regex_and_duplicate_rules_still_match_per_filter_counting() {
        // `all` / 扩展名 / 与扩展名重叠的正则 / 重复 id / `other` 并存：
        // 任务可同时属于多个分类，「其他」只在没有具体分类命中时命中。
        let mut dtos = CustomCategoryDto::builtin_defaults();
        dtos.push(custom("overlap", "regex", &[], r"\.(mp4|zip)$"));
        dtos.push(custom("builtin_video", "extension", &["zip"], ""));
        dtos.push(custom("broken", "regex", &[], "("));
        let categories = CategoryIndex::from_dtos(dtos);
        assert_equivalent(&categories, &sample_tasks());
    }

    #[test]
    fn no_categories_and_empty_store_count_zero() {
        let categories = CategoryIndex::default();
        assert_equivalent(&categories, &sample_tasks());
        assert_equivalent(&categories, &[]);
        let counts = SidebarCounts::compute(&[], &categories);
        assert_eq!(counts.filter(&DownloadFilter::ALL), 0);
    }
}
