//! 提交流程：底稿 → 整体提交 → 回显；文本输入防抖；开关 / 下拉的字段变更。

use fluxdown_protocol::{
    ShareConnectionMode, ShareExitBehavior, ShareLocalConfigDto, ShareLogLevel,
};
use gpui::{App, Context, Window};
use gpui_component::{WindowExt as _, input::InputEvent, notification::Notification};

use super::{ShareSettingsView, TEXT_DEBOUNCE};
use crate::{
    errors::error_text,
    model::settings_form::{
        RateUnit, TextEdits, apply_text_inputs, daily_cap_text, extra_addrs_text, parse_daily_cap,
        parse_extra_addrs, parse_port, parse_rate_limit, pinned_relay_from_value, port_text,
        preferred_rate_unit, prepare_commit, rate_text, tier_id_from_value,
    },
};

impl ShareSettingsView {
    /// 当前是否可以改设置：本构建 / 会话可用且与 agent 保持连接。
    pub(super) fn editable(&self, cx: &App) -> bool {
        let hub = self.hub.read(cx);
        hub.available() && !hub.is_stale()
    }

    /// 与 agent 保持连接（总开关在功能未启用、`available == false` 时也要能改）。
    pub(super) fn connected(&self, cx: &App) -> bool {
        !self.hub.read(cx).is_stale()
    }

    // ───────────────────────── 提交 ─────────────────────────

    /// 以底稿为基础改一个字段并整体提交。
    pub(super) fn commit(
        &mut self,
        edit: impl FnOnce(&mut ShareLocalConfigDto),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut next = self.shown.clone();
        edit(&mut next);
        self.submit(next, window, cx);
    }

    pub(super) fn submit(
        &mut self,
        next: ShareLocalConfigDto,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // 总开关本身只要求连接；其余字段要求功能可用。
        let allowed = if next.enabled == self.shown.enabled {
            self.editable(cx)
        } else {
            self.connected(cx)
        };
        if !allowed {
            return;
        }
        let offset = chrono::Local::now().offset().local_minus_utc();
        let Some(next) = prepare_commit(&self.shown, next, offset) else {
            return;
        };
        self.shown = next.clone();
        self.commit_seq += 1;
        self.inflight += 1;
        let seq = self.commit_seq;
        let future = self.hub.read(cx).api().set_local_config(&next);
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = future.await;
            let Ok(()) = this.update_in(cx, |this, window, cx| {
                this.finish_commit(seq, result, window, cx);
            }) else {
                // 视图已随窗口释放，结果无处展示。
                return;
            };
        })
        .detach();
    }

    pub(super) fn finish_commit(
        &mut self,
        seq: u64,
        result: Result<ShareLocalConfigDto, fluxdown_protocol::RpcErrorData>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.inflight = self.inflight.saturating_sub(1);
        let latest = seq == self.commit_seq;
        match result {
            Ok(saved) => {
                if latest {
                    self.shown.clone_from(&saved);
                    self.sync_inputs(&saved, window, cx);
                }
            }
            Err(error) => {
                let text = error_text(self.translator.read(cx), &error);
                window.push_notification(Notification::error(text), cx);
                if latest {
                    let current = self.hub.read(cx).state().local_config.clone();
                    self.hub_seen.clone_from(&current);
                    self.shown.clone_from(&current);
                    self.text_pending = false;
                    self.sync_inputs(&current, window, cx);
                }
            }
        }
        cx.notify();
    }

    // ───────────────────────── 文本输入 ─────────────────────────

    pub(super) fn on_text_event(
        &mut self,
        event: &InputEvent,
        single_line: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => self.schedule_text_commit(window, cx),
            InputEvent::Blur => self.flush_text(window, cx),
            InputEvent::PressEnter { .. } if single_line => self.flush_text(window, cx),
            _ => {}
        }
    }

    pub(super) fn schedule_text_commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.text_generation += 1;
        self.text_pending = true;
        let generation = self.text_generation;
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(TEXT_DEBOUNCE).await;
            let Ok(()) = this.update_in(cx, |this, window, cx| {
                if this.text_generation == generation {
                    this.flush_text(window, cx);
                }
            }) else {
                // 视图已随窗口释放，不再提交。
                return;
            };
        })
        .detach();
    }

    pub(super) fn text_edits(&self, cx: &App) -> TextEdits {
        apply_text_inputs(
            &self.shown,
            &self.udp_v4.read(cx).value(),
            &self.udp_v6.read(cx).value(),
            &self.extra_addrs.read(cx).value(),
            &self.rate.read(cx).value(),
            self.rate_unit,
            &self.daily_cap.read(cx).value(),
        )
    }

    /// 立即提交四个文本输入：合法字段生效，非法字段保持底稿并在行内标红。
    pub(super) fn flush_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.text_generation += 1;
        self.text_pending = false;
        let edits = self.text_edits(cx);
        let config = edits.config.clone();
        self.text_errors = edits.has_error().then_some(edits);
        cx.notify();
        self.submit(config, window, cx);
    }

    /// 把配置值回显进文本输入：已能解析成同一个值的输入保持用户的写法，其余改写。
    pub(super) fn sync_inputs(
        &mut self,
        config: &ShareLocalConfigDto,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let port_value = |port: Option<u16>| port.filter(|port| *port != 0);
        for (input, port) in [
            (&self.udp_v4, config.udp_port_v4),
            (&self.udp_v6, config.udp_port_v6),
        ] {
            if parse_port(&input.read(cx).value()) != Ok(port_value(port)) {
                let text = port_text(port);
                input.update(cx, |input, cx| input.set_value(text, window, cx));
            }
        }
        if parse_extra_addrs(&self.extra_addrs.read(cx).value()).as_ref() != Ok(&config.extra_addrs)
        {
            let text = extra_addrs_text(&config.extra_addrs);
            self.extra_addrs
                .update(cx, |input, cx| input.set_value(text, window, cx));
        }
        let bytes = config
            .send_rate_limit_bytes_per_sec
            .filter(|bytes| *bytes != 0);
        if parse_rate_limit(&self.rate.read(cx).value(), self.rate_unit) != Ok(bytes) {
            self.rate_unit = preferred_rate_unit(bytes, self.rate_unit);
            let text = rate_text(bytes, self.rate_unit);
            self.rate
                .update(cx, |input, cx| input.set_value(text, window, cx));
        }
        let daily_cap = config.provide_daily_cap_bytes;
        if parse_daily_cap(&self.daily_cap.read(cx).value()) != Ok(daily_cap) {
            let text = daily_cap_text(daily_cap);
            self.daily_cap
                .update(cx, |input, cx| input.set_value(text, window, cx));
        }
        let edits = self.text_edits(cx);
        self.text_errors = edits.has_error().then_some(edits);
    }

    // ───────────────────────── 下拉 / 开关的变更 ─────────────────────────

    pub(super) fn set_default_tier(
        &mut self,
        value: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commit(
            |c| c.default_tier_id = tier_id_from_value(value),
            window,
            cx,
        );
    }

    pub(super) fn set_default_claims(
        &mut self,
        value: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Ok(claims) = value.parse::<u32>() else {
            return;
        };
        self.commit(|c| c.default_max_claims = claims, window, cx);
    }

    pub(super) fn set_default_notify(
        &mut self,
        value: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commit(|c| c.default_notify_on_claim = value, window, cx);
    }

    pub(super) fn set_default_swarm(
        &mut self,
        value: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commit(|c| c.default_swarm = Some(value), window, cx);
    }

    pub(super) fn set_enabled(&mut self, value: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.commit(|c| c.enabled = value, window, cx);
    }

    pub(super) fn set_auto_provide(
        &mut self,
        value: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commit(|c| c.auto_provide = Some(value), window, cx);
    }

    pub(super) fn set_portmapper(
        &mut self,
        value: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let portmapper = match value {
            "on" => Some(true),
            "off" => Some(false),
            _ => None,
        };
        self.commit(|c| c.portmapper = portmapper, window, cx);
    }

    pub(super) fn set_mode(&mut self, value: &str, window: &mut Window, cx: &mut Context<Self>) {
        let mode = match value {
            "relayOnly" => ShareConnectionMode::RelayOnly,
            "directOnly" => ShareConnectionMode::DirectOnly,
            _ => ShareConnectionMode::Auto,
        };
        self.commit(|c| c.mode = mode, window, cx);
    }

    pub(super) fn set_relay(&mut self, value: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.commit(
            |c| c.pinned_relay = pinned_relay_from_value(value),
            window,
            cx,
        );
    }

    pub(super) fn set_exit_behavior(
        &mut self,
        value: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let behavior = match value {
            "quit" => ShareExitBehavior::Quit,
            "convertToCloud" => ShareExitBehavior::ConvertToCloud,
            _ => ShareExitBehavior::Prompt,
        };
        self.commit(|c| c.exit_behavior = behavior, window, cx);
    }

    pub(super) fn set_log_level(
        &mut self,
        value: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let level = match value {
            "error" => ShareLogLevel::Error,
            "info" => ShareLogLevel::Info,
            "debug" => ShareLogLevel::Debug,
            "trace" => ShareLogLevel::Trace,
            _ => ShareLogLevel::Warn,
        };
        self.commit(|c| c.iroh_log_level = level, window, cx);
    }

    /// 换单位保持限速的字节值不变（输入框数字随之换算），所以不触发提交。
    pub(super) fn set_rate_unit(
        &mut self,
        value: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(unit) = RateUnit::from_id(value) else {
            return;
        };
        let current = self.rate.read(cx).value();
        let parsed = parse_rate_limit(&current, self.rate_unit);
        self.rate_unit = unit;
        if let Ok(bytes) = parsed {
            let text = rate_text(bytes, unit);
            self.rate
                .update(cx, |input, cx| input.set_value(text, window, cx));
        }
        let edits = self.text_edits(cx);
        self.text_errors = edits.has_error().then_some(edits);
        cx.notify();
        // 输入框内容变了（且可能是未提交的文本）：按新单位重新提交一次。
        self.flush_text(window, cx);
    }
}
