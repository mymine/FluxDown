//! iroh 日志查看器的纯规则：级别列表 / 文案键、日志行展示、正则参数与「正则非法」错误判别。

use chrono::{Local, TimeZone};
use fluxdown_protocol::{
    ApplicationErrorCode, RpcErrorData, ShareLogLevel, ShareLogLineDto, ShareLogsParams,
};

use super::diagnostics::format_clock_in;

/// 查看器一次最多取回的行数。
pub const LOG_LIMIT: u32 = 300;

/// 级别下拉的全部档位（由严重到详细）。
pub const LOG_LEVELS: [ShareLogLevel; 5] = [
    ShareLogLevel::Error,
    ShareLogLevel::Warn,
    ShareLogLevel::Info,
    ShareLogLevel::Debug,
    ShareLogLevel::Trace,
];

/// 级别文案键。
#[must_use]
pub const fn log_level_key(level: ShareLogLevel) -> &'static str {
    match level {
        ShareLogLevel::Error => "shareLogLevelError",
        ShareLogLevel::Warn => "shareLogLevelWarn",
        ShareLogLevel::Info => "shareLogLevelInfo",
        ShareLogLevel::Debug => "shareLogLevelDebug",
        ShareLogLevel::Trace => "shareLogLevelTrace",
    }
}

/// 级别在日志行里的固定宽度标记（等宽对齐用，不翻译：与 tracing 输出一致）。
#[must_use]
pub const fn log_level_tag(level: ShareLogLevel) -> &'static str {
    match level {
        ShareLogLevel::Error => "ERROR",
        ShareLogLevel::Warn => "WARN",
        ShareLogLevel::Info => "INFO",
        ShareLogLevel::Debug => "DEBUG",
        ShareLogLevel::Trace => "TRACE",
    }
}

/// `agent.share.logs` 的请求参数：空白正则不下发（trim 后为空 = 不过滤）。
#[must_use]
pub fn logs_params(level: ShareLogLevel, regex: &str) -> ShareLogsParams {
    let regex = regex.trim();
    ShareLogsParams {
        level: Some(level),
        regex: (!regex.is_empty()).then(|| regex.to_owned()),
        limit: Some(LOG_LIMIT),
    }
}

/// agent 对非法正则返回 `InvalidArgument` 且 `field == "regex"`。
#[must_use]
pub fn is_invalid_regex(error: &RpcErrorData) -> bool {
    error.code == ApplicationErrorCode::InvalidArgument && error.field.as_deref() == Some("regex")
}

/// 一行日志的展示形式。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogRow {
    /// `HH:MM:SS.mmm`（本机时区）。
    pub time: String,
    pub level: ShareLogLevel,
    pub target: String,
    /// 折成单行：换行与制表符替换为空格，连续空白保持原样。
    pub message: String,
}

/// 按给定时区生成展示行。
#[must_use]
pub fn log_row_in<Tz>(line: &ShareLogLineDto, zone: &Tz) -> LogRow
where
    Tz: TimeZone,
    Tz::Offset: std::fmt::Display,
{
    LogRow {
        time: format_clock_in(&line.at, zone),
        level: line.level,
        target: line.target.clone(),
        message: single_line(&line.message),
    }
}

/// 按本机时区生成展示行。
#[must_use]
pub fn log_row(line: &ShareLogLineDto) -> LogRow {
    log_row_in(line, &Local)
}

fn single_line(text: &str) -> String {
    text.chars()
        .map(|c| {
            if matches!(c, '\n' | '\r' | '\t') {
                ' '
            } else {
                c
            }
        })
        .collect()
}

impl LogRow {
    /// 整行纯文本（悬停提示 / 复制）：`时间 级别 target message`。
    #[must_use]
    pub fn text(&self) -> String {
        format!(
            "{} {} {} {}",
            self.time,
            log_level_tag(self.level),
            self.target,
            self.message
        )
    }
}

#[cfg(test)]
mod tests {
    use chrono::FixedOffset;
    use fluxdown_protocol::{ApplicationErrorCode, RpcErrorData, ShareLogLevel, ShareLogLineDto};

    use super::{
        LOG_LEVELS, LOG_LIMIT, is_invalid_regex, log_level_key, log_level_tag, log_row_in,
        logs_params,
    };

    #[test]
    fn every_level_has_a_distinct_key_and_tag() {
        let keys: std::collections::BTreeSet<&str> = LOG_LEVELS
            .iter()
            .map(|level| log_level_key(*level))
            .collect();
        let tags: std::collections::BTreeSet<&str> = LOG_LEVELS
            .iter()
            .map(|level| log_level_tag(*level))
            .collect();
        assert_eq!(keys.len(), LOG_LEVELS.len());
        assert_eq!(tags.len(), LOG_LEVELS.len());
        assert_eq!(log_level_key(ShareLogLevel::Warn), "shareLogLevelWarn");
    }

    #[test]
    fn params_drop_blank_regex_and_trim_the_rest() {
        let blank = logs_params(ShareLogLevel::Info, "   ");
        assert_eq!(blank.regex, None);
        assert_eq!(blank.level, Some(ShareLogLevel::Info));
        assert_eq!(blank.limit, Some(LOG_LIMIT));
        let filtered = logs_params(ShareLogLevel::Debug, "  relay|stun ");
        assert_eq!(filtered.regex.as_deref(), Some("relay|stun"));
    }

    #[test]
    fn only_invalid_argument_on_the_regex_field_is_a_regex_error() {
        let mut error = RpcErrorData::new(ApplicationErrorCode::InvalidArgument, false);
        assert!(!is_invalid_regex(&error));
        error.field = Some("regex".into());
        assert!(is_invalid_regex(&error));
        error.field = Some("level".into());
        assert!(!is_invalid_regex(&error));
        let mut other = RpcErrorData::new(ApplicationErrorCode::Unavailable, true);
        other.field = Some("regex".into());
        assert!(!is_invalid_regex(&other));
    }

    #[test]
    fn rows_are_single_line_and_carry_a_local_clock() {
        let Some(zone) = FixedOffset::east_opt(2 * 3600) else {
            return;
        };
        let line = ShareLogLineDto {
            at: "2026-10-27T10:30:00.007Z".into(),
            level: ShareLogLevel::Warn,
            target: "iroh::magicsock".into(),
            message: "path lost\n  retrying\t(2/3) 设备 🚀".into(),
        };
        let row = log_row_in(&line, &zone);
        assert_eq!(row.time, "12:30:00.007");
        assert_eq!(row.message, "path lost   retrying (2/3) 设备 🚀");
        assert_eq!(
            row.text(),
            "12:30:00.007 WARN iroh::magicsock path lost   retrying (2/3) 设备 🚀"
        );
    }
}
