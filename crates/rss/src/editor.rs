//! RSS 订阅向导与管理表单：验证 feed 后创建，编辑时保留只读运行态字段。

use std::sync::Arc;

use fluxdown_protocol::{
    ApplicationErrorCode, PluginDto, QueueDto, RpcErrorData, RssSourceDto, RssValidateRequest,
    RssValidateResponse, method,
};
use fluxdown_ui_components::{
    BusyExt as _, ControlExt as _, FluxIcon, card, dialog_scroll_body, dialog_title, field_error,
    field_hint, form, form_field, form_row, input_with_action, option_group, option_row,
    segmented_tabs,
};
use fluxdown_ui_i18n::Translator;
use fluxdown_ui_theme::active_theme;
use gpui::{
    Anchor, AnyElement, App, AppContext as _, ClickEvent, Context, Div, Entity, FontWeight,
    IntoElement, ParentElement, PathPromptOptions, Render, SharedString, Styled, Window, div,
    prelude::FluentBuilder as _, px,
};
use gpui_component::{
    Disableable as _, Icon, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    menu::{DropdownMenu as _, PopupMenuItem},
    switch::Switch,
    v_flex,
};

use crate::{
    RssPort,
    provider::{
        BUILTIN_PROVIDER_ID, ProviderChoice, ProviderKind, normalize_provider_id, provider_choices,
        validate_provider_config,
    },
};

const INTERVALS: [i32; 7] = [10, 30, 60, 120, 360, 720, 1440];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Basic,
    Filter,
    Advanced,
}

/// 验证结果绑定其实际请求参数；输入发生变化后旧结果不能授权创建新 URL。
struct ValidatedFeed {
    request: RssValidateRequest,
    title: String,
    item_count: usize,
}

struct Editor {
    translator: Entity<Translator>,
    port: Arc<dyn RssPort>,
    existing: Option<RssSourceDto>,
    queues: Vec<QueueDto>,
    choices: Vec<ProviderChoice>,
    provider_id: String,
    tab: Tab,
    name: Entity<InputState>,
    url: Entity<InputState>,
    provider_config: Entity<InputState>,
    save_dir: Entity<InputState>,
    include: Entity<InputState>,
    exclude: Entity<InputState>,
    size_min: Entity<InputState>,
    size_max: Entity<InputState>,
    cookies: Entity<InputState>,
    user_agent: Entity<InputState>,
    proxy: Entity<InputState>,
    max_per_fetch: Entity<InputState>,
    interval: i32,
    queue_id: String,
    enabled: bool,
    auto_download: bool,
    start_paused: bool,
    use_regex: bool,
    smart_episode: bool,
    send_referer: bool,
    notify_on_download: bool,
    validating: bool,
    saving: bool,
    picking_dir: bool,
    validated: Option<ValidatedFeed>,
    error: Option<String>,
}

pub(super) fn open_editor(
    translator: Entity<Translator>,
    port: Arc<dyn RssPort>,
    source: Option<RssSourceDto>,
    queues: Vec<QueueDto>,
    plugins: Vec<PluginDto>,
    window: &mut Window,
    cx: &mut App,
) {
    let title = SharedString::from(
        translator
            .read(cx)
            .text(if source.is_some() {
                "rssManageTitle"
            } else {
                "rssAddSource"
            })
            .to_owned(),
    );
    let editor = cx.new(|cx| Editor::new(translator, port, source, queues, plugins, window, cx));
    let url = editor.read(cx).url.clone();
    window.open_dialog(cx, move |dialog, _, cx| {
        let editor = editor.clone();
        dialog
            .title(dialog_title(title.clone(), cx))
            .w(active_theme(cx).text_extent(640.))
            .overlay_closable(false)
            .keyboard(false)
            .close_button(false)
            .content(move |content, _, _| content.min_h_0().child(editor.clone()))
    });
    url.update(cx, |input, cx| input.focus(window, cx));
}

impl Editor {
    fn new(
        translator: Entity<Translator>,
        port: Arc<dyn RssPort>,
        existing: Option<RssSourceDto>,
        queues: Vec<QueueDto>,
        plugins: Vec<PluginDto>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let source = existing.as_ref();
        let mut input = |value: String, hint: &'static str, cx: &mut Context<Self>| {
            let placeholder = translator.read(cx).text(hint).to_owned();
            cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(value)
                    .placeholder(placeholder)
            })
        };
        let name = input(
            source.map_or_else(String::new, |s| s.name.clone()),
            "rssNameHint",
            cx,
        );
        let url = input(
            source.map_or_else(String::new, |s| s.url.clone()),
            "rssUrlHint",
            cx,
        );
        let provider_config = input(
            source.map_or_else(String::new, |s| s.provider_config.clone()),
            "rssProviderConfigPlaceholder",
            cx,
        );
        let save_dir = input(
            source.map_or_else(String::new, |s| s.save_dir.clone()),
            "rssSaveDirHint",
            cx,
        );
        let include = input(
            source.map_or_else(String::new, |s| s.include_pattern.clone()),
            "rssIncludeHint",
            cx,
        );
        let exclude = input(
            source.map_or_else(String::new, |s| s.exclude_pattern.clone()),
            "rssExcludeHint",
            cx,
        );
        let size_min = input(
            source.map_or_else(String::new, |s| format_size(s.size_min_bytes)),
            "rssSizeMinLabel",
            cx,
        );
        let size_max = input(
            source.map_or_else(String::new, |s| format_size(s.size_max_bytes)),
            "rssSizeMaxLabel",
            cx,
        );
        let cookies = input(
            source.map_or_else(String::new, |s| s.cookies.clone()),
            "rssCookiesHint",
            cx,
        );
        let user_agent = input(
            source.map_or_else(String::new, |s| s.user_agent.clone()),
            "rssInheritGlobalHint",
            cx,
        );
        let proxy = input(
            source.map_or_else(String::new, |s| s.proxy_url.clone()),
            "rssInheritGlobalHint",
            cx,
        );
        let max_per_fetch = input(
            source.map_or_else(
                || "20".to_owned(),
                |s| {
                    if s.max_per_fetch <= 0 {
                        "20".to_owned()
                    } else {
                        s.max_per_fetch.to_string()
                    }
                },
            ),
            "rssMaxPerFetchLabel",
            cx,
        );
        for field in [&url, &provider_config, &cookies, &user_agent, &proxy] {
            cx.subscribe(field, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.validated = None;
                    this.error = None;
                    cx.notify();
                }
            })
            .detach();
        }
        cx.observe(&translator, |_, _, cx| cx.notify()).detach();
        let provider_id =
            normalize_provider_id(source.map_or("", |s| s.provider_id.as_str())).to_owned();
        let choices = provider_choices(&plugins, &provider_id);
        let queue_id = source
            .map(|s| s.queue_id.clone())
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| fluxdown_protocol::MAIN_QUEUE_ID.to_owned());
        Self {
            translator,
            port,
            tab: Tab::Basic,
            interval: source.map_or(30, |s| {
                if s.interval_minutes <= 0 {
                    30
                } else {
                    s.interval_minutes
                }
            }),
            queue_id,
            enabled: source.is_none_or(|s| s.enabled),
            auto_download: source.is_none_or(|s| s.auto_download),
            start_paused: source.is_some_and(|s| s.start_paused),
            use_regex: source.is_some_and(|s| s.use_regex),
            smart_episode: source.is_some_and(|s| s.smart_episode),
            send_referer: source.is_none_or(|s| s.send_referer),
            notify_on_download: source.is_none_or(|s| s.notify_on_download),
            existing,
            queues,
            choices,
            provider_id,
            name,
            url,
            provider_config,
            save_dir,
            include,
            exclude,
            size_min,
            size_max,
            cookies,
            user_agent,
            proxy,
            max_per_fetch,
            validating: false,
            saving: false,
            picking_dir: false,
            validated: None,
            error: None,
        }
    }

    fn t(&self, key: &str, cx: &App) -> SharedString {
        SharedString::from(self.translator.read(cx).text(key).to_owned())
    }

    fn value(field: &Entity<InputState>, cx: &App) -> String {
        field.read(cx).value().trim().to_owned()
    }

    fn request(&self, cx: &App) -> RssValidateRequest {
        RssValidateRequest {
            provider_id: self.provider_id.clone(),
            provider_config: if self.provider_id == BUILTIN_PROVIDER_ID {
                String::new()
            } else {
                Self::value(&self.provider_config, cx)
            },
            url: Self::value(&self.url, cx),
            cookies: Self::value(&self.cookies, cx),
            user_agent: Self::value(&self.user_agent, cx),
            proxy_url: Self::value(&self.proxy, cx),
        }
    }

    /// 插件 provider 的配置必须留空或为合法 JSON；非法时回到基础页提示。
    fn check_provider_config(&mut self, cx: &mut Context<Self>) -> bool {
        if self.provider_id == BUILTIN_PROVIDER_ID
            || validate_provider_config(&Self::value(&self.provider_config, cx)).is_some()
        {
            return true;
        }
        self.tab = Tab::Basic;
        self.fail("rssProviderConfigInvalid", cx);
        false
    }

    fn provider_label(&self, choice: &ProviderChoice, cx: &App) -> SharedString {
        match choice.kind {
            ProviderKind::Builtin => self.t("rssProviderBuiltin", cx),
            ProviderKind::Plugin => SharedString::from(choice.name.clone()),
            ProviderKind::Missing => SharedString::from(choice.id.clone()),
        }
    }

    fn fail(&mut self, key: &str, cx: &mut Context<Self>) {
        self.error = Some(self.t(key, cx).to_string());
        cx.notify();
    }

    fn validate(&mut self, cx: &mut Context<Self>) {
        if self.validating || self.saving {
            return;
        }
        let request = self.request(cx);
        if request.url.is_empty() {
            self.tab = Tab::Basic;
            self.fail("rssFeedRequired", cx);
            return;
        }
        if !self.check_provider_config(cx) {
            return;
        }
        let Ok(params) = serde_json::to_value(&request) else {
            return;
        };
        self.validating = true;
        self.validated = None;
        self.error = None;
        cx.notify();
        let future = self.port.call(method::DAEMON_RSS_VALIDATE, params);
        cx.spawn(async move |this, cx| {
            let result = future.await;

            let Ok(()) = this.update(cx, |this, cx| {
                this.validating = false;
                // 编辑过程中即使旧请求晚到，也不接受与当前输入不同的结果。
                if !same_request(&this.request(cx), &request) {
                    cx.notify();
                    return;
                }
                match result {
                    Ok(value) => match serde_json::from_value::<RssValidateResponse>(value) {
                        Ok(response) if response.error.is_empty() => {
                            this.validated = Some(ValidatedFeed {
                                request,
                                title: response.feed_title,
                                item_count: response.items.len(),
                            });
                            this.tab = Tab::Basic;
                        }
                        Ok(response) => this.error = Some(response.error),
                        Err(error) => this.error = Some(error.to_string()),
                    },
                    Err(error) => {
                        this.error = Some(rpc_error(this.translator.read(cx), &error));
                    }
                }
                cx.notify();
            }) else {
                // 订阅视图已释放，结束回调，不再提交后续操作。
                return;
            };
        })
        .detach();
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving || self.validating {
            return;
        }
        let request = self.request(cx);
        if request.url.is_empty() {
            self.tab = Tab::Basic;
            self.fail("rssFeedRequired", cx);
            return;
        }
        if !self.check_provider_config(cx) {
            return;
        }
        if self.existing.is_none()
            && self
                .validated
                .as_ref()
                .is_none_or(|feed| !same_request(&feed.request, &request))
        {
            self.tab = Tab::Basic;
            self.fail("rssValidateBeforeSave", cx);
            return;
        }
        let min = match parse_size(&Self::value(&self.size_min, cx)) {
            Some(value) => value,
            None => {
                self.tab = Tab::Filter;
                self.fail("rssInvalidNumber", cx);
                return;
            }
        };
        let max = match parse_size(&Self::value(&self.size_max, cx)) {
            Some(value) => value,
            None => {
                self.tab = Tab::Filter;
                self.fail("rssInvalidNumber", cx);
                return;
            }
        };
        if min > 0 && max > 0 && max < min {
            self.tab = Tab::Filter;
            self.fail("rssInvalidSizeRange", cx);
            return;
        }
        let Some(max_per_fetch) = parse_fetch_limit(&Self::value(&self.max_per_fetch, cx)) else {
            self.tab = Tab::Advanced;
            self.fail("rssInvalidNumber", cx);
            return;
        };
        let mut source = self.existing.clone().unwrap_or_else(|| RssSourceDto {
            source_id: String::new(),
            provider_id: "rss".to_owned(),
            provider_config: String::new(),
            url: String::new(),
            name: String::new(),
            enabled: true,
            auto_download: true,
            start_paused: false,
            queue_id: String::new(),
            save_dir: String::new(),
            interval_minutes: 30,
            include_pattern: String::new(),
            exclude_pattern: String::new(),
            use_regex: false,
            smart_episode: false,
            size_min_bytes: 0,
            size_max_bytes: 0,
            send_referer: true,
            notify_on_download: true,
            max_per_fetch: 20,
            cookies: String::new(),
            user_agent: String::new(),
            proxy_url: String::new(),
            last_fetch_at: 0,
            last_success_at: 0,
            last_error: String::new(),
            fail_count: 0,
            seeded: false,
            position: 0,
            unread_count: 0,
        });
        source.provider_id = self.provider_id.clone();
        source.provider_config = request.provider_config;
        source.url = request.url;
        source.name = Self::value(&self.name, cx);
        if self.existing.is_none() && source.name.is_empty() {
            source.name = self
                .validated
                .as_ref()
                .map_or_else(String::new, |feed| feed.title.clone());
        }
        source.enabled = self.enabled;
        source.auto_download = self.auto_download;
        source.start_paused = self.start_paused;
        source.interval_minutes = self.interval;
        source.queue_id = self.queue_id.clone();
        source.save_dir = Self::value(&self.save_dir, cx);
        source.include_pattern = Self::value(&self.include, cx);
        source.exclude_pattern = Self::value(&self.exclude, cx);
        source.use_regex = self.use_regex;
        source.smart_episode = self.smart_episode;
        source.size_min_bytes = min;
        source.size_max_bytes = max;
        source.cookies = request.cookies;
        source.user_agent = request.user_agent;
        source.proxy_url = request.proxy_url;
        source.max_per_fetch = max_per_fetch;
        source.send_referer = self.send_referer;
        source.notify_on_download = self.notify_on_download;
        let method = if self.existing.is_some() {
            method::DAEMON_RSS_UPDATE_SOURCE
        } else {
            method::DAEMON_RSS_CREATE_SOURCE
        };
        let Ok(params) = serde_json::to_value(source) else {
            return;
        };
        self.error = None;
        self.saving = true;
        cx.notify();
        let future = self.port.call(method, params);
        cx.spawn_in(window, async move |this, cx| {
            let result = future.await;

            let Ok(()) = this.update_in(cx, |this, window, cx| {
                this.saving = false;
                match result {
                    Ok(_) => window.close_dialog(cx),
                    Err(error) => {
                        this.error = Some(rpc_error(this.translator.read(cx), &error));
                        cx.notify();
                    }
                }
            }) else {
                // 编辑器或窗口已释放，停止回写异步结果。
                return;
            };
        })
        .detach();
    }

    fn pick_dir(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.picking_dir {
            return;
        }
        self.picking_dir = true;
        cx.notify();
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            let picked = match receiver.await {
                Ok(Ok(Some(paths))) => paths.first().map(|path| path.display().to_string()),
                _ => None,
            };

            let Ok(()) = this.update_in(cx, |this, window, cx| {
                this.picking_dir = false;
                if let Some(path) = picked {
                    this.save_dir
                        .update(cx, |input, cx| input.set_value(path, window, cx));
                }
                cx.notify();
            }) else {
                // 编辑器或窗口已释放，停止回写异步结果。
                return;
            };
        })
        .detach();
    }

    fn hint(&self, key: &str, cx: &App) -> Div {
        field_hint(self.t(key, cx), cx)
    }

    fn field(&self, key: &str, input: &Entity<InputState>, cx: &App) -> Div {
        form_field(
            self.t(key, cx),
            Input::new(input).control(cx).w_full(),
            None,
            cx,
        )
    }

    /// 开关行（放进 `option_group`）。
    fn toggle(
        &self,
        id: &'static str,
        key: &str,
        description: &str,
        checked: bool,
        on_change: fn(&mut Self, bool),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        option_row(
            self.t(key, cx),
            Some(self.t(description, cx)),
            Switch::new(id).checked(checked).on_click(cx.listener(
                move |this, checked: &bool, _, cx| {
                    on_change(this, *checked);
                    cx.notify();
                },
            )),
            cx,
        )
        .into_any_element()
    }

    /// 下拉选择：与输入框同高、铺满字段宽度，右侧下拉箭头。
    fn menu(
        &self,
        id: &'static str,
        label: SharedString,
        choices: Vec<(String, SharedString)>,
        on_select: fn(&mut Self, String),
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let this = cx.weak_entity();
        Button::new(id)
            .outline()
            .label(label)
            .dropdown_caret(true)
            .control(cx)
            .w_full()
            .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, _| {
                choices.iter().fold(menu, |menu, (value, label)| {
                    let this = this.clone();
                    let value = value.clone();
                    menu.item(PopupMenuItem::new(label.clone()).on_click(move |_, _, cx| {
                        let value = value.clone();

                        let Ok(()) = this.update(cx, |this, cx| {
                            on_select(this, value);
                            cx.notify();
                        }) else {
                            // 订阅视图已释放，结束回调，不再提交后续操作。
                            return;
                        };
                    }))
                })
            })
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> Div {
        const TABS: [Tab; 3] = [Tab::Basic, Tab::Filter, Tab::Advanced];
        let labels = ["rssTabBasic", "rssTabFilter", "rssTabAdvanced"].map(|key| self.t(key, cx));
        let selected = TABS.iter().position(|tab| *tab == self.tab).unwrap_or(0);
        let this = cx.weak_entity();
        segmented_tabs(
            "rss-editor-tabs",
            labels,
            selected,
            move |index, _, cx| {
                let Some(tab) = TABS.get(index).copied() else {
                    return;
                };

                let Ok(()) = this.update(cx, |this, cx| {
                    this.tab = tab;
                    cx.notify();
                }) else {
                    // 订阅视图已释放，结束回调，不再提交后续操作。
                    return;
                };
            },
            cx,
        )
    }

    /// 来源类型：没有任何可用插件 provider 时整行隐藏，界面与仅内置 RSS 时一致。
    fn render_provider_field(&self, cx: &mut Context<Self>) -> Option<Div> {
        if self.choices.len() <= 1 {
            return None;
        }
        let current = self.choices.iter().find(|c| c.id == self.provider_id);
        let label = current.map_or_else(
            || SharedString::from(self.provider_id.clone()),
            |choice| self.provider_label(choice, cx),
        );
        let hint = match current.map(|c| c.kind) {
            Some(ProviderKind::Plugin) => Some(self.t("rssProviderPluginHint", cx)),
            Some(ProviderKind::Missing) => Some(SharedString::from(
                self.t("rssProviderUnavailable", cx)
                    .replace("{id}", &self.provider_id),
            )),
            _ => None,
        };
        let options = self
            .choices
            .iter()
            .map(|choice| (choice.id.clone(), self.provider_label(choice, cx)))
            .collect();
        Some(form_field(
            self.t("rssProviderLabel", cx),
            self.menu(
                "rss-editor-provider",
                label,
                options,
                |this, value| {
                    if this.provider_id != value {
                        this.provider_id = value;
                        this.validated = None;
                        this.error = None;
                    }
                },
                cx,
            ),
            hint,
            cx,
        ))
    }

    fn render_basic(&self, cx: &mut Context<Self>) -> Div {
        let provider_field = self.render_provider_field(cx);
        let theme = active_theme(cx);
        let tokens = theme.tokens().clone();
        let tertiary = theme.extended().colors.text_tertiary;
        let queue_label = self
            .queues
            .iter()
            .find(|q| q.queue_id == self.queue_id)
            .map(|queue| match queue.queue_id.as_str() {
                fluxdown_protocol::MAIN_QUEUE_ID => self.t("mainQueue", cx),
                fluxdown_protocol::LATER_QUEUE_ID => self.t("laterQueue", cx),
                _ => SharedString::from(queue.name.clone()),
            })
            .unwrap_or_else(|| SharedString::from(self.queue_id.clone()));
        let choices = self
            .queues
            .iter()
            .map(|queue| {
                let label = match queue.queue_id.as_str() {
                    fluxdown_protocol::MAIN_QUEUE_ID => self.t("mainQueue", cx),
                    fluxdown_protocol::LATER_QUEUE_ID => self.t("laterQueue", cx),
                    _ => SharedString::from(queue.name.clone()),
                };
                (queue.queue_id.clone(), label)
            })
            .collect();
        let intervals = INTERVALS
            .iter()
            .copied()
            .chain((!INTERVALS.contains(&self.interval)).then_some(self.interval))
            .map(|minutes| (minutes.to_string(), self.interval_label(minutes, cx)))
            .collect();

        // 订阅链接 + 行内「验证」：验证是新建流程的第一步，和链接放在同一行，
        // 不再挤在底栏左端。
        let can_validate =
            !self.validating && !self.saving && !Self::value(&self.url, cx).is_empty();
        let url_hint = if self.existing.is_none() {
            Some(self.t(
                if self.validating {
                    "rssWizardValidating"
                } else {
                    "rssEditorAuthHint"
                },
                cx,
            ))
        } else {
            None
        };
        let builtin = self.provider_id == BUILTIN_PROVIDER_ID;
        let url_field = form_field(
            self.t(
                if builtin {
                    "rssUrlLabel"
                } else {
                    "rssProviderUrlLabel"
                },
                cx,
            ),
            input_with_action(
                Input::new(&self.url).control(cx).w_full(),
                Button::new("rss-editor-validate")
                    .outline()
                    .label(self.t("rssWizardValidate", cx))
                    .control(cx)
                    .busy(self.validating)
                    .disabled(!can_validate)
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.validate(cx))),
                cx,
            ),
            url_hint,
            cx,
        );

        let mut body = form(cx);
        if let Some(provider_field) = provider_field {
            body = body.child(provider_field);
        }
        body = body.child(url_field);
        if !builtin {
            body = body.child(form_field(
                self.t("rssProviderConfigLabel", cx),
                Input::new(&self.provider_config).control(cx).w_full(),
                Some(self.t("rssProviderConfigHint", cx)),
                cx,
            ));
        }
        if let Some(feed) = self.validated.as_ref().filter(|_| self.existing.is_none()) {
            body = body.child(
                card(cx)
                    .flex()
                    .items_center()
                    .gap(tokens.spacing.sm)
                    .px(tokens.spacing.md)
                    .py(tokens.spacing.sm)
                    .child(
                        Icon::new(FluxIcon::CircleCheck)
                            .size(theme.extended().icon.lg)
                            .text_color(theme.extended().colors.success),
                    )
                    .child(
                        v_flex()
                            .min_w_0()
                            .flex_1()
                            .child(
                                div()
                                    .truncate()
                                    .text_size(tokens.typography.sm.size)
                                    .line_height(tokens.typography.sm.line_height)
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(tokens.colors.foreground)
                                    .child(SharedString::from(feed.title.clone())),
                            )
                            .child(
                                div()
                                    .text_size(tokens.typography.xs.size)
                                    .line_height(tokens.typography.xs.line_height)
                                    .text_color(tertiary)
                                    .child(SharedString::from(
                                        self.t("rssWizardFeedSummary", cx)
                                            .replace("{n}", &feed.item_count.to_string()),
                                    )),
                            ),
                    ),
            );
        }
        if self.existing.is_none() && self.validated.is_none() {
            return body;
        }

        let interval_field = form_field(
            self.t("rssIntervalLabel", cx),
            self.menu(
                "rss-editor-interval",
                self.interval_label(self.interval, cx),
                intervals,
                |this, value| {
                    if let Ok(value) = value.parse() {
                        this.interval = value;
                    }
                },
                cx,
            ),
            Some(self.t("rssIntervalHint", cx)),
            cx,
        );
        let queue_field = form_field(
            self.t("rssQueueLabel", cx),
            self.menu(
                "rss-editor-queue",
                queue_label,
                choices,
                |this, value| this.queue_id = value,
                cx,
            ),
            None,
            cx,
        );
        let save_dir_field = form_field(
            self.t("rssSaveDirLabel", cx),
            input_with_action(
                Input::new(&self.save_dir).control(cx).w_full(),
                Button::new("rss-editor-pick-dir")
                    .outline()
                    .icon(FluxIcon::FolderOpen)
                    .label(self.t("browse", cx))
                    .control(cx)
                    .disabled(self.picking_dir)
                    .on_click(
                        cx.listener(|this, _: &ClickEvent, window, cx| this.pick_dir(window, cx)),
                    ),
                cx,
            ),
            Some(self.t("rssSaveDirHint", cx)),
            cx,
        );
        let options = option_group(
            [
                self.toggle(
                    "rss-editor-enabled",
                    "rssEnabledLabel",
                    "rssEnabledDesc",
                    self.enabled,
                    |this, v| this.enabled = v,
                    cx,
                ),
                self.toggle(
                    "rss-editor-autodownload",
                    "rssAutoDownloadLabel",
                    "rssAutoDownloadDesc",
                    self.auto_download,
                    |this, v| this.auto_download = v,
                    cx,
                ),
                self.toggle(
                    "rss-editor-paused",
                    "rssStartPausedLabel",
                    "rssStartPausedDesc",
                    self.start_paused,
                    |this, v| this.start_paused = v,
                    cx,
                ),
            ],
            cx,
        );

        body.child(self.field("rssNameLabel", &self.name, cx))
            .child(form_row(
                [
                    interval_field.into_any_element(),
                    queue_field.into_any_element(),
                ],
                cx,
            ))
            .child(save_dir_field)
            .child(
                v_flex()
                    .gap(tokens.spacing.xs + tokens.spacing.xxs)
                    .child(options)
                    .child(self.hint("rssWizardSeedNote", cx)),
            )
    }

    fn interval_label(&self, minutes: i32, cx: &App) -> SharedString {
        let (key, n) = if minutes > 0 && minutes % 60 == 0 {
            ("rssEveryHours", minutes / 60)
        } else {
            ("rssEveryMinutes", minutes)
        };
        SharedString::from(self.t(key, cx).replace("{n}", &n.to_string()))
    }

    fn render_filter(&self, cx: &mut Context<Self>) -> Div {
        form(cx)
            .child(self.field("rssIncludeLabel", &self.include, cx))
            .child(self.field("rssExcludeLabel", &self.exclude, cx))
            .child(form_row(
                [
                    self.field("rssSizeMinLabel", &self.size_min, cx)
                        .into_any_element(),
                    self.field("rssSizeMaxLabel", &self.size_max, cx)
                        .into_any_element(),
                ],
                cx,
            ))
            .child(option_group(
                [
                    self.toggle(
                        "rss-editor-regex",
                        "rssUseRegexLabel",
                        "rssUseRegexDesc",
                        self.use_regex,
                        |this, v| this.use_regex = v,
                        cx,
                    ),
                    self.toggle(
                        "rss-editor-episode",
                        "rssSmartEpisodeLabel",
                        "rssSmartEpisodeDesc",
                        self.smart_episode,
                        |this, v| this.smart_episode = v,
                        cx,
                    ),
                ],
                cx,
            ))
    }

    fn render_advanced(&self, cx: &mut Context<Self>) -> Div {
        form(cx)
            .child(self.field("rssCookiesLabel", &self.cookies, cx))
            .child(self.field("rssUserAgentLabel", &self.user_agent, cx))
            .child(form_row(
                [
                    self.field("rssProxyLabel", &self.proxy, cx)
                        .into_any_element(),
                    self.field("rssMaxPerFetchLabel", &self.max_per_fetch, cx)
                        .into_any_element(),
                ],
                cx,
            ))
            .child(option_group(
                [
                    self.toggle(
                        "rss-editor-referer",
                        "rssSendRefererLabel",
                        "rssSendRefererDesc",
                        self.send_referer,
                        |this, v| this.send_referer = v,
                        cx,
                    ),
                    self.toggle(
                        "rss-editor-notify",
                        "rssNotifyLabel",
                        "rssNotifyDesc",
                        self.notify_on_download,
                        |this, v| this.notify_on_download = v,
                        cx,
                    ),
                ],
                cx,
            ))
    }
}

impl Render for Editor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = active_theme(cx).tokens().clone();
        let body = match self.tab {
            Tab::Basic => self.render_basic(cx),
            Tab::Filter => self.render_filter(cx),
            Tab::Advanced => self.render_advanced(cx),
        };
        let can_save = !self.saving
            && !self.validating
            && (self.existing.is_some() || self.validated.is_some());
        // 底栏只放「取消 / 订阅」，右对齐；「验证」已移到订阅链接行内。
        let footer = h_flex()
            .w_full()
            .pt(tokens.spacing.sm)
            .gap(tokens.spacing.sm)
            .items_center()
            .justify_end()
            .child(
                Button::new("rss-editor-cancel")
                    .outline()
                    .label(self.t("cancel", cx))
                    .control(cx)
                    .disabled(self.saving)
                    .on_click(|_, window, cx| window.close_dialog(cx)),
            )
            .child(
                Button::new("rss-editor-save")
                    .primary()
                    .label(self.t(
                        if self.existing.is_some() {
                            "confirm"
                        } else {
                            "rssWizardSubscribe"
                        },
                        cx,
                    ))
                    .control(cx)
                    .busy(self.saving)
                    .disabled(!can_save)
                    .on_click(
                        cx.listener(|this, _: &ClickEvent, window, cx| this.save(window, cx)),
                    ),
            );
        // 内容区高度随内容伸缩，超出上限或窗口高度时纵向滚动；标签条、错误行、底栏常驻可见。
        v_flex()
            .w_full()
            .min_h_0()
            .gap(tokens.spacing.lg)
            .child(h_flex().child(self.render_tabs(cx)))
            .child(dialog_scroll_body(
                "rss-editor-body",
                Some(px(460.)),
                body,
                cx,
            ))
            .when_some(self.error.clone(), |root, error| {
                root.child(field_error(SharedString::from(error), cx))
            })
            .child(footer)
    }
}

fn same_request(a: &RssValidateRequest, b: &RssValidateRequest) -> bool {
    a.provider_id == b.provider_id
        && a.provider_config == b.provider_config
        && a.url == b.url
        && a.cookies == b.cookies
        && a.user_agent == b.user_agent
        && a.proxy_url == b.proxy_url
}

fn rpc_error(translator: &Translator, error: &RpcErrorData) -> String {
    // agent 端口不透传服务端 message：按错误码给本地化文案，字段错误附带字段名。
    let key = match error.code {
        ApplicationErrorCode::Unavailable | ApplicationErrorCode::Timeout => {
            "localServiceDisconnected"
        }
        ApplicationErrorCode::InvalidArgument | ApplicationErrorCode::NotFound => {
            "localServiceInvalidArgument"
        }
        ApplicationErrorCode::Conflict => "localServiceConflict",
        _ => "localServiceActionFailed",
    };
    let text = translator.text(key);
    match &error.field {
        Some(field) => format!("{text} ({field})"),
        None => text.to_owned(),
    }
}

/// 空体积 = 不限；接受纯字节或 K/M/G/T 和可选 B 的 1024 进制表示。
/// 整数运算防止浮点精度损失与 i64 溢出。
fn parse_size(text: &str) -> Option<i64> {
    let text = text.trim();
    if text.is_empty() {
        return Some(0);
    }
    let number_end = text
        .bytes()
        .take_while(|byte| byte.is_ascii_digit() || *byte == b'.')
        .count();
    let (number, unit) = text.split_at(number_end);
    let (whole, fraction) = match number.split_once('.') {
        Some((whole, fraction)) if !fraction.is_empty() => (whole, fraction),
        Some(_) => return None,
        None => (number, ""),
    };
    if whole.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || !fraction.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let multiplier: i128 = match unit.trim().to_ascii_uppercase().as_str() {
        "" | "B" => 1,
        "K" | "KB" => 1024,
        "M" | "MB" => 1024_i128.pow(2),
        "G" | "GB" => 1024_i128.pow(3),
        "T" | "TB" => 1024_i128.pow(4),
        _ => return None,
    };
    let whole = whole.parse::<i128>().ok()?.checked_mul(multiplier)?;
    let mut fractional = 0_i128;
    let mut denominator = 1_i128;
    for digit in fraction.bytes() {
        fractional = fractional
            .checked_mul(10)?
            .checked_add(i128::from(digit - b'0'))?;
        denominator = denominator.checked_mul(10)?;
    }
    let bytes = whole.checked_add(fractional.checked_mul(multiplier)? / denominator)?;
    i64::try_from(bytes).ok()
}

fn parse_fetch_limit(text: &str) -> Option<i32> {
    text.trim()
        .parse::<i32>()
        .ok()
        .filter(|value| (1..=100).contains(value))
}

fn format_size(bytes: i64) -> String {
    if bytes <= 0 {
        return String::new();
    }
    for (unit, suffix) in [
        (1024_i64.pow(4), "T"),
        (1024_i64.pow(3), "G"),
        (1024_i64.pow(2), "M"),
        (1024, "K"),
    ] {
        if bytes >= unit && bytes % unit == 0 {
            return format!("{}{suffix}", bytes / unit);
        }
    }
    bytes.to_string()
}

#[cfg(test)]
mod tests {
    use super::{format_size, parse_fetch_limit, parse_size};

    #[test]
    fn size_parser_preserves_fractional_bytes_and_rejects_overflow_or_invalid_suffix() {
        assert_eq!(parse_size(" 1.5 GB "), Some(1_610_612_736));
        assert_eq!(parse_size("0.5K"), Some(512));
        assert_eq!(parse_size(""), Some(0));
        assert_eq!(parse_size("-2G"), None);
        assert_eq!(parse_size("1.5.2M"), None);
        assert_eq!(parse_size("2PB"), None);
        assert_eq!(parse_size("9223372036854775808"), None);
        assert_eq!(parse_size("9223372036854775807"), Some(i64::MAX));
    }

    #[test]
    fn fetch_limit_rejects_out_of_range_and_non_integer_values() {
        assert_eq!(parse_fetch_limit(" 1 "), Some(1));
        assert_eq!(parse_fetch_limit("100"), Some(100));
        for invalid in ["0", "101", "1.5", "2147483648", ""] {
            assert_eq!(parse_fetch_limit(invalid), None);
        }
    }

    #[test]
    fn formatted_sizes_round_trip_for_exact_units_and_non_multiple() {
        for size in [0, 123, 200 * 1024 * 1024, 2 * 1024_i64.pow(3), i64::MAX] {
            assert_eq!(parse_size(&format_size(size)), Some(size));
        }
    }
}
