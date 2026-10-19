//! 桌面脚本的粗粒度审计：加载 / 卸载 / `fluxdown.call` / `fluxdown.invoke`。
//!
//! 记录先进入进程内缓冲，满 [`FLUSH_COUNT`] 条或距首条入缓冲 [`FLUSH_INTERVAL`] 后经
//! port 调用一次 `daemon.plugin.audit.record`（每次 ≤ [`MAX_BATCH`] 条，daemon 上限 100）。
//! 没有记录时不起定时器、不发请求。脚本的原生文件 / 进程 / 网络操作不逐次记录。

use std::time::{Duration, Instant};

use gpui::{App, AppContext as _};
use serde_json::json;

pub(crate) const FLUSH_COUNT: usize = 50;
pub(crate) const FLUSH_INTERVAL: Duration = Duration::from_secs(5);
const MAX_BATCH: usize = 100;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AuditEntry {
    pub identity: String,
    pub action: &'static str,
    pub target: String,
    pub outcome: &'static str,
    pub detail: String,
}

impl AuditEntry {
    pub(crate) fn new(
        identity: &str,
        action: &'static str,
        target: impl Into<String>,
        outcome: &'static str,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            identity: identity.to_owned(),
            action,
            target: target.into(),
            outcome,
            detail: detail.into(),
        }
    }
}

/// 纯数据缓冲（时间由调用方传入，便于测试）。
#[derive(Default)]
pub(crate) struct AuditBuffer {
    entries: Vec<AuditEntry>,
    first_at: Option<Instant>,
    /// 已有一个到期 flush 的定时器在途。
    timer_armed: bool,
}

impl AuditBuffer {
    pub(crate) fn push(&mut self, entry: AuditEntry, now: Instant) {
        if self.entries.is_empty() {
            self.first_at = Some(now);
        }
        self.entries.push(entry);
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// 满数量，或最老一条已等满间隔。
    pub(crate) fn due(&self, now: Instant) -> bool {
        self.entries.len() >= FLUSH_COUNT
            || self
                .first_at
                .is_some_and(|first| now.saturating_duration_since(first) >= FLUSH_INTERVAL)
    }

    pub(crate) fn take(&mut self) -> Vec<AuditEntry> {
        self.first_at = None;
        std::mem::take(&mut self.entries)
    }

    /// 需要新建定时器时返回 `true`（并记为已在途）。
    fn arm_timer(&mut self) -> bool {
        if self.timer_armed || self.entries.is_empty() {
            return false;
        }
        self.timer_armed = true;
        true
    }
}

/// 记录一条审计；没有初始化插件宿主时丢弃。
pub(crate) fn record(cx: &mut App, entry: AuditEntry) {
    let now = Instant::now();
    let (due, arm) = {
        let Some(host) = crate::host_mut(cx) else {
            return;
        };
        host.audit.push(entry, now);
        (host.audit.due(now), host.audit.arm_timer())
    };
    if due {
        flush(cx);
    } else if arm {
        cx.spawn(async move |cx| {
            cx.background_executor().timer(FLUSH_INTERVAL).await;
            cx.update(|cx| {
                if let Some(host) = crate::host_mut(cx) {
                    host.audit.timer_armed = false;
                }
                flush(cx);
            });
        })
        .detach();
    }
}

/// 把缓冲里的记录发给 daemon；失败只记 warn 并丢弃。
pub(crate) fn flush(cx: &mut App) {
    let Some(host) = crate::host_mut(cx) else {
        return;
    };
    let entries = host.audit.take();
    if entries.is_empty() {
        return;
    }
    let port = host.port.clone();
    for chunk in entries.chunks(MAX_BATCH) {
        let rows: Vec<_> = chunk
            .iter()
            .map(|entry| {
                json!({
                    "identity": entry.identity,
                    "action": entry.action,
                    "target": entry.target,
                    "outcome": entry.outcome,
                    "detail": entry.detail,
                })
            })
            .collect();
        let call = port.call(
            "daemon.plugin.audit.record".to_owned(),
            json!({ "entries": rows }),
        );
        cx.background_spawn(async move {
            if let Err(error) = call.await {
                log::warn!("plugin audit record dropped: {error:?}");
            }
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(n: usize) -> AuditEntry {
        AuditEntry::new("a@b", "desktop.call", format!("m{n}"), "ok", "")
    }

    #[test]
    fn flushes_when_full() {
        let now = Instant::now();
        let mut buffer = AuditBuffer::default();
        for n in 0..FLUSH_COUNT - 1 {
            buffer.push(entry(n), now);
        }
        assert!(!buffer.due(now));
        buffer.push(entry(99), now);
        assert!(buffer.due(now));
        assert_eq!(buffer.take().len(), FLUSH_COUNT);
        assert_eq!(buffer.len(), 0);
        assert!(!buffer.due(now + FLUSH_INTERVAL * 2));
    }

    #[test]
    fn flushes_after_interval_from_first_entry() {
        let start = Instant::now();
        let mut buffer = AuditBuffer::default();
        buffer.push(entry(0), start);
        buffer.push(entry(1), start + Duration::from_secs(4));
        assert!(!buffer.due(start + Duration::from_secs(4)));
        assert!(buffer.due(start + FLUSH_INTERVAL));
    }

    #[test]
    fn timer_is_armed_once_and_only_with_entries() {
        let mut buffer = AuditBuffer::default();
        assert!(!buffer.arm_timer());
        buffer.push(entry(0), Instant::now());
        assert!(buffer.arm_timer());
        assert!(!buffer.arm_timer());
    }
}
