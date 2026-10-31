//! 可移植视图会话存储：每个会话保存 `(seq, 规范化树)`，把插件宿主给出的整棵新树
//! 转成发给客户端的增量帧。
//!
//! - `open` 存 `(0, tree)`；之后每次渲染 `normalize → diff`，ops 非空才 `seq += 1`。
//! - ops 序列化后超过 [`MAX_FRAME_BYTES`] 时退化为根节点整体替换，避免大树的碎片化帧。
//! - 渲染失败发 `Error` 帧，并把同一个 `Error` op 应用到存储树上，使下一帧 diff 能带出
//!   根节点的 `Props.unset: ["__error"]`，客户端因此清除错误条。
//! - 客户端经 `view.snapshot` 取回 `(seq, tree)` 来在序号断档时重新同步。

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

use fluxdown_protocol::{
    PluginViewFrameDto, VIEW_ROOT_ID, ViewEffect, ViewNode, ViewOp, apply_view_ops, diff_view,
    normalize_view,
};

/// 单帧 ops 序列化后的上限（1 MiB）。
const MAX_FRAME_BYTES: usize = 1024 * 1024;

struct StoredView {
    seq: u64,
    tree: ViewNode,
}

/// 会话存储。由 [`crate::event_hub::DaemonEventHub`] 持有，事件映射与 RPC 共享同一份。
#[derive(Default)]
pub struct PluginViewStore {
    sessions: Mutex<HashMap<String, StoredView>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl PluginViewStore {
    /// 登记一个刚打开的会话（`seq = 0`）。
    pub fn open(&self, session: &str, tree: ViewNode) {
        lock(&self.sessions).insert(session.to_owned(), StoredView { seq: 0, tree });
    }

    /// 取 `(seq, tree)` 快照；未知会话返回 `None`。
    #[must_use]
    pub fn snapshot(&self, session: &str) -> Option<(u64, ViewNode)> {
        lock(&self.sessions)
            .get(session)
            .map(|stored| (stored.seq, stored.tree.clone()))
    }

    /// 删除会话；返回它此前是否存在。
    pub fn close(&self, session: &str) -> bool {
        lock(&self.sessions).remove(session).is_some()
    }

    /// 宿主渲染出新树：规范化失败发 `Error` 帧，否则 diff；无变化不出帧。
    /// 未知会话（已关闭 / 尚未登记）返回 `None`。
    #[must_use]
    pub fn rendered(&self, session: &str, raw: &serde_json::Value) -> Option<PluginViewFrameDto> {
        let new_tree = match normalize_view(raw) {
            Ok(tree) => tree,
            Err(error) => return self.failed(session, &error.to_string()),
        };
        let mut sessions = lock(&self.sessions);
        let stored = sessions.get_mut(session)?;
        let mut ops = diff_view(&stored.tree, &new_tree);
        if ops.is_empty() {
            return None;
        }
        if serde_json::to_vec(&ops).map_or(true, |bytes| bytes.len() > MAX_FRAME_BYTES) {
            ops = vec![ViewOp::Replace {
                id: VIEW_ROOT_ID.to_owned(),
                node: new_tree.clone(),
            }];
        }
        stored.tree = new_tree;
        stored.seq += 1;
        Some(PluginViewFrameDto {
            session: session.to_owned(),
            seq: stored.seq,
            ops,
            effects: Vec::new(),
        })
    }

    /// 渲染 / 事件处理失败：发 `Error` 帧并把它落到存储树上（会话保持打开）。
    #[must_use]
    pub fn failed(&self, session: &str, message: &str) -> Option<PluginViewFrameDto> {
        let mut sessions = lock(&self.sessions);
        let stored = sessions.get_mut(session)?;
        let ops = vec![ViewOp::Error {
            message: message.to_owned(),
        }];
        if apply_view_ops(&mut stored.tree, &ops).is_err() {
            // `Error` op 只写根节点 prop，不会失败；失败说明存储树已损坏，丢弃会话让
            // 客户端经 snapshot 重新打开。
            sessions.remove(session);
            return None;
        }
        stored.seq += 1;
        Some(PluginViewFrameDto {
            session: session.to_owned(),
            seq: stored.seq,
            ops,
            effects: Vec::new(),
        })
    }

    /// 动作帧：携带宿主副作用（跳转等），seq 取会话当前值且不递增、ops 为空。
    /// 客户端先执行 effects，再按原 seq 规则处理 ops（空 ops 不会推进序号）。
    ///
    /// 会话尚未登记（插件在 `init` 里发起动作，open 应答还没返回）时 seq 取 0：客户端在 open
    /// 进行中会按会话缓存动作，登记后执行，因此动作不能因存储里暂时没有该会话而丢弃。
    #[must_use]
    pub fn effect(&self, session: &str, effect: ViewEffect) -> PluginViewFrameDto {
        let seq = lock(&self.sessions)
            .get(session)
            .map_or(0, |stored| stored.seq);
        PluginViewFrameDto {
            session: session.to_owned(),
            seq,
            ops: Vec::new(),
            effects: vec![effect],
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use serde_json::json;

    use super::*;

    fn text_tree(text: &str) -> serde_json::Value {
        json!({ "k": "stack", "c": [{ "k": "text", "p": { "text": text } }] })
    }

    fn opened(store: &PluginViewStore, session: &str, text: &str) {
        store.open(session, normalize_view(&text_tree(text)).unwrap());
    }

    #[test]
    fn effect_frame_keeps_seq_and_is_emitted_before_registration() {
        let store = PluginViewStore::default();
        let navigate = || ViewEffect::Navigate {
            target: "rss".to_owned(),
        };
        // 插件在 init 里跳转：会话尚未登记，动作照样成帧（seq 0），交给客户端缓存。
        let early = store.effect("s", navigate());
        assert_eq!((early.seq, early.effects.len()), (0, 1));
        opened(&store, "s", "0");
        let _ = store.rendered("s", &text_tree("1")).unwrap();
        let frame = store.effect("s", navigate());
        assert_eq!(frame.seq, 1);
        assert!(frame.ops.is_empty());
        assert_eq!(frame.effects, vec![navigate()]);
        assert_eq!(store.snapshot("s").unwrap().0, 1);
    }

    #[test]
    fn rendered_twice_yields_sequential_seqs_and_skips_unchanged() {
        let store = PluginViewStore::default();
        opened(&store, "s", "0");
        assert!(store.rendered("s", &text_tree("0")).is_none());

        let first = store.rendered("s", &text_tree("1")).unwrap();
        assert_eq!(first.seq, 1);
        let second = store.rendered("s", &text_tree("2")).unwrap();
        assert_eq!(second.seq, 2);
        let (seq, tree) = store.snapshot("s").unwrap();
        assert_eq!(seq, 2);
        assert_eq!(tree.c[0].p["text"], "2");
    }

    #[test]
    fn failure_frame_is_cleared_by_the_next_render() {
        let store = PluginViewStore::default();
        opened(&store, "s", "0");
        let failed = store.failed("s", "boom").unwrap();
        assert_eq!(failed.seq, 1);
        assert!(matches!(&failed.ops[..], [ViewOp::Error { message }] if message == "boom"));
        assert!(store.snapshot("s").unwrap().1.p.contains_key("__error"));

        // 同样的内容再次渲染：必须清掉根节点上的 __error。
        let next = store.rendered("s", &text_tree("0")).unwrap();
        assert_eq!(next.seq, 2);
        let mut client = store.snapshot("s").unwrap().1;
        assert!(!client.p.contains_key("__error"));
        // 客户端按 ops 应用后与存储树一致。
        let mut replay = normalize_view(&text_tree("0")).unwrap();
        replay.p.insert("__error".into(), json!("boom"));
        apply_view_ops(&mut replay, &next.ops).unwrap();
        assert_eq!(replay, client);
        client.p.clear();
    }

    #[test]
    fn invalid_tree_becomes_error_frame_and_unknown_session_is_ignored() {
        let store = PluginViewStore::default();
        opened(&store, "s", "0");
        let frame = store
            .rendered("s", &json!({ "k": "no-such-kind" }))
            .unwrap();
        assert!(matches!(&frame.ops[..], [ViewOp::Error { .. }]));
        assert!(store.rendered("missing", &text_tree("1")).is_none());
        assert!(store.snapshot("missing").is_none());
        assert!(store.close("s"));
        assert!(!store.close("s"));
    }

    #[test]
    fn oversized_ops_collapse_into_a_root_replace() {
        let store = PluginViewStore::default();
        let rows = |value: &str| {
            (0..4000)
                .map(|i| json!({ "key": format!("r{i}"), "name": format!("{value}-{i}-{}", "x".repeat(400)) }))
                .collect::<Vec<_>>()
        };
        let table = |value: &str| {
            json!({
                "k": "table",
                "p": {
                    "columns": [{ "key": "name", "title": "Name" }],
                    "rows": rows(value),
                }
            })
        };
        store.open("s", normalize_view(&table("a")).unwrap());
        let frame = store.rendered("s", &table("b")).unwrap();
        assert!(matches!(&frame.ops[..], [ViewOp::Replace { id, .. }] if id == VIEW_ROOT_ID));
    }
}
