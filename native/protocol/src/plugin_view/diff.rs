//! `diff_view` 与 `apply_view_ops`：严格互逆。

use std::collections::{HashMap, HashSet};

use serde_json::{Map, Value};

use super::schema::view_schema;
use super::{VIEW_ERROR_PROP, ViewError, ViewNode, ViewOp};

fn keyed_prop(kind: &str) -> Option<&'static str> {
    view_schema().ok()?.keyed_prop(kind)
}

/// 计算 `old → new` 的增量 op。
///
/// - kind 或 id 不同 → `Replace`（不再向下）。
/// - 否则依次输出：根节点的 `Error`、`Props`、`Rows`、`Children`，再按新子序列递归进入保留下来的子节点。
/// - 键控数组（`table.rows` / `list.items`）在两侧都是“元素带唯一字符串 key 的数组”时走 `Rows`，
///   否则退回整体 `Props.set`。
pub fn diff_view(old: &ViewNode, new: &ViewNode) -> Vec<ViewOp> {
    let mut ops = Vec::new();
    diff_node(old, new, true, &mut ops);
    ops
}

fn diff_node(old: &ViewNode, new: &ViewNode, is_root: bool, ops: &mut Vec<ViewOp>) {
    if old.k != new.k || old.id != new.id {
        ops.push(ViewOp::Replace {
            id: old.id.clone(),
            node: new.clone(),
        });
        return;
    }
    diff_props(old, new, is_root, ops);
    diff_children(old, new, ops);
}

fn diff_props(old: &ViewNode, new: &ViewNode, is_root: bool, ops: &mut Vec<ViewOp>) {
    let keyed = keyed_prop(&new.k);
    let rows_op = keyed.and_then(|name| diff_rows(&new.id, name, &old.p, &new.p));
    let rows_handled = rows_op.is_some();
    let mut error_op = None;
    let mut set = Map::new();
    for (name, value) in &new.p {
        let message = if is_root && name == VIEW_ERROR_PROP {
            value.as_str()
        } else {
            None
        };
        if let Some(message) = message {
            error_op = (old.p.get(name) != Some(value)).then(|| ViewOp::Error {
                message: message.to_owned(),
            });
            continue;
        }
        if rows_handled && keyed == Some(name.as_str()) {
            continue;
        }
        if old.p.get(name) != Some(value) {
            set.insert(name.clone(), value.clone());
        }
    }
    let mut unset: Vec<String> = old
        .p
        .keys()
        .filter(|name| !new.p.contains_key(*name))
        .cloned()
        .collect();
    unset.sort_unstable();
    if let Some(op) = error_op {
        ops.push(op);
    }
    if !set.is_empty() || !unset.is_empty() {
        ops.push(ViewOp::Props {
            id: new.id.clone(),
            set,
            unset,
        });
    }
    if let Some(Some(op)) = rows_op {
        ops.push(op);
    }
}

/// 外层 `None` = 不能按 key 增量（回退 Props）；`Some(None)` = 无变化。
fn diff_rows(
    id: &str,
    name: &str,
    old_p: &Map<String, Value>,
    new_p: &Map<String, Value>,
) -> Option<Option<ViewOp>> {
    let old_rows = row_keys(old_p.get(name)?)?;
    let new_rows = row_keys(new_p.get(name)?)?;
    let old_by_key: HashMap<&str, &Value> = old_rows.iter().copied().collect();
    let new_keys: HashSet<&str> = new_rows.iter().map(|(key, _)| *key).collect();

    let remove: Vec<String> = old_rows
        .iter()
        .filter(|(key, _)| !new_keys.contains(key))
        .map(|(key, _)| (*key).to_owned())
        .collect();
    let mut upsert = Vec::new();
    for (key, row) in &new_rows {
        if old_by_key.get(key) != Some(row) {
            upsert.push(Value::clone(row));
        }
    }
    // 不带 order 时，apply 的结果顺序 = 保留的旧行（旧顺序）+ 新 key（按 upsert 顺序）。
    let predicted = old_rows
        .iter()
        .map(|(key, _)| *key)
        .filter(|key| new_keys.contains(key))
        .chain(
            new_rows
                .iter()
                .map(|(key, _)| *key)
                .filter(|key| !old_by_key.contains_key(key)),
        );
    let order = (!predicted.eq(new_rows.iter().map(|(key, _)| *key))).then(|| {
        new_rows
            .iter()
            .map(|(key, _)| (*key).to_owned())
            .collect::<Vec<_>>()
    });
    if remove.is_empty() && upsert.is_empty() && order.is_none() {
        return Some(None);
    }
    Some(Some(ViewOp::Rows {
        id: id.to_owned(),
        upsert,
        remove,
        order,
    }))
}

fn row_keys(value: &Value) -> Option<Vec<(&str, &Value)>> {
    let rows = value.as_array()?;
    let mut out = Vec::with_capacity(rows.len());
    let mut seen = HashSet::with_capacity(rows.len());
    for row in rows {
        let key = row.get("key")?.as_str()?;
        if !seen.insert(key) {
            return None;
        }
        out.push((key, row));
    }
    Some(out)
}

fn diff_children(old: &ViewNode, new: &ViewNode, ops: &mut Vec<ViewOp>) {
    let same_ids =
        old.c.len() == new.c.len() && old.c.iter().zip(&new.c).all(|(a, b)| a.id == b.id);
    if same_ids {
        for (a, b) in old.c.iter().zip(&new.c) {
            diff_node(a, b, false, ops);
        }
        return;
    }
    let old_by_id: HashMap<&str, &ViewNode> = old.c.iter().map(|n| (n.id.as_str(), n)).collect();
    let insert = new
        .c
        .iter()
        .filter(|n| !old_by_id.contains_key(n.id.as_str()))
        .cloned()
        .collect();
    ops.push(ViewOp::Children {
        id: new.id.clone(),
        order: new.c.iter().map(|n| n.id.clone()).collect(),
        insert,
    });
    for n in &new.c {
        if let Some(prev) = old_by_id.get(n.id.as_str()) {
            diff_node(prev, n, false, ops);
        }
    }
}

/// 依次应用 `ops`。返回 `Err` 后 `tree` 状态未定义，调用方必须重新取快照。
pub fn apply_view_ops(tree: &mut ViewNode, ops: &[ViewOp]) -> Result<(), ViewError> {
    for op in ops {
        apply_op(tree, op)?;
    }
    Ok(())
}

fn apply_op(tree: &mut ViewNode, op: &ViewOp) -> Result<(), ViewError> {
    match op {
        ViewOp::Error { message } => {
            tree.p
                .insert(VIEW_ERROR_PROP.to_owned(), Value::String(message.clone()));
            Ok(())
        }
        ViewOp::Replace { id, node } => {
            *target(tree, id)? = node.clone();
            Ok(())
        }
        ViewOp::Props { id, set, unset } => {
            let node = target(tree, id)?;
            for name in unset {
                node.p.remove(name);
            }
            for (name, value) in set {
                node.p.insert(name.clone(), value.clone());
            }
            Ok(())
        }
        ViewOp::Children { id, order, insert } => {
            apply_children(target(tree, id)?, id, order, insert)
        }
        ViewOp::Rows {
            id,
            upsert,
            remove,
            order,
        } => apply_rows(target(tree, id)?, id, upsert, remove, order.as_deref()),
    }
}

fn target<'a>(tree: &'a mut ViewNode, id: &str) -> Result<&'a mut ViewNode, ViewError> {
    find_mut(tree, id).ok_or_else(|| ViewError::apply(format!("no node with id {id:?}")))
}

/// 沿 id 路径向下找节点；id 段不含 `/`，所以每层只需匹配下一段。
fn find_mut<'a>(root: &'a mut ViewNode, id: &str) -> Option<&'a mut ViewNode> {
    let mut cur = root;
    while cur.id != id {
        let rest = id.strip_prefix(cur.id.as_str())?.strip_prefix('/')?;
        let segment = rest.split('/').next().unwrap_or(rest);
        let child_id = id.get(..cur.id.len() + 1 + segment.len())?;
        let index = cur.c.iter().position(|n| n.id == child_id)?;
        cur = cur.c.get_mut(index)?;
    }
    Some(cur)
}

fn apply_children(
    node: &mut ViewNode,
    id: &str,
    order: &[String],
    insert: &[ViewNode],
) -> Result<(), ViewError> {
    let mut pool: HashMap<String, ViewNode> = std::mem::take(&mut node.c)
        .into_iter()
        .map(|n| (n.id.clone(), n))
        .collect();
    for n in insert {
        if pool.insert(n.id.clone(), n.clone()).is_some() {
            return Err(ViewError::apply(format!(
                "children of {id:?}: inserted id {:?} already exists",
                n.id
            )));
        }
    }
    let mut next = Vec::with_capacity(order.len());
    for child_id in order {
        let Some(child) = pool.remove(child_id) else {
            return Err(ViewError::apply(format!(
                "children of {id:?}: order names unknown or repeated child {child_id:?}"
            )));
        };
        next.push(child);
    }
    if let Some(n) = insert.iter().find(|n| pool.contains_key(&n.id)) {
        return Err(ViewError::apply(format!(
            "children of {id:?}: inserted node {:?} is missing from order",
            n.id
        )));
    }
    node.c = next;
    Ok(())
}

fn row_key(row: &Value) -> Option<&str> {
    row.get("key").and_then(Value::as_str)
}

fn apply_rows(
    node: &mut ViewNode,
    id: &str,
    upsert: &[Value],
    remove: &[String],
    order: Option<&[String]>,
) -> Result<(), ViewError> {
    let Some(name) = keyed_prop(&node.k) else {
        return Err(ViewError::apply(format!(
            "node {id:?} of kind {:?} has no keyed array",
            node.k
        )));
    };
    let Some(Value::Array(rows)) = node.p.get_mut(name) else {
        return Err(ViewError::apply(format!(
            "node {id:?}: prop {name:?} is not an array"
        )));
    };
    if !remove.is_empty() {
        let doomed: HashSet<&str> = remove.iter().map(String::as_str).collect();
        let before = rows.len();
        rows.retain(|row| !row_key(row).is_some_and(|key| doomed.contains(key)));
        if before - rows.len() != doomed.len() {
            return Err(ViewError::apply(format!(
                "node {id:?}: remove names a row key that does not exist"
            )));
        }
    }
    let mut index: HashMap<String, usize> = rows
        .iter()
        .enumerate()
        .filter_map(|(i, row)| row_key(row).map(|key| (key.to_owned(), i)))
        .collect();
    for row in upsert {
        let Some(key) = row_key(row) else {
            return Err(ViewError::apply(format!(
                "node {id:?}: upsert row has no string key"
            )));
        };
        match index.get(key).copied() {
            Some(position) => {
                if let Some(slot) = rows.get_mut(position) {
                    *slot = row.clone();
                }
            }
            None => {
                index.insert(key.to_owned(), rows.len());
                rows.push(row.clone());
            }
        }
    }
    if let Some(order) = order {
        if order.len() != rows.len() {
            return Err(ViewError::apply(format!(
                "node {id:?}: order has {} keys but there are {} rows",
                order.len(),
                rows.len()
            )));
        }
        let mut by_key: HashMap<String, Value> = rows
            .drain(..)
            .filter_map(|row| {
                let key = row_key(&row)?.to_owned();
                Some((key, row))
            })
            .collect();
        for key in order {
            let Some(row) = by_key.remove(key) else {
                return Err(ViewError::apply(format!(
                    "node {id:?}: order names unknown or repeated row key {key:?}"
                )));
            };
            rows.push(row);
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use serde_json::{Value, json};

    use super::{apply_view_ops, diff_view};
    use crate::plugin_view::{ViewNode, ViewOp, normalize_view};

    fn tree(raw: Value) -> ViewNode {
        normalize_view(&raw).expect("normalizes")
    }

    fn roundtrip(old: &ViewNode, new: &ViewNode) -> Vec<ViewOp> {
        let ops = diff_view(old, new);
        let mut applied = old.clone();
        apply_view_ops(&mut applied, &ops).expect("ops apply");
        assert_eq!(&applied, new);
        ops
    }

    fn pair(a: &str, b: &str) -> Value {
        json!({"k": "stack", "c": [
            {"k": "text", "p": {"key": a, "text": "1"}},
            {"k": "text", "p": {"key": b, "text": "2"}}]})
    }

    fn big_table(changed: Option<usize>) -> ViewNode {
        let rows: Vec<Value> = (0..1000)
            .map(|i| {
                let name = if changed == Some(i) {
                    "changed".to_owned()
                } else {
                    format!("v{i}")
                };
                json!({"key": format!("r{i}"), "n": name})
            })
            .collect();
        tree(json!({"k": "table", "p": {"columns": [{"key": "n", "title": "N"}], "rows": rows}}))
    }

    #[test]
    fn plugin_view_reorder_emits_single_children_op() {
        let old = tree(pair("a", "b"));
        let new = tree(pair("b", "a"));
        // 交换 key 但内容跟随 key：把第二个文本内容也对调
        let mut new_swapped = new.clone();
        new_swapped.c[0].p.insert("text".into(), json!("2"));
        new_swapped.c[1].p.insert("text".into(), json!("1"));
        let ops = roundtrip(&old, &new_swapped);
        assert_eq!(
            ops,
            vec![ViewOp::Children {
                id: "root".into(),
                order: vec!["root/b".into(), "root/a".into()],
                insert: vec![]
            }]
        );
    }

    #[test]
    fn plugin_view_table_1000_rows_one_changed_emits_single_rows_op() {
        let old = big_table(None);
        let new = big_table(Some(500));
        let ops = roundtrip(&old, &new);
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            ViewOp::Rows {
                id,
                upsert,
                remove,
                order,
            } => {
                assert_eq!(id, "root");
                assert_eq!(upsert.len(), 1);
                assert_eq!(upsert[0]["key"], "r500");
                assert!(remove.is_empty());
                assert!(order.is_none());
            }
            other => panic!("expected Rows, got {other:?}"),
        }
    }

    #[test]
    fn plugin_view_rows_order_only_when_it_differs_from_default_apply() {
        let t = |keys: &[&str]| {
            let rows: Vec<Value> = keys.iter().map(|k| json!({"key": k})).collect();
            tree(
                json!({"k": "table", "p": {"columns": [{"key": "n", "title": "N"}], "rows": rows}}),
            )
        };
        // 末尾追加 / 删除：无 order
        let ops = roundtrip(&t(&["a", "b"]), &t(&["a", "b", "c"]));
        assert!(matches!(&ops[..], [ViewOp::Rows { order: None, .. }]));
        let ops = roundtrip(&t(&["a", "b", "c"]), &t(&["a", "c"]));
        assert!(matches!(&ops[..], [ViewOp::Rows { order: None, remove, .. }] if remove == &["b"]));
        // 中间插入 / 重排：有 order
        let ops = roundtrip(&t(&["a", "c"]), &t(&["a", "b", "c"]));
        assert!(matches!(&ops[..], [ViewOp::Rows { order: Some(o), .. }] if o == &["a", "b", "c"]));
        let ops = roundtrip(&t(&["a", "b", "c"]), &t(&["c", "a", "b"]));
        assert!(
            matches!(&ops[..], [ViewOp::Rows { upsert, order: Some(_), .. }] if upsert.is_empty())
        );
    }

    #[test]
    fn plugin_view_list_items_use_rows_op() {
        let l = |items: Value| tree(json!({"k": "list", "p": {"items": items}}));
        let old = l(json!([{"key": "a", "title": "A"}, {"key": "b", "title": "B"}]));
        let new = l(json!([{"key": "a", "title": "A2"}, {"key": "b", "title": "B"}]));
        let ops = roundtrip(&old, &new);
        assert!(matches!(&ops[..], [ViewOp::Rows { upsert, .. }] if upsert.len() == 1));
    }

    #[test]
    fn plugin_view_kind_change_replaces_subtree() {
        let old = tree(json!({"k": "stack", "c": [{"k": "text", "p": {"key": "a", "text": "x"}}]}));
        let new = tree(json!({"k": "stack", "c": [{"k": "code", "p": {"key": "a", "text": "x"}}]}));
        let ops = roundtrip(&old, &new);
        assert!(matches!(&ops[..], [ViewOp::Replace { id, .. }] if id == "root/a"));
        let new_root = tree(json!({"k": "scroll"}));
        let ops = roundtrip(&old, &new_root);
        assert!(matches!(&ops[..], [ViewOp::Replace { id, .. }] if id == "root"));
    }

    #[test]
    fn plugin_view_props_set_and_unset() {
        let old = tree(json!({"k": "card", "p": {"title": "T", "padding": "md"}}));
        let new = tree(json!({"k": "card", "p": {"padding": "lg"}}));
        let ops = roundtrip(&old, &new);
        assert_eq!(
            ops,
            vec![ViewOp::Props {
                id: "root".into(),
                set: json!({"padding": "lg"}).as_object().cloned().unwrap(),
                unset: vec!["title".into()]
            }]
        );
    }

    #[test]
    fn plugin_view_identical_trees_produce_no_ops() {
        let t = tree(pair("a", "b"));
        assert!(diff_view(&t, &t.clone()).is_empty());
    }

    #[test]
    fn plugin_view_error_frame_is_written_then_cleared_by_next_frame() {
        let good =
            tree(json!({"k": "scroll", "c": [{"k": "heading", "p": {"key": "a", "text": "A"}}]}));
        let mut shown = good.clone();
        apply_view_ops(
            &mut shown,
            &[ViewOp::Error {
                message: "boom".into(),
            }],
        )
        .unwrap();
        assert_eq!(shown.p["__error"], "boom");

        // 仅内容相同的下一帧也必须清除错误
        let ops = diff_view(&shown, &good);
        assert_eq!(
            ops,
            vec![ViewOp::Props {
                id: "root".into(),
                set: Default::default(),
                unset: vec!["__error".into()]
            }]
        );
        apply_view_ops(&mut shown, &ops).unwrap();
        assert_eq!(shown, good);

        // 错误未变化时不重复发 Error
        let mut errored = good.clone();
        errored.p.insert("__error".into(), json!("boom"));
        assert!(diff_view(&errored, &errored.clone()).is_empty());
        let mut changed = errored.clone();
        changed.p.insert("__error".into(), json!("bang"));
        assert_eq!(
            roundtrip(&errored, &changed),
            vec![ViewOp::Error {
                message: "bang".into()
            }]
        );
    }

    #[test]
    fn plugin_view_rows_fall_back_to_props_without_keys() {
        let mk = |rows: Value| {
            let mut node =
                tree(json!({"k": "table", "p": {"columns": [{"key": "n", "title": "N"}]}}));
            node.p.insert("rows".into(), rows);
            node
        };
        let old = mk(json!([{"n": "x"}]));
        let new = mk(json!([{"n": "y"}]));
        let ops = roundtrip(&old, &new);
        assert!(matches!(&ops[..], [ViewOp::Props { set, .. }] if set.contains_key("rows")));
    }

    #[test]
    fn plugin_view_apply_rejects_bad_ops() {
        let mut t = tree(json!({"k": "stack", "c": [{"k": "divider"}]}));
        let props = |id: &str| ViewOp::Props {
            id: id.into(),
            set: Default::default(),
            unset: vec![],
        };
        assert!(apply_view_ops(&mut t.clone(), &[props("root/nope")]).is_err());
        assert!(apply_view_ops(&mut t.clone(), &[props("rootx")]).is_err());
        let unknown_order = ViewOp::Children {
            id: "root".into(),
            order: vec!["root/9".into()],
            insert: vec![],
        };
        assert!(apply_view_ops(&mut t.clone(), &[unknown_order]).is_err());
        let missing_from_order = ViewOp::Children {
            id: "root".into(),
            order: vec!["root/0".into()],
            insert: vec![tree(json!({"k": "divider"}))],
        };
        assert!(apply_view_ops(&mut t.clone(), &[missing_from_order]).is_err());
        let rows = ViewOp::Rows {
            id: "root".into(),
            upsert: vec![],
            remove: vec![],
            order: None,
        };
        assert!(apply_view_ops(&mut t, &[rows]).is_err());

        let mut table = tree(
            json!({"k": "table", "p": {"columns": [{"key": "n", "title": "N"}],
            "rows": [{"key": "a"}]}}),
        );
        let remove_unknown = ViewOp::Rows {
            id: "root".into(),
            upsert: vec![],
            remove: vec!["zz".into()],
            order: None,
        };
        assert!(apply_view_ops(&mut table.clone(), &[remove_unknown]).is_err());
        let bad_order = ViewOp::Rows {
            id: "root".into(),
            upsert: vec![],
            remove: vec![],
            order: Some(vec!["zz".into()]),
        };
        assert!(apply_view_ops(&mut table, &[bad_order]).is_err());
    }
}
