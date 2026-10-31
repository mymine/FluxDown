//! 接收确认窗口的纯模型：输入判别、文件勾选、来源行、请求参数与重试策略。

use fluxdown_protocol::{
    ErrorReason, RpcErrorData, ShareFileDto, SharePolicySummaryDto, ShareReceiveParams,
    ShareSourcesDto,
};

use super::pickup::{DEFAULT_PICKUP_CODE_LEN, ReceiveInput, parse_receive_input};

/// 输入框内容的判别结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputStage {
    /// 空（全空白）：只显示可编辑输入态。
    Empty,
    /// 既不是取件码也不是分享链接。
    Invalid,
    Valid(ReceiveInput),
}

/// 云端策略的取件码位数；未下发（0）取默认值。
#[must_use]
pub fn pickup_code_len(policy: &SharePolicySummaryDto) -> usize {
    match usize::from(policy.pickup_code_length) {
        0 => DEFAULT_PICKUP_CODE_LEN,
        len => len,
    }
}

/// 判别输入框文本。
#[must_use]
pub fn classify_input(text: &str, code_len: usize) -> InputStage {
    if text.trim().is_empty() {
        return InputStage::Empty;
    }
    parse_receive_input(text, code_len).map_or(InputStage::Invalid, InputStage::Valid)
}

/// 文件清单的勾选状态（按 `files` 下标）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileSelection {
    selected: Vec<bool>,
}

impl FileSelection {
    /// 全部选中。
    #[must_use]
    pub fn all(len: usize) -> Self {
        Self {
            selected: vec![true; len],
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.selected.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.selected.is_empty()
    }

    #[must_use]
    pub fn is_selected(&self, index: usize) -> bool {
        self.selected.get(index).copied().unwrap_or(false)
    }

    /// 翻转一项；越界忽略。
    pub fn toggle(&mut self, index: usize) {
        if let Some(flag) = self.selected.get_mut(index) {
            *flag = !*flag;
        }
    }

    pub fn set_all(&mut self, on: bool) {
        self.selected.fill(on);
    }

    #[must_use]
    pub fn selected_count(&self) -> usize {
        self.selected.iter().filter(|flag| **flag).count()
    }

    /// 非空且全部选中。
    #[must_use]
    pub fn all_selected(&self) -> bool {
        !self.selected.is_empty() && self.selected.iter().all(|flag| *flag)
    }

    /// 至少选中一项（才允许开始接收）。
    #[must_use]
    pub fn any_selected(&self) -> bool {
        self.selected.iter().any(|flag| *flag)
    }

    /// 选中文件的总字节数（饱和求和）。
    #[must_use]
    pub fn selected_bytes(&self, files: &[ShareFileDto]) -> u64 {
        files
            .iter()
            .zip(&self.selected)
            .filter(|(_, selected)| **selected)
            .fold(0u64, |sum, (file, _)| sum.saturating_add(file.size))
    }

    /// `agent.share.receive` 的 `fileIndexes`：全部选中时为 `None`（接收全部），否则是选中的下标。
    #[must_use]
    pub fn file_indexes(&self) -> Option<Vec<u32>> {
        if self.all_selected() || self.selected.is_empty() {
            return None;
        }
        Some(
            self.selected
                .iter()
                .enumerate()
                .filter(|(_, selected)| **selected)
                .filter_map(|(index, _)| u32::try_from(index).ok())
                .collect(),
        )
    }
}

/// 路径输入只做 `trim`（不归一大小写 / 分隔符）；空为 `None`（用 daemon 默认目录）。
#[must_use]
pub fn normalized_save_dir(dir: Option<&str>) -> Option<String> {
    dir.map(str::trim)
        .filter(|dir| !dir.is_empty())
        .map(str::to_owned)
}

/// 凭 `resolveId` 接收的请求参数。
#[must_use]
pub fn receive_params(
    resolve_id: &str,
    save_dir: Option<&str>,
    selection: &FileSelection,
) -> ShareReceiveParams {
    ShareReceiveParams {
        resolve_id: Some(resolve_id.to_owned()),
        code_or_link: None,
        save_dir: normalized_save_dir(save_dir),
        file_indexes: selection.file_indexes(),
    }
}

/// 来源行的语气：决定状态点颜色。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceTone {
    /// 可用。
    Good,
    /// 当前不可用（分享方离线）。
    Warn,
    /// 状态未知。
    Unknown,
}

/// 「当前可用来源」的一行。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceLine {
    pub key: &'static str,
    pub tone: SourceTone,
    /// 文案里的 `{count}`（共享设备数等）；没有占位符的行为 `None`。
    pub count: Option<u32>,
}

/// 「当前可用来源」要显示的行：分享方设备状态始终显示，云端副本 / 桥接只在可用时显示。
#[must_use]
pub fn source_lines(sources: &ShareSourcesDto) -> Vec<SourceLine> {
    let mut lines = vec![match sources.sender_online {
        Some(true) => SourceLine {
            key: "shareSourceSenderOnline",
            tone: SourceTone::Good,
            count: None,
        },
        Some(false) => SourceLine {
            key: "shareSourceSenderOffline",
            tone: SourceTone::Warn,
            count: None,
        },
        None => SourceLine {
            key: "shareSourceSenderUnknown",
            tone: SourceTone::Unknown,
            count: None,
        },
    }];
    if sources.cloud_copy {
        lines.push(SourceLine {
            key: "shareSourceCloud",
            tone: SourceTone::Good,
            count: None,
        });
    }
    if sources.bridge {
        lines.push(SourceLine {
            key: "shareSourceBridge",
            tone: SourceTone::Good,
            count: None,
        });
    }
    if sources.swarm_peers > 0 {
        lines.push(SourceLine {
            key: "shareSourceSwarm",
            tone: SourceTone::Good,
            count: Some(sources.swarm_peers),
        });
    }
    lines
}

/// 分享方设备名；空 / 全空白为 `None`（显示「未知设备」）。
#[must_use]
pub fn sender_name(name: &str) -> Option<&str> {
    let name = name.trim();
    (!name.is_empty()).then_some(name)
}

/// 把来自对端 / 文件系统的名称压成单行展示文本：换行、制表等控制字符换成空格（保留连续空格与
/// 其余字符，按字符而不是字节处理）。
#[must_use]
pub fn single_line(text: &str) -> String {
    text.chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect()
}

/// 失败发生在哪一步。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailedStep {
    Resolve,
    Receive,
}

/// 点「重试」要重做的步骤。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetryAction {
    Resolve,
    Receive,
}

/// 云端要求人机验证：应用内无法完成，只给出提示。
#[must_use]
pub fn needs_turnstile(error: &RpcErrorData) -> bool {
    error.reason == Some(ErrorReason::ShareTurnstileRequired)
}

/// `resolveId` 是 agent 内存里的短时句柄，过期 / 被清理后凭它接收会报找不到或已过期。
fn handle_lost(error: &RpcErrorData) -> bool {
    matches!(
        error.reason,
        Some(ErrorReason::ShareNotFound | ErrorReason::ShareExpired)
    )
}

/// 重试策略：解析失败重新解析；接收失败默认重做接收，句柄失效且还有原始输入可重新解析时重新解析。
#[must_use]
pub fn retry_action(step: FailedStep, error: &RpcErrorData, can_resolve: bool) -> RetryAction {
    match step {
        FailedStep::Resolve => RetryAction::Resolve,
        FailedStep::Receive if can_resolve && handle_lost(error) => RetryAction::Resolve,
        FailedStep::Receive => RetryAction::Receive,
    }
}

#[cfg(test)]
mod tests {
    use fluxdown_protocol::{
        ApplicationErrorCode, ErrorReason, RpcErrorData, ShareFileDto, SharePolicySummaryDto,
        ShareSourcesDto,
    };

    use super::{
        FailedStep, FileSelection, InputStage, RetryAction, SourceTone, classify_input,
        needs_turnstile, normalized_save_dir, pickup_code_len, receive_params, retry_action,
        sender_name, single_line, source_lines,
    };
    use crate::model::pickup::ReceiveInput;

    fn files(sizes: &[u64]) -> Vec<ShareFileDto> {
        sizes
            .iter()
            .enumerate()
            .map(|(index, size)| ShareFileDto {
                name: format!("f{index}"),
                size: *size,
            })
            .collect()
    }

    #[test]
    fn input_is_empty_invalid_or_valid() {
        assert_eq!(classify_input("", 6), InputStage::Empty);
        assert_eq!(classify_input("  \n ", 6), InputStage::Empty);
        assert_eq!(classify_input("hello", 6), InputStage::Invalid);
        assert_eq!(classify_input("12345", 6), InputStage::Invalid);
        assert_eq!(
            classify_input(" 482 931 ", 6),
            InputStage::Valid(ReceiveInput::Code("482931".into()))
        );
        assert_eq!(
            classify_input("https://fluxdown.com/s/abc", 6),
            InputStage::Valid(ReceiveInput::Link("https://fluxdown.com/s/abc".into()))
        );
    }

    #[test]
    fn code_length_comes_from_policy_with_a_default() {
        let mut policy = SharePolicySummaryDto::default();
        assert_eq!(pickup_code_len(&policy), 6);
        policy.pickup_code_length = 8;
        assert_eq!(pickup_code_len(&policy), 8);
    }

    #[test]
    fn selection_starts_full_and_reports_indexes_only_when_partial() {
        let list = files(&[10, 20, 30]);
        let mut selection = FileSelection::all(list.len());
        assert!(selection.all_selected());
        assert_eq!(selection.selected_count(), 3);
        assert_eq!(selection.selected_bytes(&list), 60);
        assert_eq!(selection.file_indexes(), None);

        selection.toggle(1);
        assert!(!selection.all_selected());
        assert!(selection.any_selected());
        assert_eq!(selection.selected_bytes(&list), 40);
        assert_eq!(selection.file_indexes(), Some(vec![0, 2]));

        selection.set_all(false);
        assert!(!selection.any_selected());
        assert_eq!(selection.file_indexes(), Some(Vec::new()));
        assert_eq!(selection.selected_bytes(&list), 0);

        selection.set_all(true);
        assert_eq!(selection.file_indexes(), None);
        // 越界下标忽略。
        selection.toggle(99);
        assert!(selection.all_selected());
        assert!(!selection.is_selected(99));
    }

    #[test]
    fn empty_and_huge_file_lists_are_total() {
        let empty = FileSelection::all(0);
        assert!(empty.is_empty());
        assert!(!empty.all_selected());
        assert!(!empty.any_selected());
        assert_eq!(empty.file_indexes(), None);

        let list = files(&[u64::MAX, u64::MAX]);
        let selection = FileSelection::all(2);
        assert_eq!(selection.selected_bytes(&list), u64::MAX);
    }

    #[test]
    fn receive_params_carry_the_handle_trimmed_dir_and_partial_indexes() {
        let mut selection = FileSelection::all(3);
        let all = receive_params("r1", None, &selection);
        assert_eq!(all.resolve_id.as_deref(), Some("r1"));
        assert_eq!(all.code_or_link, None);
        assert_eq!(all.save_dir, None);
        assert_eq!(all.file_indexes, None);

        selection.toggle(0);
        let partial = receive_params("r1", Some("  D:\\下载 2024\\  深层  "), &selection);
        assert_eq!(partial.save_dir.as_deref(), Some("D:\\下载 2024\\  深层"));
        assert_eq!(partial.file_indexes, Some(vec![1, 2]));

        assert_eq!(normalized_save_dir(Some("   ")), None);
        assert_eq!(normalized_save_dir(None), None);
    }

    #[test]
    fn sources_always_list_the_sender_and_only_available_extras() {
        let keys = |sources: &ShareSourcesDto| -> Vec<(&'static str, SourceTone)> {
            source_lines(sources)
                .into_iter()
                .map(|line| (line.key, line.tone))
                .collect()
        };
        let mut sources = ShareSourcesDto::default();
        assert_eq!(
            keys(&sources),
            [("shareSourceSenderUnknown", SourceTone::Unknown)]
        );
        sources.sender_online = Some(true);
        sources.cloud_copy = true;
        assert_eq!(
            keys(&sources),
            [
                ("shareSourceSenderOnline", SourceTone::Good),
                ("shareSourceCloud", SourceTone::Good)
            ]
        );
        sources.sender_online = Some(false);
        sources.bridge = true;
        assert_eq!(
            keys(&sources),
            [
                ("shareSourceSenderOffline", SourceTone::Warn),
                ("shareSourceCloud", SourceTone::Good),
                ("shareSourceBridge", SourceTone::Good)
            ]
        );
    }

    #[test]
    fn swarm_peers_add_a_counted_last_line() {
        let sources = ShareSourcesDto {
            sender_online: Some(true),
            swarm_peers: 2,
            ..ShareSourcesDto::default()
        };
        let lines = source_lines(&sources);
        let last = lines.last().copied();
        assert_eq!(
            last.map(|line| (line.key, line.tone, line.count)),
            Some(("shareSourceSwarm", SourceTone::Good, Some(2)))
        );
    }

    #[test]
    fn blank_sender_is_unknown() {
        assert_eq!(sender_name(""), None);
        assert_eq!(sender_name("   "), None);
        assert_eq!(sender_name(" MacBook "), Some("MacBook"));
    }

    fn error(reason: Option<ErrorReason>) -> RpcErrorData {
        let error = RpcErrorData::new(ApplicationErrorCode::Conflict, false);
        match reason {
            Some(reason) => error.with_reason(reason),
            None => error,
        }
    }

    #[test]
    fn retry_re_resolves_when_the_handle_is_lost_and_an_input_exists() {
        let lost = error(Some(ErrorReason::ShareExpired));
        assert_eq!(
            retry_action(FailedStep::Resolve, &lost, false),
            RetryAction::Resolve
        );
        assert_eq!(
            retry_action(FailedStep::Receive, &lost, true),
            RetryAction::Resolve
        );
        assert_eq!(
            retry_action(FailedStep::Receive, &lost, false),
            RetryAction::Receive
        );
        assert_eq!(
            retry_action(
                FailedStep::Receive,
                &error(Some(ErrorReason::ShareSenderOffline)),
                true
            ),
            RetryAction::Receive
        );
        assert_eq!(
            retry_action(FailedStep::Receive, &error(None), true),
            RetryAction::Receive
        );
    }

    #[test]
    fn turnstile_is_detected_by_reason_only() {
        assert!(needs_turnstile(&error(Some(
            ErrorReason::ShareTurnstileRequired
        ))));
        assert!(!needs_turnstile(&error(Some(
            ErrorReason::ShareRateLimited
        ))));
        assert!(!needs_turnstile(&error(None)));
    }

    #[test]
    fn control_characters_become_spaces_and_everything_else_survives() {
        assert_eq!(single_line("a\nb\r\nc\td"), "a b  c d");
        assert_eq!(single_line("  两  个 空格 😀"), "  两  个 空格 😀");
        assert_eq!(single_line(""), "");
    }
}
