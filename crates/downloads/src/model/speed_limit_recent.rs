//! 状态栏限速「最近自定义值」：最新在前、去重、封顶，单位 KB/s。
//!
//! 偏好值为整数数组（设备本地，不进云同步）；规则与 Web
//! `web/src/lib/speedLimitRecent.ts` 逐条对齐，两端共用
//! `crates/downloads/tests/fixtures/speed_limit_recent_cases.json`。

use serde_json::Value;

/// 最多保留的最近自定义值条数。
pub(crate) const SPEED_LIMIT_RECENT_LIMIT: usize = 3;

/// 内置预设（1 / 5 / 10 / 50 MB/s）折算成 KB/s；预设本身不记入最近列表。
const PRESET_KBPS: [i64; 4] = [1024, 5120, 10240, 51200];

/// 值是否可记入最近列表：正数且不等于内置预设。
fn is_recordable(kbps: i64) -> bool {
    kbps > 0 && !PRESET_KBPS.contains(&kbps)
}

/// 解析偏好值：非数组为空；丢弃非整数、非正数、预设值与重复项（保留首次出现），封顶。
pub(crate) fn read_speed_limit_recent(value: Option<&Value>) -> Vec<i64> {
    let Some(items) = value.and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut recent: Vec<i64> = Vec::with_capacity(SPEED_LIMIT_RECENT_LIMIT);
    for kbps in items.iter().filter_map(Value::as_i64) {
        if recent.len() == SPEED_LIMIT_RECENT_LIMIT {
            break;
        }
        if is_recordable(kbps) && !recent.contains(&kbps) {
            recent.push(kbps);
        }
    }
    recent
}

/// 记录一次自定义值：移到最前（已存在则前移），超出上限丢弃最旧的。
/// 值不可记录或已在最前时返回 `None`（无需写偏好）。
pub(crate) fn push_recent(recent: &[i64], kbps: i64) -> Option<Vec<i64>> {
    if !is_recordable(kbps) || recent.first() == Some(&kbps) {
        return None;
    }
    let mut next = Vec::with_capacity(SPEED_LIMIT_RECENT_LIMIT);
    next.push(kbps);
    next.extend(
        recent
            .iter()
            .copied()
            .filter(|known| *known != kbps)
            .take(SPEED_LIMIT_RECENT_LIMIT - 1),
    );
    Some(next)
}

#[cfg(test)]
mod tests {
    use super::{SPEED_LIMIT_RECENT_LIMIT, push_recent, read_speed_limit_recent};
    use serde::Deserialize;
    use serde_json::Value;

    #[derive(Deserialize)]
    struct ReadCase {
        name: String,
        value: Value,
        expected: Vec<i64>,
    }

    #[derive(Deserialize)]
    struct PushCase {
        name: String,
        recent: Vec<i64>,
        kbps: i64,
        expected: Option<Vec<i64>>,
    }

    #[derive(Deserialize)]
    struct Fixture {
        limit: usize,
        read: Vec<ReadCase>,
        push: Vec<PushCase>,
    }

    #[test]
    fn matches_shared_fixture() {
        let fixture: Fixture = serde_json::from_str(include_str!(
            "../../tests/fixtures/speed_limit_recent_cases.json"
        ))
        .expect("speed limit recent fixture");
        assert_eq!(fixture.limit, SPEED_LIMIT_RECENT_LIMIT);
        for case in fixture.read {
            let value = (!case.value.is_null()).then_some(&case.value);
            assert_eq!(
                read_speed_limit_recent(value),
                case.expected,
                "{}",
                case.name
            );
        }
        for case in fixture.push {
            assert_eq!(
                push_recent(&case.recent, case.kbps),
                case.expected,
                "{}",
                case.name
            );
        }
    }
}
