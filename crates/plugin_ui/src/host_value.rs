//! `serde_json::Value` 与 gpui-shell `HostValue` 互转：脚本与 agent RPC 之间只有纯数据。

use gpui_shell::HostValue;
use serde_json::{Map, Number, Value};

/// agent 应答 → 脚本值。JSON 数字一律成为 `f64`（脚本侧只有 number）。
pub(crate) fn json_to_host(value: Value) -> HostValue {
    match value {
        Value::Null => HostValue::Null,
        Value::Bool(flag) => HostValue::Bool(flag),
        Value::Number(number) => number.as_f64().map_or(HostValue::Null, HostValue::Number),
        Value::String(text) => HostValue::Str(text),
        Value::Array(items) => HostValue::Array(items.into_iter().map(json_to_host).collect()),
        Value::Object(map) => HostValue::Object(
            map.into_iter()
                .map(|(key, value)| (key, json_to_host(value)))
                .collect(),
        ),
    }
}

/// 脚本参数 → RPC 参数。非有限数（NaN / ±∞）JSON 无法表示，按 `null` 传。
pub(crate) fn host_to_json(value: &HostValue) -> Value {
    match value {
        HostValue::Null => Value::Null,
        HostValue::Bool(flag) => Value::Bool(*flag),
        HostValue::Number(number) => number_to_json(*number),
        HostValue::Str(text) => Value::String(text.clone()),
        HostValue::Array(items) => Value::Array(items.iter().map(host_to_json).collect()),
        HostValue::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(key, value)| (key.clone(), host_to_json(value)))
                .collect::<Map<String, Value>>(),
        ),
    }
}

/// 整数值的 `f64`（|n| ≤ 2^53）写成 JSON 整数：daemon 的 `u64` / `i64` 参数（任务 id、
/// 限速字节数等）拒收 `1.0` 这种浮点写法。
fn number_to_json(number: f64) -> Value {
    const MAX_EXACT: f64 = 9_007_199_254_740_992.0;
    if number.fract() == 0.0 && number.abs() <= MAX_EXACT {
        // 已检查无小数且在 2^53 内，转换精确。
        let integer = number as i64;
        return Value::Number(Number::from(integer));
    }
    Number::from_f64(number).map_or(Value::Null, Value::Number)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn integral_numbers_cross_as_json_integers() {
        let value = HostValue::Object(vec![
            ("limit".into(), HostValue::Number(1_048_576.0)),
            ("ratio".into(), HostValue::Number(0.5)),
            ("negative".into(), HostValue::Number(-3.0)),
        ]);
        assert_eq!(
            host_to_json(&value),
            json!({ "limit": 1_048_576, "ratio": 0.5, "negative": -3 })
        );
    }

    #[test]
    fn non_finite_numbers_become_null() {
        assert_eq!(host_to_json(&HostValue::Number(f64::NAN)), Value::Null);
        assert_eq!(host_to_json(&HostValue::Number(f64::INFINITY)), Value::Null);
    }

    #[test]
    fn rpc_reply_keeps_nesting() {
        let host = json_to_host(json!({ "tasks": [{ "id": "a" }, { "size": 7 }] }));
        assert_eq!(
            host,
            HostValue::Object(vec![(
                "tasks".into(),
                HostValue::Array(vec![
                    HostValue::Object(vec![("id".into(), HostValue::Str("a".into()))]),
                    HostValue::Object(vec![("size".into(), HostValue::Number(7.0))]),
                ])
            )])
        );
    }
}
