//! 下载：保存位置、行为、连接与性能、自动重试、高级。

use fluxdown_protocol::{HIGH_SEGMENTS_WARN_ABOVE, SEVERE_SEGMENTS_WARN_ABOVE};
use fluxdown_ui_components::{
    ButtonVariant, CalloutTone, FluxIcon, RevealCallout, button, loading_button, tabular_numbers,
};
use fluxdown_ui_i18n::Translator;
use fluxdown_ui_theme::active_theme;
use gpui::{App, ParentElement, SharedString, Styled, Window};
use gpui_component::h_flex;

use super::{SectionContext, auto_quantity, rate_limit, user_agent};
use crate::ui::{Control, Explain, SettingsPage, SettingsSection, body_text, meta_text};

pub(crate) fn page(ctx: &SectionContext, cx: &mut App) -> SettingsPage {
    let store = ctx.store.read(cx);
    // 先只读判定：渲染期写 store 会让其他读它的 retained 视图白白重建。
    if store.conn_policy().is_none()
        && !store.load_attempted("connPolicy")
        && ctx
            .store
            .update(cx, |store, _| store.begin_load("connPolicy"))
    {
        ctx.store.update(cx, |store, cx| store.load_conn_policy(cx));
    }
    SettingsPage::new(
        "download",
        ctx.t("settingsCatDownload"),
        ctx.t("settingsCatDownloadDesc"),
        FluxIcon::Download,
    )
    .sections([
        save_location_section(ctx),
        behavior_section(ctx, cx),
        connection_section(ctx, cx),
        retry_section(ctx),
        advanced_section(ctx),
    ])
}

fn save_location_section(ctx: &SectionContext) -> SettingsSection {
    SettingsSection::new()
        .title(ctx.t("settingsGroupSaveLocation"))
        .row(ctx.item(
            "defaultSaveDir",
            Some("defaultSaveDirDesc"),
            save_dir_control(ctx),
        ))
        .row(ctx.item(
            "rememberLastSaveDir",
            Some("rememberLastSaveDirDesc"),
            ctx.pref_switch("download.remember_last_save_dir", false),
        ))
}

/// 目录选择：文本输入 + 系统目录选择器。
fn save_dir_control(ctx: &SectionContext) -> Control {
    let store = ctx.store();
    let browse = ctx.t("browse");
    Control::custom(move |disabled, _key, _window, cx: &mut App| {
        let tokens = active_theme(cx).tokens();
        let current = store.read(cx).daemon_str("default_save_dir");
        let pick_store = store.clone();
        h_flex()
            .gap(tokens.spacing.sm)
            .items_center()
            .child(
                body_text(cx)
                    .max_w(active_theme(cx).text_extent(320.))
                    .truncate()
                    .text_color(if current.is_empty() {
                        tokens.colors.muted_foreground
                    } else {
                        tokens.colors.foreground
                    })
                    .child(SharedString::from(current)),
            )
            .child(
                button(
                    "download-pick-save-dir",
                    browse.clone(),
                    ButtonVariant::Secondary,
                    cx,
                )
                .on_click(move |_, _, cx| {
                    let store = pick_store.clone();
                    let receiver = cx.prompt_for_paths(gpui::PathPromptOptions {
                        files: false,
                        directories: true,
                        multiple: false,
                        prompt: None,
                    });
                    cx.spawn(async move |cx| {
                        if let Ok(Ok(Some(paths))) = receiver.await
                            && let Some(path) = paths.first()
                        {
                            let text = path.display().to_string();
                            store.update(cx, |store, cx| {
                                store.set_daemon("default_save_dir", text, cx);
                            });
                        }
                    })
                    .detach();
                })
                .disabled(disabled),
            )
    })
}

fn behavior_section(ctx: &SectionContext, cx: &mut App) -> SettingsSection {
    let silent = ctx
        .store
        .read(cx)
        .pref_bool("download.silent_download", false);
    let mut section = SettingsSection::new()
        .title(ctx.t("settingsGroupBehavior"))
        .row(ctx.item(
            "silentDownload",
            Some("silentDownloadDesc"),
            ctx.pref_switch("download.silent_download", false),
        ));
    if silent {
        section = section.row(ctx.item(
            "silentSkipSelection",
            Some("silentSkipSelectionDesc"),
            ctx.pref_switch("download.silent_skip_selection", false),
        ));
    }
    section
        .row(ctx.item(
            "useServerTime",
            Some("useServerTimeDesc"),
            ctx.daemon_switch("use_server_time"),
        ))
        .row(ctx.item(
            "fileExistsBehavior",
            Some("fileExistsBehaviorDesc"),
            ctx.daemon_enum_dropdown("file_exists_behavior", "fileExists"),
        ))
        .row(ctx.item(
            "fileMissingAction",
            Some("fileMissingActionDesc"),
            ctx.daemon_enum_dropdown("file_missing_action", "fileMissing"),
        ))
        .row(ctx.item(
            "idleFileScan",
            Some("idleFileScanDesc"),
            ctx.daemon_switch("idle_file_scan"),
        ))
        .row(ctx.item(
            "defaultQueueSetting",
            Some("defaultQueueSettingDesc"),
            default_queue_control(ctx, cx),
        ))
}

fn default_queue_control(ctx: &SectionContext, cx: &mut App) -> Control {
    let mut options = vec![(SharedString::from(""), ctx.t("defaultQueue"))];
    options.extend(ctx.store.read(cx).queues().iter().map(|queue| {
        (
            SharedString::from(queue.queue_id.clone()),
            SharedString::from(queue.name.clone()),
        )
    }));
    ctx.daemon_dropdown("default_queue_id", options)
}

/// 「最大连接数」行下方的风险提示：值 > [`HIGH_SEGMENTS_WARN_ABOVE`] 时展开，
/// > [`SEVERE_SEGMENTS_WARN_ABOVE`] 升级为危险档；每帧读取 store，输入即时联动。
fn threads_risk_footer(
    ctx: &SectionContext,
) -> impl Fn(bool, &SharedString, &mut Window, &mut App) -> RevealCallout + 'static {
    let store = ctx.store();
    let translator = ctx.translator.clone();
    move |_, key, _, cx| {
        let segments = store.read(cx).daemon_i64("default_segments");
        threads_risk_callout(
            SharedString::from(format!("{key}-risk")),
            segments,
            &translator,
        )
    }
}

fn threads_risk_callout(id: SharedString, segments: i64, translator: &Translator) -> RevealCallout {
    let severe = segments > i64::from(SEVERE_SEGMENTS_WARN_ABOVE);
    let (tone, title_key, desc_key) = if severe {
        (
            CalloutTone::Danger,
            "threadsRiskSevereTitle",
            "threadsRiskSevereDesc",
        )
    } else {
        (CalloutTone::Warning, "threadsRiskTitle", "threadsRiskDesc")
    };
    let count = segments.to_string();
    let limit = HIGH_SEGMENTS_WARN_ABOVE.to_string();
    RevealCallout::new(
        id,
        segments > i64::from(HIGH_SEGMENTS_WARN_ABOVE),
        tone,
        translator.text(title_key).to_owned(),
        translator.text_with(desc_key, &[("count", &count), ("limit", &limit)]),
    )
}

fn connection_section(ctx: &SectionContext, cx: &mut App) -> SettingsSection {
    let store = ctx.store.read(cx);
    let auto_segments = store.daemon_i64("default_segments") == 0;
    let cdn_multi = store.daemon_bool("cdn_multi_enabled");
    let mut section = SettingsSection::new()
        .title(ctx.t("settingsGroupConnection"))
        .row(
            ctx.item(
                "defaultThreads",
                Some("defaultThreadsDesc"),
                auto_quantity::control(ctx, "default_segments", "default_segments_custom", 16),
            )
            .footer(threads_risk_footer(ctx)),
        );
    if auto_segments {
        section = section.row(ctx.item(
            "autoMaxConnections",
            Some("autoMaxConnectionsDesc"),
            ctx.daemon_number("auto_max_connections"),
        ));
    }
    section = section.row(ctx.item(
        "cdnMultiEnabled",
        Some("cdnMultiEnabledDesc"),
        ctx.daemon_switch("cdn_multi_enabled"),
    ));
    if cdn_multi {
        section = section.row(ctx.item(
            "cdnMaxNodes",
            Some("cdnMaxNodesDesc"),
            auto_quantity::control(ctx, "cdn_max_nodes", "cdn_max_nodes_custom", 2),
        ));
    }
    section = section.row(
        ctx.item(
            "multiNicEnabled",
            Some("multiNicEnabledDesc"),
            ctx.daemon_switch("multi_nic_enabled"),
        )
        .explain(Explain {
            hint: ctx.t("multiNicHelpHint"),
            title: ctx.t("multiNicHelpTitle"),
            body: ctx.t("multiNicHelp"),
            close: ctx.t("close"),
        }),
    );
    section
        .row(ctx.item(
            "connPolicyCache",
            Some("connPolicyCacheDesc"),
            conn_policy_control(ctx),
        ))
        .row(ctx.item(
            "maxConcurrent",
            Some("maxConcurrentDesc"),
            ctx.daemon_number("max_concurrent_tasks"),
        ))
        .row(ctx.item(
            "speedLimit",
            Some("speedLimitDesc"),
            rate_limit::control(ctx, "speed_limit_bytes", "speed_limit_unit"),
        ))
        .row(ctx.item(
            "uploadLimit",
            Some("uploadLimitDesc"),
            rate_limit::control(ctx, "upload_limit_bytes", "upload_limit_unit"),
        ))
}

fn conn_policy_control(ctx: &SectionContext) -> Control {
    let store = ctx.store();
    let clear = ctx.t("connPolicyCacheClear");
    let empty = ctx.t("connPolicyCacheEmpty");
    Control::custom(move |_disabled, _key, _window, cx: &mut App| {
        let tokens = active_theme(cx).tokens();
        let count = store
            .read(cx)
            .conn_policy()
            .map_or(0, |summary| summary.domain_count);
        let busy = store.read(cx).is_busy("connPolicy");
        let clearing = store.read(cx).is_busy_tagged("connPolicy", "clear");
        let clear_store = store.clone();
        h_flex()
            .gap(tokens.spacing.sm)
            .items_center()
            .child(
                meta_text(cx)
                    .font_features(tabular_numbers())
                    .child(if count == 0 {
                        empty.clone()
                    } else {
                        SharedString::from(count.to_string())
                    }),
            )
            .child(
                loading_button(
                    "download-clear-conn-policy",
                    clear.clone(),
                    ButtonVariant::Secondary,
                    clearing,
                    cx,
                )
                .disabled(busy || count == 0)
                .on_click(move |_, _, cx| {
                    clear_store.update(cx, |store, cx| {
                        store.clear_conn_policy(cx);
                        store.tag_busy("connPolicy", "clear");
                    });
                }),
            )
    })
}

fn retry_section(ctx: &SectionContext) -> SettingsSection {
    SettingsSection::new()
        .title(ctx.t("settingsGroupRetry"))
        .row(ctx.item(
            "autoRetryCount",
            Some("autoRetryCountDesc"),
            ctx.daemon_number("max_auto_retries"),
        ))
        .row(ctx.item(
            "autoRetryDelay",
            Some("autoRetryDelayDesc"),
            ctx.daemon_number("auto_retry_delay_secs"),
        ))
        .row(ctx.item(
            "autoResumeOnStart",
            Some("autoResumeOnStartDesc"),
            ctx.daemon_switch("auto_resume_on_start"),
        ))
}

fn advanced_section(ctx: &SectionContext) -> SettingsSection {
    SettingsSection::new()
        .title(ctx.t("settingsGroupAdvanced"))
        .row(user_agent::row(ctx))
        .row(
            ctx.item(
                "revealFileCmdLabel",
                Some("revealFileCmdShortDesc"),
                ctx.pref_input("reveal_file_cmd", ""),
            )
            .help(ctx.t("revealFileCmdDesc")),
        )
}
