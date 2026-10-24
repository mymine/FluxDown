//! GPUI 对同级 `fluxdown-agent` 的单飞启动与异步回收。
//!
//! 拉起失败可见：reaper 记录子进程的退出状态与存活时长，连续快速异常退出（崩溃循环）或
//! 无法定位 / 拉起可执行文件时暂停拉起并返回 [`Blocked`]，由调用方转成界面可见的
//! [`crate::service_failure::ServiceFailure`]，而不是按重试周期反复拉起一个会被杀软扫描的进程。

use std::process::{ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::net::TcpStream;
use tokio::sync::Mutex;

/// 连续快速异常退出达到该次数即视为崩溃循环（与 `fluxdown-agent` 的 daemon 监督器一致）。
const CRASH_LOOP_THRESHOLD: u32 = 3;
/// 存活不足该时长的异常退出计入崩溃循环。
const CRASH_FAST_EXIT: Duration = Duration::from_secs(30);
/// 崩溃循环期间暂停拉起的冷却；到期允许再试一次，仍然快速失败就继续冷却。
const CRASH_COOLDOWN: Duration = Duration::from_secs(120);
/// 定位 / 拉起失败（可执行文件缺失、被安全软件隔离、无权限）后再次尝试前的间隔。
const LAUNCH_RETRY_INTERVAL: Duration = Duration::from_secs(10);

#[derive(Default)]
struct BootstrapState {
    generation: u64,
    running: bool,
    reapers: Vec<tokio::task::JoinHandle<()>>,
    health: SpawnHealth,
}

/// 子进程退出的观测结果。
struct ChildExit {
    success: bool,
    uptime: Duration,
    /// 可读的退出状态文本（含退出码 / 信号）。
    status: String,
    /// 拉起该子进程的时刻（Unix 毫秒）。
    spawned_at_ms: u64,
}

struct ExitRecord {
    at: Instant,
    status: String,
    spawned_at_ms: u64,
}

struct LaunchRecord {
    at: Instant,
    program: String,
    detail: String,
}

/// 拉起健康度：崩溃循环计数与定位 / 拉起失败记录。
#[derive(Default)]
struct SpawnHealth {
    crash_streak: u32,
    last_exit: Option<ExitRecord>,
    launch_failure: Option<LaunchRecord>,
}

fn next_crash_streak(previous: u32, success: bool, uptime: Duration) -> u32 {
    if success || uptime >= CRASH_FAST_EXIT {
        0
    } else {
        previous.saturating_add(1)
    }
}

impl SpawnHealth {
    fn record_exit(&mut self, now: Instant, exit: ChildExit) {
        self.crash_streak = next_crash_streak(self.crash_streak, exit.success, exit.uptime);
        self.last_exit = Some(ExitRecord {
            at: now,
            status: exit.status,
            spawned_at_ms: exit.spawned_at_ms,
        });
    }

    fn record_launch_failure(&mut self, now: Instant, failure: LaunchError) -> Blocked {
        let blocked = Blocked::Launch {
            program: failure.program.clone(),
            detail: failure.detail.clone(),
        };
        self.launch_failure = Some(LaunchRecord {
            at: now,
            program: failure.program,
            detail: failure.detail,
        });
        blocked
    }

    /// 此刻是否应暂停拉起；崩溃循环冷却自最近一次退出起算。
    fn blocked(&self, now: Instant) -> Option<Blocked> {
        if let Some(launch) = &self.launch_failure
            && now.saturating_duration_since(launch.at) < LAUNCH_RETRY_INTERVAL
        {
            return Some(Blocked::Launch {
                program: launch.program.clone(),
                detail: launch.detail.clone(),
            });
        }
        let exit = self.last_exit.as_ref()?;
        (self.crash_streak >= CRASH_LOOP_THRESHOLD
            && now.saturating_duration_since(exit.at) < CRASH_COOLDOWN)
            .then(|| Blocked::CrashLoop {
                status: exit.status.clone(),
                spawned_at_ms: exit.spawned_at_ms,
            })
    }

    /// 已成功连上 agent：之前的失败不再算数。
    fn clear(&mut self) {
        *self = Self::default();
    }
}

/// 暂停拉起的原因。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Blocked {
    /// 无法定位或拉起 agent 可执行文件。
    Launch { program: String, detail: String },
    /// 子进程反复快速异常退出。
    CrashLoop {
        /// 最近一次退出的可读状态。
        status: String,
        /// 触发崩溃判定的那次拉起时刻（Unix 毫秒）。
        spawned_at_ms: u64,
    },
}

/// [`ServiceBootstrap::ensure_running`] 的结果。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum EnsureOutcome {
    /// 本次新拉起了 agent。
    Spawned,
    /// 无需拉起：已有子进程在运行、已停止拉起，或端口已有监听者。
    NotNeeded,
    /// 暂停拉起（崩溃循环冷却 / 定位或拉起失败）。
    Blocked(Blocked),
}

pub struct ServiceBootstrap {
    state: Arc<Mutex<BootstrapState>>,
    /// 完全退出或 agent 已声明退出后置位：此后连接拒绝不再拉起 agent。
    stopped: AtomicBool,
    /// 本进程曾拉起过 agent（即本次启动是 FluxDown 服务的冷启动）。
    spawned: AtomicBool,
}

impl ServiceBootstrap {
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(BootstrapState::default())),
            stopped: AtomicBool::new(false),
            spawned: AtomicBool::new(false),
        }
    }

    /// 永久停止拉起 agent（不可恢复）。
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
    }

    /// 本进程是否拉起过 agent：界面启动时据此区分「冷启动服务」与「打开已驻留的应用」。
    #[must_use]
    pub fn spawned_agent(&self) -> bool {
        self.spawned.load(Ordering::Acquire)
    }

    /// 已成功连上 agent：崩溃循环计数与拉起失败记录清零。
    pub(crate) async fn note_connected(&self) {
        self.state.lock().await.health.clear();
    }

    /// 仅由 connection-refused/no-listener 路径调用。
    ///
    /// `probe_listener` 为 true（本机尚无 bearer）时先探测目标端口：另一桌面进程或直接启动的
    /// agent 可能已监听、只是还没写出 bearer，此时拉起的子进程会立即因独占锁退出。连接已被
    /// 拒绝时此刻确定无人监听，跳过探测直接拉起。
    pub(crate) async fn ensure_running(
        &self,
        rpc_url: &str,
        probe_listener: bool,
    ) -> Result<EnsureOutcome, BootstrapError> {
        let mut state = self.state.lock().await;
        state.reapers.retain(|task| !task.is_finished());
        if state.running || self.stopped.load(Ordering::Acquire) {
            return Ok(EnsureOutcome::NotNeeded);
        }
        if probe_listener && agent_is_listening(rpc_url).await? {
            return Ok(EnsureOutcome::NotNeeded);
        }
        if let Some(blocked) = state.health.blocked(Instant::now()) {
            return Ok(EnsureOutcome::Blocked(blocked));
        }
        let spawned_at_ms = unix_ms_now();
        let (mut child, program) = match launch_agent() {
            Ok(started) => started,
            Err(failure) => {
                log::warn!("{}: {}", failure.program, failure.detail);
                let blocked = state.health.record_launch_failure(Instant::now(), failure);
                return Ok(EnsureOutcome::Blocked(blocked));
            }
        };
        state.health.launch_failure = None;
        state.generation = state.generation.saturating_add(1);
        let generation = state.generation;
        state.running = true;
        self.spawned.store(true, Ordering::Release);
        log::info!(
            "spawned fluxdown-agent {program} (generation {generation}, pid {:?})",
            child.id()
        );
        let spawned_at = Instant::now();
        let bootstrap_state = self.state.clone();
        state.reapers.push(tokio::spawn(async move {
            // agent 常驻、与界面解耦，通常比界面活得久；先于界面退出（崩溃、锁冲突）才会记到。
            // macOS 经 `open -g` 拉起时这里等的是 `open` 本身，立即成功退出属正常。
            let exit = match child.wait().await {
                Ok(status) => {
                    let uptime = spawned_at.elapsed();
                    let status_text = describe_exit(status);
                    if status.success() {
                        log::info!(
                            "fluxdown-agent (generation {generation}) exited after {}s",
                            uptime.as_secs()
                        );
                    } else {
                        log::warn!(
                            "fluxdown-agent (generation {generation}) exited with {status_text} after {}s",
                            uptime.as_secs()
                        );
                    }
                    Some(ChildExit {
                        success: status.success(),
                        uptime,
                        status: status_text,
                        spawned_at_ms,
                    })
                }
                Err(error) => {
                    log::warn!("failed to reap fluxdown-agent: {error}");
                    None
                }
            };
            let mut state = bootstrap_state.lock().await;
            if state.generation == generation {
                if let Some(exit) = exit {
                    state.health.record_exit(Instant::now(), exit);
                }
                state.running = false;
            }
        }));
        Ok(EnsureOutcome::Spawned)
    }
}

#[cfg(test)]
impl ServiceBootstrap {
    /// 测试接缝：直接进入「拉起失败」冷却，不必真的缺失 / 无法拉起可执行文件。
    pub(crate) async fn record_launch_failure_for_test(&self, program: &str, detail: &str) {
        self.state.lock().await.health.record_launch_failure(
            Instant::now(),
            LaunchError {
                program: program.to_owned(),
                detail: detail.to_owned(),
            },
        );
    }
}

fn unix_ms_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

/// 退出状态的可读文本：含退出码；Windows 的 NTSTATUS（如 0xC0000005）同时给出十六进制。
fn describe_exit(status: ExitStatus) -> String {
    #[cfg(unix)]
    let signal = std::os::unix::process::ExitStatusExt::signal(&status);
    #[cfg(not(unix))]
    let signal = None;
    format_exit_status(status.code(), signal)
}

fn format_exit_status(code: Option<i32>, signal: Option<i32>) -> String {
    match (code, signal) {
        (Some(code), _) if code < 0 => format!(
            "exit code {code} (0x{:08X})",
            u32::from_ne_bytes(code.to_ne_bytes())
        ),
        (Some(code), _) => format!("exit code {code}"),
        (None, Some(signal)) => format!("terminated by signal {signal}"),
        (None, None) => "unknown exit status".to_owned(),
    }
}

/// 定位或拉起 agent 失败。
struct LaunchError {
    program: String,
    detail: String,
}

fn launch_agent() -> Result<(tokio::process::Child, String), LaunchError> {
    let mut command = agent_command().map_err(|error| LaunchError {
        program: AGENT_BINARY.to_owned(),
        detail: format!("could not locate fluxdown-agent: {error:#}"),
    })?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let program = command.get_program().to_string_lossy().into_owned();
    match tokio::process::Command::from(command).spawn() {
        Ok(child) => Ok((child, program)),
        Err(error) => Err(LaunchError {
            detail: format!("could not spawn fluxdown-agent: {error:#}"),
            program,
        }),
    }
}

/// 探测（`ensure_running` 的 probe、`wait_until_stopped`）的快速判定上限。端口在监听时握手由内核
/// 即时完成（亚毫秒级，不依赖 agent 是否已开始 accept）；Windows 对未监听的回环端口收到 RST 后
/// 还会重传 SYN（约 0.5s + 1s），`connect` 要约 2s 才返回 `ConnectionRefused`。超时即按「无人
/// 监听」处理，冷启动不再白等这 2s；极端负载下的误判只会多拉起一个因 agent 数据目录独占锁
/// 立即退出的进程。真正建连另用逐轮加倍的超时（见 `agent_client`），不受此值限制。
pub(crate) const PROBE_TIMEOUT: Duration = Duration::from_millis(300);

/// 一次回环连接尝试的结果；被拒与超时必须区分：杀软 WFP 过滤或高负载下连接可能慢于探测上限。
#[derive(Debug)]
pub(crate) enum ListenerProbe {
    Listening(TcpStream),
    /// 此刻确定无人监听。
    Refused,
    /// 在 `timeout` 内未完成握手：无法确定是否有人监听。
    TimedOut,
}

/// 连接 agent 监听端口，最多等 `timeout`。
pub(crate) async fn connect_listener(
    rpc_url: &str,
    timeout: Duration,
) -> Result<ListenerProbe, BootstrapError> {
    let target = agent_socket_target(rpc_url)?;
    match tokio::time::timeout(timeout, TcpStream::connect(target)).await {
        Ok(Ok(stream)) => Ok(ListenerProbe::Listening(stream)),
        Ok(Err(error))
            if matches!(
                error.kind(),
                std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
            ) =>
        {
            Ok(ListenerProbe::Refused)
        }
        Ok(Err(error)) => Err(BootstrapError::Probe(error.to_string())),
        Err(_) => Ok(ListenerProbe::TimedOut),
    }
}

/// 探测端口是否有人监听；超时按「无人监听」处理（见 [`PROBE_TIMEOUT`]）。
async fn agent_is_listening(rpc_url: &str) -> Result<bool, BootstrapError> {
    Ok(matches!(
        connect_listener(rpc_url, PROBE_TIMEOUT).await?,
        ListenerProbe::Listening(_)
    ))
}

/// 等被替换的旧 agent 关闭监听（关停 daemon 后退出）；超时后交回重连循环自愈。
pub async fn wait_until_stopped(rpc_url: &str, timeout: Duration) {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if matches!(agent_is_listening(rpc_url).await, Ok(false)) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn agent_socket_target(rpc_url: &str) -> Result<std::net::SocketAddr, BootstrapError> {
    crate::agent_endpoint::socket_target(rpc_url)
        .map_err(|error| BootstrapError::Probe(error.to_string()))
}

/// agent 可执行文件名（平台相关）。
const AGENT_BINARY: &str = if cfg!(windows) {
    "fluxdown-agent.exe"
} else {
    "fluxdown-agent"
};

/// macOS 打包布局中 agent 所在的辅助 bundle（相对外层 `Contents/`）；与
/// `fluxdown_agent::platform` 的布局约定及 `scripts/package_gpui_macos.sh` 保持一致。
#[cfg(target_os = "macos")]
const MACOS_AGENT_HELPER_APP: &str = "Helpers/FluxDownAgent.app";
#[cfg(target_os = "macos")]
const MACOS_AGENT_HELPER_EXE: &str = "Contents/MacOS/fluxdown-agent";

/// 拉起 agent 的命令。
///
/// macOS 打包布局下经 Launch Services（`open -g`）启动辅助 bundle：直接 spawn 的
/// agent 会被系统记为桌面 App 的附属进程，桌面退出后 Dock 仍以
/// `exited-with-subordinates` 保留其图标，直到托盘驻留的 agent 退出。`open` 在辅助
/// App 已运行时不会再起第二个实例，`-g` 不抢前台。
fn agent_command() -> Result<std::process::Command, std::io::Error> {
    #[cfg(target_os = "macos")]
    if std::env::var_os("FLUXDOWN_AGENT_BIN").is_none()
        && let Some(helper_app) = bundled_agent_app(&std::env::current_exe()?)
    {
        let mut command = std::process::Command::new("/usr/bin/open");
        command.arg("-g").arg(helper_app);
        return Ok(command);
    }
    let mut command = std::process::Command::new(agent_executable()?);
    detach_background_process(&mut command);
    Ok(command)
}

/// agent 可执行文件：`FLUXDOWN_AGENT_BIN` 覆盖，否则取桌面程序同级（macOS 打包布局由
/// [`agent_command`] 经辅助 bundle 启动，不走这里）。
pub(crate) fn agent_executable() -> Result<std::path::PathBuf, std::io::Error> {
    if let Some(path) = std::env::var_os("FLUXDOWN_AGENT_BIN") {
        return Ok(path.into());
    }
    Ok(std::env::current_exe()?.with_file_name(AGENT_BINARY))
}

/// 桌面程序位于 `<App>.app/Contents/MacOS/` 且辅助 bundle 内存在 agent 时返回辅助
/// bundle 路径；开发期平铺布局返回 `None`，回退同级查找。
#[cfg(target_os = "macos")]
fn bundled_agent_app(desktop_exe: &std::path::Path) -> Option<std::path::PathBuf> {
    let macos_dir = desktop_exe.parent()?;
    if !macos_dir.ends_with("Contents/MacOS") {
        return None;
    }
    let helper_app = macos_dir.parent()?.join(MACOS_AGENT_HELPER_APP);
    helper_app
        .join(MACOS_AGENT_HELPER_EXE)
        .is_file()
        .then_some(helper_app)
}

/// 后台服务与界面解耦：Windows 不弹控制台窗；Unix 进入独立进程组，终端里对桌面程序的
/// Ctrl-C 不连带终止常驻 agent。
#[cfg(windows)]
fn detach_background_process(command: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x0800_0000);
}

#[cfg(unix)]
fn detach_background_process(command: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(not(any(windows, unix)))]
fn detach_background_process(_command: &mut std::process::Command) {}

#[derive(Debug, thiserror::Error)]
pub enum BootstrapError {
    #[error("could not probe fluxdown-agent listener: {0}")]
    Probe(String),
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{
        Blocked, CRASH_COOLDOWN, CRASH_FAST_EXIT, CRASH_LOOP_THRESHOLD, ChildExit,
        LAUNCH_RETRY_INTERVAL, LaunchError, ListenerProbe, PROBE_TIMEOUT, SpawnHealth,
        agent_socket_target, connect_listener, format_exit_status,
    };

    #[test]
    fn listener_probe_uses_loopback_without_dns_and_accepts_ipv6() {
        assert_eq!(
            agent_socket_target("ws://localhost:17800/rpc").expect("localhost target"),
            "127.0.0.1:17800"
                .parse::<std::net::SocketAddr>()
                .expect("IPv4 address")
        );
        assert_eq!(
            agent_socket_target("ws://[::1]:17800/rpc").expect("IPv6 target"),
            "[::1]:17800"
                .parse::<std::net::SocketAddr>()
                .expect("IPv6 address")
        );
        assert!(agent_socket_target("ws://192.0.2.1:17800/rpc").is_err());
    }

    /// 冷启动时 agent 先绑定端口、装配完才开始 accept：已绑定但尚未 accept 的端口必须算「在监听」，
    /// 否则桌面会在装配期间重复拉起 agent；端口释放后必须立即判为被拒（而不是超时）。
    #[tokio::test]
    async fn bound_port_counts_as_listening_before_accept_and_refused_after_close() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let url = format!("ws://{}/rpc", listener.local_addr().expect("local addr"));
        assert!(matches!(
            connect_listener(&url, PROBE_TIMEOUT)
                .await
                .expect("probe bound port"),
            ListenerProbe::Listening(_)
        ));
        drop(listener);
        assert!(matches!(
            connect_listener(&url, PROBE_TIMEOUT)
                .await
                .expect("probe released port"),
            ListenerProbe::Refused
        ));
    }

    fn crash(uptime: Duration) -> ChildExit {
        ChildExit {
            success: false,
            uptime,
            status: "exit code 101".to_owned(),
            spawned_at_ms: 1_000,
        }
    }

    fn quick_crashes(health: &mut SpawnHealth, count: u32, at: Instant) {
        for _ in 0..count {
            health.record_exit(at, crash(Duration::from_secs(1)));
        }
    }

    #[test]
    fn crash_loop_needs_consecutive_quick_abnormal_exits() {
        let now = Instant::now();
        let mut health = SpawnHealth::default();
        quick_crashes(&mut health, CRASH_LOOP_THRESHOLD - 1, now);
        assert_eq!(health.blocked(now), None);
        quick_crashes(&mut health, 1, now);
        assert_eq!(
            health.blocked(now),
            Some(Blocked::CrashLoop {
                status: "exit code 101".to_owned(),
                spawned_at_ms: 1_000
            })
        );
    }

    /// 成功退出（另一个 agent 已持有数据目录锁）与存活足够久的退出都清零计数。
    #[test]
    fn success_exit_and_long_uptime_reset_the_streak() {
        let now = Instant::now();
        for reset in [
            ChildExit {
                success: true,
                uptime: Duration::from_millis(50),
                status: "exit code 0".to_owned(),
                spawned_at_ms: 1,
            },
            crash(CRASH_FAST_EXIT),
        ] {
            let mut health = SpawnHealth::default();
            quick_crashes(&mut health, CRASH_LOOP_THRESHOLD - 1, now);
            health.record_exit(now, reset);
            quick_crashes(&mut health, CRASH_LOOP_THRESHOLD - 1, now);
            assert_eq!(health.blocked(now), None);
        }
    }

    #[test]
    fn connecting_clears_crash_loop() {
        let now = Instant::now();
        let mut health = SpawnHealth::default();
        quick_crashes(&mut health, CRASH_LOOP_THRESHOLD, now);
        assert!(health.blocked(now).is_some());
        health.clear();
        assert_eq!(health.blocked(now), None);
    }

    /// 冷却自最近一次退出起算：到期放行一次，仍然快速失败就重新进入冷却。
    #[test]
    fn cooldown_expires_once_and_a_failed_retry_restarts_it() {
        let start = Instant::now();
        let mut health = SpawnHealth::default();
        quick_crashes(&mut health, CRASH_LOOP_THRESHOLD, start);
        assert!(
            health
                .blocked(start + CRASH_COOLDOWN - Duration::from_secs(1))
                .is_some()
        );
        let retry_at = start + CRASH_COOLDOWN;
        assert_eq!(health.blocked(retry_at), None);

        let failed_at = retry_at + Duration::from_secs(2);
        health.record_exit(failed_at, crash(Duration::from_secs(2)));
        assert!(health.blocked(failed_at).is_some());
        assert!(
            health
                .blocked(failed_at + CRASH_COOLDOWN - Duration::from_secs(1))
                .is_some()
        );
        assert_eq!(health.blocked(failed_at + CRASH_COOLDOWN), None);
    }

    #[test]
    fn launch_failure_blocks_until_the_retry_interval_then_clears_on_connect() {
        let now = Instant::now();
        let mut health = SpawnHealth::default();
        let blocked = health.record_launch_failure(
            now,
            LaunchError {
                program: "fluxdown-agent".to_owned(),
                detail: "could not spawn fluxdown-agent: denied".to_owned(),
            },
        );
        assert_eq!(
            blocked,
            Blocked::Launch {
                program: "fluxdown-agent".to_owned(),
                detail: "could not spawn fluxdown-agent: denied".to_owned()
            }
        );
        assert_eq!(health.blocked(now + Duration::from_secs(1)), Some(blocked));
        assert_eq!(health.blocked(now + LAUNCH_RETRY_INTERVAL), None);
        health.clear();
        assert_eq!(health.blocked(now), None);
    }

    #[test]
    fn exit_status_text_carries_code_and_hex_ntstatus() {
        assert_eq!(format_exit_status(Some(101), None), "exit code 101");
        assert_eq!(
            format_exit_status(Some(-1_073_741_819), None),
            "exit code -1073741819 (0xC0000005)"
        );
        assert_eq!(
            format_exit_status(None, Some(11)),
            "terminated by signal 11"
        );
        assert_eq!(format_exit_status(None, None), "unknown exit status");
    }
}
