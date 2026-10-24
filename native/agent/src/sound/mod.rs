//! 下载事件提示音（仅桌面宿主）：agent 消费 daemon 的 `TaskNotice`，按设备本地偏好
//! `desktop.sound.*` 在本机播放用户选择的声音。
//!
//! - `TaskNotice` 经 [`crate::cloud_notify::route_daemon_event`] 分流到本模块的有界通道，
//!   仍不会进入 UI 事件流；通道满则丢弃（提示音是尽力而为的旁路效果）。
//! - 首个事件到达后合并 [`MERGE_WINDOW`]：窗口内多种事件只播优先级最高者；同一种事件
//!   至少间隔 [`KIND_DEBOUNCE`]（策略见 [`policy`]）。
//! - headless server 与移动端嵌入式宿主不启动本服务；`agent.sound.preview` 在这些宿主上被网关拒绝。

mod backend;
mod policy;

use std::path::PathBuf;
use std::time::Instant;

use fluxdown_protocol::sound::SOUND_FILE_EXTENSIONS;
use fluxdown_protocol::{AgentSnapshot, TaskNoticeDto};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::event_hub::AgentEventHub;
pub(crate) use backend::PlaybackError;
use policy::{Arbiter, MERGE_WINDOW};
pub(crate) use policy::{SoundKind, SoundSetting};

/// daemon 事件投影 → 提示音服务的通道容量（事件稀疏；满则丢弃）。
const CHANNEL_CAPACITY: usize = 64;

pub(crate) type SoundSender = mpsc::Sender<SoundKind>;
pub(crate) type SoundReceiver = mpsc::Receiver<SoundKind>;

pub(crate) fn channel() -> (SoundSender, SoundReceiver) {
    mpsc::channel(CHANNEL_CAPACITY)
}

/// 把一条 `TaskNotice` 交给提示音服务；与提示音无关的事件被忽略，绝不阻塞调用方。
pub(crate) fn offer(sender: &SoundSender, notice: &TaskNoticeDto) {
    let Some(kind) = SoundKind::from_wire(&notice.event) else {
        return;
    };
    if let Err(error) = sender.try_send(kind) {
        tracing::debug!(error = %error, "sound intake unavailable; dropped a sound event");
    }
}

/// 读取某事件在快照里的提示音设置（缺失 / 非法 = 关闭）。
pub(crate) fn setting(snapshot: &AgentSnapshot, kind: SoundKind) -> SoundSetting {
    SoundSetting::from_pref(snapshot.preferences.values.get(kind.pref_key()))
}

/// 启动提示音服务；`enabled` 为假（server / 非桌面宿主）时不接收事件，返回已结束的任务句柄。
pub(crate) fn start(
    events: AgentEventHub,
    cancel: CancellationToken,
    enabled: bool,
) -> (Option<SoundSender>, tokio::task::JoinHandle<()>) {
    if !enabled {
        return (None, tokio::spawn(async {}));
    }
    let (sender, receiver) = channel();
    (Some(sender), tokio::spawn(run(events, receiver, cancel)))
}

async fn run(events: AgentEventHub, receiver: SoundReceiver, cancel: CancellationToken) {
    serve(events, receiver, cancel).await;
    // 外部播放器子进程不随 agent 退出：长音频会在退出后继续播放，必须显式终止。
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    if let Err(error) = tokio::task::spawn_blocking(backend::stop_all).await {
        tracing::warn!(error = %error, "could not stop sound players on shutdown");
    }
}

async fn serve(events: AgentEventHub, mut receiver: SoundReceiver, cancel: CancellationToken) {
    let mut arbiter = Arbiter::default();
    loop {
        let first = tokio::select! {
            () = cancel.cancelled() => return,
            kind = receiver.recv() => match kind {
                Some(kind) => kind,
                None => return,
            },
        };
        let mut batch = vec![first];
        let deadline = tokio::time::Instant::now() + MERGE_WINDOW;
        while let Ok(Some(kind)) = tokio::time::timeout_at(deadline, receiver.recv()).await {
            batch.push(kind);
        }
        // 只让用户已启用的种类参与裁决：关闭的失败音不应压掉已启用的完成音。
        let enabled: Vec<(SoundKind, SoundSetting)> = events.inspect(|snapshot| {
            batch
                .iter()
                .map(|kind| (*kind, setting(snapshot, *kind)))
                .filter(|(_, setting)| setting.is_enabled())
                .collect()
        });
        let kinds: Vec<SoundKind> = enabled.iter().map(|(kind, _)| *kind).collect();
        let Some(chosen) = arbiter.pick(&kinds, Instant::now()) else {
            continue;
        };
        let Some((_, setting)) = enabled.into_iter().find(|(kind, _)| *kind == chosen) else {
            continue;
        };
        // 播放可能短暂阻塞（子进程创建 / 线程启动）：放进 blocking 线程，不拖慢事件循环。
        tokio::task::spawn_blocking(move || {
            if let Err(error) = play_setting(chosen, &setting, false) {
                tracing::warn!(error = %error, kind = ?chosen, "could not play sound");
            }
        });
    }
}

fn play_setting(kind: SoundKind, setting: &SoundSetting, watch: bool) -> Result<(), PlaybackError> {
    let path = match setting {
        SoundSetting::Off => return Ok(()),
        SoundSetting::System => backend::system_sound(kind)?,
        SoundSetting::File(path) => path.clone(),
    };
    if !path.is_file() {
        return Err(PlaybackError::Missing(path));
    }
    backend::play(&path, watch)
}

/// `agent.sound.preview` 失败原因。
#[derive(Debug, thiserror::Error)]
pub(crate) enum PreviewError {
    /// 值无法解析、是 `off`（无声可放）或不是绝对路径。
    #[error("invalid sound value")]
    InvalidValue,
    /// 扩展名不在 [`SOUND_FILE_EXTENSIONS`] 内。
    #[error("unsupported sound file type: {}", .0.display())]
    UnsupportedType(PathBuf),
    #[error(transparent)]
    Playback(#[from] PlaybackError),
}

/// 试听一次偏好值（阻塞到播放器确认启动或早退失败；调用方放进 blocking 线程）。
pub(crate) fn preview(value: &str) -> Result<(), PreviewError> {
    let setting = SoundSetting::parse(value).map_err(|_| PreviewError::InvalidValue)?;
    if let SoundSetting::File(path) = &setting {
        let supported = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                SOUND_FILE_EXTENSIONS
                    .iter()
                    .any(|allowed| allowed.eq_ignore_ascii_case(extension))
            });
        if !supported {
            return Err(PreviewError::UnsupportedType(path.clone()));
        }
    }
    if !setting.is_enabled() {
        return Err(PreviewError::InvalidValue);
    }
    // `system` 试听用「任务完成」的默认音。
    play_setting(SoundKind::TaskCompleted, &setting, true).map_err(PreviewError::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_rejects_off_malformed_and_unsupported_values() {
        assert!(matches!(preview("off"), Err(PreviewError::InvalidValue)));
        assert!(matches!(preview("loud"), Err(PreviewError::InvalidValue)));
        assert!(matches!(
            preview("file:relative.wav"),
            Err(PreviewError::InvalidValue)
        ));
        #[cfg(unix)]
        assert!(matches!(
            preview("file:/tmp/fluxdown-sound.txt"),
            Err(PreviewError::UnsupportedType(_))
        ));
    }

    #[test]
    fn preview_of_a_missing_file_reports_it() {
        #[cfg(unix)]
        assert!(matches!(
            preview("file:/nonexistent-fluxdown-dir/ding.wav"),
            Err(PreviewError::Playback(PlaybackError::Missing(_)))
        ));
    }

    #[tokio::test]
    async fn only_sound_relevant_notices_are_offered() {
        let (sender, mut receiver) = channel();
        let notice = |event: &str| TaskNoticeDto {
            delivery_id: "d".to_owned(),
            event: event.to_owned(),
            timestamp_ms: 0,
            queue_id: "q".to_owned(),
            queue_name: "Q".to_owned(),
            task: None,
        };
        offer(&sender, &notice("task.created"));
        offer(&sender, &notice("queue.drained"));
        assert_eq!(receiver.try_recv().ok(), Some(SoundKind::QueueFinished));
        assert!(receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn disabled_host_has_no_intake() {
        let events = AgentEventHub::new(AgentSnapshot::default());
        let (sender, task) = start(events, CancellationToken::new(), false);
        assert!(sender.is_none());
        assert!(task.await.is_ok());
    }
}
