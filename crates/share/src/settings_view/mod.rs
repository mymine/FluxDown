//! 客户端设置「FluxMesh」页：渲染 `ShareLocalConfigDto` 的全部字段。
//!
//! 页面由 `crates/settings` 作为内容槽位托管（不依赖本 crate），所以行 / 分组的外观在这里
//! 按 `crates/settings/src/ui.rs` 的同一套 token 本地复刻：标题 + 描述在左，控件在右。
//!
//! 持久化：`agent.share.setLocalConfig` 整体替换配置。视图始终以 [`ShareSettingsView::shown`]
//! （无在途请求时就是 hub 里的当前值）为底稿改一个字段后整体提交；开关 / 下拉立即提交，
//! 文本输入防抖 [`TEXT_DEBOUNCE`]（失焦 / 回车立即提交）。失败时弹出带类型的错误文案并把输入回显成 hub 的值。

mod commit;
mod controls;
mod groups;
mod render;
mod rows;

use std::time::Duration;

use fluxdown_protocol::ShareLocalConfigDto;
use fluxdown_ui_i18n::Translator;
use gpui::{AppContext as _, Context, Entity, Subscription, Window};
use gpui_component::input::{InputEvent, InputState, TextareaState};

use crate::{
    hub::ShareHub,
    model::format_bytes,
    model::settings_form::{
        RateUnit, TextEdits, daily_cap_text, extra_addrs_text, port_text, preferred_rate_unit,
        rate_text,
    },
};

/// 文本输入的提交防抖。
const TEXT_DEBOUNCE: Duration = Duration::from_millis(500);
/// 行内数字输入 / 下拉的宽度档位（基准 px，经 `text_extent` 随字号缩放），与设置页同档。
pub(super) const NUMBER_WIDTH: f32 = 160.;
pub(super) const DROPDOWN_MIN_WIDTH: f32 = 160.;
pub(super) const DROPDOWN_MAX_WIDTH: f32 = 320.;
pub(super) const UNIT_DROPDOWN_WIDTH: f32 = 88.;
/// 横排行最小行高 = 控件高 + 上下各 10px 留白。
pub(super) const ROW_MIN_PADDING: f32 = 20.;
/// 下拉里单个条目 / 触发器显示的最大字符数（超出中间省略）。
pub(super) const OPTION_MAX_CHARS: usize = 44;
/// 额外地址输入框的自动增高行数范围。
pub(super) const EXTRA_ADDRS_ROWS: (usize, usize) = (2, 5);
/// 端口输入最多 5 位；限速输入最多 12 个字符（含小数点）。
pub(super) const PORT_MAX_CHARS: usize = 5;
pub(super) const RATE_MAX_CHARS: usize = 12;
/// 每日上限输入最多 12 个字符（含小数点）。
pub(super) const DAILY_CAP_MAX_CHARS: usize = 12;

pub struct ShareSettingsView {
    hub: Entity<ShareHub>,
    translator: Entity<Translator>,
    /// 当前展示 / 提交底稿：无在途请求时等于 hub 的 `local_config`，提交后先乐观更新，
    /// 返回时换成 agent 规范化后的值。
    shown: ShareLocalConfigDto,
    /// 最近一次见到的 hub 值：只有它变化才重新同步（避免传输进度事件反复覆盖正在输入的文本）。
    hub_seen: ShareLocalConfigDto,
    udp_v4: Entity<InputState>,
    udp_v6: Entity<InputState>,
    extra_addrs: Entity<TextareaState>,
    rate: Entity<InputState>,
    daily_cap: Entity<InputState>,
    rate_unit: RateUnit,
    /// 最近一次提交时的文本校验结果（只用其错误标记）。
    text_errors: Option<TextEdits>,
    text_generation: u64,
    /// 有尚未提交的文本编辑：此时不用 hub 的值覆盖输入框。
    text_pending: bool,
    commit_seq: u64,
    inflight: u32,
    _subscriptions: Vec<Subscription>,
}

impl ShareSettingsView {
    pub fn new(hub: Entity<ShareHub>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let translator = hub.read(cx).translator().clone();
        let config = hub.read(cx).state().local_config.clone();
        let rate_unit =
            preferred_rate_unit(config.send_rate_limit_bytes_per_sec, RateUnit::MbPerSec);
        let (port_hint, addrs_hint, rate_hint) = {
            let translator = translator.read(cx);
            (
                translator.text("shareSetPortPlaceholder").to_owned(),
                translator.text("shareSetExtraAddrsPlaceholder").to_owned(),
                translator.text("shareSetRatePlaceholder").to_owned(),
            )
        };
        let port_input = |value: Option<u16>, window: &mut Window, cx: &mut Context<Self>| {
            let hint = port_hint.clone();
            cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(port_text(value))
                    .placeholder(hint)
                    .validate(|text, _| {
                        text.len() <= PORT_MAX_CHARS && text.bytes().all(|b| b.is_ascii_digit())
                    })
            })
        };
        let udp_v4 = port_input(config.udp_port_v4, window, cx);
        let udp_v6 = port_input(config.udp_port_v6, window, cx);
        let rate = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(rate_text(config.send_rate_limit_bytes_per_sec, rate_unit))
                .placeholder(rate_hint)
                .validate(|text, _| {
                    text.len() <= RATE_MAX_CHARS
                        && text.bytes().all(|b| b.is_ascii_digit() || b == b'.')
                })
        });
        let daily_cap_hint = daily_cap_hint(
            translator.read(cx),
            hub.read(cx)
                .state()
                .policy_summary
                .provide_daily_cap_default_bytes,
        );
        let daily_cap = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(daily_cap_text(config.provide_daily_cap_bytes))
                .placeholder(daily_cap_hint)
                .validate(|text, _| {
                    text.len() <= DAILY_CAP_MAX_CHARS
                        && text.bytes().all(|b| b.is_ascii_digit() || b == b'.')
                })
        });
        let extra_addrs = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(EXTRA_ADDRS_ROWS.0, EXTRA_ADDRS_ROWS.1)
                .default_value(extra_addrs_text(&config.extra_addrs))
                .placeholder(addrs_hint)
        });

        let mut subscriptions = vec![
            cx.observe_in(&hub, window, |this, hub, window, cx| {
                let incoming = hub.read(cx).state().local_config.clone();
                if incoming != this.hub_seen {
                    this.hub_seen.clone_from(&incoming);
                    if this.inflight == 0 {
                        this.shown.clone_from(&incoming);
                        if !this.text_pending {
                            this.sync_inputs(&incoming, window, cx);
                        }
                    }
                }
                this.refresh_daily_cap_hint(window, cx);
                cx.notify();
            }),
            cx.observe_in(&translator, window, |this, translator, window, cx| {
                let (port_hint, addrs_hint, rate_hint) = {
                    let translator = translator.read(cx);
                    (
                        translator.text("shareSetPortPlaceholder").to_owned(),
                        translator.text("shareSetExtraAddrsPlaceholder").to_owned(),
                        translator.text("shareSetRatePlaceholder").to_owned(),
                    )
                };
                for port in [&this.udp_v4, &this.udp_v6] {
                    let hint = port_hint.clone();
                    port.update(cx, |input, cx| input.set_placeholder(hint, window, cx));
                }
                this.rate
                    .update(cx, |input, cx| input.set_placeholder(rate_hint, window, cx));
                this.extra_addrs.update(cx, |input, cx| {
                    input.set_placeholder(addrs_hint, window, cx)
                });
                this.refresh_daily_cap_hint(window, cx);
                cx.notify();
            }),
        ];
        for input in [&udp_v4, &udp_v6, &rate, &daily_cap] {
            subscriptions.push(cx.subscribe_in(
                input,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    this.on_text_event(event, true, window, cx)
                },
            ));
        }
        subscriptions.push(cx.subscribe_in(
            &extra_addrs,
            window,
            |this, _, event: &InputEvent, window, cx| this.on_text_event(event, false, window, cx),
        ));

        Self {
            hub,
            translator,
            hub_seen: config.clone(),
            shown: config,
            udp_v4,
            udp_v6,
            extra_addrs,
            rate,
            daily_cap,
            rate_unit,
            text_errors: None,
            text_generation: 0,
            text_pending: false,
            commit_seq: 0,
            inflight: 0,
            _subscriptions: subscriptions,
        }
    }
}

/// 每日上限输入的占位文字：「默认（{云端缺省}）」；缺省为 0 = 不限。
fn daily_cap_hint(translator: &Translator, default_bytes: u64) -> String {
    let value = if default_bytes == 0 {
        translator.text("shareUsageUnlimited").to_owned()
    } else {
        format_bytes(default_bytes)
    };
    translator.text_with("shareSetProvideDailyCapDefault", &[("value", &value)])
}

impl ShareSettingsView {
    fn refresh_daily_cap_hint(&self, window: &mut Window, cx: &mut Context<Self>) {
        let hint = daily_cap_hint(
            self.translator.read(cx),
            self.hub
                .read(cx)
                .state()
                .policy_summary
                .provide_daily_cap_default_bytes,
        );
        self.daily_cap
            .update(cx, |input, cx| input.set_placeholder(hint, window, cx));
    }
}
