//! 单条 agent 连接上在途的 `agent.preferences.patch` 写入覆盖层。
//!
//! gateway 对「响应」与「事件」两条出站队列做无偏 `select`：一次写入的响应可能先于、也可能
//! 晚于携带它的 `PreferencesChanged`，写入前已排队的旧偏好事件也可能在响应之后才到。任何
//! 订阅方若直接应用这些载荷，就会把本进程刚写入的值短暂（或在无后续事件时永久）回弹成旧值。
//!
//! 这里在写入发出时登记目标值，并把它盖到之后转发的每个偏好载荷上，直到出现 revision 不低于
//! 该写入 revision（见 [`AgentPreferencesPatchResult`]）的载荷——那时 agent 自己的状态已包含
//! 本次写入，覆盖层退场，后续其他来源（云同步、其他客户端）的变化照常生效。写入失败立即退场。
//!
//! 覆盖层随连接存亡：断线后在途请求全部以失败结束，重连快照即事实。

use std::collections::BTreeMap;

use fluxdown_protocol::{
    AgentEvent, AgentPreferencesDto, AgentPreferencesPatchResult, EventFrame, ServiceEvent,
    SettingOwner, method, setting_spec,
};
use serde::Deserialize as _;
use serde_json::Value;

struct PendingWrite {
    value: Value,
    request: i64,
    /// 响应给出的写入 revision；响应到达前为 `None`。
    revision: Option<u64>,
}

pub(crate) struct PreferenceWrites {
    pending: BTreeMap<String, PendingWrite>,
    /// 已转发偏好载荷的最高 revision。
    seen_revision: u64,
}

impl PreferenceWrites {
    /// `seen_revision`：本连接首个快照的偏好 revision。
    pub(crate) fn new(seen_revision: u64) -> Self {
        Self {
            pending: BTreeMap::new(),
            seen_revision,
        }
    }

    /// 请求发出前登记；非偏好写入忽略。同键的新写入覆盖旧写入（旧请求的响应不再影响它）。
    pub(crate) fn stage(&mut self, request: i64, method_name: &str, params: Option<&Value>) {
        if method_name != method::AGENT_PREFERENCES_PATCH {
            return;
        }
        let Some(values) = params
            .and_then(|params| params.get("values"))
            .and_then(Value::as_object)
        else {
            return;
        };
        for (key, value) in values {
            if lands_in_preferences(key) {
                self.pending.insert(
                    key.clone(),
                    PendingWrite {
                        value: value.clone(),
                        request,
                        revision: None,
                    },
                );
            }
        }
    }

    /// 请求结束：`result` 为 `None` 表示失败。成功时保留到 revision 被载荷追上。
    pub(crate) fn settle(&mut self, request: i64, result: Option<&Value>) {
        let revision = result
            .and_then(|result| AgentPreferencesPatchResult::deserialize(result).ok())
            .map(|result| result.revision);
        let seen = self.seen_revision;
        self.pending.retain(|_, write| {
            if write.request != request {
                return true;
            }
            match revision {
                Some(revision) if revision > seen => {
                    write.revision = Some(revision);
                    true
                }
                _ => false,
            }
        });
    }

    /// 转发前修正事件里的偏好载荷。
    pub(crate) fn overlay_frame(&mut self, frame: &mut EventFrame) {
        if let ServiceEvent::Agent(AgentEvent::PreferencesChanged(preferences)) = &mut frame.event {
            self.overlay(preferences);
        }
    }

    fn overlay(&mut self, preferences: &mut AgentPreferencesDto) {
        self.seen_revision = self.seen_revision.max(preferences.revision);
        let seen = self.seen_revision;
        self.pending
            .retain(|_, write| write.revision.is_none_or(|revision| revision > seen));
        for (key, write) in &self.pending {
            if write.value.is_null() {
                preferences.values.remove(key);
            } else {
                preferences.values.insert(key.clone(), write.value.clone());
            }
        }
    }
}

/// agent 把该键写进偏好表（而非只落 daemon 配置）。
fn lands_in_preferences(key: &str) -> bool {
    setting_spec(key).is_none_or(|spec| spec.owner != SettingOwner::Daemon)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const THEME: &str = "appearance.theme_mode";

    fn patch(values: Value) -> Value {
        json!({ "values": values })
    }

    fn ok(revision: u64) -> Value {
        json!({ "ok": true, "revision": revision })
    }

    /// 转发一条 `PreferencesChanged`，返回订阅方看到的值。
    fn deliver(writes: &mut PreferenceWrites, revision: u64, values: Value) -> Value {
        let values = serde_json::from_value(values).expect("values");
        let mut frame = EventFrame {
            epoch: "e".into(),
            sequence: 1,
            previous: None,
            event: ServiceEvent::Agent(AgentEvent::PreferencesChanged(AgentPreferencesDto {
                revision,
                values,
            })),
        };
        writes.overlay_frame(&mut frame);
        let ServiceEvent::Agent(AgentEvent::PreferencesChanged(preferences)) = frame.event else {
            panic!("preferences event");
        };
        serde_json::to_value(preferences.values).expect("json")
    }

    #[test]
    fn stale_event_after_the_response_does_not_revert_the_write() {
        let mut writes = PreferenceWrites::new(4);
        writes.stage(
            10,
            method::AGENT_PREFERENCES_PATCH,
            Some(&patch(json!({ THEME: "dark" }))),
        );
        // 写入前排队的无关事件（revision 5）在写入之前、之后到达都不得回弹。
        assert_eq!(
            deliver(&mut writes, 5, json!({ THEME: "light", "x": 1 })),
            json!({ THEME: "dark", "x": 1 })
        );
        writes.settle(10, Some(&ok(7)));
        assert_eq!(
            deliver(&mut writes, 6, json!({ THEME: "light", "x": 2 })),
            json!({ THEME: "dark", "x": 2 })
        );
        // 携带本次写入的事件确认后退场，此后其他来源的变化照常生效。
        assert_eq!(
            deliver(&mut writes, 7, json!({ THEME: "dark" })),
            json!({ THEME: "dark" })
        );
        assert_eq!(
            deliver(&mut writes, 8, json!({ THEME: "light" })),
            json!({ THEME: "light" })
        );
    }

    #[test]
    fn event_arriving_before_the_response_confirms_the_write() {
        let mut writes = PreferenceWrites::new(0);
        writes.stage(
            10,
            method::AGENT_PREFERENCES_PATCH,
            Some(&patch(json!({ THEME: "dark" }))),
        );
        assert_eq!(
            deliver(&mut writes, 1, json!({ THEME: "dark" })),
            json!({ THEME: "dark" })
        );
        writes.settle(10, Some(&ok(1)));
        assert_eq!(
            deliver(&mut writes, 2, json!({ THEME: "light" })),
            json!({ THEME: "light" })
        );
    }

    #[test]
    fn failed_write_stops_overlaying() {
        let mut writes = PreferenceWrites::new(0);
        writes.stage(
            10,
            method::AGENT_PREFERENCES_PATCH,
            Some(&patch(json!({ THEME: "dark" }))),
        );
        writes.settle(10, None);
        assert_eq!(
            deliver(&mut writes, 1, json!({ THEME: "light" })),
            json!({ THEME: "light" })
        );
    }

    #[test]
    fn newer_write_to_the_same_key_outlives_the_older_response() {
        let mut writes = PreferenceWrites::new(0);
        writes.stage(
            10,
            method::AGENT_PREFERENCES_PATCH,
            Some(&patch(json!({ THEME: "dark" }))),
        );
        writes.stage(
            11,
            method::AGENT_PREFERENCES_PATCH,
            Some(&patch(json!({ THEME: "light" }))),
        );
        writes.settle(10, Some(&ok(1)));
        assert_eq!(
            deliver(&mut writes, 1, json!({ THEME: "dark" })),
            json!({ THEME: "light" })
        );
        writes.settle(11, Some(&ok(2)));
        assert_eq!(
            deliver(&mut writes, 2, json!({ THEME: "light" })),
            json!({ THEME: "light" })
        );
        assert_eq!(
            deliver(&mut writes, 3, json!({ THEME: "system" })),
            json!({ THEME: "system" })
        );
    }

    #[test]
    fn tombstone_removes_the_key_and_daemon_keys_are_not_overlaid() {
        let mut writes = PreferenceWrites::new(0);
        writes.stage(
            10,
            method::AGENT_PREFERENCES_PATCH,
            Some(&patch(json!({ THEME: null, "bt.enable_dht": false }))),
        );
        assert_eq!(deliver(&mut writes, 1, json!({ THEME: "dark" })), json!({}));
    }
}
