//! 分类保存目录路由（外部捕获的「跟随自定义分类」）。
//!
//! 宿主（daemon）对未替用户选定保存目录的建任务请求附带路由规则。引擎在三个时刻按同一规则
//! 定落点，中间过程（`.fdownloading` 临时文件、HLS 顺序写入、DASH / 轨对的 sidecar 与
//! ffmpeg 转封装）一律留在当时的目录，不受影响：
//!
//! 1. 建任务：按初始名（显式名 / URL 末段；解析插件任务只看显式名）选初始目录；
//! 2. 开始写盘前（启动序幕定名之后、「文件已存在」判断与去重之前）：按探测得到的服务器
//!    文件名、按类型推断的名字、插件 resolve 改的名字重选目录，只改任务记录、不移动文件，
//!    多数「名字到开始下载才知道」的任务（脚本地址、查询参数里的类型）因此直接写进分类目录；
//! 3. 完成前（插件 `finalize` 之后、写状态 3 之前）：按最终文件名复核——跳过探测的任务、
//!    按实际响应精修的名、插件替换的产物、HLS 转封装失败留下的 `.ts` 都在此时才确定——
//!    命中另一个分类目录时把产物移过去（同卷 rename，跨卷可中断复制后删除源）。
//!
//! HLS / DASH 的成品是 MP4，前两步按 `<stem>.mp4` 预判（[`output_name`]）。
//!
//! 不参与路由：BT / eD2K（做种与共享要求文件原地不动）、任务组成员（组以基准目录为根，成员
//! 保持组内结构）、带子路径的单条目清单与 `out=子目录/名字`（相对结构由来源决定）。
//!
//! 匹配语义与 agent `category_dir.rs`（确认表单预填）逐条对齐，两端共用夹具
//! `fixtures/category_route_cases.json`。

use std::path::Path;

use serde::{Deserialize, Serialize};

use tokio_util::sync::CancellationToken;

use crate::downloader::{DownloadError, DownloadParams, claim_final_name};
use crate::events::EngineEvent;
use crate::logger::{log_info, log_warn};

/// 任务的分类目录路由（持久化于 `tasks.category_route`，JSON；空串 = 不路由）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CategoryRoute {
    /// 路由未命中时的落点：建任务时解析出的保存目录。任务组与带子路径的清单条目以它为根。
    pub base_dir: String,
    /// 可见普通分类，按显示顺序。
    pub rules: Vec<CategoryRule>,
    /// 「其他」分类的目录；空 = 不兜底。
    pub other_dir: String,
}

/// [`CategoryRoute::rules`] 的单条普通分类。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CategoryRule {
    /// `regex` = 按正则匹配文件名；其余按扩展名。
    pub match_mode: String,
    pub extensions: Vec<String>,
    pub regex_pattern: String,
    /// 空 = 不改变落点，只阻止「其他」兜底。
    pub save_dir: String,
}

impl CategoryRoute {
    /// 文件名命中的分类目录；未命中 / 命中分类未配置目录时为 `None`（留在当前目录）。
    ///
    /// 文件名不含 `.` 时用 URL 路径末段（百分号解码后含 `.`）匹配。先取首个配置了目录且
    /// 命中的普通分类；所有普通分类都不命中时取「其他」的目录。
    ///
    /// # Examples
    ///
    /// ```
    /// use fluxdown_engine::category_route::{CategoryRoute, CategoryRule};
    ///
    /// let route = CategoryRoute {
    ///     rules: vec![CategoryRule {
    ///         match_mode: "extension".into(),
    ///         extensions: vec!["jpg".into()],
    ///         save_dir: "/pictures".into(),
    ///         ..Default::default()
    ///     }],
    ///     ..Default::default()
    /// };
    /// assert_eq!(route.save_dir_for("a.JPG", ""), Some("/pictures"));
    /// assert_eq!(route.save_dir_for("a.zip", ""), None);
    /// ```
    #[must_use]
    pub fn save_dir_for(&self, file_name: &str, url: &str) -> Option<&str> {
        let url_name;
        let name = if file_name.contains('.') {
            file_name
        } else {
            url_name = file_name_from_url(url);
            url_name.as_deref().unwrap_or(file_name)
        };
        if name.is_empty() {
            return None;
        }
        if let Some(rule) = self
            .rules
            .iter()
            .find(|rule| !rule.save_dir.is_empty() && rule.matches(name))
        {
            return Some(rule.save_dir.as_str());
        }
        (!self.other_dir.is_empty() && !self.rules.iter().any(|rule| rule.matches(name)))
            .then_some(self.other_dir.as_str())
    }

    /// 解析 `tasks.category_route`；空串为 `None`，损坏的值记日志后按不路由处理。
    pub(crate) fn from_column(task_id: &str, text: &str) -> Option<Self> {
        if text.is_empty() {
            return None;
        }
        match serde_json::from_str(text) {
            Ok(route) => Some(route),
            Err(error) => {
                log_warn!("[category-route] task {task_id}: ignoring malformed route: {error}");
                None
            }
        }
    }

    /// 编码为 `tasks.category_route` 列值。
    pub(crate) fn to_column(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

impl CategoryRule {
    fn matches(&self, name: &str) -> bool {
        if self.match_mode == "regex" {
            return !self.regex_pattern.is_empty()
                && regex::RegexBuilder::new(&self.regex_pattern)
                    .case_insensitive(true)
                    .build()
                    .is_ok_and(|regex| regex.is_match(name));
        }
        let Some((_, extension)) = name.rsplit_once('.') else {
            return false;
        };
        !extension.is_empty()
            && self.extensions.iter().any(|candidate| {
                candidate
                    .trim_start_matches('.')
                    .eq_ignore_ascii_case(extension)
            })
    }
}

/// URL 路径末段（百分号解码）且含 `.` 时作为文件名。
fn file_name_from_url(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url.trim()).ok()?;
    let last = parsed.path_segments()?.next_back()?;
    let decoded = percent_decode(last);
    decoded.contains('.').then_some(decoded)
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && let Some(byte) = bytes
                .get(index + 1..index + 3)
                .and_then(|hex| std::str::from_utf8(hex).ok())
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            out.push(byte);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 完成前按最终文件名复核落点（见模块文档第 2 步）；产物已移入分类目录且文件名因目标目录
/// 占名而改变时返回 `(新文件名, 大小)`，供完成帧上报。任何失败都只记日志与任务活动、产物
/// 留在原目录，绝不让已下载完成的任务失败。
pub(crate) async fn relocate_completed(p: &DownloadParams) -> Option<(String, i64)> {
    let route = match p.db.load_task_category_route(&p.task_id).await {
        Ok(text) => CategoryRoute::from_column(&p.task_id, &text)?,
        Err(error) => {
            crate::logger::report_warning("category-route", "load task route", &error);
            return None;
        }
    };
    let row = match p.db.load_task_by_id(&p.task_id).await {
        Ok(Some(row)) => row,
        Ok(None) => return None,
        Err(error) => {
            crate::logger::report_warning("category-route", "load completed task", &error);
            return None;
        }
    };
    if !row.group_id.is_empty() {
        return None;
    }
    let url = if row.origin_url.is_empty() {
        row.url.as_str()
    } else {
        row.origin_url.as_str()
    };
    let target = route.save_dir_for(&row.file_name, url)?;
    if same_dir(target, &row.save_dir).await {
        return None;
    }
    let src = Path::new(&row.save_dir).join(&row.file_name);
    let notes = crate::intercept::ActivityNotes::of(p);
    // mux 降级留下独立音轨（DASH / HLS 音频组 / 轨对）时产物是两个文件：不拆散，整体
    // 留在原目录。无法确认是否存在时同样保守处理。
    let sidecar = crate::dash_downloader::build_audio_path(&src);
    if tokio::fs::try_exists(&sidecar).await.unwrap_or(true) {
        notes
            .add("存在独立音轨文件，未移入分类目录".to_string())
            .await;
        return None;
    }
    let target_dir = Path::new(target);
    // 全局「覆盖」对目标目录同样生效；逐任务覆盖答复只绑定询问时的目录与名字，不外推。
    let overwrite = match p.overwrite {
        crate::file_exists::OverwritePolicy::Any => crate::file_exists::OverwritePolicy::Any,
        _ => crate::file_exists::OverwritePolicy::Never,
    };
    let chosen = match move_into(
        &src,
        target_dir,
        &row.file_name,
        &overwrite,
        &p.cancel_token,
    )
    .await
    {
        Ok(chosen) => chosen,
        Err(error) => {
            log_warn!(
                "[category-route] task {} could not move into {}: {error:#}",
                p.task_id,
                target_dir.display()
            );
            notes
                .add(format!(
                    "未能移入分类目录 {target}（{error}），文件保留在原目录"
                ))
                .await;
            return None;
        }
    };
    let moved = target_dir.join(&chosen);
    if let Err(error) = p.db.set_task_location(&p.task_id, target, &chosen).await {
        crate::logger::report_error("category-route", "persist relocated task", &error);
        // 库里仍是旧位置：把文件移回，保证任务记录与磁盘一致。
        let never = crate::file_exists::OverwritePolicy::Never;
        let uncancellable = CancellationToken::new();
        let restore = move_into(
            &moved,
            Path::new(&row.save_dir),
            &row.file_name,
            &never,
            &uncancellable,
        );
        if let Err(back) = restore.await {
            crate::logger::report_error("category-route", "restore relocated file", &back);
        }
        return None;
    }
    log_info!(
        "[category-route] task {} moved into category dir {}",
        p.task_id,
        target_dir.display()
    );
    notes.add(format!("已按分类移至 {target}")).await;
    publish_location(p).await;
    let size = match tokio::fs::metadata(&moved).await {
        Ok(meta) => i64::try_from(meta.len()).unwrap_or(i64::MAX),
        Err(error) => {
            crate::logger::report_warning("category-route", "stat relocated file", &error);
            row.total_bytes
        }
    };
    (chosen != row.file_name).then_some((chosen, size))
}

/// 新位置随完整任务事实广播（进度帧不携带保存目录）；与整表快照互斥，见 `Db::lock_event_order`。
async fn publish_location(p: &DownloadParams) {
    let _order = p.db.lock_event_order().await;
    match p.db.load_task_by_id(&p.task_id).await {
        Ok(Some(task)) => p.sink.emit(EngineEvent::TaskChanged(Box::new(task))),
        Ok(None) => {}
        Err(error) => {
            crate::logger::report_warning("category-route", "load relocated task", &error)
        }
    }
}

/// 分类判断用的成品名：HLS 一律转封装为 MP4，DASH 的空名 / `.mpd` 名合流为 MP4（与启动
/// 序幕的命名归一化一致），按 `<stem>.mp4` 预判；其余返回 `None`（用原名）。
pub(crate) fn output_name(file_name: &str, url: &str) -> Option<String> {
    let hls = crate::hls_downloader::is_hls_url(url);
    let dash = crate::dash_downloader::is_dash_url(url)
        && (file_name.is_empty() || file_name.to_ascii_lowercase().ends_with(".mpd"));
    if !hls && !dash {
        return None;
    }
    let base = if file_name.is_empty() {
        crate::naming::extract_from_url(url)?
    } else {
        file_name.to_owned()
    };
    let stem = Path::new(&base).file_stem()?.to_str()?;
    Some(format!("{stem}.mp4"))
}

/// 开始写盘前按当前最佳文件名重选目录（见模块文档）：启动序幕定名（探测得到的服务器
/// 文件名、按类型推断的名字、插件 resolve 改的名字）之后、「文件已存在」判断与去重
/// 之前调用，只改任务记录，不移动任何文件。HLS 按转封装后的 `.mp4` 预判；未命中任何
/// 分类时回到基准目录。
pub(crate) async fn route_before_start(p: &mut DownloadParams) -> Result<(), crate::db::DbError> {
    if p.file_name.is_empty() {
        return Ok(());
    }
    let column = p.db.load_task_category_route(&p.task_id).await?;
    let Some(route) = CategoryRoute::from_column(&p.task_id, &column) else {
        return Ok(());
    };
    if route.base_dir.is_empty() {
        return Ok(());
    }
    let output = output_name(&p.file_name, &p.url);
    let name = output.as_deref().unwrap_or(&p.file_name);
    let target = route
        .save_dir_for(name, &p.url)
        .unwrap_or(&route.base_dir)
        .to_owned();
    if same_dir(&target, &p.save_dir).await {
        return Ok(());
    }
    p.db.set_task_location(&p.task_id, &target, &p.file_name)
        .await?;
    log_info!(
        "[category-route] task {} starts in category dir {target}",
        p.task_id
    );
    p.save_dir = target;
    publish_location(p).await;
    Ok(())
}

/// 两个目录是否同一处：字面相同，或都存在且规范化后相同（大小写不敏感的文件系统、
/// 符号链接、`..`）。同一目录内「移动」会被占名协议当成冲突而编号改名，必须先排除。
async fn same_dir(a: &str, b: &str) -> bool {
    if Path::new(a) == Path::new(b) {
        return true;
    }
    match (
        tokio::fs::canonicalize(a).await,
        tokio::fs::canonicalize(b).await,
    ) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// 把 `src` 移到 `dir/name`（占名冲突按编号改名，`overwrite` 授权时替换同名旧文件），
/// 返回实际文件名。同卷 rename；跨卷先复制为 `dir` 内的临时文件（保留修改时间，可被
/// 暂停 / 删除打断）再占名，成功后删除源。
async fn move_into(
    src: &Path,
    dir: &Path,
    name: &str,
    overwrite: &crate::file_exists::OverwritePolicy,
    cancel: &CancellationToken,
) -> Result<String, DownloadError> {
    crate::output::ensure_parent(&dir.join(name)).await?;
    let avoid = std::collections::HashSet::new();
    match claim_final_name(src, dir, name, overwrite, &avoid).await {
        Err(DownloadError::Io(error)) if error.kind() == std::io::ErrorKind::CrossesDevices => {}
        result => return result,
    }
    let staging = dir.join(format!(".{name}.fdmoving"));
    if let Err(error) = copy_cancellable(src, &staging, cancel).await {
        remove_staging(&staging).await;
        return Err(error);
    }
    let chosen = match claim_final_name(&staging, dir, name, overwrite, &avoid).await {
        Ok(chosen) => chosen,
        Err(error) => {
            remove_staging(&staging).await;
            return Err(error);
        }
    };
    if let Err(error) = tokio::fs::remove_file(src).await {
        crate::logger::report_warning("category-route", "remove moved source", &error);
    }
    Ok(chosen)
}

/// 分块复制并逐块检查取消；完成后同步落盘并带上源文件的修改时间（「使用服务器文件时间」）。
async fn copy_cancellable(
    src: &Path,
    dst: &Path,
    cancel: &CancellationToken,
) -> Result<(), DownloadError> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut reader = tokio::fs::File::open(src).await?;
    let modified = reader.metadata().await?.modified().ok();
    let mut writer = tokio::fs::File::create(dst).await?;
    let mut buffer = vec![0u8; COPY_CHUNK_BYTES];
    loop {
        if cancel.is_cancelled() {
            return Err(DownloadError::Cancelled);
        }
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        writer.write_all(&buffer[..read]).await?;
    }
    writer.flush().await?;
    let file = writer.into_std().await;
    tokio::task::spawn_blocking(move || {
        if let Some(modified) = modified {
            file.set_modified(modified)?;
        }
        file.sync_all()
    })
    .await
    .map_err(std::io::Error::other)??;
    Ok(())
}

/// 跨卷复制的分块大小。
const COPY_CHUNK_BYTES: usize = 1024 * 1024;

async fn remove_staging(staging: &Path) {
    if let Err(error) = tokio::fs::remove_file(staging).await
        && error.kind() != std::io::ErrorKind::NotFound
    {
        crate::logger::report_warning("category-route", "remove move staging", &error);
    }
}

#[cfg(test)]
mod tests {
    use super::CategoryRoute;

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Case {
        name: String,
        route: CategoryRoute,
        file_name: String,
        url: String,
        expected: Option<String>,
    }

    #[test]
    fn shared_cases() {
        let cases: Vec<Case> =
            serde_json::from_str(include_str!("fixtures/category_route_cases.json"))
                .expect("category route fixture");
        assert!(!cases.is_empty());
        for case in cases {
            assert_eq!(
                case.route.save_dir_for(&case.file_name, &case.url),
                case.expected.as_deref(),
                "{}",
                case.name
            );
        }
    }

    #[test]
    fn column_round_trips_and_tolerates_corruption() {
        let route = CategoryRoute {
            base_dir: "/downloads".into(),
            other_dir: "/other".into(),
            ..Default::default()
        };
        let text = route.to_column().expect("encode route");
        assert_eq!(CategoryRoute::from_column("t", &text), Some(route));
        assert_eq!(CategoryRoute::from_column("t", ""), None);
        assert_eq!(CategoryRoute::from_column("t", "{"), None);
    }

    #[test]
    fn streaming_outputs_are_classified_as_mp4() {
        use super::output_name;
        assert_eq!(
            output_name("clip.ts", "https://cdn.test/live/index.m3u8").as_deref(),
            Some("clip.mp4")
        );
        assert_eq!(
            output_name("", "https://cdn.test/live/index.m3u8").as_deref(),
            Some("index.mp4")
        );
        assert_eq!(
            output_name("manifest.mpd", "https://cdn.test/v/manifest.mpd").as_deref(),
            Some("manifest.mp4")
        );
        // DASH 的显式非 .mpd 名原样使用；普通链接不预判。
        assert_eq!(
            output_name("movie.mkv", "https://cdn.test/v/manifest.mpd"),
            None
        );
        assert_eq!(output_name("a.ts", "https://cdn.test/a.ts"), None);
    }

    #[tokio::test]
    async fn move_keeps_existing_files_and_cross_volume_copy_keeps_mtime() {
        let root =
            std::env::temp_dir().join(format!("fluxdown_route_move_{}", uuid::Uuid::new_v4()));
        let source_dir = root.join("base");
        let target_dir = root.join("pictures");
        tokio::fs::create_dir_all(&source_dir)
            .await
            .expect("source dir");
        tokio::fs::create_dir_all(&target_dir)
            .await
            .expect("target dir");
        tokio::fs::write(target_dir.join("a.jpg"), b"existing")
            .await
            .expect("existing file");
        let source = source_dir.join("a.jpg");
        tokio::fs::write(&source, b"payload")
            .await
            .expect("source file");
        let never = crate::file_exists::OverwritePolicy::Never;
        let cancel = tokio_util::sync::CancellationToken::new();

        let chosen = super::move_into(&source, &target_dir, "a.jpg", &never, &cancel)
            .await
            .expect("move");
        assert_eq!(chosen, "a (1).jpg");
        assert_eq!(
            tokio::fs::read(target_dir.join("a.jpg"))
                .await
                .expect("kept"),
            b"existing"
        );
        assert_eq!(
            tokio::fs::read(target_dir.join(&chosen))
                .await
                .expect("moved"),
            b"payload"
        );
        assert!(!tokio::fs::try_exists(&source).await.expect("stat source"));

        // 跨卷分支：分块复制带上源文件的修改时间；已取消时中止复制。
        let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000);
        let copy_src = target_dir.join(&chosen);
        std::fs::File::options()
            .write(true)
            .open(&copy_src)
            .and_then(|file| file.set_modified(old))
            .expect("set source mtime");
        let copy_dst = source_dir.join("copy.jpg");
        super::copy_cancellable(&copy_src, &copy_dst, &cancel)
            .await
            .expect("copy");
        assert_eq!(
            tokio::fs::read(&copy_dst).await.expect("copied"),
            b"payload"
        );
        let copied = tokio::fs::metadata(&copy_dst).await.expect("stat copy");
        assert_eq!(copied.modified().expect("mtime"), old);
        cancel.cancel();
        let cancelled =
            super::copy_cancellable(&copy_src, &source_dir.join("x.jpg"), &cancel).await;
        assert!(matches!(cancelled, Err(super::DownloadError::Cancelled)));

        if let Err(error) = tokio::fs::remove_dir_all(&root).await {
            tracing::warn!(%error, "test directory cleanup failed");
        }
    }
}
