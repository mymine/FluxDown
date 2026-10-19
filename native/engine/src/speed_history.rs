//! 任务整个下载期的速度曲线与统计（详情「速度」页的持久数据源）。
//!
//! 进度报告器（`download_manager::progress_reporter`）对下载态（status=1）的任务
//! 按墙钟逐秒把平滑速度喂给 [`SpeedHistory`]；横轴只计下载秒数，暂停 / 排队 /
//! 做种不计入。曲线点数封顶 [`SPEED_HISTORY_MAX_POINTS`]：满额时相邻两点合并、
//! 桶宽翻倍，任意时长的任务都以有界存储覆盖全程。
//!
//! 活跃任务的历史常驻内存（[`SpeedHistoryLive`]，挂在 [`Db`] 上供进度报告器与
//! daemon 查询共享），每 [`SAVE_INTERVAL`] 及离开下载态时落库到
//! `task_speed_history`；只在任务下载期间写库，不破坏空闲静默。

use std::collections::HashMap;
use std::time::{Duration, Instant};

use sqlx::Row as _;

use crate::db::{Db, DbError};

/// 曲线点数上限（偶数：满额时两两合并）。
pub const SPEED_HISTORY_MAX_POINTS: usize = 240;

/// 下载期间的落库间隔；离开下载态时另行立即落库。
const SAVE_INTERVAL: Duration = Duration::from_secs(30);

/// 相邻两帧间隔超过该秒数时只补这么多秒：进度帧在下载态至少每秒数次，
/// 更长的空窗来自进程挂起 / 系统休眠，不是下载时间。
const MAX_GAP_SECS: u64 = 10;

/// 一个任务的速度曲线与统计。
///
/// ```
/// use fluxdown_engine::speed_history::SpeedHistory;
/// let mut history = SpeedHistory::default();
/// history.push_second(100);
/// history.push_second(300);
/// assert_eq!(history.points(), vec![100, 300]);
/// assert_eq!(history.peak_bps, 300);
/// assert_eq!(history.transferred_bytes, 400);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpeedHistory {
    /// 每个已完成点覆盖的下载秒数。
    pub bucket_secs: u32,
    /// 已完成的点（每点为桶内平均速率，字节/秒），最旧在前；长度恒小于上限。
    pub samples: Vec<i64>,
    /// 当前未满桶的速率累计。
    pub pending_sum: i64,
    /// 当前未满桶已累计的秒数（恒小于 `bucket_secs`）。
    pub pending_secs: u32,
    /// 累计下载秒数。
    pub active_secs: i64,
    /// 按逐秒速率累计的传输字节。
    pub transferred_bytes: i64,
    /// 逐秒速率峰值。
    pub peak_bps: i64,
}

impl Default for SpeedHistory {
    fn default() -> Self {
        Self {
            bucket_secs: 1,
            samples: Vec::new(),
            pending_sum: 0,
            pending_secs: 0,
            active_secs: 0,
            transferred_bytes: 0,
            peak_bps: 0,
        }
    }
}

impl SpeedHistory {
    /// 记录一秒下载态的速率（负值按 0 计）。
    pub fn push_second(&mut self, bps: i64) {
        let bps = bps.max(0);
        self.active_secs = self.active_secs.saturating_add(1);
        self.transferred_bytes = self.transferred_bytes.saturating_add(bps);
        self.peak_bps = self.peak_bps.max(bps);
        self.pending_sum = self.pending_sum.saturating_add(bps);
        self.pending_secs += 1;
        if self.pending_secs >= self.bucket_secs {
            self.samples
                .push(self.pending_sum / i64::from(self.pending_secs));
            self.pending_sum = 0;
            self.pending_secs = 0;
            self.compact();
        }
    }

    /// 展示用点序列：已完成的点，外加未满桶的当前均值（若有）。
    #[must_use]
    pub fn points(&self) -> Vec<i64> {
        let mut points = Vec::with_capacity(self.samples.len() + 1);
        points.extend_from_slice(&self.samples);
        if self.pending_secs > 0 {
            points.push(self.pending_sum / i64::from(self.pending_secs));
        }
        points
    }

    /// 满额时相邻两点取均值合并、桶宽翻倍，直到低于上限（落库数据异常时可能多轮）。
    fn compact(&mut self) {
        while self.samples.len() >= SPEED_HISTORY_MAX_POINTS {
            self.samples = self
                .samples
                .chunks(2)
                .map(|pair| {
                    pair.iter().fold(0_i64, |sum, v| sum.saturating_add(*v)) / pair.len() as i64
                })
                .collect();
            self.bucket_secs = self.bucket_secs.saturating_mul(2);
        }
    }

    /// 修正落库值里不满足不变式的字段（桶宽 ≥ 1、未满桶小于桶宽、点数低于上限）。
    fn normalized(mut self) -> Self {
        self.bucket_secs = self.bucket_secs.max(1);
        self.compact();
        if self.pending_secs >= self.bucket_secs {
            self.samples
                .push(self.pending_sum / i64::from(self.pending_secs));
            self.pending_sum = 0;
            self.pending_secs = 0;
            self.compact();
        }
        self
    }
}

/// 下载中任务的内存历史；进度报告器写、daemon 查询读，落库与「重新下载」清空
/// 都在同一把锁内完成，过期的落库不会让已清空的历史复活。
#[derive(Default)]
pub struct SpeedHistoryLive {
    entries: tokio::sync::Mutex<HashMap<String, LiveEntry>>,
}

struct LiveEntry {
    history: SpeedHistory,
    /// 已计入历史的最后一秒边界。
    last_tick: Instant,
    last_saved: Instant,
}

impl Db {
    /// 任务的速度历史：下载中取内存最新值，否则读库；从未记录过为 `None`。
    pub async fn task_speed_history(&self, task_id: &str) -> Result<Option<SpeedHistory>, DbError> {
        let entries = self.speed_history_live().entries.lock().await;
        if let Some(entry) = entries.get(task_id) {
            return Ok(Some(entry.history.clone()));
        }
        load(self, task_id).await
    }

    /// 清空任务的速度历史（「重新下载」从零开始）。
    pub async fn reset_task_speed_history(&self, task_id: &str) -> Result<(), DbError> {
        let mut entries = self.speed_history_live().entries.lock().await;
        entries.remove(task_id);
        sqlx::query("DELETE FROM task_speed_history WHERE task_id = $1")
            .bind(task_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// 把下载中任务的内存历史全部落库；停机前调用，免丢最近一个落库间隔。
    pub async fn flush_speed_history(&self) -> Result<(), DbError> {
        let entries = self.speed_history_live().entries.lock().await;
        for (task_id, entry) in entries.iter() {
            save(self, task_id, &entry.history).await?;
        }
        Ok(())
    }
}

/// 进度报告器每处理一帧调用一次：下载态逐秒累计（首帧只起表），
/// 离开下载态时落库并移出内存。错误只上报，不中断进度链路。
pub(crate) async fn observe(db: &Db, task_id: &str, downloading: bool, bps: i64, now: Instant) {
    let mut entries = db.speed_history_live().entries.lock().await;
    if !downloading {
        if let Some(entry) = entries.remove(task_id)
            && let Err(error) = save(db, task_id, &entry.history).await
        {
            crate::logger::report_error("speed-history", "persist on stop", &error);
        }
        return;
    }
    let Some(entry) = entries.get_mut(task_id) else {
        // 读失败（含损坏数据）时从空历史起步并上报；后续落库覆盖不可读的旧行。
        let history = match load(db, task_id).await {
            Ok(history) => history.unwrap_or_default(),
            Err(error) => {
                crate::logger::report_error("speed-history", "load", &error);
                SpeedHistory::default()
            }
        };
        entries.insert(
            task_id.to_owned(),
            LiveEntry {
                history,
                last_tick: now,
                last_saved: now,
            },
        );
        return;
    };
    let elapsed = now.saturating_duration_since(entry.last_tick).as_secs();
    if elapsed == 0 {
        return;
    }
    entry.last_tick += Duration::from_secs(elapsed);
    for _ in 0..elapsed.min(MAX_GAP_SECS) {
        entry.history.push_second(bps);
    }
    if now.saturating_duration_since(entry.last_saved) >= SAVE_INTERVAL {
        entry.last_saved = now;
        if let Err(error) = save(db, task_id, &entry.history).await {
            crate::logger::report_error("speed-history", "persist", &error);
        }
    }
}

/// 任务删除后丢弃内存历史（库行随任务级联删除）。
pub(crate) async fn forget(db: &Db, task_ids: &[String]) {
    let mut entries = db.speed_history_live().entries.lock().await;
    for task_id in task_ids {
        entries.remove(task_id);
    }
}

async fn load(db: &Db, task_id: &str) -> Result<Option<SpeedHistory>, DbError> {
    let Some(row) = sqlx::query(
        "SELECT bucket_secs, samples, pending_sum, pending_secs, active_secs, \
         transferred_bytes, peak_bps FROM task_speed_history WHERE task_id = $1",
    )
    .bind(task_id)
    .fetch_optional(db.pool())
    .await?
    else {
        return Ok(None);
    };
    let samples: String = row.try_get("samples")?;
    let samples: Vec<i64> = serde_json::from_str(&samples)
        .map_err(|error| DbError::Sqlx(sqlx::Error::Decode(Box::new(error))))?;
    let bucket_secs: i64 = row.try_get("bucket_secs")?;
    let pending_secs: i64 = row.try_get("pending_secs")?;
    Ok(Some(
        SpeedHistory {
            bucket_secs: u32::try_from(bucket_secs).unwrap_or(u32::MAX),
            samples,
            pending_sum: row.try_get("pending_sum")?,
            pending_secs: u32::try_from(pending_secs.max(0)).unwrap_or(u32::MAX),
            active_secs: row.try_get("active_secs")?,
            transferred_bytes: row.try_get("transferred_bytes")?,
            peak_bps: row.try_get("peak_bps")?,
        }
        .normalized(),
    ))
}

/// 覆盖写入；任务已删除时不产生孤儿行。
async fn save(db: &Db, task_id: &str, history: &SpeedHistory) -> Result<(), DbError> {
    let samples = serde_json::to_string(&history.samples)
        .map_err(|error| DbError::Sqlx(sqlx::Error::Encode(Box::new(error))))?;
    sqlx::query(
        "INSERT INTO task_speed_history (task_id, bucket_secs, samples, pending_sum, \
         pending_secs, active_secs, transferred_bytes, peak_bps) \
         SELECT $1, $2, $3, $4, $5, $6, $7, $8 WHERE EXISTS (SELECT 1 FROM tasks WHERE id = $1) \
         ON CONFLICT(task_id) DO UPDATE SET bucket_secs = excluded.bucket_secs, \
         samples = excluded.samples, pending_sum = excluded.pending_sum, \
         pending_secs = excluded.pending_secs, active_secs = excluded.active_secs, \
         transferred_bytes = excluded.transferred_bytes, peak_bps = excluded.peak_bps",
    )
    .bind(task_id)
    .bind(i64::from(history.bucket_secs))
    .bind(samples)
    .bind(history.pending_sum)
    .bind(i64::from(history.pending_secs))
    .bind(history.active_secs)
    .bind(history.transferred_bytes)
    .bind(history.peak_bps)
    .execute(db.pool())
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{MAX_GAP_SECS, SPEED_HISTORY_MAX_POINTS, SpeedHistory, observe};
    use crate::db::Db;

    #[test]
    fn full_curve_merges_pairs_and_keeps_exact_totals() {
        let mut history = SpeedHistory::default();
        for second in 0..SPEED_HISTORY_MAX_POINTS as i64 {
            history.push_second(second * 10);
        }
        assert_eq!(history.bucket_secs, 2);
        assert_eq!(history.samples.len(), SPEED_HISTORY_MAX_POINTS / 2);
        assert_eq!(history.samples[0], 5);
        assert_eq!(history.active_secs, SPEED_HISTORY_MAX_POINTS as i64);
        assert_eq!(history.peak_bps, 2390);
        assert_eq!(
            history.transferred_bytes,
            (0..SPEED_HISTORY_MAX_POINTS as i64)
                .map(|s| s * 10)
                .sum::<i64>()
        );

        // 未满 2 秒的当前桶作为展示末点，满桶后才进入已完成点。
        history.push_second(1000);
        assert_eq!(history.points().len(), SPEED_HISTORY_MAX_POINTS / 2 + 1);
        assert_eq!(history.points().last(), Some(&1000));
        history.push_second(0);
        assert_eq!(history.samples.last(), Some(&500));
        assert_eq!(history.pending_secs, 0);
    }

    #[tokio::test]
    async fn observe_counts_download_seconds_persists_on_stop_and_resets() {
        let db = Db::connect("sqlite::memory:").await.expect("db");
        db.insert_task(
            "t",
            "http://example.com/file.bin",
            "file.bin",
            "/tmp",
            1,
            0,
            "",
            "",
            "",
            0,
        )
        .await
        .expect("insert task");
        let t0 = Instant::now();
        let at = |secs: u64| t0 + Duration::from_secs(secs);

        observe(&db, "t", true, 100, t0).await;
        observe(&db, "t", true, 100, t0 + Duration::from_millis(2500)).await;
        observe(&db, "t", true, 300, at(3)).await;
        // 休眠后的长空窗只补 MAX_GAP_SECS 秒。
        observe(&db, "t", true, 300, at(60)).await;
        let live = db
            .task_speed_history("t")
            .await
            .expect("live")
            .expect("recorded");
        assert_eq!(live.active_secs, 3 + MAX_GAP_SECS as i64);
        assert_eq!(live.peak_bps, 300);
        assert_eq!(&live.points()[..3], &[100, 100, 300]);

        // 离开下载态落库；暂停时长不计入，恢复后接着累计。
        observe(&db, "t", false, 0, at(61)).await;
        assert_eq!(
            db.task_speed_history("t").await.expect("stored"),
            Some(live.clone())
        );
        observe(&db, "t", true, 50, at(500)).await;
        observe(&db, "t", true, 50, at(501)).await;
        let resumed = db
            .task_speed_history("t")
            .await
            .expect("resumed")
            .expect("recorded");
        assert_eq!(resumed.active_secs, live.active_secs + 1);

        // 「重新下载」清空后，迟到的停止帧不得让旧历史复活。
        db.reset_task_speed_history("t").await.expect("reset");
        observe(&db, "t", false, 0, at(502)).await;
        assert_eq!(db.task_speed_history("t").await.expect("after reset"), None);
    }
}
