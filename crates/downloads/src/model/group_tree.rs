//! 任务组「文件夹行」模型：成员计数 / 聚合状态、组内目录树展开、排序代理视图。
//!
//! 任务表把一个任务组折叠成一行（类似文件夹），展开后按组内相对目录列出成员。本模块只做
//! 纯计算，与 Web `web/src/pages/downloads/model/groupTree.ts` 逐条对齐；两端测试共用
//! `crates/downloads/tests/fixtures/group_tree_cases.json`。

use std::cmp::Ordering;

use super::{DownloadTaskView, RowKey, TaskSource, TaskState, view_prefs::natural_cmp};

/// 组成员按状态计数。准备中（引擎 status 5）计入下载中：它已占用并发名额。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct GroupCounts {
    pub(crate) total: usize,
    pub(crate) done: usize,
    pub(crate) downloading: usize,
    pub(crate) pending: usize,
    pub(crate) paused: usize,
    pub(crate) failed: usize,
}

impl GroupCounts {
    fn add(&mut self, task: &DownloadTaskView) {
        self.total += 1;
        match task.state {
            TaskState::Completed => self.done += 1,
            TaskState::Downloading => self.downloading += 1,
            TaskState::Pending if task.preparing => self.downloading += 1,
            TaskState::Pending => self.pending += 1,
            TaskState::Paused => self.paused += 1,
            TaskState::Failed => self.failed += 1,
        }
    }

    /// 组的聚合状态：有下载中 → 下载中；否则有排队 → 等待；否则有暂停 → 已暂停（用户刚暂停
    /// 整组时不能被个别失败成员盖成「失败」，失败数仍在计数行里醒目显示）；否则有失败 →
    /// 失败；否则（全部完成）→ 已完成。
    pub(crate) fn state(&self) -> TaskState {
        if self.downloading > 0 {
            TaskState::Downloading
        } else if self.pending > 0 {
            TaskState::Pending
        } else if self.paused > 0 {
            TaskState::Paused
        } else if self.failed > 0 {
            TaskState::Failed
        } else {
            TaskState::Completed
        }
    }

    /// 有成员可暂停（下载中 / 准备中 / 排队）。
    pub(crate) fn any_active(&self) -> bool {
        self.downloading > 0 || self.pending > 0
    }

    /// 有成员可继续（暂停 / 失败）。
    pub(crate) fn any_resumable(&self) -> bool {
        self.paused > 0 || self.failed > 0
    }
}

/// 组聚合值（行展示与排序共用）。大小未知的成员按 0 计入字节合计。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct GroupAggregate {
    pub(crate) counts: GroupCounts,
    pub(crate) size_bytes: u64,
    pub(crate) downloaded_bytes: u64,
    /// 下载中成员的速度合计。
    pub(crate) speed: u64,
    /// 有被「优先下载」的未完成成员（智能排序置顶）。
    pub(crate) boosted: bool,
    /// 字节进度；总大小未知时退回完成数 / 成员数。
    pub(crate) progress: f32,
    /// 速度 > 0 且未下完时的剩余秒数。
    pub(crate) eta_seconds: Option<u64>,
}

impl GroupAggregate {
    pub(crate) fn of<'a>(members: impl IntoIterator<Item = &'a DownloadTaskView>) -> Self {
        let mut aggregate = Self::default();
        for task in members {
            aggregate.counts.add(task);
            aggregate.size_bytes += task.size_bytes;
            aggregate.downloaded_bytes += task.downloaded_bytes;
            if task.state == TaskState::Downloading {
                aggregate.speed += task.speed_bytes_per_second.unwrap_or(0);
            }
            aggregate.boosted |=
                task.boosted && matches!(task.state, TaskState::Downloading | TaskState::Pending);
        }
        aggregate.progress = if aggregate.size_bytes > 0 {
            (aggregate.downloaded_bytes as f64 / aggregate.size_bytes as f64).clamp(0.0, 1.0) as f32
        } else if aggregate.counts.total > 0 {
            aggregate.counts.done as f32 / aggregate.counts.total as f32
        } else {
            0.0
        };
        aggregate.eta_seconds = (aggregate.speed > 0
            && aggregate.downloaded_bytes < aggregate.size_bytes)
            .then(|| (aggregate.size_bytes - aggregate.downloaded_bytes) / aggregate.speed);
        aggregate
    }

    pub(crate) fn state(&self) -> TaskState {
        self.counts.state()
    }
}

/// 统一为 `/` 分隔、去掉末尾分隔符（根 `/` 保留）。
fn normalize_dir(dir: &str) -> String {
    let unified = dir.trim().replace('\\', "/");
    let trimmed = unified.trim_end_matches('/');
    if trimmed.is_empty() && unified.starts_with('/') {
        "/".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// 成员在组内的相对目录（`/` 分隔，空 = 组根）：任务保存目录相对组根目录；不在组根之下
/// （用户改过保存位置等）一律视为组根。
pub(crate) fn member_dir(group_dir: &str, task_dir: &str) -> String {
    let root = normalize_dir(group_dir);
    let dir = normalize_dir(task_dir);
    if root.is_empty() || dir == root {
        return String::new();
    }
    let prefix = if root.ends_with('/') {
        root
    } else {
        format!("{root}/")
    };
    dir.strip_prefix(&prefix)
        .map(|rest| rest.trim_matches('/').to_owned())
        .unwrap_or_default()
}

/// 组显示名：组名为空时取组根目录末段，再空取组 id。
pub(crate) fn display_name(name: &str, save_dir: &str, group_id: &str) -> String {
    let name = name.trim();
    if !name.is_empty() {
        return name.to_owned();
    }
    normalize_dir(save_dir)
        .rsplit('/')
        .find(|segment| !segment.is_empty())
        .map_or_else(|| group_id.to_owned(), str::to_owned)
}

/// 目录行显示路径：不超过两段原样以 ` / ` 连接；更深只留最后两段（`… / 父目录 / 目录`）。
/// 深层目录的兄弟之间往往只有末段不同，保留末段才分得清；完整路径放在悬停提示里。
pub(crate) fn compress_path(path: &str) -> String {
    let segments: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    match segments.as_slice() {
        [.., parent, last] if segments.len() > 2 => format!("… / {parent} / {last}"),
        _ => segments.join(" / "),
    }
}

/// [`flatten`] 的输入：成员在组内的相对目录、折叠后的名称与大小。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TreeMember {
    pub(crate) dir: String,
    pub(crate) name_fold: String,
    pub(crate) size_bytes: u64,
}

/// 组展开后的一行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TreeRow {
    /// 成员（下标对应 [`flatten`] 输入）。`nested` = 位于某个目录行之下。
    Member { index: usize, nested: bool },
    /// 目录分段行：完整相对路径、该目录下的成员数与大小合计。
    Dir {
        path: String,
        count: usize,
        size_bytes: u64,
        collapsed: bool,
    },
}

/// 组进度柱状图最多的柱数（超过时等距抽样）。
pub(crate) const SPARKLINE_MAX_BARS: usize = 24;

/// 柱状图要画的成员下标：成员数不超过 `max` 时全部，否则等距抽样（`⌊i·len/max⌋`）。
pub(crate) fn sparkline_sample(len: usize, max: usize) -> Vec<usize> {
    let count = len.min(max);
    (0..count).map(|i| i * len / count).collect()
}

/// 单根柱的高度比例：至少 14%（未开始的成员也看得见），失败成员至少 1/3（醒目）。
pub(crate) fn sparkline_height(progress: f32, state: TaskState) -> f32 {
    let floor = if state == TaskState::Failed {
        1. / 3.
    } else {
        0.14
    };
    let progress = if state == TaskState::Completed {
        1.
    } else {
        progress
    };
    progress.clamp(0., 1.).max(floor)
}

/// 按目录展开组成员：组根成员在前，其后每个目录各一行（目录按自然序），目录行后跟其成员
/// （折叠的目录只保留目录行）；同一目录内按名称自然序。目录行平铺（显示完整相对路径），
/// 不做多级嵌套，每个目录只出现一次。
pub(crate) fn flatten(
    members: &[TreeMember],
    dir_collapsed: impl Fn(&str) -> bool,
) -> Vec<TreeRow> {
    let mut order: Vec<usize> = (0..members.len()).collect();
    order.sort_by(|&left, &right| {
        let (left, right) = (&members[left], &members[right]);
        compare_dirs(&left.dir, &right.dir)
            .then_with(|| natural_cmp(&left.name_fold, &right.name_fold))
    });
    let mut rows = Vec::with_capacity(members.len());
    let mut ix = 0;
    while ix < order.len() {
        let dir = &members[order[ix]].dir;
        let end = order[ix..]
            .iter()
            .position(|&member| members[member].dir != *dir)
            .map_or(order.len(), |offset| ix + offset);
        if dir.is_empty() {
            rows.extend(order[ix..end].iter().map(|&index| TreeRow::Member {
                index,
                nested: false,
            }));
        } else {
            let collapsed = dir_collapsed(dir);
            rows.push(TreeRow::Dir {
                path: dir.clone(),
                count: end - ix,
                size_bytes: order[ix..end]
                    .iter()
                    .map(|&member| members[member].size_bytes)
                    .sum(),
                collapsed,
            });
            if !collapsed {
                rows.extend(order[ix..end].iter().map(|&index| TreeRow::Member {
                    index,
                    nested: true,
                }));
            }
        }
        ix = end;
    }
    rows
}

/// 组根（空目录）最前，其余目录按大小写折叠后的自然序；折叠后相同再按原文定序。
fn compare_dirs(left: &str, right: &str) -> Ordering {
    match (left.is_empty(), right.is_empty()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (false, false) => {
            natural_cmp(&left.to_lowercase(), &right.to_lowercase()).then_with(|| left.cmp(right))
        }
    }
}

/// 表格需要的组元信息（由 `GroupDto` 投影）。
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct GroupMeta {
    /// 显示名（见 [`display_name`]）。
    pub(crate) name: String,
    pub(crate) name_fold: String,
    pub(crate) save_dir: String,
    pub(crate) source_url: String,
    pub(crate) created_at_secs: i64,
}

impl GroupMeta {
    pub(crate) fn from_dto(group: &fluxdown_protocol::GroupDto) -> Self {
        let name = display_name(&group.name, &group.save_dir, &group.group_id);
        Self {
            name_fold: name.to_lowercase(),
            name,
            save_dir: group.save_dir.clone(),
            source_url: group.source_url.clone(),
            created_at_secs: super::parse_timestamp_secs(&group.created_at),
        }
    }
}

/// 组行的排序 / 分桶代理：把组投影成一个「任务视图」，复用任务的排序比较器与分桶规则。
/// 名称 = 组显示名，大小 / 进度 / 速度 / 状态取聚合值，创建时间取组的创建时间（缺失时取
/// 成员最早的），排队键与插入序取成员最小值，来源取组的来源链接。`key` 只用于比较器的
/// 最终平局（组 id 与任务 id 都是 UUID，不会相撞），不能拿去查任务存储。
pub(crate) fn group_sort_view<'a>(
    group_id: &str,
    group: &GroupMeta,
    aggregate: &GroupAggregate,
    members: impl IntoIterator<Item = &'a DownloadTaskView>,
) -> DownloadTaskView {
    let mut created = group.created_at_secs;
    let mut queue_order = i32::MAX;
    let mut queue_key: Option<i64> = None;
    let mut queue_id: Option<&str> = None;
    for member in members {
        if group.created_at_secs <= 0 && member.created_at_secs > 0 {
            created = if created <= 0 {
                member.created_at_secs
            } else {
                created.min(member.created_at_secs)
            };
        }
        queue_order = queue_order.min(member.queue_order);
        queue_key = match (queue_key, member.queue_key) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (left, right) => left.or(right),
        };
        queue_id.get_or_insert(member.queue_id.as_str());
    }
    let state = aggregate.state();
    let mut view = DownloadTaskView::new(
        RowKey::Local(group_id.to_owned()),
        TaskSource::Local,
        queue_id.unwrap_or_default().to_owned(),
        group.name.clone(),
        i64::try_from(aggregate.size_bytes).unwrap_or(i64::MAX),
        i64::try_from(aggregate.downloaded_bytes).unwrap_or(i64::MAX),
        Some(aggregate.speed),
        created,
        match state {
            TaskState::Pending => 0,
            TaskState::Downloading => 1,
            TaskState::Paused => 2,
            TaskState::Completed => 3,
            TaskState::Failed => 4,
        },
        &group.source_url,
    );
    view.progress = aggregate.progress;
    view.progress_label = format!("{:.1}%", aggregate.progress * 100.0);
    view.eta_seconds = aggregate.eta_seconds;
    view.boosted = aggregate.boosted;
    view.queue_order = if queue_order == i32::MAX {
        0
    } else {
        queue_order
    };
    view.queue_key = queue_key;
    view.group_id = group_id.to_owned();
    view.origin_url.clone_from(&group.source_url);
    view
}

#[cfg(test)]
#[path = "group_tree_tests.rs"]
mod tests;
