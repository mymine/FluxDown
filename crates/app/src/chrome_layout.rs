//! 主窗口 chrome 布局的装配：布局目录（活动栏条目 + 下载控件）、控件注册与偏好回流。
//!
//! 布局存设备本地偏好 [`CHROME_LAYOUT_PREF`]（不在云同步目录，不随账号同步）；Web 各自
//! 存在浏览器本地，两端互不影响。条目 id 与 Web `web/src/shell/chromeCatalog.ts` 一致。

use std::collections::BTreeMap;

use fluxdown_ui_downloads::{DownloadView, DownloadWidget, WidgetPlacement};
use fluxdown_ui_shell::{
    ChromeFamilies, ChromeFamily, ChromeItemSpec, ChromeZone, ShellView, ShellWidget,
};
use gpui::{Anchor, Context, WeakEntity};

use crate::activity::ActivityEntry;

/// 主窗口 chrome 布局偏好键（设备本地）。
pub(crate) const CHROME_LAYOUT_PREF: &str = "ui.chrome_layout";

/// 布局目录：活动栏条目在前（路由在上、动作在下），随后是下载控件（状态栏自左向右）。
pub(crate) fn catalog() -> Vec<ChromeItemSpec> {
    let activity = ActivityEntry::ALL.into_iter().map(|entry| {
        let (zone, families) = match entry {
            ActivityEntry::Downloads | ActivityEntry::Rss | ActivityEntry::Webhooks => {
                (ChromeZone::RailTop, ChromeFamilies::ALL)
            }
            // 账户头像是固定 32px 的自绘视图，高于状态栏。
            ActivityEntry::Account => (ChromeZone::RailBottom, ChromeFamilies::RAIL_TITLE),
            ActivityEntry::Settings => (ChromeZone::RailBottom, ChromeFamilies::ALL),
        };
        ChromeItemSpec::new(entry.layout_id(), zone, families)
    });
    let widgets = DownloadWidget::ALL.into_iter().map(|widget| {
        let zone = match widget {
            DownloadWidget::DownloadSpeed
            | DownloadWidget::UploadSpeed
            | DownloadWidget::PauseAll
            | DownloadWidget::ResumeAll => ChromeZone::StatusLeading,
            DownloadWidget::DownloadLimit
            | DownloadWidget::UploadLimit
            | DownloadWidget::Proxy
            | DownloadWidget::Shutdown
            | DownloadWidget::DiskFree => ChromeZone::StatusTrailing,
        };
        let families = if widget.is_text_readout() {
            ChromeFamilies::TITLE_STATUS
        } else {
            ChromeFamilies::ALL
        };
        ChromeItemSpec::new(widget.id(), zone, families)
    });
    activity.chain(widgets).collect()
}

/// 下载控件注册为 shell 控件：在 shell 的渲染里读取下载页状态绘制。
pub(crate) fn widgets(downloads: &WeakEntity<DownloadView>) -> Vec<ShellWidget> {
    DownloadWidget::ALL
        .into_iter()
        .map(|widget| {
            let downloads = downloads.clone();
            ShellWidget::new(widget.id(), widget.label_key(), move |zone, _, cx| {
                let view = downloads.upgrade()?;
                view.read(cx)
                    .render_widget(widget, placement(zone), &downloads, cx)
            })
        })
        .collect()
}

/// 区域 → 控件呈现：活动栏紧凑；底部区域菜单向上弹，顶部区域向下弹，右侧区域右对齐。
fn placement(zone: ChromeZone) -> WidgetPlacement {
    let menu_anchor = match zone {
        ChromeZone::TitleLeading | ChromeZone::RailTop => Anchor::TopLeft,
        ChromeZone::TitleTrailing => Anchor::TopRight,
        ChromeZone::RailBottom | ChromeZone::StatusLeading => Anchor::BottomLeft,
        ChromeZone::StatusTrailing => Anchor::BottomRight,
    };
    WidgetPlacement {
        compact: zone.family() == ChromeFamily::Rail,
        menu_anchor,
    }
}

/// 偏好 → 布局；幂等，窗口创建与每次偏好快照 / 事件都走这里。
pub(crate) fn apply(
    shell: &mut ShellView,
    preferences: &BTreeMap<String, serde_json::Value>,
    cx: &mut Context<ShellView>,
) {
    shell.apply_layout(preferences.get(CHROME_LAYOUT_PREF), cx);
}

#[cfg(test)]
mod tests {
    use fluxdown_ui_shell::{ChromeLayout, ChromeZone};

    use super::catalog;

    /// 默认布局必须还原改造前的界面：活动栏路由在上、动作在下，状态栏左 = 速度 + 暂停 / 开始，
    /// 右 = 限速、代理、关机、剩余空间，顶栏不放任何条目。
    #[test]
    fn default_layout_matches_classic_chrome() {
        let layout = ChromeLayout::default_for(&catalog());
        let ids =
            |zone| -> Vec<String> { layout.items(zone).iter().map(ToString::to_string).collect() };
        assert!(ids(ChromeZone::TitleLeading).is_empty());
        assert!(ids(ChromeZone::TitleTrailing).is_empty());
        assert_eq!(ids(ChromeZone::RailTop), ["downloads", "rss", "webhooks"]);
        assert_eq!(ids(ChromeZone::RailBottom), ["account", "settings"]);
        assert_eq!(
            ids(ChromeZone::StatusLeading),
            ["downloadSpeed", "uploadSpeed", "pauseAll", "resumeAll"]
        );
        assert_eq!(
            ids(ChromeZone::StatusTrailing),
            [
                "downloadLimit",
                "uploadLimit",
                "proxy",
                "shutdown",
                "diskFree"
            ]
        );
    }
}
