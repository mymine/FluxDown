//! 分享面板的纯模型：阶段推导、创建时的哈希进度、有效期 / 次数候选、接收方行的展示值。

use fluxdown_protocol::{ShareDto, ShareHashingDto, ShareReceiverDto, ShareTierDto};

use super::{
    format::{CLAIM_PRESETS, CountLabel, tier_label},
    status::is_editable,
};

/// 面板自己的流程阶段（与 hub 里的分享状态相互独立）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelWait {
    /// `agent.share.create` 在途。
    Creating,
    /// 创建失败。
    Failed,
    /// 已持有分享 ID，内容以 hub 为准。
    Holding,
    /// 用户刚在本面板停止了这个分享。
    Stopped,
}

/// 面板应渲染的内容。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelPhase {
    Creating,
    Failed,
    /// 分享已不存在（hub 里找不到）。
    Gone,
    /// 有效分享：链接、设置与接收方。
    Live,
    /// 已过期 / 已停止。
    Closed,
}

/// 由流程阶段与 hub 里的分享推出渲染阶段。
#[must_use]
pub fn panel_phase(wait: PanelWait, share: Option<&ShareDto>) -> PanelPhase {
    match (wait, share) {
        (PanelWait::Creating, _) => PanelPhase::Creating,
        (PanelWait::Failed, _) => PanelPhase::Failed,
        (PanelWait::Holding | PanelWait::Stopped, None) => PanelPhase::Gone,
        (PanelWait::Stopped, Some(_)) => PanelPhase::Closed,
        (PanelWait::Holding, Some(share)) => {
            if is_editable(share) {
                PanelPhase::Live
            } else {
                PanelPhase::Closed
            }
        }
    }
}

/// 创建分享时块哈希的合计进度。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HashingProgress {
    pub done_bytes: u64,
    pub total_bytes: u64,
    /// 只有一个任务在哈希时的任务标题。
    pub title: Option<String>,
}

impl HashingProgress {
    /// `0.0..=100.0` 的进度；总量未知（0）为 `None`，显示不定进度。
    #[must_use]
    pub fn percent(&self) -> Option<f32> {
        if self.total_bytes == 0 {
            return None;
        }
        let done = self.done_bytes.min(self.total_bytes);
        Some((done as f64 / self.total_bytes as f64 * 100.) as f32)
    }
}

/// 合计与本次创建相关的哈希进度：优先只算 `ShareHashingDto.id` 属于 `task_ids` 的条目；
/// 没有一个匹配时（条目 ID 口径不同）退回合计全部条目；没有任何条目为 `None`。
#[must_use]
pub fn hashing_progress(
    hashing: &[ShareHashingDto],
    task_ids: &[String],
) -> Option<HashingProgress> {
    let matched: Vec<&ShareHashingDto> = hashing
        .iter()
        .filter(|entry| task_ids.contains(&entry.id))
        .collect();
    let entries: Vec<&ShareHashingDto> = if matched.is_empty() {
        hashing.iter().collect()
    } else {
        matched
    };
    if entries.is_empty() {
        return None;
    }
    let done_bytes = entries
        .iter()
        .fold(0u64, |sum, entry| sum.saturating_add(entry.done_bytes));
    let total_bytes = entries
        .iter()
        .fold(0u64, |sum, entry| sum.saturating_add(entry.total_bytes));
    let title = match entries.as_slice() {
        [only] if !only.title.is_empty() => Some(only.title.clone()),
        _ => None,
    };
    Some(HashingProgress {
        done_bytes,
        total_bytes,
        title,
    })
}

/// 有效期菜单里的一项。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpiryOption {
    pub tier_id: String,
    pub label: CountLabel,
    pub selected: bool,
}

/// 云端策略的有效期档位 → 菜单项（顺序同策略）。
#[must_use]
pub fn expiry_options(tiers: &[ShareTierDto], current_tier_id: &str) -> Vec<ExpiryOption> {
    tiers
        .iter()
        .map(|tier| ExpiryOption {
            tier_id: tier.id.clone(),
            label: tier_label(tier.hours),
            selected: tier.id == current_tier_id,
        })
        .collect()
}

/// 当前档位的展示；策略里已没有该档位（策略更新过）为 `None`。
#[must_use]
pub fn current_expiry_label(tiers: &[ShareTierDto], current_tier_id: &str) -> Option<CountLabel> {
    tiers
        .iter()
        .find(|tier| tier.id == current_tier_id)
        .map(|tier| tier_label(tier.hours))
}

/// 领取次数菜单的取值：预设档位；当前值不在预设内（别处改过）时插入到有序位置，保证当前值可见。
/// `0`（不限）固定排第一。
#[must_use]
pub fn claim_options(current: u32) -> Vec<u32> {
    let mut options = CLAIM_PRESETS.to_vec();
    if !options.contains(&current) {
        options.push(current);
        options.sort_by_key(|value| (*value != 0, *value));
    }
    options
}

/// `0.0..=1.0` 的进度 → 进度条的 `0.0..=100.0`；非有限值按 0 处理。
#[must_use]
pub fn progress_percent(progress: f64) -> f32 {
    if progress.is_finite() {
        (progress.clamp(0., 1.) * 100.) as f32
    } else {
        0.
    }
}

/// 接收方设备名；游客或未知（空 / 全空白）为 `None`。
#[must_use]
pub fn receiver_name(receiver: &ShareReceiverDto) -> Option<&str> {
    let name = receiver.device_name.trim();
    (!name.is_empty()).then_some(name)
}

#[cfg(test)]
mod tests {
    use fluxdown_protocol::{
        ShareDto, ShareHashingDto, ShareReceiverDto, ShareStatus, ShareTierDto,
    };

    use super::{
        HashingProgress, PanelPhase, PanelWait, claim_options, current_expiry_label,
        expiry_options, hashing_progress, panel_phase, progress_percent, receiver_name,
    };

    fn share(status: ShareStatus) -> ShareDto {
        ShareDto {
            id: "s".into(),
            status,
            ..ShareDto::default()
        }
    }

    #[test]
    fn phase_follows_wait_and_hub_share() {
        let active = share(ShareStatus::Active);
        let stopped = share(ShareStatus::Stopped);
        assert_eq!(panel_phase(PanelWait::Creating, None), PanelPhase::Creating);
        assert_eq!(
            panel_phase(PanelWait::Creating, Some(&active)),
            PanelPhase::Creating
        );
        assert_eq!(panel_phase(PanelWait::Failed, None), PanelPhase::Failed);
        assert_eq!(panel_phase(PanelWait::Holding, None), PanelPhase::Gone);
        assert_eq!(
            panel_phase(PanelWait::Holding, Some(&active)),
            PanelPhase::Live
        );
        assert_eq!(
            panel_phase(PanelWait::Holding, Some(&stopped)),
            PanelPhase::Closed
        );
        assert_eq!(
            panel_phase(PanelWait::Holding, Some(&share(ShareStatus::Expired))),
            PanelPhase::Closed
        );
        assert_eq!(
            panel_phase(PanelWait::Holding, Some(&share(ShareStatus::Unknown))),
            PanelPhase::Closed
        );
        // 刚在本面板停止：hub 事件还没到时即使仍是 active 也按已结束渲染。
        assert_eq!(
            panel_phase(PanelWait::Stopped, Some(&active)),
            PanelPhase::Closed
        );
        assert_eq!(panel_phase(PanelWait::Stopped, None), PanelPhase::Gone);
    }

    fn hashing(id: &str, done: u64, total: u64) -> ShareHashingDto {
        ShareHashingDto {
            id: id.into(),
            title: format!("title-{id}"),
            done_bytes: done,
            total_bytes: total,
        }
    }

    #[test]
    fn hashing_sums_matching_tasks_and_falls_back_to_all() {
        let entries = vec![
            hashing("a", 10, 100),
            hashing("b", 50, 100),
            hashing("c", 1, 1),
        ];
        let ids = vec!["a".to_owned(), "b".to_owned()];
        let progress = hashing_progress(&entries, &ids);
        assert_eq!(
            progress,
            Some(HashingProgress {
                done_bytes: 60,
                total_bytes: 200,
                title: None
            })
        );
        assert_eq!(progress.and_then(|p| p.percent()), Some(30.));

        let single = hashing_progress(&entries, &["c".to_owned()]);
        assert_eq!(single.and_then(|p| p.title), Some("title-c".to_owned()));

        // 没有一个 ID 匹配：合计全部。
        let fallback = hashing_progress(&entries, &["zzz".to_owned()]);
        assert_eq!(
            fallback.map(|p| (p.done_bytes, p.total_bytes)),
            Some((61, 201))
        );

        assert_eq!(hashing_progress(&[], &ids), None);
    }

    #[test]
    fn hashing_percent_is_total_over_empty_overflowing_and_overshooting_data() {
        let unknown = HashingProgress {
            done_bytes: 5,
            total_bytes: 0,
            title: None,
        };
        assert_eq!(unknown.percent(), None);
        let over = HashingProgress {
            done_bytes: 9,
            total_bytes: 3,
            title: None,
        };
        assert_eq!(over.percent(), Some(100.));
        let huge = vec![
            hashing("a", u64::MAX, u64::MAX),
            hashing("b", u64::MAX, u64::MAX),
        ];
        let sum = hashing_progress(&huge, &[]);
        assert_eq!(sum.and_then(|p| p.percent()), Some(100.));
    }

    #[test]
    fn expiry_menu_marks_the_current_tier() {
        let tiers = vec![
            ShareTierDto {
                id: "t1".into(),
                hours: 1,
            },
            ShareTierDto {
                id: "t24".into(),
                hours: 24,
            },
        ];
        let options = expiry_options(&tiers, "t24");
        assert_eq!(options.len(), 2);
        assert!(!options[0].selected);
        assert!(options[1].selected);
        assert_eq!(options[1].label.key, "shareDurationDays");
        assert_eq!(
            current_expiry_label(&tiers, "t1").map(|label| label.key),
            Some("shareDurationHours")
        );
        assert_eq!(current_expiry_label(&tiers, "gone"), None);
        assert!(expiry_options(&[], "x").is_empty());
    }

    #[test]
    fn claim_menu_keeps_presets_and_inserts_a_foreign_current_value() {
        assert_eq!(claim_options(5), [0, 1, 3, 5, 10, 20, 50]);
        assert_eq!(claim_options(0), [0, 1, 3, 5, 10, 20, 50]);
        assert_eq!(claim_options(7), [0, 1, 3, 5, 7, 10, 20, 50]);
        assert_eq!(claim_options(999), [0, 1, 3, 5, 10, 20, 50, 999]);
    }

    #[test]
    fn progress_percent_is_clamped_and_total() {
        assert!((progress_percent(0.5) - 50.).abs() < f32::EPSILON);
        assert!((progress_percent(7.) - 100.).abs() < f32::EPSILON);
        assert!(progress_percent(-1.).abs() < f32::EPSILON);
        assert!(progress_percent(f64::NAN).abs() < f32::EPSILON);
        assert!(progress_percent(f64::INFINITY).abs() < f32::EPSILON);
    }

    #[test]
    fn blank_device_name_is_a_guest() {
        let mut receiver = ShareReceiverDto {
            device_name: "  ".into(),
            ..ShareReceiverDto::default()
        };
        assert_eq!(receiver_name(&receiver), None);
        receiver.device_name = " Pixel 9 ".into();
        assert_eq!(receiver_name(&receiver), Some("Pixel 9"));
    }
}
