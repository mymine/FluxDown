//! 任务表状态列的FluxMesh来源角标：FluxMesh接收任务显示当前承载数据的路径（直连局域网 / 直连公网 /
//! 中继 / 连接中），走中继时附「切换桥接」入口；已完成且仍在回传（共享）的接收任务显示「共享中」。
//! 角标与状态文案同字号档，单行省略，不改变行高。

use fluxdown_protocol::{SharePathKind, ShareProvideState, ShareSwarmBlock, ShareTaskFactsDto};
use fluxdown_ui_components::{FluxIcon, tooltip_view};
use fluxdown_ui_theme::active_theme;
use gpui::{
    AnyElement, App, Hsla, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, WeakEntity, div,
};
use gpui_component::{Icon, h_flex};

use crate::{pages::downloads::DownloadView, strings::DownloadStrings};

/// 角标着色语气：直连、云端副本与共享中 = 成功，中继与桥接 = 警示（限速 / 回退来源），未选定路径 = 中性。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ShareTone {
    Good,
    Warn,
    Neutral,
}

/// 「切换桥接」入口的样式：仍卡在中继（打洞超时）时高亮，其余只给弱提示。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SwitchBridge {
    Hint,
    Highlight,
}

/// 角标内容（纯数据，便于测试）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ShareBadge {
    pub(crate) kind: SharePathKind,
    /// 已完成任务仍在回传（共享）：显示「共享中」而不是传输路径。
    pub(crate) providing: bool,
    /// 共享对端数；`> 0` 时角标追加「+N 台设备」。
    pub(crate) peers: u32,
    pub(crate) tone: ShareTone,
    /// 「切换桥接」入口：进行中的任务只要来源还不是桥接就提供。
    pub(crate) switch_bridge: Option<SwitchBridge>,
    /// 共享加速被云端拒绝的原因；只追加弱化 chip，不改变主状态。
    pub(crate) swarm_block: Option<ShareSwarmBlock>,
}

/// 任务事实 → 角标；不是FluxMesh接收任务时没有角标。`active` = 任务仍在进行（等待 / 下载中 / 暂停）：
/// 进行中显示传输路径；未进行（已完成 / 失败）且接收记录仍在共享时显示「共享中」，其余未进行的
/// 任务保留来源角标但不再提供切换。
pub(crate) fn badge_for(facts: &ShareTaskFactsDto, active: bool) -> Option<ShareBadge> {
    if !facts.is_share_receive {
        return None;
    }
    if !active && facts.provide_state == ShareProvideState::Providing {
        return Some(ShareBadge {
            kind: facts.path_kind,
            providing: true,
            peers: 0,
            tone: ShareTone::Good,
            switch_bridge: None,
            swarm_block: None,
        });
    }
    let tone = match facts.path_kind {
        SharePathKind::DirectLan
        | SharePathKind::DirectWan
        | SharePathKind::Cdn
        | SharePathKind::Swarm => ShareTone::Good,
        SharePathKind::Relay | SharePathKind::Bridge => ShareTone::Warn,
        SharePathKind::Unknown => ShareTone::Neutral,
    };
    let switch_bridge =
        (active && facts.path_kind != SharePathKind::Bridge).then_some(if facts.relay_stuck {
            SwitchBridge::Highlight
        } else {
            SwitchBridge::Hint
        });
    Some(ShareBadge {
        kind: facts.path_kind,
        providing: false,
        peers: facts.swarm_peers,
        tone,
        switch_bridge,
        swarm_block: facts
            .swarm_block
            .filter(|block| *block != ShareSwarmBlock::Unknown),
    })
}

fn tone_color(tone: ShareTone, cx: &App) -> Hsla {
    let theme = active_theme(cx);
    match tone {
        ShareTone::Good => theme.extended().colors.success,
        ShareTone::Warn => theme.extended().colors.warning,
        ShareTone::Neutral => theme.tokens().colors.muted_foreground,
    }
}

/// 渲染角标。`switch_available` = 宿主提供了「切换桥接」入口；点击动作不落到行上。
pub(crate) fn share_badge(
    task_id: &str,
    badge: ShareBadge,
    strings: &DownloadStrings,
    switch_available: bool,
    open_usage_available: bool,
    host: Option<&WeakEntity<DownloadView>>,
    cx: &App,
) -> AnyElement {
    let theme = active_theme(cx);
    let tokens = theme.tokens();
    let text = &tokens.typography.xs;
    let color = tone_color(badge.tone, cx);
    let mut label = if badge.providing {
        strings.share_providing_badge.clone()
    } else {
        strings.share_path(badge.kind)
    };
    if badge.peers > 0 {
        label = SharedString::from(format!(
            "{label} {}",
            strings.format_share_peers(badge.peers)
        ));
    }
    let pill = h_flex()
        .id(SharedString::from(format!("share-badge-{task_id}")))
        .min_w_0()
        .items_center()
        .gap(tokens.spacing.xxs)
        .px(tokens.spacing.xs)
        .rounded(tokens.radius.sm)
        .bg(color.opacity(0.14))
        .text_color(color)
        .text_size(text.size)
        .line_height(text.line_height)
        .tooltip({
            let label = label.clone();
            move |window, cx| tooltip_view(label.clone(), window, cx)
        })
        .child(Icon::new(FluxIcon::Share2).size(theme.extended().icon.sm))
        .child(div().min_w_0().truncate().child(label));

    let switch = badge
        .switch_bridge
        .filter(|_| switch_available)
        .zip(host)
        .map(|(style, host)| (style, host.clone()))
        .map(|(style, host)| {
            let task_id = task_id.to_owned();
            let action = strings.share_switch_bridge.clone();
            let color = match style {
                SwitchBridge::Highlight => tone_color(ShareTone::Warn, cx),
                SwitchBridge::Hint => tokens.colors.muted_foreground,
            };
            div()
                .id(SharedString::from(format!("share-switch-bridge-{task_id}")))
                .min_w_0()
                .truncate()
                .px(tokens.spacing.xs)
                .rounded(tokens.radius.sm)
                .text_size(text.size)
                .line_height(text.line_height)
                .text_color(color)
                .cursor_pointer()
                .hover(move |style| style.bg(color.opacity(0.14)))
                // 不让点击落到行上（选中 / 双击打开详情）。
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    let task_id = task_id.clone();
                    let Ok(()) = host.update(cx, |view, cx| {
                        view.share_switch_to_bridge(task_id, window, cx);
                    }) else {
                        // 视图已释放，结束这次回调而不再更新状态。
                        return;
                    };
                })
                .child(action)
        });

    let chip = badge.swarm_block.map(|block| {
        let label = match block {
            ShareSwarmBlock::QuotaExhausted => strings.share_swarm_quota_chip.clone(),
            _ => strings.share_swarm_login_chip.clone(),
        };
        let color = tokens.colors.muted_foreground;
        let mut chip = div()
            .id(SharedString::from(format!("share-swarm-chip-{task_id}")))
            .min_w_0()
            .truncate()
            .px(tokens.spacing.xs)
            .rounded(tokens.radius.sm)
            .text_size(text.size)
            .line_height(text.line_height)
            .text_color(color);
        if let Some(host) = host.filter(|_| open_usage_available) {
            let host = host.clone();
            let task_id = task_id.to_owned();
            chip = chip
                .cursor_pointer()
                .hover(move |style| style.bg(color.opacity(0.14)))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    let task_id = task_id.clone();
                    let Ok(()) = host.update(cx, |view, cx| {
                        view.share_open_usage(task_id, window, cx);
                    }) else {
                        return;
                    };
                });
        }
        chip.child(label)
    });
    h_flex()
        .min_w_0()
        .items_center()
        .gap(tokens.spacing.xxs)
        .child(pill)
        .children(switch)
        .children(chip)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(kind: SharePathKind, stuck: bool, receive: bool) -> ShareTaskFactsDto {
        ShareTaskFactsDto {
            is_share_receive: receive,
            path_kind: kind,
            relay_stuck: stuck,
            ..ShareTaskFactsDto::default()
        }
    }

    #[test]
    fn badge_carries_swarm_peers() {
        let with_peers = ShareTaskFactsDto {
            swarm_peers: 3,
            ..facts(SharePathKind::DirectWan, false, true)
        };
        assert_eq!(
            badge_for(&with_peers, true).map(|badge| badge.peers),
            Some(3)
        );
        assert_eq!(
            badge_for(&facts(SharePathKind::DirectWan, false, true), true).map(|badge| badge.peers),
            Some(0)
        );
    }

    #[test]
    fn only_share_receive_tasks_get_a_badge() {
        assert_eq!(
            badge_for(&facts(SharePathKind::Relay, true, false), true),
            None
        );
        assert_eq!(badge_for(&ShareTaskFactsDto::default(), true), None);
    }

    #[test]
    fn tone_follows_path_kind() {
        let tone = |kind| badge_for(&facts(kind, false, true), true).map(|badge| badge.tone);
        assert_eq!(tone(SharePathKind::DirectLan), Some(ShareTone::Good));
        assert_eq!(tone(SharePathKind::DirectWan), Some(ShareTone::Good));
        assert_eq!(tone(SharePathKind::Cdn), Some(ShareTone::Good));
        assert_eq!(tone(SharePathKind::Relay), Some(ShareTone::Warn));
        assert_eq!(tone(SharePathKind::Bridge), Some(ShareTone::Warn));
        assert_eq!(tone(SharePathKind::Unknown), Some(ShareTone::Neutral));
        assert_eq!(tone(SharePathKind::Swarm), Some(ShareTone::Good));
    }

    #[test]
    fn switch_bridge_is_offered_to_every_active_task_not_already_bridged() {
        let switch = |kind, stuck, active| {
            badge_for(&facts(kind, stuck, true), active).map(|badge| badge.switch_bridge)
        };
        for kind in [
            SharePathKind::DirectLan,
            SharePathKind::DirectWan,
            SharePathKind::Relay,
            SharePathKind::Cdn,
            SharePathKind::Unknown,
        ] {
            assert_eq!(switch(kind, false, true), Some(Some(SwitchBridge::Hint)));
        }
        assert_eq!(switch(SharePathKind::Bridge, false, true), Some(None));
        assert_eq!(switch(SharePathKind::Bridge, true, true), Some(None));
    }

    #[test]
    fn switch_bridge_is_highlighted_only_when_the_relay_is_stuck() {
        let switch = |kind, stuck| {
            badge_for(&facts(kind, stuck, true), true).map(|badge| badge.switch_bridge)
        };
        assert_eq!(
            switch(SharePathKind::Relay, true),
            Some(Some(SwitchBridge::Highlight))
        );
        assert_eq!(
            switch(SharePathKind::Unknown, true),
            Some(Some(SwitchBridge::Highlight))
        );
        assert_eq!(
            switch(SharePathKind::Relay, false),
            Some(Some(SwitchBridge::Hint))
        );
    }

    fn providing(kind: SharePathKind, receive: bool) -> ShareTaskFactsDto {
        ShareTaskFactsDto {
            provide_state: ShareProvideState::Providing,
            ..facts(kind, false, receive)
        }
    }

    #[test]
    fn finished_providing_task_shows_the_providing_badge_without_switch() {
        let badge = badge_for(&providing(SharePathKind::Unknown, true), false);
        assert_eq!(
            badge,
            Some(ShareBadge {
                kind: SharePathKind::Unknown,
                providing: true,
                peers: 0,
                tone: ShareTone::Good,
                switch_bridge: None,
                swarm_block: None,
            })
        );
    }

    #[test]
    fn finished_task_that_is_not_providing_gets_no_providing_badge() {
        // 已完成且非共享：agent 不下发 facts，下载页没有角标。
        assert_eq!(badge_for(&ShareTaskFactsDto::default(), false), None);
        for state in [
            ShareProvideState::Off,
            ShareProvideState::Stopped,
            ShareProvideState::Ended,
        ] {
            let facts = ShareTaskFactsDto {
                provide_state: state,
                ..facts(SharePathKind::DirectWan, false, true)
            };
            assert!(!badge_for(&facts, false).is_some_and(|badge| badge.providing));
        }
        // 不是接收任务时，即使状态是 providing 也没有角标。
        assert_eq!(
            badge_for(&providing(SharePathKind::Unknown, false), false),
            None
        );
    }

    #[test]
    fn active_task_keeps_the_path_badge_even_when_providing() {
        let badge = badge_for(&providing(SharePathKind::Relay, true), true);
        assert_eq!(badge.map(|badge| badge.providing), Some(false));
        assert_eq!(badge.map(|badge| badge.kind), Some(SharePathKind::Relay));
        assert_eq!(
            badge.map(|badge| badge.switch_bridge),
            Some(Some(SwitchBridge::Hint))
        );
    }

    #[test]
    fn finished_tasks_keep_the_badge_without_a_switch() {
        let badge = badge_for(&facts(SharePathKind::Relay, true, true), false);
        assert_eq!(badge.map(|badge| badge.switch_bridge), Some(None));
        assert_eq!(badge.map(|badge| badge.kind), Some(SharePathKind::Relay));
    }

    #[test]
    fn swarm_block_is_carried_and_unknown_is_dropped() {
        let with = |block| {
            badge_for(
                &ShareTaskFactsDto {
                    swarm_block: Some(block),
                    ..facts(SharePathKind::DirectWan, false, true)
                },
                true,
            )
            .and_then(|badge| badge.swarm_block)
        };
        assert_eq!(
            with(ShareSwarmBlock::QuotaExhausted),
            Some(ShareSwarmBlock::QuotaExhausted)
        );
        assert_eq!(with(ShareSwarmBlock::Unknown), None);
    }
}
