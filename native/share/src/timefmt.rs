//! 时间辅助：Unix 毫秒与 RFC 3339 文本（不引入日期库）。

use std::time::{SystemTime, UNIX_EPOCH};

/// 当前 Unix 毫秒（系统时钟早于 1970 时为 0）。
pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// 当前 Unix 秒。
pub fn now_unix_secs() -> u64 {
    now_unix_ms() / 1000
}

/// 把 Unix 毫秒格式化为 UTC 的 RFC 3339 文本，毫秒精度：`2026-10-27T01:02:03.456Z`。
pub fn rfc3339_ms(unix_ms: u64) -> String {
    let ms = unix_ms % 1000;
    let secs = unix_ms / 1000;
    let days = i64::try_from(secs / 86_400).unwrap_or(i64::MAX / 2);
    let rem = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{ms:03}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// 当前时间的 RFC 3339 文本。
pub fn now_rfc3339() -> String {
    rfc3339_ms(now_unix_ms())
}

/// 自 1970-01-01 起的天数 → 公历 (年, 月, 日)（Howard Hinnant 的 civil_from_days）。
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (if month <= 2 { year + 1 } else { year }, month, day)
}
