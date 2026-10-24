//! 提示音策略：事件种类、偏好值解析、合并与防抖（纯逻辑，不依赖平台与异步运行时）。

use std::path::PathBuf;
use std::time::{Duration, Instant};

use fluxdown_protocol::sound::{
    SOUND_PREF_QUEUE_FINISHED, SOUND_PREF_QUEUE_STARTED, SOUND_PREF_TASK_COMPLETED,
    SOUND_PREF_TASK_FAILED, SOUND_PREF_TASK_STARTED, SOUND_VALUE_FILE_PREFIX, SOUND_VALUE_OFF,
    SOUND_VALUE_SYSTEM,
};
use serde_json::Value;

/// 同一种事件两次发声的最小间隔。
pub(crate) const KIND_DEBOUNCE: Duration = Duration::from_millis(1500);
/// 首个事件到达后的合并窗口：窗口内多种事件只播优先级最高的一个。
pub(crate) const MERGE_WINDOW: Duration = Duration::from_millis(300);

/// 可配置提示音的下载事件。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum SoundKind {
    TaskStarted,
    TaskCompleted,
    TaskFailed,
    QueueStarted,
    QueueFinished,
}

impl SoundKind {
    #[cfg(test)]
    pub(crate) const ALL: [Self; 5] = [
        Self::TaskStarted,
        Self::TaskCompleted,
        Self::TaskFailed,
        Self::QueueStarted,
        Self::QueueFinished,
    ];

    /// 由 `TaskNotice.event` 的 wire 名映射；其余事件（`task.created` / `task.paused` 等）不发声。
    pub(crate) fn from_wire(event: &str) -> Option<Self> {
        match event {
            "task.started" => Some(Self::TaskStarted),
            "task.completed" => Some(Self::TaskCompleted),
            "task.failed" => Some(Self::TaskFailed),
            "queue.started" => Some(Self::QueueStarted),
            "queue.drained" => Some(Self::QueueFinished),
            _ => None,
        }
    }

    /// 设备本地偏好键（不进同步目录）。
    pub(crate) const fn pref_key(self) -> &'static str {
        match self {
            Self::TaskStarted => SOUND_PREF_TASK_STARTED,
            Self::TaskCompleted => SOUND_PREF_TASK_COMPLETED,
            Self::TaskFailed => SOUND_PREF_TASK_FAILED,
            Self::QueueStarted => SOUND_PREF_QUEUE_STARTED,
            Self::QueueFinished => SOUND_PREF_QUEUE_FINISHED,
        }
    }

    /// 合并窗口内的优先级（大者胜）：失败 > 队列完成 > 任务完成 > 队列开始 > 任务开始。
    const fn priority(self) -> u8 {
        match self {
            Self::TaskFailed => 4,
            Self::QueueFinished => 3,
            Self::TaskCompleted => 2,
            Self::QueueStarted => 1,
            Self::TaskStarted => 0,
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::TaskStarted => 0,
            Self::TaskCompleted => 1,
            Self::TaskFailed => 2,
            Self::QueueStarted => 3,
            Self::QueueFinished => 4,
        }
    }
}

/// 一个事件的提示音设置。
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SoundSetting {
    Off,
    System,
    File(PathBuf),
}

/// 偏好值无法解析的原因。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SettingError {
    /// 既不是 `off` / `system`，也不是 `file:` 前缀。
    Unrecognized,
    /// `file:` 后的路径为空或不是绝对路径。
    NotAbsolute,
}

impl SoundSetting {
    pub(crate) fn parse(value: &str) -> Result<Self, SettingError> {
        match value {
            SOUND_VALUE_OFF => Ok(Self::Off),
            SOUND_VALUE_SYSTEM => Ok(Self::System),
            _ => {
                let path = value
                    .strip_prefix(SOUND_VALUE_FILE_PREFIX)
                    .ok_or(SettingError::Unrecognized)?;
                let path = PathBuf::from(path);
                if path.is_absolute() {
                    Ok(Self::File(path))
                } else {
                    Err(SettingError::NotAbsolute)
                }
            }
        }
    }

    /// 读偏好：缺失或无法解析一律视为关闭（宁可静音也不误发声）。
    pub(crate) fn from_pref(value: Option<&Value>) -> Self {
        value
            .and_then(Value::as_str)
            .and_then(|value| Self::parse(value).ok())
            .unwrap_or(Self::Off)
    }

    pub(crate) const fn is_enabled(&self) -> bool {
        !matches!(self, Self::Off)
    }
}

/// 防抖与合并裁决：记录每种事件上次发声时间。
#[derive(Debug, Default)]
pub(crate) struct Arbiter {
    last_played: [Option<Instant>; 5],
}

impl Arbiter {
    /// 在合并窗口结束时裁决：`batch` 是窗口内已启用的事件。距上次发声不足
    /// [`KIND_DEBOUNCE`] 的种类被剔除，其余取优先级最高者并记录发声时间。
    pub(crate) fn pick(&mut self, batch: &[SoundKind], now: Instant) -> Option<SoundKind> {
        let chosen = batch
            .iter()
            .copied()
            .filter(|kind| {
                self.last_played[kind.index()]
                    .is_none_or(|at| now.saturating_duration_since(at) >= KIND_DEBOUNCE)
            })
            .max_by_key(|kind| kind.priority())?;
        self.last_played[chosen.index()] = Some(now);
        Some(chosen)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn wire_names_map_to_the_five_sound_kinds() {
        assert_eq!(
            SoundKind::from_wire("task.started"),
            Some(SoundKind::TaskStarted)
        );
        assert_eq!(
            SoundKind::from_wire("task.completed"),
            Some(SoundKind::TaskCompleted)
        );
        assert_eq!(
            SoundKind::from_wire("task.failed"),
            Some(SoundKind::TaskFailed)
        );
        assert_eq!(
            SoundKind::from_wire("queue.started"),
            Some(SoundKind::QueueStarted)
        );
        assert_eq!(
            SoundKind::from_wire("queue.drained"),
            Some(SoundKind::QueueFinished)
        );
        assert_eq!(SoundKind::from_wire("task.created"), None);
        assert_eq!(SoundKind::from_wire("task.paused"), None);
    }

    #[test]
    fn pref_keys_are_device_local_and_unique() {
        let keys: std::collections::HashSet<_> =
            SoundKind::ALL.iter().map(|kind| kind.pref_key()).collect();
        assert_eq!(keys.len(), SoundKind::ALL.len());
        assert!(keys.iter().all(|key| key.starts_with("desktop.sound.")));
        for (index, kind) in SoundKind::ALL.iter().enumerate() {
            assert_eq!(kind.index(), index);
        }
    }

    #[test]
    fn setting_values_parse_strictly() {
        assert_eq!(SoundSetting::parse("off"), Ok(SoundSetting::Off));
        assert_eq!(SoundSetting::parse("system"), Ok(SoundSetting::System));
        #[cfg(unix)]
        assert_eq!(
            SoundSetting::parse("file:/tmp/a b.wav"),
            Ok(SoundSetting::File(std::path::PathBuf::from("/tmp/a b.wav")))
        );
        assert_eq!(SoundSetting::parse("file:"), Err(SettingError::NotAbsolute));
        assert_eq!(
            SoundSetting::parse("file:relative/a.wav"),
            Err(SettingError::NotAbsolute)
        );
        assert_eq!(SoundSetting::parse(""), Err(SettingError::Unrecognized));
        assert_eq!(
            SoundSetting::parse("System"),
            Err(SettingError::Unrecognized)
        );
        assert_eq!(
            SoundSetting::parse("/tmp/a.wav"),
            Err(SettingError::Unrecognized)
        );
    }

    #[test]
    fn missing_or_malformed_prefs_are_off() {
        assert_eq!(SoundSetting::from_pref(None), SoundSetting::Off);
        assert_eq!(
            SoundSetting::from_pref(Some(&json!(true))),
            SoundSetting::Off
        );
        assert_eq!(
            SoundSetting::from_pref(Some(&json!("file:rel.wav"))),
            SoundSetting::Off
        );
        assert_eq!(
            SoundSetting::from_pref(Some(&json!("system"))),
            SoundSetting::System
        );
        assert!(!SoundSetting::Off.is_enabled());
        assert!(SoundSetting::System.is_enabled());
    }

    #[test]
    fn highest_priority_kind_wins_inside_one_window() {
        let mut arbiter = Arbiter::default();
        let now = Instant::now();
        let all = SoundKind::ALL;
        assert_eq!(arbiter.pick(&all, now), Some(SoundKind::TaskFailed));
        let mut arbiter = Arbiter::default();
        assert_eq!(
            arbiter.pick(
                &[
                    SoundKind::TaskStarted,
                    SoundKind::QueueStarted,
                    SoundKind::TaskCompleted
                ],
                now
            ),
            Some(SoundKind::TaskCompleted)
        );
        let mut arbiter = Arbiter::default();
        assert_eq!(
            arbiter.pick(&[SoundKind::TaskCompleted, SoundKind::QueueFinished], now),
            Some(SoundKind::QueueFinished)
        );
        let mut arbiter = Arbiter::default();
        assert_eq!(
            arbiter.pick(&[SoundKind::TaskStarted, SoundKind::QueueStarted], now),
            Some(SoundKind::QueueStarted)
        );
        assert_eq!(arbiter.pick(&[], now), None);
    }

    #[test]
    fn same_kind_is_debounced_but_other_kinds_are_not() {
        let mut arbiter = Arbiter::default();
        let start = Instant::now();
        assert_eq!(
            arbiter.pick(&[SoundKind::TaskCompleted], start),
            Some(SoundKind::TaskCompleted)
        );
        let soon = start + KIND_DEBOUNCE - Duration::from_millis(1);
        assert_eq!(arbiter.pick(&[SoundKind::TaskCompleted], soon), None);
        // 被防抖剔除的高优先级以外的种类仍可发声，且不重置被抑制种类的计时。
        assert_eq!(
            arbiter.pick(&[SoundKind::TaskCompleted, SoundKind::TaskStarted], soon),
            Some(SoundKind::TaskStarted)
        );
        let later = start + KIND_DEBOUNCE;
        assert_eq!(
            arbiter.pick(&[SoundKind::TaskCompleted], later),
            Some(SoundKind::TaskCompleted)
        );
    }

    #[test]
    fn suppressed_lower_priority_kind_keeps_its_own_clock() {
        let mut arbiter = Arbiter::default();
        let start = Instant::now();
        // 同窗口内失败胜出，开始音未播放，因此紧接着仍可播放。
        assert_eq!(
            arbiter.pick(&[SoundKind::TaskStarted, SoundKind::TaskFailed], start),
            Some(SoundKind::TaskFailed)
        );
        assert_eq!(
            arbiter.pick(&[SoundKind::TaskStarted], start),
            Some(SoundKind::TaskStarted)
        );
    }
}
