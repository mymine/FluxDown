//! iroh / FluxMesh的内存 tracing 环形缓冲（诊断页「iroh 日志」）。
//!
//! 全进程一份：`logging.rs` / `server_mode.rs` 装订阅者时挂上 [`layer`]，它只收 `iroh*` 与
//! `fluxdown_share` 目标、级别不低于当前设置的事件；缓冲有界，只在内存。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, PoisonError};

use fluxdown_protocol::{ShareLogLevel, ShareLogLineDto};
use fluxdown_share::timefmt::{now_unix_ms, rfc3339_ms};
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;

const CAPACITY: usize = 2000;

struct Line {
    at_ms: u64,
    level: ShareLogLevel,
    target: String,
    message: String,
}

static RING: Mutex<VecDeque<Line>> = Mutex::new(VecDeque::new());
/// 当前收集下限（`ShareLogLevel` 的序号：越大越详细）。
static LEVEL: AtomicU8 = AtomicU8::new(1);

fn rank(level: ShareLogLevel) -> u8 {
    match level {
        ShareLogLevel::Error => 0,
        ShareLogLevel::Warn => 1,
        ShareLogLevel::Info => 2,
        ShareLogLevel::Debug => 3,
        ShareLogLevel::Trace => 4,
    }
}

fn tracing_rank(level: &Level) -> u8 {
    match *level {
        Level::ERROR => 0,
        Level::WARN => 1,
        Level::INFO => 2,
        Level::DEBUG => 3,
        Level::TRACE => 4,
    }
}

fn dto_level(level: &Level) -> ShareLogLevel {
    match *level {
        Level::ERROR => ShareLogLevel::Error,
        Level::WARN => ShareLogLevel::Warn,
        Level::INFO => ShareLogLevel::Info,
        Level::DEBUG => ShareLogLevel::Debug,
        Level::TRACE => ShareLogLevel::Trace,
    }
}

/// 设置收集级别（本机设置「iroh 日志级别」）。
pub(super) fn set_level(level: ShareLogLevel) {
    LEVEL.store(rank(level), Ordering::Relaxed);
}

fn wanted(target: &str) -> bool {
    target.starts_with("iroh") || target.starts_with("fluxdown_share")
}

struct Message(String);

impl Visit for Message {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0.push_str(&format!("{value:?}"));
        } else {
            self.0.push_str(&format!(" {}={value:?}", field.name()));
        }
    }
}

struct RingLayer;

impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for RingLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        let mut message = Message(String::new());
        event.record(&mut message);
        let mut ring = RING.lock().unwrap_or_else(PoisonError::into_inner);
        if ring.len() >= CAPACITY {
            ring.pop_front();
        }
        ring.push_back(Line {
            at_ms: now_unix_ms(),
            level: dto_level(meta.level()),
            target: meta.target().to_owned(),
            message: message.0,
        });
    }
}

/// 挂到全局订阅者上的环形缓冲层（自带每层过滤，不受主输出的 `EnvFilter` 影响）。
pub fn layer<S: Subscriber + for<'a> LookupSpan<'a>>() -> impl Layer<S> {
    RingLayer.with_filter(filter_fn(|meta| {
        wanted(meta.target()) && tracing_rank(meta.level()) <= LEVEL.load(Ordering::Relaxed)
    }))
}

/// 读取最新 `limit` 行（时间升序），按级别下限与可选正则（对 `target message`）过滤。
pub(super) fn read(
    min: ShareLogLevel,
    regex: Option<&regex::Regex>,
    limit: usize,
) -> Vec<ShareLogLineDto> {
    let ring = RING.lock().unwrap_or_else(PoisonError::into_inner);
    let mut out: Vec<ShareLogLineDto> = ring
        .iter()
        .rev()
        .filter(|line| rank(line.level) <= rank(min))
        .filter(|line| {
            regex.is_none_or(|re| re.is_match(&format!("{} {}", line.target, line.message)))
        })
        .take(limit)
        .map(|line| ShareLogLineDto {
            at: rfc3339_ms(line.at_ms),
            level: line.level,
            target: line.target.clone(),
            message: line.message.clone(),
        })
        .collect();
    out.reverse();
    out
}
