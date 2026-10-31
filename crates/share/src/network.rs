//! 设置 › FluxMesh › 「网络诊断」页签：端点与 NAT / 中继 / 公网地址、各中继延迟、传输会话（可展开路径与事件时间线）、
//! 完整 iroh 指标表、iroh 日志查看器与原始快照 JSON。
//!
//! 空闲静默：只在视图实际渲染（可见）时取数。hub 变化与定时器只会把 `dirty` 置位并通知，真正的取数由
//! 渲染发起，页面被隐藏（不渲染）期间不会有任何请求；隐藏后再显示若数据已陈旧也会补取一次。

use std::{ops::Range, time::Instant};

use fluxdown_protocol::{
    RpcErrorData, ShareDiagnosticsDto, ShareDiagnosticsParams, ShareLogLevel, ShareLogsDto,
    ShareSessionDetailDto, ShareSessionRole,
};
use fluxdown_ui_components::{
    BusyExt as _, BusyHold, ButtonVariant, ControlExt as _, FluxIcon, card, icon_button,
    loading_icon_button, tabular_numbers, tooltip_view,
};
use fluxdown_ui_i18n::Translator;
use fluxdown_ui_theme::active_theme;
use gpui::{
    Anchor, AnyElement, App, AppContext as _, ClipboardItem, Context, Div, ElementId, Entity,
    FontWeight, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, relative, uniform_list,
};
use gpui_component::{
    Disableable as _, Icon, WindowExt as _,
    button::Button,
    h_flex,
    input::{Input, InputEvent, InputState},
    menu::{DropdownMenu as _, PopupMenuItem},
    notification::Notification,
    scroll::ScrollableElement as _,
    v_flex,
};

use crate::{
    hub::ShareHub,
    manage_ui::{
        body_text, chip, clipped, empty_state, kv_row, meta_text, mono_text, section_header, text,
        text_with, tooltip_builder,
    },
    model::{
        diagnostics::{
            MetricRow, PLACEHOLDER, SessionExpansion, filter_metrics, format_millis,
            ordered_sessions, relay_rows, short_id, snapshot_lines,
        },
        format_datetime,
        logs::{
            LOG_LEVELS, LogRow, is_invalid_regex, log_level_key, log_level_tag, log_row,
            logs_params,
        },
        nat_key, net_health, path_kind_key,
        refresh::{Debounce, GateAction, VisibleRefresh},
        status::NetHealth,
    },
    nested_scroll::NestedScroll,
    session_view::{path_tone, session_detail},
};

/// 两次取数之间的最短间隔（毫秒）：传输进度会让 hub 频繁变化，诊断不必跟得那么紧。
const MIN_REFRESH_MS: u64 = 2_000;
/// 日志正则输入的防抖时间（毫秒）。
const LOG_DEBOUNCE_MS: u64 = 400;
/// 指标表 / 日志 / 快照列表一屏最多显示的行数，更多的在列表内滚动。
const METRIC_ROWS_VISIBLE: usize = 12;
const LOG_ROWS_VISIBLE: usize = 14;
const JSON_ROWS_VISIBLE: usize = 16;
/// 日志行各列宽（默认字号下的 px）。时间列放得下 `HH:MM:SS.mmm` 12 个等宽字符且留余量。
const LOG_TIME_WIDTH: f32 = 100.;
const LOG_LEVEL_WIDTH: f32 = 44.;
const LOG_TARGET_WIDTH: f32 = 180.;
/// 过滤输入框宽度（默认字号下的 px）。
const FILTER_WIDTH: f32 = 240.;
/// 端点地址最多逐条列出的条数。
const MAX_ADDRS: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Load {
    /// 还没有取过。
    Idle,
    Ready,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LogsError {
    None,
    InvalidRegex,
    Failed,
}

struct LogsState {
    level: ShareLogLevel,
    regex: Entity<InputState>,
    regex_placeholder: SharedString,
    rows: Vec<LogRow>,
    loading: bool,
    /// 用户点「刷新日志」后的最短转圈保持。
    hold: BusyHold,
    error: LogsError,
    debounce: Debounce,
    request: u64,
}

/// 网络诊断视图：自管纵向滚动，放进设置页 fill_height 的 Tab。
pub struct ShareNetworkView {
    hub: Entity<ShareHub>,
    translator: Entity<Translator>,
    epoch: Instant,
    /// 只在可见时取数的状态机（hub 变化 / 首次 / 隐藏后再显示）。
    refresh: VisibleRefresh,
    request: u64,
    load: Load,
    loading: bool,
    /// 用户点头部「刷新」后的最短转圈保持。
    refresh_hold: BusyHold,
    /// 用户点「重试」后的最短转圈保持。
    retry_hold: BusyHold,
    diagnostics: Option<ShareDiagnosticsDto>,
    metric_rows: Vec<MetricRow>,
    json_lines: Vec<String>,
    metrics_filter: Entity<InputState>,
    metrics_placeholder: SharedString,
    sessions: SessionExpansion,
    logs: LogsState,
    /// 页内三个定高列表各自的滚动位置与滚轮手势归属（与页面滚动链式衔接）。
    metrics_scroll: NestedScroll,
    logs_scroll: NestedScroll,
    snapshot_scroll: NestedScroll,
}

impl ShareNetworkView {
    /// 创建网络诊断视图；观察 hub 并吸收 [`ShareHub::request_diagnostics`] 的定位请求（含创建前已挂起的）。
    pub fn new(hub: Entity<ShareHub>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let translator = hub.read(cx).translator().clone();
        let level = hub.read(cx).state().local_config.iroh_log_level;
        cx.observe(&hub, |this, _, cx| {
            this.absorb_request(cx);
            this.refresh.mark_dirty();
            cx.notify();
        })
        .detach();
        cx.observe(&translator, |_, _, cx| cx.notify()).detach();

        let metrics_placeholder = text(translator.read(cx), "shareNetMetricsFilter");
        let metrics_filter =
            cx.new(|cx| InputState::new(window, cx).placeholder(metrics_placeholder.clone()));
        cx.subscribe(&metrics_filter, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.rebuild_metrics(cx);
                cx.notify();
            }
        })
        .detach();

        let regex_placeholder = text(translator.read(cx), "shareLogFilterPlaceholder");
        let regex = cx.new(|cx| InputState::new(window, cx).placeholder(regex_placeholder.clone()));
        cx.subscribe(&regex, |this, _, event: &InputEvent, cx| match event {
            InputEvent::Change => this.schedule_logs(cx),
            InputEvent::PressEnter { .. } => this.start_logs(cx),
            _ => {}
        })
        .detach();

        let mut view = Self {
            hub,
            translator,
            epoch: Instant::now(),
            request: 0,
            load: Load::Idle,
            loading: false,
            refresh_hold: BusyHold::default(),
            retry_hold: BusyHold::default(),
            refresh: VisibleRefresh::new(MIN_REFRESH_MS),
            diagnostics: None,
            metric_rows: Vec::new(),
            json_lines: Vec::new(),
            metrics_filter,
            metrics_placeholder,
            sessions: SessionExpansion::default(),
            logs: LogsState {
                level,
                regex,
                regex_placeholder,
                rows: Vec::new(),
                loading: false,
                hold: BusyHold::default(),
                error: LogsError::None,
                debounce: Debounce::default(),
                request: 0,
            },
            metrics_scroll: NestedScroll::new(),
            logs_scroll: NestedScroll::new(),
            snapshot_scroll: NestedScroll::new(),
        };
        // 视图创建前就已挂起的诊断请求（app 先请求再创建设置页）。
        view.absorb_request(cx);
        view
    }

    fn now_ms(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// 吸收 hub 上挂起的诊断请求：带会话 ID 则聚焦并展开该会话，并刷新数据。
    fn absorb_request(&mut self, cx: &mut Context<Self>) {
        let request = self.hub.update(cx, |hub, _| hub.take_diagnostics_request());
        if let Some(Some(session_id)) = request {
            self.focus_session(&session_id, cx);
        }
    }

    fn focus_session(&mut self, session_id: &str, cx: &mut Context<Self>) {
        self.sessions.focus(session_id);
        self.refresh.mark_dirty();
        cx.notify();
    }

    // ───────────────────────── 取数 ─────────────────────────

    /// 渲染时调用：判定是否需要刷新（隐藏后再显示 / hub 变化），经节流后发起取数。
    fn refresh_if_needed(&mut self, cx: &mut Context<Self>) {
        let now = self.now_ms();
        match self.refresh.on_render(now) {
            Some(GateAction::Fetch) => self.start_fetch(cx),
            Some(GateAction::Wait(delay)) => {
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(delay).await;
                    let Ok(()) = this.update(cx, |this, cx| {
                        // 到点只解除排队并请求重绘；页面此刻仍可见才会真正取数。
                        this.refresh.timer_fired();
                        cx.notify();
                    }) else {
                        return;
                    };
                })
                .detach();
            }
            Some(GateAction::Skip) | None => {}
        }
    }

    /// 取诊断与日志（不通知：由渲染或事件处理方负责重绘）。
    fn start_fetch(&mut self, cx: &mut Context<Self>) {
        if !self.hub.read(cx).available() {
            return;
        }
        self.request += 1;
        let request = self.request;
        self.loading = true;
        let future = self
            .hub
            .read(cx)
            .api()
            .diagnostics(&ShareDiagnosticsParams { session_id: None });
        cx.spawn(async move |this, cx| {
            let result = future.await;
            let Ok(()) = this.update(cx, |this, cx| this.finish_fetch(request, result, cx)) else {
                return;
            };
        })
        .detach();
        self.start_logs(cx);
    }

    fn finish_fetch(
        &mut self,
        request: u64,
        result: Result<ShareDiagnosticsDto, RpcErrorData>,
        cx: &mut Context<Self>,
    ) {
        if request != self.request {
            return;
        }
        self.loading = false;
        match result {
            Ok(diagnostics) => {
                self.json_lines = snapshot_lines(&diagnostics.snapshot_json);
                self.diagnostics = Some(diagnostics);
                self.load = Load::Ready;
                self.rebuild_metrics(cx);
            }
            Err(_) => self.load = Load::Failed,
        }
        cx.notify();
    }

    fn rebuild_metrics(&mut self, cx: &App) {
        let query = self.metrics_filter.read(cx).value().to_string();
        self.metric_rows = self
            .diagnostics
            .as_ref()
            .and_then(|diagnostics| diagnostics.snapshot.as_ref())
            .map(|snapshot| filter_metrics(&snapshot.metrics, &query))
            .unwrap_or_default();
    }

    /// 用户点「刷新」：不经节流立即取数。
    fn refresh_now(&mut self, cx: &mut Context<Self>) {
        self.refresh.manual_started(self.now_ms());
        self.start_fetch(cx);
        cx.notify();
    }

    /// 日志取数（级别 / 正则变化或随诊断一起刷新）。
    fn start_logs(&mut self, cx: &mut Context<Self>) {
        if !self.hub.read(cx).available() {
            return;
        }
        self.logs.request += 1;
        let request = self.logs.request;
        self.logs.loading = true;
        let regex = self.logs.regex.read(cx).value().to_string();
        let future = self
            .hub
            .read(cx)
            .api()
            .logs(&logs_params(self.logs.level, &regex));
        cx.spawn(async move |this, cx| {
            let result = future.await;
            let Ok(()) = this.update(cx, |this, cx| this.finish_logs(request, result, cx)) else {
                return;
            };
        })
        .detach();
    }

    fn finish_logs(
        &mut self,
        request: u64,
        result: Result<ShareLogsDto, RpcErrorData>,
        cx: &mut Context<Self>,
    ) {
        if request != self.logs.request {
            return;
        }
        self.logs.loading = false;
        match result {
            Ok(logs) => {
                self.logs.rows = logs.lines.iter().map(log_row).collect();
                self.logs.error = LogsError::None;
            }
            Err(error) if is_invalid_regex(&error) => self.logs.error = LogsError::InvalidRegex,
            Err(_) => self.logs.error = LogsError::Failed,
        }
        cx.notify();
    }

    /// 键入正则：防抖后再取数，期间的继续键入会取代它。
    fn schedule_logs(&mut self, cx: &mut Context<Self>) {
        let generation = self.logs.debounce.bump();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(LOG_DEBOUNCE_MS))
                .await;
            let Ok(()) = this.update(cx, |this, cx| {
                if this.logs.debounce.is_current(generation) {
                    this.start_logs(cx);
                    cx.notify();
                }
            }) else {
                return;
            };
        })
        .detach();
    }

    fn set_level(&mut self, level: ShareLogLevel, cx: &mut Context<Self>) {
        if self.logs.level == level {
            return;
        }
        self.logs.level = level;
        self.start_logs(cx);
        cx.notify();
    }

    // ───────────────────────── 动作 ─────────────────────────

    fn copy(&self, value: String, message_key: &str, window: &mut Window, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(value));
        let message = text(self.translator.read(cx), message_key);
        window.push_notification(Notification::success(message), cx);
    }

    fn sync_placeholders(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let translator = self.translator.read(cx);
        let metrics = text(translator, "shareNetMetricsFilter");
        let regex = text(translator, "shareLogFilterPlaceholder");
        if metrics != self.metrics_placeholder {
            self.metrics_placeholder = metrics.clone();
            self.metrics_filter
                .update(cx, |input, cx| input.set_placeholder(metrics, window, cx));
        }
        if regex != self.logs.regex_placeholder {
            self.logs.regex_placeholder = regex.clone();
            self.logs
                .regex
                .update(cx, |input, cx| input.set_placeholder(regex, window, cx));
        }
    }
}

// ───────────────────────── 渲染 ─────────────────────────

/// 等宽列表的行高。
fn row_height(cx: &App) -> Pixels {
    let theme = active_theme(cx);
    let tokens = theme.tokens();
    tokens.typography.xs.line_height + tokens.spacing.xs
}

fn list_height(count: usize, max_rows: usize, cx: &App) -> Pixels {
    row_height(cx) * count.min(max_rows) as f32
}

fn health_color(health: NetHealth, cx: &App) -> Hsla {
    let theme = active_theme(cx);
    let colors = theme.extended().colors;
    match health {
        NetHealth::Direct => colors.success,
        NetHealth::RelayOnly => colors.warning,
        NetHealth::Offline => theme.tokens().colors.destructive,
        NetHealth::Idle => colors.text_tertiary,
    }
}

fn level_color(level: ShareLogLevel, cx: &App) -> Hsla {
    let theme = active_theme(cx);
    match level {
        ShareLogLevel::Error => theme.tokens().colors.destructive,
        ShareLogLevel::Warn => theme.extended().colors.warning,
        ShareLogLevel::Info => theme.tokens().colors.foreground,
        ShareLogLevel::Debug | ShareLogLevel::Trace => theme.extended().colors.text_tertiary,
    }
}

impl Render for ShareNetworkView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_placeholders(window, cx);
        let tokens = active_theme(cx).tokens().clone();
        let translator = self.translator.read(cx).clone();
        // 功能未启用 / 不可用：不取数，只给空态。
        if !self.hub.read(cx).available() {
            return empty_state(
                FluxIcon::Network,
                None,
                text(&translator, "shareSetUnavailable"),
                None,
                cx,
            )
            .into_any_element();
        }
        self.refresh_if_needed(cx);
        let stale = self.hub.read(cx).is_stale();

        let mut page = v_flex()
            .w_full()
            .flex_1()
            .min_h_0()
            .gap(tokens.spacing.lg)
            .pb(tokens.spacing.lg)
            .child(self.render_header(&translator, stale, cx));

        let snapshot_present = self
            .diagnostics
            .as_ref()
            .is_some_and(|diagnostics| diagnostics.snapshot.is_some());
        match (&self.diagnostics, self.load) {
            (None, Load::Failed) => {
                page = page.child(self.render_failed(&translator, stale, cx));
            }
            (None, _) => {
                page = page.child(empty_state(
                    FluxIcon::Network,
                    None,
                    text(&translator, "shareDiagLoading"),
                    None,
                    cx,
                ));
            }
            (Some(_), _) if !snapshot_present => {
                page = page.child(empty_state(
                    FluxIcon::Network,
                    None,
                    text(&translator, "shareNetUnbound"),
                    None,
                    cx,
                ));
            }
            (Some(_), _) => {
                page = page.child(self.render_endpoint(&translator, cx));
                page = page.child(self.render_relays(&translator, cx));
            }
        }
        if self.diagnostics.is_some() {
            page = page.child(self.render_sessions(&translator, cx));
        }
        if snapshot_present {
            page = page.child(self.render_metrics(&translator, cx));
        }
        page = page.child(self.render_logs(&translator, stale, cx));
        if snapshot_present && !self.json_lines.is_empty() {
            page = page.child(self.render_snapshot(&translator, cx));
        }
        page.overflow_y_scrollbar()
            .id("share-net-scroll")
            .into_any_element()
    }
}

impl ShareNetworkView {
    fn refresh_button(&self, stale: bool, cx: &mut Context<Self>) -> AnyElement {
        let label = text(self.translator.read(cx), "shareNetRefresh");
        let icon = Icon::new(FluxIcon::RotateCw).size(active_theme(cx).extended().icon.md);
        loading_icon_button(
            "share-net-refresh",
            label.clone(),
            icon,
            ButtonVariant::Ghost,
            self.loading || self.refresh_hold.is_held(),
            cx,
        )
        .tooltip(tooltip_builder(label))
        .disabled(stale || self.loading || self.refresh_hold.is_held())
        .on_click(cx.listener(|this, _, _, cx| {
            this.refresh_hold.start(cx);
            this.refresh_now(cx);
        }))
        .into_any_element()
    }

    fn render_header(&self, translator: &Translator, stale: bool, cx: &mut Context<Self>) -> Div {
        let theme = active_theme(cx);
        let tokens = theme.tokens();
        let health = net_health(&self.hub.read(cx).state().net_status);
        let dot = health_color(health, cx);
        h_flex()
            .w_full()
            .items_center()
            .gap(tokens.spacing.sm)
            .child(
                div()
                    .flex_none()
                    .size(tokens.spacing.sm)
                    .rounded_full()
                    .bg(dot),
            )
            .child(
                body_text(cx)
                    .flex_1()
                    .min_w_0()
                    .child(clipped("share-net-health", text(translator, health.key()))),
            )
            .child(self.refresh_button(stale, cx))
    }

    fn render_failed(&self, translator: &Translator, stale: bool, cx: &mut Context<Self>) -> Div {
        let tokens = active_theme(cx).tokens().clone();
        empty_state(
            FluxIcon::CircleAlert,
            Some(tokens.colors.destructive),
            text(translator, "shareNetLoadFailed"),
            None,
            cx,
        )
        .child(
            Button::new("share-net-retry")
                .outline()
                .label(text(translator, "shareRetry"))
                .control(cx)
                .busy(self.loading || self.retry_hold.is_held())
                .disabled(stale || self.loading || self.retry_hold.is_held())
                .on_click(cx.listener(|this, _, _, cx| {
                    this.retry_hold.start(cx);
                    this.refresh_now(cx);
                })),
        )
    }

    fn section(&self, header: Div, body: impl IntoElement, cx: &App) -> Div {
        let tokens = active_theme(cx).tokens().clone();
        card(cx)
            .w_full()
            .flex()
            .flex_col()
            .p(tokens.spacing.md)
            .gap(tokens.spacing.sm)
            .child(header)
            .child(body)
    }

    fn render_endpoint(&self, translator: &Translator, cx: &mut Context<Self>) -> Div {
        let theme = active_theme(cx);
        let tokens = theme.tokens().clone();
        let colors = tokens.colors;
        let net = self.hub.read(cx).state().net_status.clone();
        let Some(snapshot) = self
            .diagnostics
            .as_ref()
            .and_then(|diagnostics| diagnostics.snapshot.as_ref())
        else {
            return div();
        };
        let report = snapshot.net_report.as_ref();
        let nat = report.map_or(net.nat_type, |report| report.nat_type);
        let endpoint_id = snapshot.endpoint_id.clone();
        let copy_id = endpoint_id.clone();
        let copy_label = text(translator, "shareNetCopyEndpoint");

        let mut rows = v_flex().w_full().gap(tokens.spacing.xs);
        rows = rows.child(kv_row(
            text(translator, "shareNetEndpointId"),
            h_flex()
                .w_full()
                .items_center()
                .gap(tokens.spacing.xs)
                .child(
                    mono_text(cx)
                        .flex_1()
                        .min_w_0()
                        .child(clipped("share-net-endpoint-id", endpoint_id)),
                )
                .child(
                    icon_button(
                        "share-net-copy-endpoint",
                        copy_label.clone(),
                        Icon::new(FluxIcon::Copy).size(theme.extended().icon.md),
                        ButtonVariant::Ghost,
                        cx,
                    )
                    .tooltip(tooltip_builder(copy_label))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.copy(copy_id.clone(), "shareNetEndpointCopied", window, cx);
                    })),
                ),
            cx,
        ));
        if !snapshot.client_version.is_empty() || !snapshot.platform.is_empty() {
            let client = [snapshot.client_version.as_str(), snapshot.platform.as_str()]
                .iter()
                .filter(|part| !part.is_empty())
                .copied()
                .collect::<Vec<_>>()
                .join(" · ");
            rows = rows.child(kv_row(
                text(translator, "shareNetClient"),
                clipped("share-net-client", client),
                cx,
            ));
        }
        if let Some(relay) = snapshot.home_relay.as_ref() {
            let (state_key, tone) = if relay.connected {
                ("shareNetConnected", theme.extended().colors.success)
            } else {
                ("shareNetNotConnected", colors.destructive)
            };
            rows = rows.child(kv_row(
                text(translator, "shareNetHomeRelay"),
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(tokens.spacing.sm)
                    .child(
                        mono_text(cx)
                            .flex_1()
                            .min_w_0()
                            .child(clipped("share-net-home-relay", relay.url.clone())),
                    )
                    .child(chip(
                        "share-net-home-relay-state",
                        text(translator, state_key),
                        Some(tone),
                        cx,
                    )),
                cx,
            ));
        }
        rows = rows.child(kv_row(
            text(translator, "shareNetNat"),
            text(translator, nat_key(nat)),
            cx,
        ));
        if let Some(report) = report {
            let mut udp = Vec::new();
            if report.udp_v4 {
                udp.push(text(translator, "shareNetAddrV4"));
            }
            if report.udp_v6 {
                udp.push(text(translator, "shareNetAddrV6"));
            }
            let udp = if udp.is_empty() {
                text(translator, "shareNetUdpNone")
            } else {
                SharedString::from(udp.join(" · "))
            };
            rows = rows.child(kv_row(text(translator, "shareNetUdp"), udp, cx));

            let mut public = v_flex().w_full().gap(tokens.spacing.xxs);
            let mut any_public = false;
            for (index, (label_key, address)) in [
                ("shareNetAddrV4", report.global_v4.as_deref()),
                ("shareNetAddrV6", report.global_v6.as_deref()),
            ]
            .into_iter()
            .enumerate()
            {
                let Some(address) = address.filter(|address| !address.is_empty()) else {
                    continue;
                };
                any_public = true;
                public = public.child(
                    h_flex()
                        .w_full()
                        .gap(tokens.spacing.sm)
                        .child(meta_text(cx).flex_none().child(text(translator, label_key)))
                        .child(mono_text(cx).flex_1().min_w_0().child(clipped(
                            ElementId::NamedInteger(
                                SharedString::from("share-net-public"),
                                index as u64,
                            ),
                            address.to_owned(),
                        ))),
                );
            }
            if !any_public {
                public = public.child(body_text(cx).child(SharedString::from(PLACEHOLDER)));
            }
            rows = rows.child(kv_row(text(translator, "shareNetGlobalAddrs"), public, cx));
        }
        let carrier = [net.isp.as_deref(), net.province.as_deref()]
            .into_iter()
            .flatten()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
        rows = rows.child(kv_row(
            text(translator, "shareNetCarrier"),
            clipped(
                "share-net-carrier",
                if carrier.is_empty() {
                    PLACEHOLDER.to_owned()
                } else {
                    carrier
                },
            ),
            cx,
        ));
        if !snapshot.addrs.is_empty() {
            let mut addrs = v_flex().w_full().gap(tokens.spacing.xxs);
            for (index, address) in snapshot.addrs.iter().take(MAX_ADDRS).enumerate() {
                addrs = addrs.child(mono_text(cx).child(clipped(
                    ElementId::NamedInteger(SharedString::from("share-net-addr"), index as u64),
                    address.clone(),
                )));
            }
            rows = rows.child(kv_row(text(translator, "shareNetLocalAddrs"), addrs, cx));
        }
        if report.and_then(|report| report.captive_portal) == Some(true) {
            rows = rows.child(
                div()
                    .text_size(tokens.typography.xs.size)
                    .line_height(tokens.typography.xs.line_height)
                    .text_color(theme.extended().colors.warning)
                    .child(text(translator, "shareNetCaptivePortal")),
            );
        }

        let captured = (!snapshot.captured_at.is_empty()).then(|| {
            text_with(
                translator,
                "shareNetCapturedAt",
                &[("time", &format_datetime(&snapshot.captured_at))],
            )
        });
        self.section(
            section_header(text(translator, "shareNetTitle"), None, None, cx),
            v_flex()
                .w_full()
                .gap(tokens.spacing.sm)
                .child(rows)
                .when_some(captured, |this, captured| {
                    this.child(meta_text(cx).child(captured))
                }),
            cx,
        )
    }

    fn render_relays(&self, translator: &Translator, cx: &mut Context<Self>) -> Div {
        let tokens = active_theme(cx).tokens().clone();
        let net = self.hub.read(cx).state().net_status.clone();
        let Some(snapshot) = self
            .diagnostics
            .as_ref()
            .and_then(|diagnostics| diagnostics.snapshot.as_ref())
        else {
            return div();
        };
        let report = snapshot.net_report.as_ref();
        let latency = report
            .map(|report| &report.relay_latency_ms)
            .filter(|latency| !latency.is_empty())
            .unwrap_or(&net.relay_latency_ms);
        let home = snapshot
            .home_relay
            .as_ref()
            .map(|relay| relay.url.as_str())
            .or_else(|| net.home_relay.as_ref().map(|relay| relay.url.as_str()));
        let preferred = report.and_then(|report| report.preferred_relay.as_deref());
        let relays = relay_rows(latency, preferred, home);

        let body = if relays.is_empty() {
            v_flex().child(meta_text(cx).child(text(translator, "shareNetNoRelays")))
        } else {
            v_flex()
                .w_full()
                .gap(tokens.spacing.xs)
                .children(relays.into_iter().enumerate().map(|(index, relay)| {
                    let row_id = |part: &str| {
                        ElementId::NamedInteger(
                            SharedString::from(format!("share-net-relay-{part}")),
                            index as u64,
                        )
                    };
                    h_flex()
                        .w_full()
                        .items_center()
                        .gap(tokens.spacing.sm)
                        .child(
                            mono_text(cx)
                                .flex_1()
                                .min_w_0()
                                .child(clipped(row_id("url"), relay.url.clone())),
                        )
                        .when(relay.preferred, |row| {
                            row.child(chip(
                                row_id("preferred"),
                                text(translator, "shareNetPreferredRelay"),
                                Some(active_theme(cx).extended().colors.accent_text),
                                cx,
                            ))
                        })
                        .when(relay.home, |row| {
                            row.child(chip(
                                row_id("home"),
                                text(translator, "shareNetHomeRelay"),
                                None,
                                cx,
                            ))
                        })
                        .child(
                            div()
                                .flex_none()
                                .font_features(tabular_numbers())
                                .text_size(tokens.typography.sm.size)
                                .line_height(tokens.typography.sm.line_height)
                                .text_color(tokens.colors.foreground)
                                .child(SharedString::from(format_millis(relay.latency_ms))),
                        )
                }))
        };
        self.section(
            section_header(text(translator, "shareNetRelays"), None, None, cx),
            body,
            cx,
        )
    }

    fn render_sessions(&self, translator: &Translator, cx: &mut Context<Self>) -> Div {
        let tokens = active_theme(cx).tokens().clone();
        let sessions: &[ShareSessionDetailDto] = self
            .diagnostics
            .as_ref()
            .map_or(&[], |diagnostics| diagnostics.sessions.as_slice());
        let count = sessions.len();
        let body = if sessions.is_empty() {
            v_flex().child(meta_text(cx).child(text(translator, "shareNetSessionsEmpty")))
        } else {
            v_flex().w_full().gap(tokens.spacing.sm).children(
                ordered_sessions(sessions)
                    .into_iter()
                    .map(|session| self.render_session(session, translator, cx)),
            )
        };
        self.section(
            section_header(
                text(translator, "shareNetSessions"),
                (count > 0).then(|| SharedString::from(count.to_string())),
                None,
                cx,
            ),
            body,
            cx,
        )
    }

    fn render_session(
        &self,
        session: &ShareSessionDetailDto,
        translator: &Translator,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = active_theme(cx);
        let tokens = theme.tokens().clone();
        let colors = tokens.colors;
        let session_id = session.session_id.clone();
        let expanded = self.sessions.is_expanded(&session.session_id);
        let focused = self.sessions.is_focused(&session.session_id);
        let scope = format!("share-net-session-{}", session.session_id);
        let peer = session
            .peer_device_name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .map_or_else(|| short_id(&session.peer_endpoint_id), str::to_owned);
        let role_key = match session.role {
            ShareSessionRole::Sender => "shareNetRoleSender",
            ShareSessionRole::Receiver => "shareNetRoleReceiver",
            ShareSessionRole::Swarm => "shareNetRoleSwarm",
        };
        let bytes = SharedString::from(crate::session_view::bytes_summary(
            translator,
            &session.summary.bytes,
        ));
        let (state_key, state_tone) = if session.ended {
            ("shareNetSessionEnded", None)
        } else {
            (
                "shareNetSessionActive",
                Some(theme.extended().colors.success),
            )
        };
        let toggle_id = session_id.clone();
        let peer_meta = [
            Some(text(translator, nat_key(session.peer_nat_type)).to_string()),
            session
                .peer_isp
                .as_deref()
                .filter(|isp| !isp.is_empty())
                .map(str::to_owned),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
        let header = h_flex()
            .id(SharedString::from(format!("{scope}-header")))
            .w_full()
            .items_center()
            .gap(tokens.spacing.sm)
            .px(tokens.spacing.sm)
            .py(tokens.spacing.xs)
            .rounded(tokens.radius.md)
            .cursor_pointer()
            .hover(move |style| style.bg(theme.extended().colors.row_hover))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.sessions.toggle(&toggle_id);
                cx.notify();
            }))
            .child(
                Icon::new(if expanded {
                    FluxIcon::ChevronDown
                } else {
                    FluxIcon::ChevronRight
                })
                .size(theme.extended().icon.md)
                .text_color(colors.muted_foreground),
            )
            .child(chip(
                SharedString::from(format!("{scope}-role")),
                text(translator, role_key),
                None,
                cx,
            ))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        body_text(cx)
                            .child(clipped(SharedString::from(format!("{scope}-peer")), peer)),
                    )
                    .child(meta_text(cx).child(clipped(
                        SharedString::from(format!("{scope}-peer-meta")),
                        peer_meta,
                    ))),
            )
            .child(chip(
                SharedString::from(format!("{scope}-kind")),
                text(translator, path_kind_key(session.summary.path_kind)),
                path_tone(session.summary.path_kind, cx),
                cx,
            ))
            .child(
                meta_text(cx)
                    .flex_none()
                    .max_w(theme.text_extent(240.))
                    .font_features(tabular_numbers())
                    .child(clipped(SharedString::from(format!("{scope}-bytes")), bytes)),
            )
            .child(chip(
                SharedString::from(format!("{scope}-state")),
                text(translator, state_key),
                state_tone,
                cx,
            ));

        let mut wrapper = v_flex()
            .w_full()
            .gap(tokens.spacing.sm)
            .rounded(tokens.radius.md)
            .border_1()
            .border_color(if focused {
                colors.primary
            } else {
                gpui::transparent_black()
            })
            .when(focused, |this| this.bg(colors.accent))
            .child(header);
        if expanded {
            wrapper = wrapper.child(
                div()
                    .w_full()
                    .px(tokens.spacing.md)
                    .pb(tokens.spacing.sm)
                    .child(session_detail(session, translator, &scope, cx)),
            );
        }
        wrapper
    }

    fn render_metrics(&self, translator: &Translator, cx: &mut Context<Self>) -> Div {
        let theme = active_theme(cx);
        let tokens = theme.tokens().clone();
        let total = self
            .diagnostics
            .as_ref()
            .and_then(|diagnostics| diagnostics.snapshot.as_ref())
            .map_or(0, |snapshot| snapshot.metrics.len());
        let shown = self.metric_rows.len();
        let count = text_with(
            translator,
            "shareNetMetricsCount",
            &[("shown", &shown.to_string()), ("total", &total.to_string())],
        );
        let filter = Input::new(&self.metrics_filter)
            .control(cx)
            .w(theme.text_extent(FILTER_WIDTH))
            .prefix(
                Icon::new(FluxIcon::Search)
                    .size(theme.extended().icon.md)
                    .text_color(theme.extended().colors.text_tertiary),
            )
            .into_any_element();

        let header = section_header(
            text(translator, "shareNetMetrics"),
            Some(count),
            Some(filter),
            cx,
        );
        let body = if shown == 0 {
            v_flex().child(meta_text(cx).child(text(translator, "shareNetMetricsEmpty")))
        } else {
            v_flex()
                .w_full()
                .child(
                    h_flex()
                        .w_full()
                        .gap(tokens.spacing.md)
                        .pb(tokens.spacing.xxs)
                        .child(
                            meta_text(cx)
                                .w(relative(0.6))
                                .flex_none()
                                .child(text(translator, "shareNetMetricName")),
                        )
                        .child(
                            meta_text(cx)
                                .flex_1()
                                .child(text(translator, "shareNetMetricValue")),
                        ),
                )
                .child(self.metrics_scroll.area(
                    "share-net-metrics-area",
                    uniform_list(
                        "share-net-metrics",
                        shown,
                        cx.processor(move |this, range: Range<usize>, _, cx| {
                            let height = row_height(cx);
                            range
                                .filter_map(|index| this.metric_rows.get(index))
                                .map(|row| {
                                    h_flex()
                                        .w_full()
                                        .h(height)
                                        .items_center()
                                        .gap(active_theme(cx).tokens().spacing.md)
                                        .child(
                                            mono_text(cx)
                                                .w(relative(0.6))
                                                .flex_none()
                                                .truncate()
                                                .child(row.key.clone()),
                                        )
                                        .child(
                                            mono_text(cx)
                                                .flex_1()
                                                .min_w_0()
                                                .truncate()
                                                .font_features(tabular_numbers())
                                                .child(row.value.clone()),
                                        )
                                })
                                .collect::<Vec<_>>()
                        }),
                    ),
                    list_height(shown, METRIC_ROWS_VISIBLE, cx),
                ))
        };
        self.section(header, body, cx)
    }

    fn level_dropdown(&self, translator: &Translator, cx: &mut Context<Self>) -> AnyElement {
        let current = self.logs.level;
        let this = cx.weak_entity();
        let options: Vec<(ShareLogLevel, SharedString)> = LOG_LEVELS
            .into_iter()
            .map(|level| (level, text(translator, log_level_key(level))))
            .collect();
        Button::new("share-net-log-level")
            .outline()
            .label(text(translator, log_level_key(current)))
            .dropdown_caret(true)
            .control(cx)
            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                options.iter().fold(menu, |menu, (level, label)| {
                    let level = *level;
                    let this = this.clone();
                    menu.item(
                        PopupMenuItem::new(label.clone())
                            .checked(level == current)
                            .on_click(move |_, _, cx| {
                                let Ok(()) = this.update(cx, |this, cx| this.set_level(level, cx))
                                else {
                                    // 视图已释放，结束这次回调而不再更新状态。
                                    return;
                                };
                            }),
                    )
                })
            })
            .into_any_element()
    }

    fn render_logs(&self, translator: &Translator, stale: bool, cx: &mut Context<Self>) -> Div {
        let (tokens, filter_width, icon_md, tertiary) = {
            let theme = active_theme(cx);
            (
                theme.tokens().clone(),
                theme.text_extent(FILTER_WIDTH),
                theme.extended().icon.md,
                theme.extended().colors.text_tertiary,
            )
        };
        let colors = tokens.colors;
        let label_refresh = text(translator, "shareLogsRefresh");
        let controls = h_flex()
            .items_center()
            .gap(tokens.spacing.sm)
            .child(meta_text(cx).child(text(translator, "shareLogLevelLabel")))
            .child(self.level_dropdown(translator, cx))
            .child(
                Input::new(&self.logs.regex)
                    .control(cx)
                    .w(filter_width)
                    .prefix(
                        Icon::new(FluxIcon::ListFilter)
                            .size(icon_md)
                            .text_color(tertiary),
                    ),
            )
            .child(
                loading_icon_button(
                    "share-net-logs-refresh",
                    label_refresh.clone(),
                    Icon::new(FluxIcon::RotateCw).size(icon_md),
                    ButtonVariant::Ghost,
                    self.logs.loading || self.logs.hold.is_held(),
                    cx,
                )
                .tooltip(tooltip_builder(label_refresh))
                .disabled(stale || self.logs.loading || self.logs.hold.is_held())
                .on_click(cx.listener(|this, _, _, cx| {
                    this.logs.hold.start(cx);
                    this.start_logs(cx);
                    cx.notify();
                })),
            )
            .into_any_element();

        let count = self.logs.rows.len();
        let header = section_header(
            text(translator, "shareNetLogs"),
            (count > 0).then(|| {
                text_with(
                    translator,
                    "shareNetLogsCount",
                    &[("count", &count.to_string())],
                )
            }),
            Some(controls),
            cx,
        );
        let error = match self.logs.error {
            LogsError::None => None,
            LogsError::InvalidRegex => Some("shareInvalidRegex"),
            LogsError::Failed => Some("shareNetLoadFailed"),
        };
        let body = v_flex()
            .w_full()
            .gap(tokens.spacing.xs)
            .when_some(error, |this, key| {
                this.child(
                    div()
                        .text_size(tokens.typography.xs.size)
                        .line_height(tokens.typography.xs.line_height)
                        .text_color(colors.destructive)
                        .child(text(translator, key)),
                )
            })
            .child(if count == 0 {
                v_flex().child(meta_text(cx).child(text(translator, "shareLogsEmpty")))
            } else {
                v_flex().w_full().child(self.logs_scroll.area(
                    "share-net-logs-area",
                    uniform_list(
                        "share-net-logs",
                        count,
                        cx.processor(move |this, range: Range<usize>, _, cx| {
                            let theme = active_theme(cx);
                            let height = row_height(cx);
                            range
                                .filter_map(|index| {
                                    this.logs.rows.get(index).map(|row| (index, row))
                                })
                                .map(|(index, row)| {
                                    let full = SharedString::from(row.text());
                                    h_flex()
                                        .id(ElementId::NamedInteger(
                                            SharedString::from("share-net-log"),
                                            index as u64,
                                        ))
                                        .w_full()
                                        .h(height)
                                        .items_center()
                                        .gap(theme.tokens().spacing.md)
                                        .tooltip(move |window, cx| {
                                            tooltip_view(full.clone(), window, cx)
                                        })
                                        .child(
                                            mono_text(cx)
                                                .w(theme.text_extent(LOG_TIME_WIDTH))
                                                .flex_none()
                                                .whitespace_nowrap()
                                                .font_features(tabular_numbers())
                                                .text_color(theme.extended().colors.text_tertiary)
                                                .child(row.time.clone()),
                                        )
                                        .child(
                                            mono_text(cx)
                                                .w(theme.text_extent(LOG_LEVEL_WIDTH))
                                                .flex_none()
                                                .whitespace_nowrap()
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(level_color(row.level, cx))
                                                .child(log_level_tag(row.level)),
                                        )
                                        .child(
                                            mono_text(cx)
                                                .w(theme.text_extent(LOG_TARGET_WIDTH))
                                                .flex_none()
                                                .truncate()
                                                .text_color(theme.tokens().colors.muted_foreground)
                                                .child(row.target.clone()),
                                        )
                                        .child(
                                            mono_text(cx)
                                                .flex_1()
                                                .min_w_0()
                                                .truncate()
                                                .child(row.message.clone()),
                                        )
                                })
                                .collect::<Vec<_>>()
                        }),
                    ),
                    list_height(count, LOG_ROWS_VISIBLE, cx),
                ))
            });
        self.section(header, body, cx)
    }

    fn render_snapshot(&self, translator: &Translator, cx: &mut Context<Self>) -> Div {
        let theme = active_theme(cx);
        let label = text(translator, "shareCopySnapshot");
        let copy = Button::new("share-net-copy-snapshot")
            .outline()
            .icon(FluxIcon::Copy)
            .label(label)
            .control(cx)
            .on_click(cx.listener(move |this, _, window, cx| {
                let json = this
                    .diagnostics
                    .as_ref()
                    .map(|diagnostics| diagnostics.snapshot_json.clone())
                    .unwrap_or_default();
                this.copy(json, "shareSnapshotCopied", window, cx);
            }))
            .into_any_element();
        let lines = self.json_lines.len();
        let header = section_header(text(translator, "shareNetSnapshot"), None, Some(copy), cx);
        let body = div()
            .w_full()
            .rounded(theme.tokens().radius.md)
            .bg(theme.tokens().colors.muted)
            .px(theme.tokens().spacing.sm)
            .child(self.snapshot_scroll.area(
                "share-net-snapshot-area",
                uniform_list(
                    "share-net-snapshot",
                    lines,
                    cx.processor(move |this, range: Range<usize>, _, cx| {
                        let height = row_height(cx);
                        range
                            .filter_map(|index| this.json_lines.get(index))
                            .map(|line| {
                                mono_text(cx)
                                    .w_full()
                                    .h(height)
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .child(line.clone())
                            })
                            .collect::<Vec<_>>()
                    }),
                ),
                list_height(lines, JSON_ROWS_VISIBLE, cx),
            ));
        self.section(header, body, cx)
    }
}
