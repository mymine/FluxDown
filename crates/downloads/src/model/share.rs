//! FluxMesh（临时文件分享）在下载页的只读投影与入口可用性判定。
//!
//! 下载页不依赖FluxMesh capability crate：只从 `AgentSnapshot.share` 抽出入口门控所需的
//! `available` / `can_send` 与任务事实（`task_facts`），其余（我的分享、网络状态…）不进下载页状态。

use std::collections::HashMap;

use fluxdown_protocol::{ShareProgressDto, ShareProvideState, ShareStateDto, ShareTaskFactsDto};

use super::TaskState;

/// FluxMesh分区在下载页关心的切片。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ShareFacts {
    available: bool,
    can_send: bool,
    /// 云端取件码位数（0 = 未知，按默认 6 位）。
    pickup_code_length: u8,
    tasks: HashMap<String, ShareTaskFactsDto>,
}

impl ShareFacts {
    pub(crate) fn from_state(state: &ShareStateDto) -> Self {
        Self {
            available: state.available,
            can_send: state.can_send,
            pickup_code_length: state.policy_summary.pickup_code_length,
            tasks: state
                .task_facts
                .iter()
                .map(|(id, facts)| (id.clone(), facts.clone()))
                .collect(),
        }
    }

    /// FluxMesh在本构建可用；为 `false` 时下载页隐藏全部FluxMesh入口。
    pub(crate) fn available(&self) -> bool {
        self.available
    }

    /// 本机当前可发起分享（已登录，或云端允许游客发起）。
    pub(crate) fn can_send(&self) -> bool {
        self.can_send
    }

    /// 云端取件码位数；0 = 未知（新建下载框按默认 6 位识别）。
    pub(crate) fn pickup_code_length(&self) -> u8 {
        self.pickup_code_length
    }

    /// 任务的FluxMesh事实；FluxMesh 不可用（未启用 / 云端未开放）时一律没有，徽标 / 停止共享 / 传输诊断随之隐藏。
    pub(crate) fn task(&self, task_id: &str) -> Option<&ShareTaskFactsDto> {
        if !self.available {
            return None;
        }
        self.tasks.get(task_id)
    }

    /// 该任务所属接收记录仍在共享（可「停止共享」）时返回记录 ID。
    pub(crate) fn providing_record(&self, task_id: &str) -> Option<&str> {
        let facts = self.task(task_id)?;
        if facts.provide_state != ShareProvideState::Providing {
            return None;
        }
        facts.record_id.as_deref()
    }

    /// 该任务是FluxMesh接收任务（才有来源徽标 / 传输诊断页）。
    pub(crate) fn is_share_receive(&self, task_id: &str) -> bool {
        self.task(task_id)
            .is_some_and(|facts| facts.is_share_receive)
    }

    /// 应用一条会话进度（与 `ShareStateDto::apply_progress` 同语义：只刷新匹配会话的任务路径）。
    /// 返回任务事实是否真的变了——高频进度帧多数不改路径，不应触发表格重绘。
    pub(crate) fn apply_progress(&mut self, progress: &ShareProgressDto) -> bool {
        let mut changed = false;
        for facts in self.tasks.values_mut() {
            if facts.session_id.as_deref() == Some(progress.session_id.as_str())
                && facts.path_kind != progress.path_kind
            {
                facts.path_kind = progress.path_kind;
                changed = true;
            }
        }
        changed
    }
}

/// 一个任务行与「能否分享」相关的事实。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ShareRow {
    /// 本机任务（非云端 / 直连设备镜像行）。
    pub(crate) is_local: bool,
    pub(crate) state: TaskState,
    /// 已完成但产物已不在下载目录。
    pub(crate) file_missing: bool,
}

/// 宿主注入了哪些FluxMesh入口（`None` 的入口对应控件隐藏 / 无动作）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ShareHostCaps {
    /// 「分享」：打开分享面板。
    pub(crate) send: bool,
    /// 「切换桥接」。
    pub(crate) switch_bridge: bool,
    /// 「设置 › 账户」额度：打开账户页的额度区。
    pub(crate) open_usage: bool,
    /// 「停止共享」：停止已接收文件的共享。
    pub(crate) stop_providing: bool,
}

/// 分享入口被禁用的原因（决定悬浮提示）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ShareBlock {
    Incomplete,
    Missing,
    Remote,
    /// 本机当前不能发起分享（未登录且云端不允许游客发起）。
    SignedOut,
    /// 多选里既有可分享的也有不可分享的任务（或各自原因不同）。
    Selection,
}

/// 分享入口的呈现：隐藏 / 可点击 / 禁用并说明原因。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ShareControl {
    Hidden,
    Enabled,
    Disabled(ShareBlock),
}

impl ShareControl {
    pub(crate) fn is_visible(self) -> bool {
        self != Self::Hidden
    }

    pub(crate) fn is_enabled(self) -> bool {
        self == Self::Enabled
    }

    pub(crate) fn block(self) -> Option<ShareBlock> {
        match self {
            Self::Disabled(block) => Some(block),
            Self::Hidden | Self::Enabled => None,
        }
    }
}

/// 单个任务自身不可分享的原因（与登录态无关）；可分享返回 `None`。
fn row_block(row: &ShareRow) -> Option<ShareBlock> {
    if !row.is_local {
        Some(ShareBlock::Remote)
    } else if row.state != TaskState::Completed {
        Some(ShareBlock::Incomplete)
    } else if row.file_missing {
        Some(ShareBlock::Missing)
    } else {
        None
    }
}

/// 选区的分享入口状态。`visible` = FluxMesh可用且宿主提供了入口；否则一律隐藏。
/// 单选给出该任务的具体原因；多选时全部任务原因一致才沿用，否则统一为
/// [`ShareBlock::Selection`]。全部可分享但本机不能发起时为 [`ShareBlock::SignedOut`]。
pub(crate) fn share_control(visible: bool, can_send: bool, rows: &[ShareRow]) -> ShareControl {
    if !visible || rows.is_empty() {
        return ShareControl::Hidden;
    }
    let mut blocks = rows.iter().map(row_block);
    let first = blocks.next().flatten();
    let rest: Vec<Option<ShareBlock>> = blocks.collect();
    let uniform = rest.iter().all(|block| *block == first);
    match (first, uniform) {
        (None, true) if can_send => ShareControl::Enabled,
        (None, true) => ShareControl::Disabled(ShareBlock::SignedOut),
        (Some(block), true) => ShareControl::Disabled(block),
        (_, false) => ShareControl::Disabled(ShareBlock::Selection),
    }
}

#[cfg(test)]
mod tests {
    use fluxdown_protocol::{SharePathKind, ShareProgressDto, ShareStateDto, ShareTaskFactsDto};

    use super::*;

    fn row(is_local: bool, state: TaskState, file_missing: bool) -> ShareRow {
        ShareRow {
            is_local,
            state,
            file_missing,
        }
    }

    fn done() -> ShareRow {
        row(true, TaskState::Completed, false)
    }

    #[test]
    fn hidden_without_visibility_or_rows() {
        assert_eq!(share_control(false, true, &[done()]), ShareControl::Hidden);
        assert_eq!(share_control(true, true, &[]), ShareControl::Hidden);
    }

    #[test]
    fn single_row_reports_its_own_reason() {
        let control = |row| share_control(true, true, &[row]);
        assert_eq!(control(done()), ShareControl::Enabled);
        assert_eq!(
            control(row(true, TaskState::Downloading, false)),
            ShareControl::Disabled(ShareBlock::Incomplete)
        );
        assert_eq!(
            control(row(true, TaskState::Completed, true)),
            ShareControl::Disabled(ShareBlock::Missing)
        );
        assert_eq!(
            control(row(false, TaskState::Completed, false)),
            ShareControl::Disabled(ShareBlock::Remote)
        );
        // 远程优先于未完成：远程任务的状态不由本机决定。
        assert_eq!(
            control(row(false, TaskState::Downloading, false)),
            ShareControl::Disabled(ShareBlock::Remote)
        );
    }

    #[test]
    fn signed_out_only_blocks_otherwise_shareable_selections() {
        assert_eq!(
            share_control(true, false, &[done(), done()]),
            ShareControl::Disabled(ShareBlock::SignedOut)
        );
        assert_eq!(
            share_control(true, false, &[row(true, TaskState::Paused, false)]),
            ShareControl::Disabled(ShareBlock::Incomplete)
        );
    }

    #[test]
    fn multi_select_uses_intersection_semantics() {
        let paused = row(true, TaskState::Paused, false);
        assert_eq!(
            share_control(true, true, &[done(), done(), done()]),
            ShareControl::Enabled
        );
        // 一个不可分享就整体禁用，原因混合统一成「选区」。
        assert_eq!(
            share_control(true, true, &[done(), paused]),
            ShareControl::Disabled(ShareBlock::Selection)
        );
        assert_eq!(
            share_control(true, true, &[paused, done()]),
            ShareControl::Disabled(ShareBlock::Selection)
        );
        assert_eq!(
            share_control(true, true, &[paused, row(true, TaskState::Completed, true)]),
            ShareControl::Disabled(ShareBlock::Selection)
        );
        // 全部同一原因时沿用该原因。
        assert_eq!(
            share_control(true, true, &[paused, paused]),
            ShareControl::Disabled(ShareBlock::Incomplete)
        );
    }

    fn facts_state() -> ShareStateDto {
        let mut state = ShareStateDto {
            available: true,
            can_send: true,
            ..ShareStateDto::default()
        };
        state.task_facts.insert(
            "t1".into(),
            ShareTaskFactsDto {
                is_share_receive: true,
                path_kind: SharePathKind::Relay,
                session_id: Some("s1".into()),
                ..ShareTaskFactsDto::default()
            },
        );
        state
    }

    #[test]
    fn facts_project_the_snapshot_slice() {
        let facts = ShareFacts::from_state(&facts_state());
        assert!(facts.available() && facts.can_send());
        assert!(facts.is_share_receive("t1"));
        assert!(!facts.is_share_receive("other"));
        assert_eq!(
            facts.task("t1").map(|f| f.path_kind),
            Some(SharePathKind::Relay)
        );
        assert_eq!(ShareFacts::default().task("t1"), None);
    }

    #[test]
    fn pickup_code_length_follows_the_policy_summary() {
        let mut state = facts_state();
        assert_eq!(ShareFacts::from_state(&state).pickup_code_length(), 0);
        state.policy_summary.pickup_code_length = 8;
        assert_eq!(ShareFacts::from_state(&state).pickup_code_length(), 8);
    }

    #[test]
    fn providing_record_needs_providing_state_and_a_record_id() {
        let providing = |state, record: Option<&str>| {
            let mut snapshot = facts_state();
            snapshot.task_facts.insert(
                "t2".into(),
                ShareTaskFactsDto {
                    is_share_receive: true,
                    provide_state: state,
                    record_id: record.map(str::to_owned),
                    ..ShareTaskFactsDto::default()
                },
            );
            ShareFacts::from_state(&snapshot)
                .providing_record("t2")
                .map(str::to_owned)
        };
        assert_eq!(
            providing(ShareProvideState::Providing, Some("r1")),
            Some("r1".to_owned())
        );
        assert_eq!(providing(ShareProvideState::Providing, None), None);
        assert_eq!(providing(ShareProvideState::Stopped, Some("r1")), None);
        assert_eq!(providing(ShareProvideState::Off, Some("r1")), None);
        assert_eq!(ShareFacts::default().providing_record("t2"), None);
    }

    #[test]
    fn progress_updates_only_matching_session_and_reports_real_changes() {
        let mut facts = ShareFacts::from_state(&facts_state());
        let progress = |session: &str, path_kind| ShareProgressDto {
            session_id: session.into(),
            path_kind,
            ..ShareProgressDto::default()
        };
        assert!(!facts.apply_progress(&progress("zzz", SharePathKind::DirectWan)));
        assert!(!facts.apply_progress(&progress("s1", SharePathKind::Relay)));
        assert!(facts.apply_progress(&progress("s1", SharePathKind::DirectWan)));
        assert_eq!(
            facts.task("t1").map(|f| f.path_kind),
            Some(SharePathKind::DirectWan)
        );
    }
}
