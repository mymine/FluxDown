//! 插件贡献点：把 `PluginDto` 列表归类成活动栏页、任务详情 tab、设置视图、独立窗口、命令面板
//! 与任务右键命令，并按 view 声明生成视图。

use std::{collections::HashMap, path::PathBuf};

use fluxdown_protocol::{PluginCommandDeclDto, PluginDto, PluginViewDeclDto};
use gpui::{
    AnyView, App, AppContext as _, Context, IntoElement, ParentElement as _, Render, Styled as _,
    Window, div,
};

use crate::{
    PluginUiError, PluginWindowSpec, ViewContainer, desktop_view, load_failure_view,
    portable::{PluginViewHost, PortableEnv},
};

/// 一个可展示的插件 view。
#[derive(Debug, Clone, PartialEq)]
pub struct ViewRef {
    pub identity: String,
    pub plugin_name: String,
    pub view: PluginViewDeclDto,
    /// 插件包在 daemon 所在机器上的目录。
    pub package_dir: Option<String>,
    /// 插件包修订号；变化即包代码已被替换，宿主须重建该 view（不复用旧脚本视图）。
    pub package_revision: u64,
}

/// 一个插件命令。
#[derive(Debug, Clone, PartialEq)]
pub struct CommandRef {
    pub identity: String,
    pub plugin_name: String,
    pub command: PluginCommandDeclDto,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Contributions {
    /// 活动栏页（`location == "activity"`）。
    pub activity: Vec<ViewRef>,
    /// 任务详情 tab（`taskDetail`）。
    pub task_tabs: Vec<ViewRef>,
    /// 插件设置视图（`settings`，每插件至多一个），键为 identity。
    pub settings: HashMap<String, ViewRef>,
    /// 登录弹窗视图（`auth`，每插件至多一个），键为 identity。
    pub auth: HashMap<String, ViewRef>,
    /// 独立窗口（`window`）。
    pub windows: Vec<ViewRef>,
    /// 命令面板命令（`palette`）。
    pub palette: Vec<CommandRef>,
    /// 任务右键命令（`taskMenu`）。
    pub task_menu: Vec<CommandRef>,
}

impl Contributions {
    /// 只纳入「已启用、已加载、授权已齐」的插件；其余插件的贡献点一律不出现。
    pub fn from_plugins(plugins: &[PluginDto]) -> Self {
        let mut out = Self::default();
        for plugin in plugins {
            if !plugin.enabled
                || plugin.load_status != "Loaded"
                || !plugin.consent_pending.is_empty()
                || !plugin.consent_pending_hosts.is_empty()
            {
                continue;
            }
            for view in &plugin.contributions.views {
                let view_ref = ViewRef {
                    identity: plugin.identity.clone(),
                    plugin_name: plugin.name.clone(),
                    view: view.clone(),
                    package_dir: plugin.package_dir.clone(),
                    package_revision: plugin.package_revision,
                };
                match view.location.as_str() {
                    "activity" => out.activity.push(view_ref),
                    "taskDetail" => out.task_tabs.push(view_ref),
                    "settings" => {
                        out.settings
                            .entry(plugin.identity.clone())
                            .or_insert(view_ref);
                    }
                    "auth" => {
                        out.auth.entry(plugin.identity.clone()).or_insert(view_ref);
                    }
                    "window" => out.windows.push(view_ref),
                    other => log::debug!(
                        "plugin `{}` view `{}` has unknown location `{other}`",
                        plugin.identity,
                        view.id
                    ),
                }
            }
            for command in &plugin.contributions.commands {
                let command_ref = CommandRef {
                    identity: plugin.identity.clone(),
                    plugin_name: plugin.name.clone(),
                    command: command.clone(),
                };
                if command.palette {
                    out.palette.push(command_ref.clone());
                }
                if command.task_menu {
                    out.task_menu.push(command_ref);
                }
            }
        }
        out
    }
}

impl ViewRef {
    /// 桌面实现的加载描述；没有 `desktop` 入口或包目录时为 `None`。
    pub fn desktop_spec(&self) -> Option<PluginWindowSpec> {
        Some(PluginWindowSpec {
            identity: self.identity.clone(),
            name: self.plugin_name.clone(),
            view_id: self.view.id.clone(),
            view_title: self.view.title.clone(),
            package_dir: PathBuf::from(self.package_dir.as_ref()?),
            entry: self.view.desktop.clone()?,
        })
    }
}

/// 为一个 view 创建内容视图。`container` 是承载它的容器（嵌入页传 `ViewContainer::default()`）。
///
/// 优先桌面实现（脚本在本机包目录可读时）；否则回退到可移植视图；两者都不可用时给出
/// 「仅桌面可用」提示。可移植视图创建后立即 `activate`。
pub fn view_for(
    r: &ViewRef,
    task_id: Option<String>,
    container: ViewContainer,
    window: &mut Window,
    cx: &mut App,
) -> AnyView {
    if let Some(spec) = r.desktop_spec()
        && crate::package_available(&spec)
    {
        return desktop_view(spec, task_id, container, window, cx);
    }
    if !r.view.portable {
        return notice_view("pluginViewDesktopOnly", cx);
    }
    let Some((port, translator)) = cx
        .try_global::<PortableEnv>()
        .map(|env| (env.port.clone(), env.translator.clone()))
    else {
        return load_failure_view(
            &PluginUiError::Runtime("portable plugin views are not installed".to_owned()),
            cx,
        );
    };
    let identity = r.identity.clone();
    let view = r.view.id.clone();
    let host =
        cx.new(|cx| PluginViewHost::new(identity, view, task_id, container, port, translator, cx));
    host.update(cx, |host, cx| host.activate(cx));
    host.into()
}

/// 一行翻译文案的占位视图。
struct Notice {
    text: String,
}

impl Render for Notice {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = fluxdown_ui_theme::active_theme(cx).tokens();
        div()
            .size_full()
            .p(tokens.spacing.lg)
            .text_size(tokens.typography.sm.size)
            .text_color(tokens.colors.muted_foreground)
            .child(self.text.clone())
    }
}

fn notice_view(key: &str, cx: &mut App) -> AnyView {
    let text = cx.try_global::<PortableEnv>().map_or_else(
        || key.to_owned(),
        |env| env.translator.read(cx).text(key).to_owned(),
    );
    cx.new(|_| Notice { text }).into()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn plugin(identity: &str, extra: serde_json::Value) -> PluginDto {
        let mut base = json!({
            "identity": identity,
            "name": identity,
            "version": "1.0.0",
            "enabled": true,
            "devMode": false,
            "disabledReason": "",
            "settings": [],
            "settingsValues": {},
            "permissions": [],
            "authSupported": false,
            "subscriptionProviderIds": [],
            "resolverUrls": [],
            "loadStatus": "Loaded",
            "loadError": "",
            "contributions": {
                "views": [
                    {"id": "a", "title": "A", "location": "activity", "portable": true},
                    {"id": "t", "title": "T", "location": "taskDetail", "portable": true},
                    {"id": "s", "title": "S", "location": "settings", "portable": true},
                    {"id": "l", "title": "L", "location": "auth", "portable": true},
                    {"id": "w", "title": "W", "location": "window", "portable": false,
                     "desktop": {"entry": "ui/w.js"}}
                ],
                "commands": [
                    {"id": "both", "title": "Both", "palette": true, "taskMenu": true},
                    {"id": "pal", "title": "Pal", "palette": true, "taskMenu": false},
                    {"id": "none", "title": "None", "palette": false, "taskMenu": false}
                ]
            },
            "risk": "standard",
            "consentPending": [],
            "consentPendingHosts": [],
            "networkHosts": [],
            "packageDir": "/tmp/p"
        });
        if let (Some(base), Some(extra)) = (base.as_object_mut(), extra.as_object()) {
            for (key, value) in extra {
                base.insert(key.clone(), value.clone());
            }
        }
        serde_json::from_value(base).expect("plugin dto")
    }

    #[test]
    fn classifies_views_and_commands() {
        let plugins = [plugin("a@ok", json!({}))];
        let contributions = Contributions::from_plugins(&plugins);
        assert_eq!(contributions.activity.len(), 1);
        assert_eq!(contributions.task_tabs.len(), 1);
        assert_eq!(contributions.windows.len(), 1);
        assert_eq!(contributions.settings.len(), 1);
        assert_eq!(contributions.settings["a@ok"].view.id, "s");
        assert_eq!(contributions.auth.len(), 1);
        assert_eq!(contributions.auth["a@ok"].view.id, "l");
        let palette: Vec<_> = contributions
            .palette
            .iter()
            .map(|c| c.command.id.as_str())
            .collect();
        assert_eq!(palette, ["both", "pal"]);
        let menu: Vec<_> = contributions
            .task_menu
            .iter()
            .map(|c| c.command.id.as_str())
            .collect();
        assert_eq!(menu, ["both"]);
        assert!(contributions.windows[0].desktop_spec().is_some());
        assert!(contributions.activity[0].desktop_spec().is_none());
    }

    #[test]
    fn excludes_plugins_that_are_not_usable() {
        let plugins = [
            plugin("a@pending", json!({"consentPending": ["tasks"]})),
            plugin("a@hosts", json!({"consentPendingHosts": ["x.com"]})),
            plugin("a@off", json!({"enabled": false})),
            plugin("a@failed", json!({"loadStatus": "Failed"})),
            plugin("a@ok", json!({})),
        ];
        let contributions = Contributions::from_plugins(&plugins);
        assert!(contributions.activity.iter().all(|v| v.identity == "a@ok"));
        assert_eq!(contributions.activity.len(), 1);
        assert_eq!(contributions.palette.len(), 2);
    }
}
