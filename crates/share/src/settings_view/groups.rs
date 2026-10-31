//! 四个设置分组：默认值 / 网络 / 传输 / 诊断。各自构建控件并装配成行。

use fluxdown_protocol::SharePolicySummaryDto;
use fluxdown_ui_components::ControlExt as _;
use fluxdown_ui_i18n::Translator;
use fluxdown_ui_theme::active_theme;
use gpui::{
    AnyElement, Context, IntoElement as _, ParentElement as _, SharedString, Styled as _, Window,
    div,
};
use gpui_component::{
    h_flex,
    input::{Input, Textarea},
};

use super::{
    NUMBER_WIDTH, OPTION_MAX_CHARS, ShareSettingsView, UNIT_DROPDOWN_WIDTH,
    controls::{exit_value, log_level_value, mode_value, option, portmapper_value},
    rows::{RowSpec, setting_card, setting_group, setting_row, text},
};
use crate::model::{
    claims_label,
    settings_form::{
        self, RateUnit, RelayChoice, TierChoice, relay_choices, relay_label, tier_choices,
        truncate_middle,
    },
};

/// 构建一个设置分组所需的只读上下文。
pub(super) struct GroupCtx<'a> {
    pub translator: &'a Translator,
    pub policy: &'a SharePolicySummaryDto,
}

impl ShareSettingsView {
    /// 「启用 FluxMesh（实验性）」总开关：功能关闭时页面里唯一的内容。
    pub(super) fn enable_card(
        &self,
        g: &GroupCtx,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let translator = g.translator;
        let enabled = self.switch_with(
            "share-set-enabled",
            self.shown.enabled,
            !self.connected(cx),
            Self::set_enabled,
            cx,
        );
        let rows = vec![setting_row(
            RowSpec::new(
                translator,
                "enabled",
                "shareSetEnabled",
                "shareSetEnabledDesc",
                enabled,
            ),
            window,
            cx,
        )];
        setting_card(rows, cx)
    }

    pub(super) fn defaults_group(
        &self,
        g: &GroupCtx,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let translator = g.translator;
        let config = self.shown.clone();
        let tier_options: Vec<_> =
            tier_choices(&g.policy.live_tiers, config.default_tier_id.as_deref())
                .into_iter()
                .map(|choice| {
                    let label = match &choice {
                        TierChoice::FollowFirst => translator.text("shareSetTierFollow").to_owned(),
                        TierChoice::Tier { label, .. } => {
                            translator.text_with(label.key, &[("count", &label.count.to_string())])
                        }
                        TierChoice::Missing { id } => truncate_middle(id, OPTION_MAX_CHARS),
                    };
                    option(choice.value().to_owned(), label)
                })
                .collect();
        let tier = self.dropdown(
            "share-set-tier",
            tier_options,
            SharedString::from(config.default_tier_id.clone().unwrap_or_default()),
            false,
            Self::set_default_tier,
            cx,
        );
        let claims_options: Vec<_> = settings_form::claims_choices(config.default_max_claims)
            .into_iter()
            .map(|claims| {
                let label = claims_label(claims);
                option(
                    claims.to_string(),
                    translator.text_with(label.key, &[("count", &label.count.to_string())]),
                )
            })
            .collect();
        let claims = self.dropdown(
            "share-set-claims",
            claims_options,
            SharedString::from(config.default_max_claims.to_string()),
            false,
            Self::set_default_claims,
            cx,
        );
        let notify = self.switch(
            "share-set-notify",
            config.default_notify_on_claim,
            Self::set_default_notify,
            cx,
        );
        let swarm = self.switch(
            "share-set-swarm",
            config.default_swarm.unwrap_or(true),
            Self::set_default_swarm,
            cx,
        );
        let rows = vec![
            setting_row(
                RowSpec::new(
                    translator,
                    "tier",
                    "shareSetDefaultTier",
                    "shareSetDefaultTierDesc",
                    tier,
                ),
                window,
                cx,
            ),
            setting_row(
                RowSpec::new(
                    translator,
                    "claims",
                    "shareSetDefaultClaims",
                    "shareSetDefaultClaimsDesc",
                    claims,
                ),
                window,
                cx,
            ),
            setting_row(
                RowSpec::new(
                    translator,
                    "notify",
                    "shareSetDefaultNotify",
                    "shareSetDefaultNotifyDesc",
                    notify,
                ),
                window,
                cx,
            ),
            setting_row(
                RowSpec::new(
                    translator,
                    "swarm",
                    "shareSetDefaultSwarm",
                    "shareSetDefaultSwarmDesc",
                    swarm,
                ),
                window,
                cx,
            ),
        ];
        setting_group(text(translator, "shareSetGroupDefaults"), None, rows, cx)
    }

    pub(super) fn network_group(
        &self,
        g: &GroupCtx,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let translator = g.translator;
        let config = self.shown.clone();
        let tokens = active_theme(cx).tokens().clone();
        let errors = self.text_errors.as_ref();
        let port_error = |invalid: bool| invalid.then(|| text(translator, "shareSetErrPort"));
        let udp_v4 = self.port_input(&self.udp_v4, cx);
        let udp_v6 = self.port_input(&self.udp_v6, cx);
        let portmapper = self.dropdown(
            "share-set-portmapper",
            vec![
                option("auto", text(translator, "shareSetPortmapperAuto")),
                option("on", text(translator, "shareSetPortmapperOn")),
                option("off", text(translator, "shareSetPortmapperOff")),
            ],
            SharedString::from(portmapper_value(config.portmapper)),
            false,
            Self::set_portmapper,
            cx,
        );
        let extra_addrs = Textarea::new(&self.extra_addrs)
            .w_full()
            .text_size(tokens.typography.sm.size)
            .line_height(tokens.typography.sm.line_height)
            .disabled(!self.editable(cx))
            .into_any_element();
        let extra_error = errors
            .and_then(|edits| edits.extra_addrs_invalid.as_ref())
            .map(|invalid| {
                SharedString::from(
                    translator.text_with("shareSetErrAddr", &[("entry", &invalid.entry)]),
                )
            });
        let mode = self.dropdown(
            "share-set-mode",
            vec![
                option("auto", text(translator, "shareSetModeAuto")),
                option("relayOnly", text(translator, "shareSetModeRelayOnly")),
                option("directOnly", text(translator, "shareSetModeDirectOnly")),
            ],
            SharedString::from(mode_value(config.mode)),
            false,
            Self::set_mode,
            cx,
        );
        let relay_options: Vec<_> = relay_choices(&g.policy.relays, config.pinned_relay.as_deref())
            .into_iter()
            .map(|choice| {
                let label = match &choice {
                    RelayChoice::Auto => translator.text("shareSetRelayAuto").to_owned(),
                    RelayChoice::Relay { url, region } => {
                        relay_label(url, region, OPTION_MAX_CHARS)
                    }
                    RelayChoice::Missing { url } => truncate_middle(url, OPTION_MAX_CHARS),
                };
                option(choice.value().to_owned(), label)
            })
            .collect();
        let relay = self.dropdown(
            "share-set-relay",
            relay_options,
            SharedString::from(config.pinned_relay.clone().unwrap_or_default()),
            false,
            Self::set_relay,
            cx,
        );
        let rows = vec![
            setting_row(
                RowSpec::new(
                    translator,
                    "udp-v4",
                    "shareSetUdpV4",
                    "shareSetUdpV4Desc",
                    udp_v4,
                )
                .error(port_error(errors.is_some_and(|edits| edits.udp_v4_invalid))),
                window,
                cx,
            ),
            setting_row(
                RowSpec::new(
                    translator,
                    "udp-v6",
                    "shareSetUdpV6",
                    "shareSetUdpV6Desc",
                    udp_v6,
                )
                .error(port_error(errors.is_some_and(|edits| edits.udp_v6_invalid))),
                window,
                cx,
            ),
            setting_row(
                RowSpec::new(
                    translator,
                    "portmapper",
                    "shareSetPortmapper",
                    "shareSetPortmapperDesc",
                    portmapper,
                ),
                window,
                cx,
            ),
            setting_row(
                RowSpec::new(
                    translator,
                    "extra-addrs",
                    "shareSetExtraAddrs",
                    "shareSetExtraAddrsDesc",
                    extra_addrs,
                )
                .error(extra_error)
                .vertical(),
                window,
                cx,
            ),
            setting_row(
                RowSpec::new(translator, "mode", "shareSetMode", "shareSetModeDesc", mode),
                window,
                cx,
            ),
            setting_row(
                RowSpec::new(
                    translator,
                    "relay",
                    "shareSetRelay",
                    "shareSetRelayDesc",
                    relay,
                ),
                window,
                cx,
            ),
        ];
        setting_group(
            text(translator, "shareSetGroupNetwork"),
            Some(text(translator, "shareSetRebuildNote")),
            rows,
            cx,
        )
    }

    pub(super) fn transfer_group(
        &self,
        g: &GroupCtx,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let translator = g.translator;
        let config = self.shown.clone();
        let tokens = active_theme(cx).tokens().clone();
        let rate_input = Input::new(&self.rate)
            .control(cx)
            .disabled(!self.editable(cx))
            .w(active_theme(cx).text_extent(NUMBER_WIDTH));
        let rate_unit = self.dropdown(
            "share-set-rate-unit",
            RateUnit::ALL
                .into_iter()
                .map(|unit| option(unit.id(), text(translator, unit.label_key())))
                .collect(),
            SharedString::from(self.rate_unit.id()),
            true,
            Self::set_rate_unit,
            cx,
        );
        let rate = h_flex()
            .gap(tokens.spacing.sm)
            .items_center()
            .child(rate_input)
            .child(
                div()
                    .w(active_theme(cx).text_extent(UNIT_DROPDOWN_WIDTH))
                    .child(rate_unit),
            )
            .into_any_element();
        let exit = self.dropdown(
            "share-set-exit",
            vec![
                option("prompt", text(translator, "shareSetExitPrompt")),
                option(
                    "convertToCloud",
                    text(translator, "shareSetExitConvertToCloud"),
                ),
                option("quit", text(translator, "shareSetExitQuit")),
            ],
            SharedString::from(exit_value(config.exit_behavior)),
            false,
            Self::set_exit_behavior,
            cx,
        );
        let rate_error = self
            .text_errors
            .as_ref()
            .is_some_and(|edits| edits.rate_invalid)
            .then(|| text(translator, "shareSetErrRate"));
        let auto_provide = self.switch(
            "share-set-auto-provide",
            config.auto_provide.unwrap_or(true),
            Self::set_auto_provide,
            cx,
        );
        let daily_cap = h_flex()
            .gap(tokens.spacing.sm)
            .items_center()
            .child(
                Input::new(&self.daily_cap)
                    .control(cx)
                    .disabled(!self.editable(cx))
                    .w(active_theme(cx).text_extent(NUMBER_WIDTH)),
            )
            // 单位是国际通用缩写，不走翻译。
            .child(div().text_color(tokens.colors.muted_foreground).child("GB"))
            .into_any_element();
        let daily_cap_error = self
            .text_errors
            .as_ref()
            .is_some_and(|edits| edits.daily_cap_invalid)
            .then(|| text(translator, "shareSetErrDailyCap"));
        let rows = vec![
            setting_row(
                RowSpec::new(
                    translator,
                    "autoProvide",
                    "shareSetAutoProvide",
                    "shareSetAutoProvideDesc",
                    auto_provide,
                ),
                window,
                cx,
            ),
            setting_row(
                RowSpec::new(
                    translator,
                    "provide-daily-cap",
                    "shareSetProvideDailyCap",
                    "shareSetProvideDailyCapDesc",
                    daily_cap,
                )
                .error(daily_cap_error),
                window,
                cx,
            ),
            setting_row(
                RowSpec::new(
                    translator,
                    "rate",
                    "shareSetRateLimit",
                    "shareSetRateLimitDesc",
                    rate,
                )
                .error(rate_error),
                window,
                cx,
            ),
            setting_row(
                RowSpec::new(translator, "exit", "shareSetExit", "shareSetExitDesc", exit),
                window,
                cx,
            ),
        ];
        setting_group(text(translator, "shareSetGroupTransfer"), None, rows, cx)
    }

    pub(super) fn diagnostics_group(
        &self,
        g: &GroupCtx,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let translator = g.translator;
        let log_level = self.dropdown(
            "share-set-log-level",
            vec![
                option("error", text(translator, "shareSetLevelError")),
                option("warn", text(translator, "shareSetLevelWarn")),
                option("info", text(translator, "shareSetLevelInfo")),
                option("debug", text(translator, "shareSetLevelDebug")),
                option("trace", text(translator, "shareSetLevelTrace")),
            ],
            SharedString::from(log_level_value(self.shown.iroh_log_level)),
            false,
            Self::set_log_level,
            cx,
        );
        let rows = vec![setting_row(
            RowSpec::new(
                translator,
                "log-level",
                "shareSetLogLevel",
                "shareSetLogLevelDesc",
                log_level,
            ),
            window,
            cx,
        )];
        setting_group(text(translator, "shareSetGroupDiagnostics"), None, rows, cx)
    }
}
