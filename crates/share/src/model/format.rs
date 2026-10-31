//! FluxMesh界面的纯展示格式化：大小 / 速率 / 进度 / 有效期档位 / 领取次数 / 时间。
//!
//! 输出文本里的单位与时间格式不随语言变化；需要本地化的片段以 `(文案键, 数量)` 给出，
//! 由视图用翻译表组装。

use chrono::{DateTime, FixedOffset, Local, TimeZone};

/// 一个带数量占位 `{count}` 的本地化片段。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CountLabel {
    pub key: &'static str,
    pub count: u32,
}

/// 领取次数上限的预设档位（`0` = 不限）。
pub const CLAIM_PRESETS: [u32; 7] = [0, 1, 3, 5, 10, 20, 50];

/// 有效期档位的展示：整天数用「天」，其余用「小时」（`24` → 1 天，`36` → 36 小时）。
#[must_use]
pub fn tier_label(hours: u32) -> CountLabel {
    if hours >= 24 && hours.is_multiple_of(24) {
        CountLabel {
            key: "shareDurationDays",
            count: hours / 24,
        }
    } else {
        CountLabel {
            key: "shareDurationHours",
            count: hours,
        }
    }
}

/// 领取次数上限的展示：`0` = 不限。
#[must_use]
pub fn claims_label(max_claims: u32) -> CountLabel {
    if max_claims == 0 {
        CountLabel {
            key: "shareClaimsUnlimited",
            count: 0,
        }
    } else {
        CountLabel {
            key: "shareClaimsTimes",
            count: max_claims,
        }
    }
}

/// 已领取次数的展示：有上限时 `3 / 5`，不限时只显示已领取数。返回 `(文案键, 已领取, 上限)`。
#[must_use]
pub fn claimed_summary(claim_count: u32, max_claims: u32) -> (&'static str, u32, u32) {
    if max_claims == 0 {
        ("shareClaimedCount", claim_count, 0)
    } else {
        ("shareClaimedOf", claim_count, max_claims)
    }
}

const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];

/// 字节数的人类可读形式（1024 进制，KB 起保留一位小数）。
#[must_use]
pub fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024. && unit < UNITS.len() - 1 {
        value /= 1024.;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// 速率（字节 / 秒）的展示。
#[must_use]
pub fn format_rate(bytes_per_sec: u64) -> String {
    format!("{}/s", format_bytes(bytes_per_sec))
}

/// `0.0..=1.0` 的进度 → 整数百分比文本；非有限值按 0 处理。
#[must_use]
pub fn format_percent(progress: f64) -> String {
    let clamped = if progress.is_finite() {
        progress.clamp(0., 1.)
    } else {
        0.
    };
    format!("{}%", (clamped * 100.).round() as u32)
}

/// RFC 3339 时间按给定时区格式化为 `YYYY-MM-DD HH:MM`；解析失败返回原文。
#[must_use]
pub fn format_datetime_in<Tz>(value: &str, zone: &Tz) -> String
where
    Tz: TimeZone,
    Tz::Offset: std::fmt::Display,
{
    match DateTime::parse_from_rfc3339(value) {
        Ok(parsed) => parsed
            .with_timezone(zone)
            .format("%Y-%m-%d %H:%M")
            .to_string(),
        Err(_) => value.to_owned(),
    }
}

/// RFC 3339 时间按本机时区格式化；解析失败返回原文。
#[must_use]
pub fn format_datetime(value: &str) -> String {
    format_datetime_in(value, &Local)
}

/// RFC 3339 时间戳（Unix 秒）；解析失败为 `None`。
#[must_use]
pub fn parse_unix_secs(value: &str) -> Option<i64> {
    DateTime::<FixedOffset>::parse_from_rfc3339(value)
        .ok()
        .map(|parsed| parsed.timestamp())
}

#[cfg(test)]
mod tests {
    use chrono::FixedOffset;

    use super::{
        CLAIM_PRESETS, CountLabel, claimed_summary, claims_label, format_bytes, format_datetime_in,
        format_percent, format_rate, parse_unix_secs, tier_label,
    };

    #[test]
    fn tiers_use_days_only_for_whole_days() {
        assert_eq!(
            tier_label(1),
            CountLabel {
                key: "shareDurationHours",
                count: 1
            }
        );
        assert_eq!(
            tier_label(36),
            CountLabel {
                key: "shareDurationHours",
                count: 36
            }
        );
        assert_eq!(
            tier_label(24),
            CountLabel {
                key: "shareDurationDays",
                count: 1
            }
        );
        assert_eq!(
            tier_label(336),
            CountLabel {
                key: "shareDurationDays",
                count: 14
            }
        );
        assert_eq!(
            tier_label(0),
            CountLabel {
                key: "shareDurationHours",
                count: 0
            }
        );
    }

    #[test]
    fn claims_zero_means_unlimited() {
        assert_eq!(claims_label(0).key, "shareClaimsUnlimited");
        assert_eq!(
            claims_label(5),
            CountLabel {
                key: "shareClaimsTimes",
                count: 5
            }
        );
        assert_eq!(CLAIM_PRESETS[0], 0);
        assert!(CLAIM_PRESETS.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(claimed_summary(3, 0), ("shareClaimedCount", 3, 0));
        assert_eq!(claimed_summary(3, 5), ("shareClaimedOf", 3, 5));
    }

    #[test]
    fn sizes_and_rates_are_humanised() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(3 * 1024 * 1024 * 1024), "3.0 GB");
        assert_eq!(format_bytes(u64::MAX), "16384.0 PB");
        assert_eq!(format_bytes(1 << 50), "1.0 PB");
        assert_eq!(format_rate(2048), "2.0 KB/s");
    }

    #[test]
    fn percent_is_clamped_and_total() {
        assert_eq!(format_percent(0.456), "46%");
        assert_eq!(format_percent(2.), "100%");
        assert_eq!(format_percent(-1.), "0%");
        assert_eq!(format_percent(f64::NAN), "0%");
    }

    #[test]
    fn datetime_formats_in_the_given_zone_and_falls_back_to_raw() {
        let Some(zone) = FixedOffset::east_opt(8 * 3600) else {
            return;
        };
        assert_eq!(
            format_datetime_in("2026-10-27T10:30:00Z", &zone),
            "2026-10-27 18:30"
        );
        assert_eq!(format_datetime_in("not a time", &zone), "not a time");
        assert_eq!(parse_unix_secs("1970-01-01T00:01:00Z"), Some(60));
        assert_eq!(parse_unix_secs("nope"), None);
    }
}
