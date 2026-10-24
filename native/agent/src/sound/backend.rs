//! 平台播放后端：Windows = WinRT `MediaPlayer`，macOS = `afplay`，Linux = `pw-play` / `paplay` / `aplay`。
//!
//! 全部「发出即返回」：`watch` 为真（试听）时额外等待一小段时间，早退的播放器 / 解码失败会作为
//! 错误返回；为假（事件提示音）时失败只记日志，绝不阻塞事件循环。

use std::path::{Path, PathBuf};

use super::policy::SoundKind;

/// 播放失败原因。
#[derive(Debug, thiserror::Error)]
pub(crate) enum PlaybackError {
    #[error("sound file is missing: {}", .0.display())]
    Missing(PathBuf),
    #[error("sound playback is unsupported on this platform")]
    Unsupported,
    #[error("sound playback failed: {0}")]
    Failed(String),
}

/// `system` 对应的平台默认提示音文件（取第一个存在的候选）。
pub(crate) fn system_sound(kind: SoundKind) -> Result<PathBuf, PlaybackError> {
    let candidates = system_candidates(kind);
    if let Some(found) = candidates.iter().find(|path| path.is_file()) {
        return Ok(found.clone());
    }
    match candidates.into_iter().next() {
        Some(missing) => Err(PlaybackError::Missing(missing)),
        None => Err(PlaybackError::Unsupported),
    }
}

#[cfg(windows)]
fn system_candidates(_kind: SoundKind) -> Vec<PathBuf> {
    let root =
        std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    ["Windows Notify System Generic.wav", "Windows Notify.wav"]
        .iter()
        .map(|name| root.join("Media").join(name))
        .collect()
}

#[cfg(target_os = "macos")]
fn system_candidates(_kind: SoundKind) -> Vec<PathBuf> {
    vec![PathBuf::from("/System/Library/Sounds/Glass.aiff")]
}

#[cfg(target_os = "linux")]
fn system_candidates(kind: SoundKind) -> Vec<PathBuf> {
    let name = match kind {
        SoundKind::TaskFailed => "dialog-error.oga",
        _ => "complete.oga",
    };
    vec![PathBuf::from("/usr/share/sounds/freedesktop/stereo").join(name)]
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn system_candidates(_kind: SoundKind) -> Vec<PathBuf> {
    Vec::new()
}

/// 播放一个已校验存在的音频文件。
pub(crate) fn play(path: &Path, watch: bool) -> Result<(), PlaybackError> {
    imp::play(path, watch)
}

/// 停止所有仍在播放的外部播放器进程（agent 退出时调用）。
///
/// macOS / Linux 的播放器是独立子进程，不随 agent 退出而结束，必须显式终止；
/// Windows 的 `MediaPlayer` 在进程内，随进程退出一并释放。
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(crate) fn stop_all() {
    imp::stop_all();
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod imp {
    use std::collections::BTreeMap;
    use std::ffi::OsStr;
    use std::path::Path;
    use std::process::{Child, Command, Stdio};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Mutex, MutexGuard, PoisonError, mpsc};
    use std::time::{Duration, Instant};

    use super::PlaybackError;

    /// 试听时等待播放器早退（文件损坏 / 格式不支持）的时间。
    const EARLY_EXIT_WINDOW: Duration = Duration::from_millis(600);
    /// 播放器的最长存活时间：超过则终止（与 Windows 后端一致；提示音通常只有几秒）。
    const MAX_PLAYBACK: Duration = Duration::from_secs(60);
    /// 回收线程检查子进程状态的间隔。
    const POLL_INTERVAL: Duration = Duration::from_millis(100);

    /// 正在播放的子进程；由回收线程轮询回收，agent 退出时由 [`stop_all`] 统一终止。
    static ACTIVE: Mutex<BTreeMap<u64, Child>> = Mutex::new(BTreeMap::new());
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    fn active() -> MutexGuard<'static, BTreeMap<u64, Child>> {
        // 表内只有子进程句柄，持锁线程 panic 不会留下不一致状态。
        ACTIVE.lock().unwrap_or_else(PoisonError::into_inner)
    }

    enum Launch {
        /// 可执行文件不存在，换下一个播放器。
        NotInstalled,
        Failed(String),
        Started,
    }

    /// 终止并回收一个播放器进程。
    fn terminate(program: &str, mut child: Child) {
        if let Err(error) = child.kill() {
            tracing::debug!(player = %program, error = %error, "could not kill sound player");
        }
        if let Err(error) = child.wait() {
            tracing::debug!(player = %program, error = %error, "could not reap sound player");
        }
    }

    /// 轮询回收子进程：正常结束返回退出是否成功；超时被终止或被 [`stop_all`] 取走返回 `None`。
    fn reap(id: u64, program: &str) -> Option<bool> {
        let started = Instant::now();
        loop {
            let mut players = active();
            let child = players.get_mut(&id)?;
            match child.try_wait() {
                Ok(Some(status)) => {
                    players.remove(&id);
                    return Some(status.success());
                }
                Ok(None) if started.elapsed() >= MAX_PLAYBACK => {
                    let child = players.remove(&id)?;
                    drop(players);
                    tracing::debug!(player = %program, "sound playback timed out; stopping the player");
                    terminate(program, child);
                    return None;
                }
                Ok(None) => {}
                Err(error) => {
                    let child = players.remove(&id)?;
                    drop(players);
                    tracing::debug!(player = %program, error = %error, "could not poll sound player");
                    terminate(program, child);
                    return Some(false);
                }
            }
            drop(players);
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    fn launch(program: &str, path: &Path, watch: bool) -> Launch {
        let mut command = Command::new(program);
        command
            .arg(OsStr::new(path))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = match command.spawn() {
            Ok(child) => child,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Launch::NotInstalled;
            }
            Err(error) => return Launch::Failed(format!("{program}: {error}")),
        };
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        active().insert(id, child);
        // 由专用线程回收子进程（避免僵尸进程、限制最长播放时间），并把退出状态交给等待方。
        let (sender, receiver) = mpsc::channel();
        let name = program.to_owned();
        let reaper = std::thread::Builder::new()
            .name("fluxdown-sound-reaper".into())
            .spawn(move || {
                if let Some(success) = reap(id, &name)
                    && sender.send(success).is_err()
                {
                    tracing::trace!(player = %name, "sound player status receiver is gone");
                }
            });
        if let Err(error) = reaper {
            if let Some(child) = active().remove(&id) {
                terminate(program, child);
            }
            return Launch::Failed(format!("{program}: could not start reaper thread: {error}"));
        }
        if !watch {
            return Launch::Started;
        }
        match receiver.recv_timeout(EARLY_EXIT_WINDOW) {
            Ok(false) => Launch::Failed(format!("{program} exited with a failure status")),
            // 极短的音频在窗口内正常播完，或仍在播放。
            Ok(true) | Err(_) => Launch::Started,
        }
    }

    pub(super) fn stop_all() {
        let players = std::mem::take(&mut *active());
        for (_, child) in players {
            terminate("sound player", child);
        }
    }

    #[cfg(target_os = "macos")]
    const PLAYERS: &[&str] = &["afplay"];
    #[cfg(target_os = "linux")]
    const PLAYERS: &[&str] = &["pw-play", "paplay", "aplay"];

    pub(super) fn play(path: &Path, watch: bool) -> Result<(), PlaybackError> {
        let mut last_error = None;
        for program in PLAYERS {
            match launch(program, path, watch) {
                Launch::Started => return Ok(()),
                Launch::NotInstalled => {}
                Launch::Failed(message) => last_error = Some(message),
            }
        }
        Err(PlaybackError::Failed(last_error.unwrap_or_else(|| {
            format!("no audio player found ({})", PLAYERS.join(", "))
        })))
    }

    #[cfg(test)]
    mod tests {
        use super::{Launch, active, launch, stop_all};
        use std::path::Path;
        use std::process::Command;

        fn alive(pid: u32) -> bool {
            Command::new("kill")
                .args(["-0", &pid.to_string()])
                .status()
                .expect("run kill -0")
                .success()
        }

        #[test]
        fn stop_all_terminates_a_still_playing_player() {
            // `sleep 30` 充当一首长音乐：发出即返回后仍在运行。
            assert!(matches!(
                launch("sleep", Path::new("30"), false),
                Launch::Started
            ));
            let pids: Vec<u32> = active().values().map(std::process::Child::id).collect();
            assert!(!pids.is_empty());
            assert!(pids.iter().all(|pid| alive(*pid)));
            stop_all();
            assert!(active().is_empty());
            assert!(pids.iter().all(|pid| !alive(*pid)));
        }
    }
}

#[cfg(windows)]
mod imp {
    use std::path::Path;
    use std::sync::mpsc;
    use std::time::Duration;

    use windows::Foundation::{TypedEventHandler, Uri};
    use windows::Media::Core::MediaSource;
    use windows::Media::Playback::{IMediaPlaybackSource, MediaPlayer, MediaPlayerFailedEventArgs};
    use windows::core::{HSTRING, IInspectable, Interface};

    use super::{PlaybackError, file_uri};

    /// 试听时等待解码失败回调的时间。
    const EARLY_FAILURE_WINDOW: Duration = Duration::from_millis(600);
    /// 播放器的最长存活时间：超过则放弃等待并释放（提示音通常只有几秒）。
    const MAX_PLAYBACK: Duration = Duration::from_secs(60);

    enum PlayerEvent {
        Ended,
        Failed(String),
    }

    pub(super) fn play(path: &Path, watch: bool) -> Result<(), PlaybackError> {
        let uri = file_uri(&path.to_string_lossy());
        let (report, outcome) = mpsc::channel();
        // 专用短命线程：从未初始化过 COM，windows-core 首次激活工厂时以 `CoIncrementMTAUsage`
        // 加入进程 MTA 且永不释放（与 toast 同理，避免 RoUninitialize 拆掉缓存的激活工厂）。
        // 线程活到播放结束 / 失败 / 超时，期间 `MediaPlayer` 保持存活。
        std::thread::Builder::new()
            .name("fluxdown-sound".into())
            .spawn(move || run(&uri, &report))
            .map_err(|error| {
                PlaybackError::Failed(format!("could not start player thread: {error}"))
            })?;
        if !watch {
            return Ok(());
        }
        match outcome.recv_timeout(EARLY_FAILURE_WINDOW + Duration::from_secs(2)) {
            Ok(result) => result.map_err(PlaybackError::Failed),
            Err(error) => Err(PlaybackError::Failed(format!(
                "media player did not report a result: {error}"
            ))),
        }
    }

    fn run(uri: &str, report: &mpsc::Sender<Result<(), String>>) {
        let (events, event_rx) = mpsc::channel();
        let player = match start(uri, &events) {
            Ok(player) => player,
            Err(error) => {
                deliver(
                    report,
                    Err(format!("could not start Windows media player: {error}")),
                );
                return;
            }
        };
        // 先观察一小段时间：解码 / 打开失败会很快触发 MediaFailed。
        match event_rx.recv_timeout(EARLY_FAILURE_WINDOW) {
            Ok(PlayerEvent::Failed(message)) => {
                deliver(report, Err(message));
                return;
            }
            Ok(PlayerEvent::Ended) => {
                deliver(report, Ok(()));
                return;
            }
            Err(_) => deliver(report, Ok(())),
        }
        match event_rx.recv_timeout(MAX_PLAYBACK) {
            Ok(PlayerEvent::Failed(message)) => {
                tracing::warn!(error = %message, "sound playback failed");
            }
            Ok(PlayerEvent::Ended) => {}
            Err(_) => tracing::debug!("sound playback timed out; releasing the player"),
        }
        drop(player);
    }

    fn deliver(report: &mpsc::Sender<Result<(), String>>, result: Result<(), String>) {
        // 事件播放不关心结果：接收端已丢弃时发送失败属正常。
        if report.send(result).is_err() {
            tracing::trace!("sound result receiver is gone");
        }
    }

    fn start(uri: &str, events: &mpsc::Sender<PlayerEvent>) -> windows::core::Result<MediaPlayer> {
        let player = MediaPlayer::new()?;
        let ended = events.clone();
        player.MediaEnded(&TypedEventHandler::<MediaPlayer, IInspectable>::new(
            move |_, _| {
                if ended.send(PlayerEvent::Ended).is_err() {
                    tracing::trace!("sound player thread already finished");
                }
                Ok(())
            },
        ))?;
        let failed = events.clone();
        player.MediaFailed(
            &TypedEventHandler::<MediaPlayer, MediaPlayerFailedEventArgs>::new(move |_, args| {
                let message = args
                    .as_ref()
                    .and_then(|args| args.ErrorMessage().ok())
                    .map_or_else(|| "media failed".to_owned(), |message| message.to_string());
                if failed.send(PlayerEvent::Failed(message)).is_err() {
                    tracing::trace!("sound player thread already finished");
                }
                Ok(())
            }),
        )?;
        let source = MediaSource::CreateFromUri(&Uri::CreateUri(&HSTRING::from(uri))?)?;
        let source: IMediaPlaybackSource = source.cast()?;
        player.SetSource(&source)?;
        player.Play()?;
        Ok(player)
    }
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
mod imp {
    use std::path::Path;

    use super::PlaybackError;

    pub(super) fn play(_path: &Path, _watch: bool) -> Result<(), PlaybackError> {
        Err(PlaybackError::Unsupported)
    }
}

/// 本地路径 → `file:` IRI（WinRT `MediaSource::CreateFromUri` 只接受 URI）。
/// 去掉 `\\?\` 前缀；UNC 路径映射为 `file://host/share/...`；ASCII 保留字符按 RFC 3986 百分号编码，
/// 非 ASCII 字符原样保留：Windows 把 file URI 中的 `%XX` 按系统 ANSI 代码页逐字节解码，
/// UTF-8 转义的中文路径会被解成乱码而打不开（#971）。
#[cfg(any(windows, test))]
fn file_uri(path: &str) -> String {
    let (prefix, rest) = if let Some(unc) = path.strip_prefix(r"\\?\UNC\") {
        ("file://", unc)
    } else if let Some(plain) = path.strip_prefix(r"\\?\") {
        ("file:///", plain)
    } else if let Some(unc) = path.strip_prefix(r"\\") {
        ("file://", unc)
    } else {
        ("file:///", path)
    };
    let mut uri = String::with_capacity(prefix.len() + rest.len() + 8);
    uri.push_str(prefix);
    for ch in rest.chars() {
        match ch {
            '\\' | '/' => uri.push('/'),
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '.' | '_' | '~' | ':' => uri.push(ch),
            ch if !ch.is_ascii() => uri.push(ch),
            other => {
                let mut buf = [0u8; 4];
                for byte in other.encode_utf8(&mut buf).bytes() {
                    uri.push('%');
                    uri.push(char::from(b"0123456789ABCDEF"[usize::from(byte >> 4)]));
                    uri.push(char::from(b"0123456789ABCDEF"[usize::from(byte & 0x0F)]));
                }
            }
        }
    }
    uri
}

#[cfg(test)]
mod tests {
    use super::file_uri;

    #[test]
    fn windows_paths_become_file_iris() {
        assert_eq!(
            file_uri(r"C:\Windows\Media\Windows Notify.wav"),
            "file:///C:/Windows/Media/Windows%20Notify.wav"
        );
        assert_eq!(
            file_uri(r"\\?\C:\音乐\a#b.wav"),
            "file:///C:/音乐/a%23b.wav"
        );
        assert_eq!(
            file_uri(r"\\nas\share\ding.mp3"),
            "file://nas/share/ding.mp3"
        );
        assert_eq!(
            file_uri(r"\\?\UNC\nas\share\ding.mp3"),
            "file://nas/share/ding.mp3"
        );
    }
}
