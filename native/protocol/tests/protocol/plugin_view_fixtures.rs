#![allow(clippy::unwrap_used, clippy::expect_used)]

use fluxdown_protocol::{ViewNode, ViewOp, apply_view_ops, diff_view};
use serde::Deserialize;
use serde_json::Value;

const FIXTURE: &str = include_str!("../fixtures/plugin_view_apply.json");

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Case {
    name: String,
    old_tree: ViewNode,
    new_tree: ViewNode,
    ops: Value,
}

fn cases() -> Vec<Case> {
    serde_json::from_str(FIXTURE).expect("plugin_view_apply.json parses")
}

#[test]
fn plugin_view_apply_fixtures_roundtrip() {
    let cases = cases();
    assert!(cases.len() >= 15, "need at least 15 fixture cases");
    for case in cases {
        let expected: Vec<ViewOp> = serde_json::from_value(case.ops.clone())
            .unwrap_or_else(|e| panic!("{}: ops do not parse: {e}", case.name));
        let diff = diff_view(&case.old_tree, &case.new_tree);
        assert_eq!(diff, expected, "{}: diff_view output", case.name);
        // 线格式（Web 实现读取的形状）也必须与夹具逐字段一致
        assert_eq!(
            serde_json::to_value(&diff).unwrap(),
            case.ops,
            "{}: wire shape",
            case.name
        );
        let mut applied = case.old_tree.clone();
        apply_view_ops(&mut applied, &expected)
            .unwrap_or_else(|e| panic!("{}: apply failed: {e}", case.name));
        assert_eq!(applied, case.new_tree, "{}: apply result", case.name);
    }
}

#[test]
fn plugin_view_apply_fixture_names_are_unique_and_include_large_table() {
    let cases = cases();
    let mut names: Vec<&str> = cases.iter().map(|c| c.name.as_str()).collect();
    names.sort_unstable();
    let before = names.len();
    names.dedup();
    assert_eq!(before, names.len(), "duplicate fixture names");
    assert!(
        names.contains(&"table_1000_rows_one_changed"),
        "run the one-off generator to append table_1000_rows_one_changed"
    );
}
