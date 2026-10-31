//! 云端暂存上传引擎（`docs/temp-share-design.md` §7 / §10）。一个分享一个 [`Job`]，在后台任务里跑：
//!
//! 登记上传方案 → 同用户秒传的持有性证明 → 分片预签名 PUT（有界并发、逐片重试）→ 逐文件 `complete`。
//!
//! - 方案只含云端缺失的分片，重复调用即续传；预签名地址过期 / 被拒时重新取方案（`Stop::Replan`）。
//! - 分片经统一出口（`egress.rs`，`verified_payload = false`，只用原地址，不接受插件改写）按文件流式发送，
//!   内存里只有一个 64 KiB 读缓冲，Content-Length 显式给出。
//! - 进度 = 已完成分片字节 + 在途字节；速率为 2 秒窗口的 EWMA；只在传输期间每 500 ms 发布一次，没有空闲定时器。
//! - 网络 / 云端暂时不可用 → `Paused` 并按退避睡眠（取消 / 账号变化即醒），不会空转；
//!   额度、权益、源文件被改等不可恢复错误 → 以原因结束（`Failed`），断点保留，用户重试即续传。
//! - 断点文件（`upload_state.rs`）只在分片 / 文件完成时写。

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::SeekFrom;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, PoisonError, Weak};
use std::time::{Duration, Instant};

use fluxdown_protocol::{ErrorReason, ShareUploadDto, ShareUploadStatus};
use fluxdown_share::ShareFile;
use futures_util::StreamExt;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::sync::{Mutex, Notify};
use tokio::task::JoinSet;
use tokio_util::io::ReaderStream;
use tokio_util::sync::CancellationToken;

use super::cloud::{
    CompleteBody, ProofBody, UploadFilePlan, UploadFileRef, UploadFileState, UploadPlan,
    UploadPlanBody, share_error,
};
use super::service::ShareService;
use super::store::ShareStore;
use super::upload_state::{DonePart, UploadState};
use crate::cloud::{CloudApi, CloudError};
use crate::egress::{Egress, EgressClient};

/// 同时在途的分片数。
const CONCURRENCY: usize = 3;
/// 单个分片的最多尝试次数。
const PART_TRIES: u32 = 5;
/// 方案被拒后连续重取的次数上限（有进展即清零）。
const MAX_REPLANS: u32 = 5;
/// 速率 EWMA 的时间常数（秒）。
const RATE_WINDOW_SECS: f64 = 2.0;
/// 读缓冲：上传期间每个在途分片占用的内存。
const READ_CHUNK: usize = 64 * 1024;

/// 各类等待的时长；生产用 [`Timing::default`]，测试缩短。
#[derive(Clone, Copy, Debug)]
pub(super) struct Timing {
    /// 分片重试的首次退避（之后翻倍）。
    pub part_backoff: Duration,
    /// 暂停重试的首次退避（之后翻倍，不超过 `retry_max`）。
    pub retry_first: Duration,
    pub retry_max: Duration,
    /// 重新取方案前的短暂停顿。
    pub replan: Duration,
    /// 在途分片这么久没有任何字节进展即判定停滞。
    pub stall: Duration,
    pub progress_every: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            part_backoff: Duration::from_secs(1),
            retry_first: Duration::from_secs(2),
            retry_max: Duration::from_secs(60),
            replan: Duration::from_millis(500),
            stall: Duration::from_secs(30),
            progress_every: Duration::from_millis(500),
        }
    }
}

/// 一个要上传的源文件（下标 = 分享内的文件序号）。
#[derive(Clone, Debug)]
pub(super) struct JobFile {
    pub index: u32,
    pub path: std::path::PathBuf,
    pub size: u64,
    pub mtime_ms: i64,
    pub root_hash: String,
}

impl JobFile {
    /// 本机文件与清单登记时一致（口径见 [`file_unchanged`]）。
    pub(super) async fn matches_disk(&self) -> bool {
        file_unchanged(&self.path, self.size, self.mtime_ms).await
    }

    pub(super) fn from_manifest(index: u32, path: std::path::PathBuf, file: &ShareFile) -> Self {
        Self {
            index,
            path,
            size: file.size,
            mtime_ms: file.mtime_ms,
            root_hash: file.root_hash.clone(),
        }
    }
}

/// 本机文件与清单登记时一致（大小 + 修改时间，与发送方“文件被改”检测同一口径）；读不到元数据也算不一致。
pub(super) async fn file_unchanged(path: &std::path::Path, size: u64, mtime_ms: i64) -> bool {
    match tokio::fs::metadata(path).await {
        Ok(meta) => unchanged(&meta, size, mtime_ms),
        Err(_) => false,
    }
}

/// 清单里的全部源文件都与本机一致（`paths[i]` 对应 `files[i]`）。
pub(super) async fn sources_intact(files: &[ShareFile], paths: &[std::path::PathBuf]) -> bool {
    if files.len() != paths.len() {
        return false;
    }
    for (file, path) in files.iter().zip(paths) {
        if !file_unchanged(path, file.size, file.mtime_ms).await {
            return false;
        }
    }
    true
}

fn unchanged(meta: &std::fs::Metadata, size: u64, mtime_ms: i64) -> bool {
    meta.is_file()
        && meta.len() == size
        && fluxdown_share::hashing::file_mtime_ms(meta).ok() == Some(mtime_ms)
}

/// 一次尝试的结局。
#[derive(Debug)]
pub(super) enum Outcome {
    /// 所有文件都已在云端完成。
    Done,
    /// 被取消（用户 / 关停 / 服务已不在）：不改状态。
    Cancelled,
    /// 不可恢复的失败。
    Failed(Option<ErrorReason>),
}

/// 一次尝试被中断的原因。
#[derive(Debug)]
enum Stop {
    Cancelled,
    /// 地址过期 / 被拒 / 云端校验未过：重新取方案再试。
    Replan,
    /// 暂时不可用：暂停并退避重试。
    Retry(Option<ErrorReason>),
    Fatal(Option<ErrorReason>),
}

/// 一个待上传的分片。
#[derive(Clone, Debug)]
struct PartJob {
    file: u32,
    number: u32,
    offset: u64,
    len: u64,
    url: String,
}

enum PutError {
    Cancelled,
    Fatal(Option<ErrorReason>),
    /// 云端 / 存储拒绝（地址过期、签名不符、上传已不存在…）：换方案。
    Rejected,
    Transient(String),
}

/// 云端错误 → 重试 / 失败。登录失效等账号问题等同暂停：重新登录（账号变化）会唤醒。
fn classify(error: &CloudError) -> Stop {
    let reason = share_error(error).reason;
    let unauthorized = error.status == Some(401) || error.code.as_deref() == Some("unauthorized");
    let transient = error.unreachable || matches!(error.status, Some(408 | 425 | 429 | 500..=599));
    if unauthorized {
        Stop::Retry(Some(ErrorReason::SessionExpired))
    } else if transient {
        Stop::Retry(reason)
    } else {
        Stop::Fatal(reason)
    }
}

/// 第 `number` 片（1 起）在文件内的 `(偏移, 长度)`；编号越界返回 `None`。
fn part_range(number: u32, part_bytes: u64, size: u64) -> Option<(u64, u64)> {
    if size == 0 {
        return (number == 1).then_some((0, 0));
    }
    if number == 0 || part_bytes == 0 {
        return None;
    }
    let offset = u64::from(number - 1).checked_mul(part_bytes)?;
    (offset < size).then(|| (offset, part_bytes.min(size - offset)))
}

/// 文件区间 `[offset, offset + len)` 的 BLAKE3（小写 hex）。
async fn blake3_range(path: std::path::PathBuf, offset: u64, len: u64) -> std::io::Result<String> {
    tokio::task::spawn_blocking(move || {
        use std::io::{Read, Seek};
        let mut file = std::fs::File::open(path)?;
        file.seek(SeekFrom::Start(offset))?;
        let mut hasher = blake3::Hasher::new();
        let mut buf = vec![0u8; 1 << 20];
        let mut left = len;
        while left > 0 {
            let want = usize::try_from(left).map_or(buf.len(), |l| l.min(buf.len()));
            file.read_exact(&mut buf[..want])?;
            hasher.update(&buf[..want]);
            left -= want as u64;
        }
        Ok(fluxdown_share::hashing::to_hex(
            hasher.finalize().as_bytes(),
        ))
    })
    .await
    .map_err(std::io::Error::other)?
}

/// 请求上传方案（`tier_id` 只在转为暂存时带）。
pub(super) async fn request_plan(
    cloud: &CloudApi,
    share_id: &str,
    files: &[JobFile],
    tier_id: Option<&str>,
) -> Result<UploadPlan, CloudError> {
    let body = UploadPlanBody {
        files: files
            .iter()
            .map(|f| UploadFileRef {
                index: f.index,
                root_hash: &f.root_hash,
                size: f.size,
            })
            .collect(),
        tier_id,
    };
    cloud.share_upload_plan(share_id, &body).await
}

/// 已上传字节的计量：已完成分片 + 在途分片已交给传输层的字节。
struct Meter {
    total: u64,
    done: AtomicU64,
}

impl Meter {
    fn get(&self) -> u64 {
        self.done.load(Ordering::Relaxed)
    }

    fn set(&self, value: u64) {
        self.done.store(value.min(self.total), Ordering::Relaxed);
    }

    fn add(&self, bytes: u64) {
        self.done.fetch_add(bytes, Ordering::Relaxed);
    }

    fn sub(&self, bytes: u64) {
        let update = self
            .done
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
                Some(v.saturating_sub(bytes))
            });
        debug_assert!(update.is_ok(), "saturating update never declines");
    }
}

struct Rate {
    last_done: u64,
    last_at: Instant,
    ewma: f64,
}

pub(super) struct JobParams {
    pub share_id: String,
    pub files: Vec<JobFile>,
    pub cloud: CloudApi,
    pub egress: Egress,
    pub store: ShareStore,
    pub service: Weak<ShareService>,
    pub cancel: CancellationToken,
    pub wake: Arc<Notify>,
    pub timing: Timing,
}

pub(super) struct Job {
    share_id: String,
    files: Vec<JobFile>,
    cloud: CloudApi,
    egress: Egress,
    http: EgressClient,
    store: ShareStore,
    service: Weak<ShareService>,
    cancel: CancellationToken,
    wake: Arc<Notify>,
    timing: Timing,
    state: StdMutex<UploadState>,
    /// 断点文件写入串行化：并发完成的分片共用同一个临时文件名。
    gate: Mutex<()>,
    meter: Meter,
    /// 自上次检查起有分片 / 文件完成：退避与重取计数清零。
    progressed: AtomicBool,
}

impl Job {
    pub(super) fn new(params: JobParams) -> Arc<Self> {
        let total = params.files.iter().map(|f| f.size).sum();
        Arc::new(Self {
            share_id: params.share_id,
            files: params.files,
            cloud: params.cloud,
            egress: params.egress,
            http: EgressClient::new(|| {
                reqwest::Client::builder()
                    .connect_timeout(Duration::from_secs(10))
                    .read_timeout(Duration::from_secs(60))
                    .user_agent(concat!("fluxdown-agent/", env!("CARGO_PKG_VERSION")))
            }),
            store: params.store,
            service: params.service,
            cancel: params.cancel,
            wake: params.wake,
            timing: params.timing,
            state: StdMutex::new(UploadState::default()),
            gate: Mutex::new(()),
            meter: Meter {
                total,
                done: AtomicU64::new(0),
            },
            progressed: AtomicBool::new(false),
        })
    }

    fn file(&self, index: u32) -> Option<&JobFile> {
        self.files.iter().find(|f| f.index == index)
    }

    fn with_state<R>(&self, f: impl FnOnce(&mut UploadState) -> R) -> R {
        f(&mut self.state.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// 睡眠 `duration`；被取消返回 `false`。
    async fn nap(&self, duration: Duration) -> bool {
        tokio::select! {
            biased;
            () = self.cancel.cancelled() => false,
            () = tokio::time::sleep(duration) => true,
        }
    }

    /// 带取消的云端控制面调用。
    async fn guarded<T>(
        &self,
        call: impl Future<Output = Result<T, CloudError>>,
    ) -> Result<T, Stop> {
        tokio::select! {
            biased;
            () = self.cancel.cancelled() => Err(Stop::Cancelled),
            result = call => result.map_err(|error| classify(&error)),
        }
    }

    /// 把当前状态交给服务（`false` = 服务已不在，任务应结束）。
    async fn report(
        &self,
        status: ShareUploadStatus,
        reason: Option<ErrorReason>,
        rate: u64,
    ) -> bool {
        let Some(service) = self.service.upgrade() else {
            return false;
        };
        service
            .upload_progress(
                &self.share_id,
                &self.wake,
                ShareUploadDto {
                    status,
                    done_bytes: self.meter.get(),
                    total_bytes: self.meter.total,
                    rate_bps: rate,
                    reason,
                },
            )
            .await;
        true
    }

    // ───────────── 主循环 ─────────────

    /// 跑到完成 / 取消 / 失败。`first_plan`：转为暂存时已同步取到的方案（避免重复请求）。
    pub(super) async fn run(self: Arc<Self>, first_plan: Option<UploadPlan>) -> Outcome {
        if let Some(saved) = self.store.load_upload(&self.share_id).await {
            self.with_state(|state| *state = saved);
        }
        let mut plan = first_plan;
        let mut replans = 0u32;
        let mut delay = self.timing.retry_first;
        loop {
            let stop = match self.attempt(plan.take()).await {
                Ok(()) => return Outcome::Done,
                Err(stop) => stop,
            };
            if self.progressed.swap(false, Ordering::Relaxed) {
                delay = self.timing.retry_first;
                replans = 0;
            }
            let reason = match stop {
                Stop::Cancelled => return Outcome::Cancelled,
                Stop::Fatal(reason) => return Outcome::Failed(reason),
                Stop::Replan if replans < MAX_REPLANS => {
                    replans += 1;
                    if !self.nap(self.timing.replan).await {
                        return Outcome::Cancelled;
                    }
                    continue;
                }
                // 云端 / 存储一再拒绝：这不是网络问题，停下来让用户决定。
                Stop::Replan => return Outcome::Failed(Some(ErrorReason::ShareCloudUnavailable)),
                Stop::Retry(reason) => reason,
            };
            if !self.report(ShareUploadStatus::Paused, reason, 0).await {
                return Outcome::Cancelled;
            }
            tokio::select! {
                biased;
                () = self.cancel.cancelled() => return Outcome::Cancelled,
                () = tokio::time::sleep(delay) => {}
                () = self.wake.notified() => {}
            }
            delay = (delay * 2).min(self.timing.retry_max);
            if !self.report(ShareUploadStatus::Uploading, None, 0).await {
                return Outcome::Cancelled;
            }
        }
    }

    async fn attempt(self: &Arc<Self>, first_plan: Option<UploadPlan>) -> Result<(), Stop> {
        for file in &self.files {
            if !file.matches_disk().await {
                return Err(Stop::Fatal(Some(ErrorReason::ShareFileChanged)));
            }
        }
        let plan = match first_plan {
            Some(plan) => plan,
            None => {
                self.guarded(request_plan(&self.cloud, &self.share_id, &self.files, None))
                    .await?
            }
        };
        let (plan, proven) = self.resolve_challenges(plan).await?;
        self.transfer(&plan, &proven).await?;
        if let Err(error) = self.store.remove_upload(&self.share_id).await {
            tracing::warn!(error = %error, "could not remove the finished upload state");
        }
        Ok(())
    }

    // ───────────── 秒传 ─────────────

    /// 对 `challenge` 状态的文件做持有性证明。返回（最终方案，已证明为完成的文件）。
    async fn resolve_challenges(
        &self,
        mut plan: UploadPlan,
    ) -> Result<(UploadPlan, HashSet<u32>), Stop> {
        let mut proven = HashSet::new();
        for _ in 0..2 {
            let mut rejected = false;
            let pending: Vec<UploadFilePlan> = plan
                .files
                .iter()
                .filter(|f| f.state == UploadFileState::Challenge && !proven.contains(&f.index))
                .cloned()
                .collect();
            for entry in pending {
                let file = self.file(entry.index).ok_or(Stop::Fatal(None))?;
                let challenge = entry
                    .challenge
                    .filter(|c| {
                        c.offset
                            .checked_add(c.len)
                            .is_some_and(|end| end <= file.size)
                    })
                    .ok_or_else(|| {
                        tracing::warn!("upload challenge is missing or out of range");
                        Stop::Fatal(None)
                    })?;
                let digest = tokio::select! {
                    biased;
                    () = self.cancel.cancelled() => return Err(Stop::Cancelled),
                    digest = blake3_range(file.path.clone(), challenge.offset, challenge.len) => digest,
                }
                .map_err(|error| {
                    tracing::warn!(error = %error, "could not read a file for the upload proof");
                    Stop::Fatal(Some(ErrorReason::ShareFileChanged))
                })?;
                let body = ProofBody {
                    offset: challenge.offset,
                    len: challenge.len,
                    blake3: digest,
                };
                let result = tokio::select! {
                    biased;
                    () = self.cancel.cancelled() => return Err(Stop::Cancelled),
                    result = self.cloud.share_upload_proof(&self.share_id, entry.index, &body) => result,
                };
                match result {
                    Ok(_) => {
                        proven.insert(entry.index);
                    }
                    Err(error) if error.code.as_deref() == Some("share_proof_failed") => {
                        rejected = true;
                    }
                    Err(error) => return Err(classify(&error)),
                }
            }
            if !rejected {
                break;
            }
            // 证明被拒：再取方案，云端这次会按 needUpload 给出分片。
            plan = self
                .guarded(request_plan(&self.cloud, &self.share_id, &self.files, None))
                .await?;
        }
        let unresolved = plan
            .files
            .iter()
            .any(|f| f.state == UploadFileState::Challenge && !proven.contains(&f.index));
        if unresolved {
            tracing::warn!("upload challenge could not be resolved");
            return Err(Stop::Fatal(None));
        }
        Ok((plan, proven))
    }

    // ───────────── 分片传输 ─────────────

    async fn save_state(&self) {
        let _gate = self.gate.lock().await;
        let snapshot = self.with_state(|state| state.clone());
        if let Err(error) = self.store.save_upload(&self.share_id, &snapshot).await {
            // 断点丢了只会让下次 `complete` 带的 etag 是子集（云端以 ListParts 为准），不影响正确性。
            tracing::warn!(error = %error, "could not persist the upload state");
        }
    }

    async fn transfer(
        self: &Arc<Self>,
        plan: &UploadPlan,
        proven: &HashSet<u32>,
    ) -> Result<(), Stop> {
        let mut queue: VecDeque<PartJob> = VecDeque::new();
        let mut remaining: HashMap<u32, usize> = HashMap::new();
        let mut complete_now: Vec<u32> = Vec::new();
        let mut done = 0u64;
        for file in &self.files {
            if proven.contains(&file.index) {
                done += file.size;
                self.with_state(|state| state.remove(file.index));
                continue;
            }
            let entry = plan
                .files
                .iter()
                .find(|e| e.index == file.index)
                .ok_or_else(|| {
                    tracing::warn!(index = file.index, "upload plan lacks a file");
                    Stop::Fatal(None)
                })?;
            if entry.state == UploadFileState::Done {
                done += file.size;
                self.with_state(|state| state.remove(file.index));
                continue;
            }
            if entry.state != UploadFileState::NeedUpload {
                tracing::warn!(index = file.index, "upload plan has an unusable file state");
                return Err(Stop::Fatal(None));
            }
            let upload_id = entry.upload_id.as_deref().unwrap_or_default();
            self.with_state(|state| {
                let progress = state.entry(file.index, file.size, file.mtime_ms, upload_id);
                // 云端列为缺失的分片，以云端为准。
                progress
                    .done_parts
                    .retain(|p| !entry.parts.iter().any(|missing| missing.number == p.number));
            });
            let mut missing_bytes = 0u64;
            for part in &entry.parts {
                let (offset, len) = part_range(part.number, entry.part_bytes, file.size)
                    .ok_or_else(|| {
                        tracing::warn!(index = file.index, "upload plan has an out-of-range part");
                        Stop::Fatal(None)
                    })?;
                missing_bytes += len;
                queue.push_back(PartJob {
                    file: file.index,
                    number: part.number,
                    offset,
                    len,
                    url: part.url.clone(),
                });
            }
            done += file.size.saturating_sub(missing_bytes);
            if entry.parts.is_empty() {
                complete_now.push(file.index);
            } else {
                remaining.insert(file.index, entry.parts.len());
            }
        }
        self.meter.set(done);
        self.save_state().await;
        for index in complete_now {
            self.complete(index).await?;
        }

        let mut clients: HashMap<String, reqwest::Client> = HashMap::new();
        let mut set: JoinSet<Result<(PartJob, String), Stop>> = JoinSet::new();
        let mut rate = Rate {
            last_done: self.meter.get(),
            last_at: Instant::now(),
            ewma: 0.0,
        };
        self.tick(&mut rate).await?;
        let mut ticker = tokio::time::interval_at(
            tokio::time::Instant::now() + self.timing.progress_every,
            self.timing.progress_every,
        );
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            while set.len() < CONCURRENCY
                && let Some(part) = queue.pop_front()
            {
                let client = self.client_for(&part.url, &mut clients).await?;
                set.spawn(Arc::clone(self).upload_part(client, part));
            }
            if set.is_empty() {
                return Ok(());
            }
            tokio::select! {
                biased;
                () = self.cancel.cancelled() => return Err(Stop::Cancelled),
                joined = set.join_next() => match joined {
                    Some(Ok(Ok((part, etag)))) => {
                        self.progressed.store(true, Ordering::Relaxed);
                        self.with_state(|state| {
                            state.record(part.file, DonePart { number: part.number, etag });
                        });
                        self.save_state().await;
                        let finished = remaining.get_mut(&part.file).is_some_and(|left| {
                            *left = left.saturating_sub(1);
                            *left == 0
                        });
                        if finished {
                            self.complete(part.file).await?;
                        }
                    }
                    Some(Ok(Err(stop))) => return Err(stop),
                    Some(Err(error)) => {
                        tracing::error!(error = %error, "share upload part task failed");
                        return Err(Stop::Fatal(None));
                    }
                    None => {}
                },
                _ = ticker.tick() => self.tick(&mut rate).await?,
            }
        }
    }

    /// 出口网络按 URL 的 origin 取一次（同一存储端点的分片共用），只用原地址。
    async fn client_for(
        &self,
        url: &str,
        cache: &mut HashMap<String, reqwest::Client>,
    ) -> Result<reqwest::Client, Stop> {
        let origin = reqwest::Url::parse(url)
            .map_err(|_| {
                tracing::warn!("upload plan has an invalid part address");
                Stop::Fatal(None)
            })?
            .origin()
            .ascii_serialization();
        if let Some(client) = cache.get(&origin) {
            return Ok(client.clone());
        }
        let plan = self.egress.plan(url, false).await;
        let client = self.http.get(&plan.network).await.map_err(|error| {
            tracing::warn!(error = %error, "upload HTTP client unavailable");
            Stop::Fatal(None)
        })?;
        cache.insert(origin, client.clone());
        Ok(client)
    }

    async fn tick(&self, rate: &mut Rate) -> Result<(), Stop> {
        let now = Instant::now();
        let done = self.meter.get();
        let dt = now.duration_since(rate.last_at).as_secs_f64();
        if dt > 0.0 {
            let instant = done.saturating_sub(rate.last_done) as f64 / dt;
            let alpha = 1.0 - (-dt / RATE_WINDOW_SECS).exp();
            rate.ewma += alpha * (instant - rate.ewma);
        }
        rate.last_done = done;
        rate.last_at = now;
        if self
            .report(
                ShareUploadStatus::Uploading,
                None,
                rate.ewma.max(0.0) as u64,
            )
            .await
        {
            Ok(())
        } else {
            Err(Stop::Cancelled)
        }
    }

    async fn complete(&self, index: u32) -> Result<(), Stop> {
        let parts = self.with_state(|state| state.done_parts(index));
        let body = CompleteBody { parts: &parts };
        let result = tokio::select! {
            biased;
            () = self.cancel.cancelled() => return Err(Stop::Cancelled),
            result = self.cloud.share_upload_complete(&self.share_id, index, &body) => result,
        };
        match result {
            Ok(_) => {}
            // 云端按 ListParts 校验后认为分片不全 / 大小不符：重取方案补齐。
            Err(error) if matches!(error.status, Some(400 | 409 | 422)) => {
                tracing::warn!(index, code = ?error.code, "upload complete was refused; re-planning");
                return Err(Stop::Replan);
            }
            Err(error) => return Err(classify(&error)),
        }
        self.progressed.store(true, Ordering::Relaxed);
        self.with_state(|state| state.remove(index));
        self.save_state().await;
        Ok(())
    }

    async fn upload_part(
        self: Arc<Self>,
        client: reqwest::Client,
        part: PartJob,
    ) -> Result<(PartJob, String), Stop> {
        let mut delay = self.timing.part_backoff;
        for attempt in 1..=PART_TRIES {
            let sent = Arc::new(AtomicU64::new(0));
            let failure = match self.put_once(&client, &part, &sent).await {
                Ok(etag) => return Ok((part, etag)),
                Err(failure) => failure,
            };
            // 这次尝试交给传输层的字节没有真正完成：回退计量。
            self.meter.sub(sent.load(Ordering::Relaxed));
            match failure {
                PutError::Cancelled => return Err(Stop::Cancelled),
                PutError::Fatal(reason) => return Err(Stop::Fatal(reason)),
                PutError::Rejected => return Err(Stop::Replan),
                PutError::Transient(why) => {
                    tracing::debug!(part = part.number, attempt, error = %why, "share upload part failed");
                    if attempt == PART_TRIES {
                        return Err(Stop::Retry(Some(ErrorReason::CloudUnreachable)));
                    }
                    if !self.nap(delay).await {
                        return Err(Stop::Cancelled);
                    }
                    delay = (delay * 2).min(self.timing.retry_max);
                }
            }
        }
        Err(Stop::Retry(Some(ErrorReason::CloudUnreachable)))
    }

    /// 一次 PUT：从源文件流式读取 `[offset, offset + len)`，返回云端给出的 etag（保持原样，含引号）。
    async fn put_once(
        self: &Arc<Self>,
        client: &reqwest::Client,
        part: &PartJob,
        sent: &Arc<AtomicU64>,
    ) -> Result<String, PutError> {
        let changed = || PutError::Fatal(Some(ErrorReason::ShareFileChanged));
        let file = self.file(part.file).ok_or(PutError::Fatal(None))?;
        let mut handle = tokio::fs::File::open(&file.path)
            .await
            .map_err(|_| changed())?;
        let meta = handle.metadata().await.map_err(|_| changed())?;
        if !unchanged(&meta, file.size, file.mtime_ms) {
            return Err(changed());
        }
        handle
            .seek(SeekFrom::Start(part.offset))
            .await
            .map_err(|_| changed())?;
        let job = Arc::clone(self);
        let counter = Arc::clone(sent);
        let stream =
            ReaderStream::with_capacity(handle.take(part.len), READ_CHUNK).map(move |chunk| {
                if let Ok(bytes) = &chunk {
                    let n = bytes.len() as u64;
                    job.meter.add(n);
                    counter.fetch_add(n, Ordering::Relaxed);
                }
                chunk
            });
        let send = client
            .put(&part.url)
            .header(reqwest::header::CONTENT_LENGTH, part.len)
            .body(reqwest::Body::wrap_stream(stream))
            .send();
        tokio::pin!(send);
        let mut last = 0u64;
        let response = loop {
            tokio::select! {
                biased;
                () = self.cancel.cancelled() => return Err(PutError::Cancelled),
                result = &mut send => {
                    // 预签名地址带签名：错误文本里不得出现 URL。
                    break result.map_err(|e| PutError::Transient(e.without_url().to_string()))?;
                }
                () = tokio::time::sleep(self.timing.stall) => {
                    let now = sent.load(Ordering::Relaxed);
                    if now == last {
                        return Err(PutError::Transient("upload stalled".to_owned()));
                    }
                    last = now;
                }
            }
        };
        let status = response.status();
        if status.is_success() {
            return response
                .headers()
                .get(reqwest::header::ETAG)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned)
                .ok_or(PutError::Rejected);
        }
        if matches!(status.as_u16(), 408 | 425 | 429 | 500..=599) {
            Err(PutError::Transient(format!("HTTP {status}")))
        } else {
            Err(PutError::Rejected)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::part_range;

    #[test]
    fn part_ranges_cover_the_file_exactly() {
        // 3 个整片 + 100 字节的尾片。
        let (p, size) = (1000u64, 3100u64);
        assert_eq!(part_range(1, p, size), Some((0, 1000)));
        assert_eq!(part_range(3, p, size), Some((2000, 1000)));
        assert_eq!(part_range(4, p, size), Some((3000, 100)));
        assert_eq!(part_range(5, p, size), None);
        assert_eq!(part_range(0, p, size), None);
        assert_eq!(part_range(1, 0, size), None);
        assert_eq!(part_range(1, p, 0), Some((0, 0)));
        assert_eq!(part_range(2, p, 0), None);
    }
}
