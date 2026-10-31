//! RFC 3339 时间戳解析（云端 `DateTime<Utc>` 序列化形态）；agent 不依赖 chrono。

/// 解析 `2026-10-27T12:34:56[.fraction](Z|±hh:mm)` 为 Unix 秒；格式不符返回 `None`。
pub(super) fn parse_rfc3339_secs(text: &str) -> Option<u64> {
    let text = text.trim();
    let (date, rest) = text.split_once(['T', 't', ' '])?;
    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: i64 = date_parts.next()?.parse().ok()?;
    let day: i64 = date_parts.next()?.parse().ok()?;
    if date_parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let split = rest.find(['Z', 'z', '+', '-'])?;
    let (clock, zone) = rest.split_at(split);
    let clock = clock.split('.').next()?;
    let mut clock_parts = clock.split(':');
    let hour: i64 = clock_parts.next()?.parse().ok()?;
    let minute: i64 = clock_parts.next()?.parse().ok()?;
    let second: i64 = clock_parts.next()?.parse().ok()?;
    if clock_parts.next().is_some() || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let offset_secs = if zone.eq_ignore_ascii_case("z") {
        0
    } else {
        let sign = if zone.starts_with('-') { -1 } else { 1 };
        let (hh, mm) = zone.get(1..)?.split_once(':')?;
        sign * (hh.parse::<i64>().ok()? * 3600 + mm.parse::<i64>().ok()? * 60)
    };
    let days = days_from_civil(year, month, day);
    let secs = days * 86_400 + hour * 3600 + minute * 60 + second - offset_secs;
    u64::try_from(secs).ok()
}

/// 公历日期 → 自 1970-01-01 起的天数（Howard Hinnant 算法）。
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::parse_rfc3339_secs;

    #[test]
    fn parses_utc_and_offsets() {
        assert_eq!(parse_rfc3339_secs("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_rfc3339_secs("2026-10-27T12:00:00.123456Z"),
            Some(1_793_102_400)
        );
        assert_eq!(
            parse_rfc3339_secs("2026-10-27T20:00:00+08:00"),
            Some(1_793_102_400)
        );
        assert_eq!(parse_rfc3339_secs("garbage"), None);
        assert_eq!(parse_rfc3339_secs("2026-13-01T00:00:00Z"), None);
    }
}
