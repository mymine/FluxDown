//! 「新建下载」保存目录的分类下拉：预测目录、选项列表与「自动」模式的跟随判定。
//!
//! 预测语义与 agent `category_dir.rs::route_save_dir` 逐条对齐（两端共用夹具
//! `native/engine/src/fixtures/category_route_cases.json`）：文件名不含 `.` 时用 URL 路径末段
//! （百分号解码）；按序取首个配置了目录且命中的规则，无规则命中才取「其他」目录；
//! 扩展名 / 正则不区分大小写，扩展名可带前导点，非法正则不命中。

use fluxdown_protocol::{CategoryRouteDto, CategoryRuleDto, CustomCategoryDto};

use super::new_download::{percent_decode_lossy, url_file_segment};

/// 下拉里的一个分类（可见、非 `all`、配置了保存目录）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NewDownloadCategory {
    pub id: String,
    /// 内置项类型（显示名走 i18n）；自定义项为 `None`，显示 `name`。
    pub builtin_type: Option<String>,
    pub name: String,
    /// 图标名（经 `category_icon` 映射，与侧栏一致）。
    pub icon: String,
    /// 已去首尾空白的分类目录，必非空。
    pub save_dir: String,
}

/// 保存目录的分类模式。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum CategoryMode {
    /// 按文件名自动选分类目录，提交时沿用 `follows` 路由。
    #[default]
    Auto,
    /// 不分类：目录框 = 初始目录，提交不路由。
    NoCategory,
    /// 显式选择某个分类：目录框 = 该分类目录，提交不路由。
    Category(String),
}

/// 下拉候选：按分类列表顺序（已按 `position` 排序），保留可见、非 `all`、目录非空的项。
pub(crate) fn category_options(categories: &[CustomCategoryDto]) -> Vec<NewDownloadCategory> {
    categories
        .iter()
        .filter(|category| {
            category.visible
                && category.builtin_type.as_deref() != Some("all")
                && !category.save_dir.trim().is_empty()
        })
        .map(|category| NewDownloadCategory {
            id: category.id.clone(),
            builtin_type: category.builtin_type.clone(),
            name: category.name.clone(),
            icon: category.icon.clone(),
            save_dir: category.save_dir.trim().to_owned(),
        })
        .collect()
}

/// 路由命中的分类保存目录；无命中 / 命中分类未配置目录时返回 `None`。
pub(crate) fn predict_save_dir<'a>(
    route: &'a CategoryRouteDto,
    file_name: &str,
    url: &str,
) -> Option<&'a str> {
    let url_name;
    let name = if file_name.contains('.') {
        file_name
    } else {
        url_name = url_file_segment(url.trim())
            .map(percent_decode_lossy)
            .filter(|decoded| decoded.contains('.'));
        url_name.as_deref().unwrap_or(file_name)
    };
    if name.is_empty() {
        return None;
    }
    if let Some(rule) = route
        .rules
        .iter()
        .find(|rule| !rule.save_dir.is_empty() && rule_matches(rule, name))
    {
        return Some(rule.save_dir.as_str());
    }
    (!route.other_dir.is_empty() && !route.rules.iter().any(|rule| rule_matches(rule, name)))
        .then_some(route.other_dir.as_str())
}

fn rule_matches(rule: &CategoryRuleDto, name: &str) -> bool {
    if rule.match_mode == "regex" {
        return !rule.regex_pattern.is_empty()
            && regex::RegexBuilder::new(&rule.regex_pattern)
                .case_insensitive(true)
                .build()
                .is_ok_and(|regex| regex.is_match(name));
    }
    let Some((_, extension)) = name.rsplit_once('.') else {
        return false;
    };
    !extension.is_empty()
        && rule.extensions.iter().any(|candidate| {
            candidate
                .trim_start_matches('.')
                .eq_ignore_ascii_case(extension)
        })
}

/// 预测目录对应的下拉项：第一个目录等于预测目录的分类。
pub(crate) fn matched_category<'a>(
    options: &'a [NewDownloadCategory],
    predicted: Option<&str>,
) -> Option<&'a NewDownloadCategory> {
    let predicted = predicted?;
    options.iter().find(|option| option.save_dir == predicted)
}

/// 「自动」模式下预测输入变化时目录框应改成的值：目录框当前值（去空白）为空、等于初始目录
/// 或等于上一次自动写入值才跟随，返回 `预测目录 ?? 初始目录`；用户手动改过则返回 `None`。
pub(crate) fn auto_follow_target(
    current: &str,
    offered: &str,
    last_auto: Option<&str>,
    predicted: Option<&str>,
) -> Option<String> {
    let current = current.trim();
    let offered = offered.trim();
    let following =
        current.is_empty() || current == offered || last_auto.is_some_and(|auto| auto == current);
    following.then(|| predicted.unwrap_or(offered).to_owned())
}

#[cfg(test)]
mod tests {
    use fluxdown_protocol::{CategoryRouteDto, CustomCategoryDto};
    use serde::Deserialize;

    use super::{auto_follow_target, category_options, matched_category, predict_save_dir};

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Case {
        name: String,
        route: CategoryRouteDto,
        file_name: String,
        url: String,
        expected: Option<String>,
    }

    #[test]
    fn prediction_matches_the_shared_route_fixture() {
        let cases: Vec<Case> = serde_json::from_str(include_str!(
            "../../../../native/engine/src/fixtures/category_route_cases.json"
        ))
        .expect("fixture parses");
        assert!(!cases.is_empty());
        for case in cases {
            assert_eq!(
                predict_save_dir(&case.route, &case.file_name, &case.url),
                case.expected.as_deref(),
                "{}",
                case.name
            );
        }
    }

    fn category(id: &str, builtin: Option<&str>, dir: &str, visible: bool) -> CustomCategoryDto {
        CustomCategoryDto {
            id: id.to_owned(),
            name: id.to_owned(),
            icon: "file".to_owned(),
            match_mode: "extension".to_owned(),
            extensions: Vec::new(),
            regex_pattern: String::new(),
            position: 0,
            visible,
            is_builtin: builtin.is_some(),
            builtin_type: builtin.map(str::to_owned),
            save_dir: dir.to_owned(),
        }
    }

    #[test]
    fn options_keep_visible_non_all_categories_with_a_directory() {
        let options = category_options(&[
            category("all", Some("all"), "/all", true),
            category("video", Some("video"), " /videos ", true),
            category("hidden", Some("audio"), "/music", false),
            category("blank", Some("image"), "  ", true),
            category("mine", None, "/mine", true),
            category("other", Some("other"), "/other", true),
        ]);
        let ids: Vec<_> = options.iter().map(|option| option.id.as_str()).collect();
        assert_eq!(ids, ["video", "mine", "other"]);
        assert_eq!(options[0].save_dir, "/videos");
        assert_eq!(
            matched_category(&options, Some("/mine")).map(|option| option.id.as_str()),
            Some("mine")
        );
        assert!(matched_category(&options, Some("/nowhere")).is_none());
        assert!(matched_category(&options, None).is_none());
    }

    #[test]
    fn auto_follow_only_while_the_user_has_not_edited_the_directory() {
        // 空 / 等于初始目录 / 等于上次自动写入值：跟随预测（无预测回到初始目录）。
        assert_eq!(
            auto_follow_target("", "/dl", None, Some("/programs")).as_deref(),
            Some("/programs")
        );
        assert_eq!(
            auto_follow_target(" /dl ", "/dl", None, None).as_deref(),
            Some("/dl")
        );
        assert_eq!(
            auto_follow_target("/programs", "/dl", Some("/programs"), Some("/videos")).as_deref(),
            Some("/videos")
        );
        assert_eq!(
            auto_follow_target("/programs", "/dl", Some("/programs"), None).as_deref(),
            Some("/dl")
        );
        // 用户手动改过：不覆盖。
        assert_eq!(
            auto_follow_target("/mine", "/dl", Some("/programs"), Some("/videos")),
            None
        );
        assert_eq!(
            auto_follow_target("/programs", "/dl", None, Some("/videos")),
            None
        );
    }
}
