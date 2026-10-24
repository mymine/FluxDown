//! 下载事件提示音：设备本地偏好键、取值语法与试听参数。
//!
//! 提示音只在桌面宿主由 agent 播放（见 `agent.sound.preview`）；偏好是设备本地键（不进云同步目录），
//! 值为字符串：[`SOUND_VALUE_OFF`]（缺省）、[`SOUND_VALUE_SYSTEM`] 或 `file:<绝对路径>`。

use serde::{Deserialize, Serialize};

/// 偏好值：关闭（键缺失时同样视为关闭）。
pub const SOUND_VALUE_OFF: &str = "off";
/// 偏好值：平台默认提示音。
pub const SOUND_VALUE_SYSTEM: &str = "system";
/// 偏好值前缀：其后是本机音频文件的绝对路径。
pub const SOUND_VALUE_FILE_PREFIX: &str = "file:";

/// 任务开始。
pub const SOUND_PREF_TASK_STARTED: &str = "desktop.sound.task_started";
/// 任务完成。
pub const SOUND_PREF_TASK_COMPLETED: &str = "desktop.sound.task_completed";
/// 任务失败。
pub const SOUND_PREF_TASK_FAILED: &str = "desktop.sound.task_failed";
/// 队列开始。
pub const SOUND_PREF_QUEUE_STARTED: &str = "desktop.sound.queue_started";
/// 队列全部完成。
pub const SOUND_PREF_QUEUE_FINISHED: &str = "desktop.sound.queue_finished";

/// 五个提示音偏好键，顺序即设置页展示顺序。
pub const SOUND_PREF_KEYS: [&str; 5] = [
    SOUND_PREF_TASK_STARTED,
    SOUND_PREF_TASK_COMPLETED,
    SOUND_PREF_TASK_FAILED,
    SOUND_PREF_QUEUE_STARTED,
    SOUND_PREF_QUEUE_FINISHED,
];

/// 自定义提示音文件允许的扩展名（小写，不带点）；agent 试听时校验，设置页选文件时预检。
pub const SOUND_FILE_EXTENSIONS: &[&str] = &[
    "wav", "mp3", "m4a", "aac", "aiff", "aif", "ogg", "oga", "flac",
];

/// `agent.sound.preview` 参数。
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct SoundPreviewParams {
    /// 待试听的偏好值（`system` / `file:<绝对路径>`；`off` 无声可放，视为无效参数）。
    pub value: String,
}
