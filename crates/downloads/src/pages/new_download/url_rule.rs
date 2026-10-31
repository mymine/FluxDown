//! 「新建下载」里的快捷添加网址规则对话框：从当前链接自动识别网址模式（CDN 子域通配、
//! 哈希路径段丢弃，见 `model::save_dir_category::suggest_url_pattern`），确认后追加进
//! `custom_categories` 偏好（云同步，与设置页同一条写入链路）并让本表单立即按新规则
//! 预测保存目录（网址规则固定优先）。
//!
//! 偏好写入由对话框自己发起：保存时在表单当前持有的最新 `custom_categories`（宿主随偏好
//! 快照推送刷新，见 `NewDownloadView::set_custom_categories`）上追加后整体写回，不使用
//! 打开时的旧快照；写入失败以表单错误呈现，成功后才在表单本地生效并关闭对话框。

use std::sync::Arc;

use fluxdown_ui_components::{
    ControlExt as _, dialog_scroll_body, dialog_title, field_error, form, form_field,
    input_with_action,
};
use fluxdown_ui_i18n::Translator;
use fluxdown_ui_theme::active_theme;
use gpui::{
    App, AppContext as _, ClickEvent, Context, Entity, IntoElement, ParentElement, Render,
    SharedString, Styled, WeakEntity, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    Disableable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputState},
};

use super::NewDownloadView;
use crate::controller::{DownloadsCommand, DownloadsPort};
use crate::model::save_dir_category::{suggest_rule_name, suggest_url_pattern};

/// 快捷添加的网址规则对话框。保存即生效：偏好整体写回，表单本地立刻按新规则预测目录。
pub(super) struct UrlRuleDialog {
    view: WeakEntity<NewDownloadView>,
    port: Arc<dyn DownloadsPort>,
    translator: Translator,
    saving: bool,
    name: Entity<InputState>,
    pattern: Entity<InputState>,
    save_dir: Entity<InputState>,
    picking_dir: bool,
    error: Option<SharedString>,
}

/// 打开对话框：模式与名称由第一条链接自动识别，目录预填当前表单的保存目录。
pub(super) fn open(view: Entity<NewDownloadView>, window: &mut Window, cx: &mut App) {
    let (translator, url, dir, port) = view.read_with(cx, |view, cx| {
        let url = view.first_http_url().unwrap_or_default();
        let current = view.save_dir.read(cx).value().trim().to_owned();
        let dir = if current.is_empty() {
            view.context.save_dir.trim().to_owned()
        } else {
            current
        };
        (
            view.translator.read(cx).clone(),
            url,
            dir,
            view.port.clone(),
        )
    });
    let title = SharedString::from(translator.text("addUrlRuleTitle").to_owned());
    let dialog = cx.new(|cx| {
        let name_hint = SharedString::from(translator.text("categoryNameHint").to_owned());
        let pattern_hint = SharedString::from(translator.text("urlPatternHint").to_owned());
        let dir_hint = SharedString::from(translator.text("selectSaveDir").to_owned());
        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(suggest_rule_name(&url))
                .placeholder(name_hint)
        });
        let pattern = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(suggest_url_pattern(&url))
                .placeholder(pattern_hint)
        });
        let save_dir = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(dir)
                .placeholder(dir_hint)
        });
        UrlRuleDialog {
            view: view.downgrade(),
            port,
            translator,
            name,
            pattern,
            save_dir,
            saving: false,
            picking_dir: false,
            error: None,
        }
    });
    let pattern_focus = dialog.read_with(cx, |this, _| this.pattern.clone());
    window.open_dialog(cx, move |inner, _, cx| {
        let dialog = dialog.clone();
        inner
            .title(dialog_title(title.clone(), cx))
            .w(active_theme(cx).text_extent(520.))
            .content(move |content, _, _| content.min_h_0().child(dialog.clone()))
    });
    pattern_focus.update(cx, |input, cx| input.focus(window, cx));
}

impl UrlRuleDialog {
    fn t(&self, key: &str) -> SharedString {
        SharedString::from(self.translator.text(key).to_owned())
    }

    fn fail(&mut self, key: &str, cx: &mut Context<Self>) {
        self.error = Some(self.t(key));
        cx.notify();
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let name = self.name.read(cx).value().trim().to_owned();
        if name.is_empty() {
            self.fail("categoryNameRequired", cx);
            return;
        }
        let pattern = self.pattern.read(cx).value().trim().to_owned();
        if pattern.is_empty() {
            self.fail("urlPatternRequired", cx);
            return;
        }
        let save_dir = self.save_dir.read(cx).value().trim().to_owned();
        if save_dir.is_empty() {
            self.fail("urlRuleDirRequired", cx);
            return;
        }
        // 以表单当前持有的最新分类列表为基准（而非打开对话框时的快照）。
        let Ok(mut list) = self
            .view
            .read_with(cx, |view, _| view.context.custom_categories.clone())
        else {
            self.fail("urlRuleSaveFailed", cx);
            return;
        };
        let entry = fluxdown_protocol::CustomCategoryDto {
            id: unique_category_id(&list),
            name,
            icon: "globe".to_owned(),
            match_mode: "url".to_owned(),
            extensions: Vec::new(),
            regex_pattern: String::new(),
            url_pattern: pattern,
            position: 999,
            visible: true,
            is_builtin: false,
            builtin_type: None,
            save_dir,
        };
        list.push(entry.clone());
        // 编码失败绝不能回退空数组写入（那会清空用户全部自定义分类）。
        let Ok(value) = serde_json::to_value(&list) else {
            self.fail("urlRuleSaveFailed", cx);
            return;
        };
        let write = self.port.execute(DownloadsCommand::SetSyncedPreference {
            key: fluxdown_protocol::CUSTOM_CATEGORIES_PREF_KEY,
            value,
        });
        self.saving = true;
        self.error = None;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = write.await;
            let Ok(()) = this.update_in(cx, |this, window, cx| {
                this.saving = false;
                if let Err(error) = result {
                    eprintln!(
                        "url rule dialog: preference write failed: {:?} ({:?})",
                        error.code, error.reason
                    );
                    this.fail("urlRuleSaveFailed", cx);
                    return;
                }
                // 表单本地立刻生效（预测目录跟随新规则）；对话框渲染在表单所在窗口里，
                // 直接用当前窗口 + 普通 update（`update_in` 会按目标实体窗口关联解析而失败）。
                let view = this.view.clone();
                if view
                    .update(cx, |view, cx| view.apply_url_rule(entry, window, cx))
                    .is_err()
                {
                    eprintln!("url rule dialog: new download view unavailable for local apply");
                }
                window.close_dialog(cx);
            }) else {
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
        let receiver = cx.prompt_for_paths(gpui::PathPromptOptions {
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
                return;
            };
        })
        .detach();
    }

    fn render_dir_field(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = active_theme(cx).tokens().clone();
        let has_value = !self.save_dir.read(cx).value().trim().is_empty();
        let actions = h_flex()
            .gap(tokens.spacing.sm)
            .child(
                Button::new("url-rule-pick-dir")
                    .outline()
                    .label(self.t("browse"))
                    .control(cx)
                    .disabled(self.picking_dir)
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.pick_dir(window, cx);
                    })),
            )
            .when(has_value, |this| {
                this.child(
                    Button::new("url-rule-clear-dir")
                        .ghost()
                        .label(self.t("restoreDefaultPath"))
                        .control(cx)
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.save_dir
                                .update(cx, |input, cx| input.set_value("", window, cx));
                            cx.notify();
                        })),
                )
            });
        form_field(
            self.t("categorySaveDir"),
            input_with_action(Input::new(&self.save_dir).control(cx).w_full(), actions, cx),
            Some(self.t("categorySaveDirDesc")),
            cx,
        )
    }
}

impl Render for UrlRuleDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = active_theme(cx).tokens().clone();
        let body = form(cx)
            .child(form_field(
                self.t("categoryName"),
                Input::new(&self.name).control(cx).w_full(),
                None,
                cx,
            ))
            .child(form_field(
                self.t("urlPatternLabel"),
                Input::new(&self.pattern).control(cx).w_full(),
                Some(self.t("urlPatternDesc")),
                cx,
            ))
            .child(self.render_dir_field(cx));
        div()
            .flex()
            .flex_col()
            .w_full()
            .min_h_0()
            .gap(tokens.spacing.lg)
            .child(dialog_scroll_body(
                "url-rule-dialog-body",
                Some(px(360.)),
                body,
                cx,
            ))
            .when_some(self.error.clone(), |this, error| {
                this.child(field_error(error, cx))
            })
            .child(
                h_flex()
                    .w_full()
                    .justify_end()
                    .gap(tokens.spacing.sm)
                    .pt(tokens.spacing.sm)
                    .child(
                        Button::new("url-rule-cancel")
                            .outline()
                            .label(self.t("cancel"))
                            .control(cx)
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("url-rule-save")
                            .primary()
                            .label(self.t("confirm"))
                            .control(cx)
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.save(window, cx);
                            })),
                    )
                    .into_any_element(),
            )
    }
}

/// 当前 Unix 毫秒；时钟异常时退化为 0（仅用于生成 id）。
fn unix_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

/// 新建分类 id：`custom_<unix 毫秒>`；与既有 id 冲突（时钟异常退化、同毫秒新建）时追加
/// `-2`、`-3` … 序号，保证追加永远发生（不因 id 撞车被静默跳过）。
fn unique_category_id(existing: &[fluxdown_protocol::CustomCategoryDto]) -> String {
    let base = format!("custom_{}", unix_ms());
    if !existing.iter().any(|known| known.id == base) {
        return base;
    }
    let mut suffix = 2_u64;
    loop {
        let candidate = format!("{base}-{suffix}");
        if !existing.iter().any(|known| known.id == candidate) {
            return candidate;
        }
        suffix += 1;
    }
}

impl NewDownloadView {
    /// 首条 http(s) 链接原文（识别建议的来源）；没有时为 `None`。
    pub(super) fn first_http_url(&self) -> Option<String> {
        self.entries
            .iter()
            .map(|entry| entry.url.trim())
            .find(|url| {
                url.split_once("://").is_some_and(|(scheme, _)| {
                    matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https")
                })
            })
            .map(str::to_owned)
    }
}
