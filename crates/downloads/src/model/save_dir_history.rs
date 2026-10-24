//! 新建下载「保存目录」历史：最近使用的本机目录，最新在前、去重、封顶。
//!
//! 偏好值为字符串数组（设备本地，不进云同步）；规则与 Web
//! `web/src/lib/saveDirHistory.ts` 逐条对齐，两端共用
//! `crates/downloads/tests/fixtures/save_dir_history_cases.json`。

use serde_json::Value;

/// 最多保留的历史条数。
pub(crate) const SAVE_DIR_HISTORY_LIMIT: usize = 10;

/// 解析偏好值：非数组为空；逐项 trim，丢弃空串 / 非字符串，保留首次出现，封顶。
pub(crate) fn read_save_dir_history(value: Option<&Value>) -> Vec<String> {
    let Some(items) = value.and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut history: Vec<String> = Vec::with_capacity(items.len().min(SAVE_DIR_HISTORY_LIMIT));
    for dir in items.iter().filter_map(Value::as_str).map(str::trim) {
        if history.len() == SAVE_DIR_HISTORY_LIMIT {
            break;
        }
        if !dir.is_empty() && !history.iter().any(|known| known == dir) {
            history.push(dir.to_owned());
        }
    }
    history
}

/// 记录一次使用：目录移到最前（已存在则前移），超出上限丢弃最旧的。
/// 空目录或已在最前时返回 `None`（无需写偏好）。
pub(crate) fn record_save_dir(history: &[String], dir: &str) -> Option<Vec<String>> {
    let dir = dir.trim();
    if dir.is_empty() || history.first().is_some_and(|first| first == dir) {
        return None;
    }
    let mut next = Vec::with_capacity(SAVE_DIR_HISTORY_LIMIT);
    next.push(dir.to_owned());
    next.extend(
        history
            .iter()
            .filter(|known| *known != dir)
            .take(SAVE_DIR_HISTORY_LIMIT - 1)
            .cloned(),
    );
    Some(next)
}

/// 下拉候选：历史中除当前输入以外的目录（保持最新在前）。
pub(crate) fn save_dir_choices<'a>(history: &'a [String], current: &str) -> Vec<&'a str> {
    let current = current.trim();
    history
        .iter()
        .map(String::as_str)
        .filter(|dir| *dir != current)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{SAVE_DIR_HISTORY_LIMIT, read_save_dir_history, record_save_dir, save_dir_choices};
    use serde::Deserialize;
    use serde_json::Value;

    #[derive(Deserialize)]
    struct Fixture {
        limit: usize,
        read: Vec<ReadCase>,
        record: Vec<RecordCase>,
        choices: Vec<ChoicesCase>,
    }

    #[derive(Deserialize)]
    struct ReadCase {
        name: String,
        value: Value,
        expected: Vec<String>,
    }

    #[derive(Deserialize)]
    struct RecordCase {
        name: String,
        history: Vec<String>,
        dir: String,
        expected: Option<Vec<String>>,
    }

    #[derive(Deserialize)]
    struct ChoicesCase {
        name: String,
        history: Vec<String>,
        current: String,
        expected: Vec<String>,
    }

    fn fixture() -> Fixture {
        serde_json::from_str(include_str!(
            "../../tests/fixtures/save_dir_history_cases.json"
        ))
        .expect("save dir history fixture")
    }

    #[test]
    fn shared_fixture_cases() {
        let fixture = fixture();
        assert_eq!(fixture.limit, SAVE_DIR_HISTORY_LIMIT);
        for case in fixture.read {
            let value = (!case.value.is_null()).then_some(&case.value);
            assert_eq!(read_save_dir_history(value), case.expected, "{}", case.name);
        }
        for case in fixture.record {
            assert_eq!(
                record_save_dir(&case.history, &case.dir),
                case.expected,
                "{}",
                case.name
            );
        }
        for case in fixture.choices {
            assert_eq!(
                save_dir_choices(&case.history, &case.current),
                case.expected,
                "{}",
                case.name
            );
        }
    }
}
