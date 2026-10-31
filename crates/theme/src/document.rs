//! 主题文件（wire）：`ThemeDocument`。
//!
//! 所有字段可选，未知键原样保留并在导出时写回；token 层保持原始 JSON，非法值只在
//! 解析（[`crate::resolve`]）时回退并记诊断，文件本身无损往返。

use std::fmt;

use gpui_component::ThemeMode;
use serde_json::{Map, Value};

use crate::builtin::{ACCENT_TOKEN_PATHS, BuiltinBase};
use crate::json::OrderedJson;
use crate::registry::{GROUPS, TOKENS, TokenKind, token};
use crate::resolve::{ResolveOptions, resolve_with};
use crate::value::{normalized_literal, ordered_shadow_wire};
use crate::{BuiltinThemeId, flutter, migrate};

/// 主题文件 `format` 字段取值。
pub const THEME_FORMAT: &str = "fluxdown.gpui-theme";
/// 当前 schema 版本。
pub const THEME_SCHEMA_VERSION: u32 = 2;
/// 导出时写入的 `$schema`。解析不校验该值：旧域名（fluxdown.zerx.dev）导出的主题照常读取。
pub const THEME_SCHEMA_URL: &str = "https://fluxdown.com/schemas/gpui-theme.v2.json";

/// 诊断类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiagnosticKind {
    /// 值类型或语法非法（含悬空引用），字段回退到下一层。
    InvalidValue,
    /// 未知键：保留并原样导出，不参与解析。
    UnknownKey,
    /// 越界：已 clamp 到范围内。
    OutOfRange,
    /// 文件 schema 版本高于本客户端：尽力加载。
    NewerVersion,
    /// `extends` 不是已知内置基底：回退 `builtin:default`。
    UnknownExtends,
    /// 引用成环：该层取值作废，回退到下一层。
    RefCycle,
    /// 由旧格式（v1 内部格式 / Flutter FluxThemeJson）转换而来。
    Migrated,
}

impl DiagnosticKind {
    #[must_use]
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::InvalidValue => "invalidValue",
            Self::UnknownKey => "unknownKey",
            Self::OutOfRange => "outOfRange",
            Self::NewerVersion => "newerVersion",
            Self::UnknownExtends => "unknownExtends",
            Self::RefCycle => "refCycle",
            Self::Migrated => "migrated",
        }
    }
}

/// 加载 / 解析过程中的非致命问题。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Diagnostic {
    /// 文件内路径（如 `dark.colors.primary`、`meta.name`、`schemaVersion`）。
    pub path: String,
    pub kind: DiagnosticKind,
    pub message: String,
}

impl Diagnostic {
    pub(crate) fn new(
        path: impl Into<String>,
        kind: DiagnosticKind,
        message: impl Into<String>,
    ) -> Self {
        Self {
            path: path.into(),
            kind,
            message: message.into(),
        }
    }

    /// `{ "path", "kind", "message" }`。
    #[must_use]
    pub fn to_json(&self) -> OrderedJson {
        ordered_json!({
            "path": (self.path.as_str()),
            "kind": (self.kind.wire_name()),
            "message": (self.message.as_str()),
        })
    }
}

/// 整体拒绝加载的错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThemeParseError {
    /// 不是合法 JSON。
    InvalidJson(String),
    /// 顶层不是 JSON 对象。
    NotAnObject,
    /// `format` 不是 `fluxdown.gpui-theme`，也不是可识别的 Flutter 主题。
    UnsupportedFormat(String),
}

impl fmt::Display for ThemeParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidJson(error) => write!(f, "主题文件不是合法 JSON：{error}"),
            Self::NotAnObject => f.write_str("主题文件顶层必须是 JSON 对象"),
            Self::UnsupportedFormat(format) => write!(f, "不支持的主题格式：{format}"),
        }
    }
}

impl std::error::Error for ThemeParseError {}

/// 导出模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportMode {
    /// 只写与基底（`extends` + 注册表默认）不同的值；引用始终保留。
    Diff,
    /// 两个模式的全部 token 解析为字面量显式写出（两模式相同的写入 `tokens`）；
    /// kitBound token 不写。
    Full,
}

/// token 层。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenLayer {
    /// `tokens`：两个模式共享。
    Shared,
    Dark,
    Light,
}

impl TokenLayer {
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Shared => "tokens",
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }

    #[must_use]
    pub fn for_mode(mode: ThemeMode) -> Self {
        match mode {
            ThemeMode::Dark => Self::Dark,
            ThemeMode::Light => Self::Light,
        }
    }
}

/// `meta` 段。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ThemeMeta {
    pub id: Option<String>,
    pub name: Option<String>,
    pub author: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    /// 未知键（原样保留）。
    pub extra: Map<String, Value>,
}

const META_KEYS: [&str; 5] = ["id", "name", "author", "version", "description"];

impl ThemeMeta {
    fn field(&mut self, key: &str) -> Option<&mut Option<String>> {
        Some(match key {
            "id" => &mut self.id,
            "name" => &mut self.name,
            "author" => &mut self.author,
            "version" => &mut self.version,
            "description" => &mut self.description,
            _ => return None,
        })
    }

    fn to_json(&self) -> OrderedJson {
        let mut entries = Vec::new();
        let mut meta = self.clone();
        for key in META_KEYS {
            if let Some(Some(value)) = meta.field(key).map(Option::take) {
                entries.push((key.to_owned(), value.into()));
            }
        }
        for (key, value) in &self.extra {
            entries.push((key.clone(), value.clone().into()));
        }
        OrderedJson::Object(entries)
    }
}

/// 主题文件。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ThemeDocument {
    /// `$schema`。
    pub schema: Option<String>,
    pub format: Option<String>,
    /// 文件声明的版本（迁移后为当前版本；更高版本原样保留）。
    pub schema_version: Option<u32>,
    pub meta: Option<ThemeMeta>,
    /// `builtin:*`；缺省为 `builtin:default`。
    pub extends: Option<String>,
    /// `tokens`：两个模式共享的覆盖（原始 JSON，按 `group.key` 嵌套）。
    pub tokens: Map<String, Value>,
    pub dark: Map<String, Value>,
    pub light: Map<String, Value>,
    /// 顶层未知键（原样保留）。
    pub extra: Map<String, Value>,
}

impl ThemeDocument {
    /// FluxDown 默认亮暗对（`extends: builtin:default`）。
    #[must_use]
    pub fn fluxdown_default() -> Self {
        Self::with_base(BuiltinBase::Default, "FluxDown Default")
    }

    /// 单个内置预设：预设自身外观的模式取其调色板，另一模式沿用默认主题。
    #[must_use]
    pub fn builtin(id: BuiltinThemeId) -> Self {
        Self::with_base(BuiltinBase::from(id), builtin_name(id))
    }

    fn with_base(base: BuiltinBase, name: &str) -> Self {
        Self {
            format: Some(THEME_FORMAT.into()),
            schema_version: Some(THEME_SCHEMA_VERSION),
            meta: Some(ThemeMeta {
                id: Some(base.extends_value().into()),
                name: Some(name.into()),
                author: Some("FluxDown".into()),
                ..ThemeMeta::default()
            }),
            extends: Some(base.extends_value().into()),
            ..Self::default()
        }
    }

    /// 解析主题 JSON（含 v1 内部格式迁移与 Flutter FluxThemeJson 适配）。
    ///
    /// # Errors
    /// 非 JSON、顶层非对象，或 `format` 既不是本格式也不是可识别的 Flutter 主题。
    pub fn parse(text: &str) -> Result<(Self, Vec<Diagnostic>), ThemeParseError> {
        let value: Value = serde_json::from_str(text)
            .map_err(|error| ThemeParseError::InvalidJson(error.to_string()))?;
        Self::from_value(value)
    }

    /// 同 [`Self::parse`]，输入为已解析的 JSON。
    ///
    /// # Errors
    /// 同 [`Self::parse`]。
    pub fn from_value(value: Value) -> Result<(Self, Vec<Diagnostic>), ThemeParseError> {
        let Value::Object(mut object) = value else {
            return Err(ThemeParseError::NotAnObject);
        };
        let mut diagnostics = Vec::new();
        let is_ours =
            matches!(object.get("format"), Some(Value::String(format)) if format == THEME_FORMAT);
        if !is_ours {
            if flutter::is_flutter_theme(&object) {
                object = flutter::convert(&object, &mut diagnostics);
            } else if let Some(format) = object.get("format") {
                return Err(ThemeParseError::UnsupportedFormat(format.to_string()));
            } else if migrate::is_v1(&object) {
                object = migrate::migrate_v1(object, &mut diagnostics);
            }
        }

        let mut document = Self::default();
        let mut version = THEME_SCHEMA_VERSION;
        for (key, value) in object {
            match key.as_str() {
                "$schema" => document.schema = string_field(&key, value, &mut diagnostics),
                "format" => document.format = string_field(&key, value, &mut diagnostics),
                "schemaVersion" => match value.as_u64().and_then(|v| u32::try_from(v).ok()) {
                    Some(declared) => version = declared,
                    None => diagnostics.push(Diagnostic::new(
                        key,
                        DiagnosticKind::InvalidValue,
                        format!(
                            "schemaVersion 须为正整数：{value}，按 {THEME_SCHEMA_VERSION} 处理"
                        ),
                    )),
                },
                "meta" => document.meta = parse_meta(value, &mut diagnostics),
                "extends" => document.extends = string_field(&key, value, &mut diagnostics),
                "tokens" | "dark" | "light" => {
                    let Value::Object(layer) = value else {
                        diagnostics.push(Diagnostic::new(
                            key,
                            DiagnosticKind::InvalidValue,
                            "token 层须为对象，已忽略",
                        ));
                        continue;
                    };
                    check_layer(&key, &layer, &mut diagnostics);
                    match key.as_str() {
                        "tokens" => document.tokens = layer,
                        "dark" => document.dark = layer,
                        _ => document.light = layer,
                    }
                }
                _ => {
                    diagnostics.push(Diagnostic::new(
                        key.clone(),
                        DiagnosticKind::UnknownKey,
                        "未知顶层键，已原样保留",
                    ));
                    document.extra.insert(key, value);
                }
            }
        }

        if version > THEME_SCHEMA_VERSION {
            diagnostics.push(Diagnostic::new(
                "schemaVersion",
                DiagnosticKind::NewerVersion,
                format!(
                    "文件 schemaVersion {version} 高于支持的 {THEME_SCHEMA_VERSION}，已尽力加载"
                ),
            ));
        } else if version < THEME_SCHEMA_VERSION {
            for layer in [
                &mut document.tokens,
                &mut document.dark,
                &mut document.light,
            ] {
                migrate::apply_steps(layer, version, migrate::MIGRATIONS);
            }
            version = THEME_SCHEMA_VERSION;
        }
        document.schema_version = Some(version);
        document.format = Some(THEME_FORMAT.into());
        Ok((document, diagnostics))
    }

    /// 指定层。
    #[must_use]
    pub fn layer(&self, layer: TokenLayer) -> &Map<String, Value> {
        match layer {
            TokenLayer::Shared => &self.tokens,
            TokenLayer::Dark => &self.dark,
            TokenLayer::Light => &self.light,
        }
    }

    fn layer_mut(&mut self, layer: TokenLayer) -> &mut Map<String, Value> {
        match layer {
            TokenLayer::Shared => &mut self.tokens,
            TokenLayer::Dark => &mut self.dark,
            TokenLayer::Light => &mut self.light,
        }
    }

    /// 读取某层某路径的原始值。
    #[must_use]
    pub fn token_value(&self, layer: TokenLayer, path: &str) -> Option<&Value> {
        get_path(self.layer(layer), path)
    }

    /// 写入某层某路径（按 `.` 嵌套创建对象）。
    pub fn set_token(&mut self, layer: TokenLayer, path: &str, value: Value) {
        set_path(self.layer_mut(layer), path, value);
    }

    /// 删除某层某路径，并清理因此变空的父对象。
    pub fn remove_token(&mut self, layer: TokenLayer, path: &str) -> Option<Value> {
        remove_path(self.layer_mut(layer), path)
    }

    /// 导出 JSON 的 `Value` 形式（对象键序由 `serde_json::Map` 的实现决定；需要规范
    /// 键序时用 [`Self::to_ordered_json`] / [`Self::to_json_pretty`]）。
    #[must_use]
    pub fn to_json_value(&self, mode: ExportMode) -> Value {
        self.to_ordered_json(mode).to_value()
    }

    /// 规范键序：`$schema format schemaVersion meta extends tokens dark light` 在前，
    /// token 层按注册表顺序，未知键排在各自对象末尾。
    #[must_use]
    pub fn to_ordered_json(&self, mode: ExportMode) -> OrderedJson {
        let mut document = match mode {
            ExportMode::Diff => self.minimized(),
            ExportMode::Full => self.expanded(),
        };
        document.normalize_literals();
        let version = document
            .schema_version
            .unwrap_or(THEME_SCHEMA_VERSION)
            .max(THEME_SCHEMA_VERSION);
        let mut entries = vec![
            ("$schema".to_owned(), THEME_SCHEMA_URL.into()),
            ("format".to_owned(), THEME_FORMAT.into()),
            ("schemaVersion".to_owned(), version.into()),
        ];
        if let Some(meta) = &document.meta {
            entries.push(("meta".into(), meta.to_json()));
        }
        if let Some(extends) = &document.extends {
            entries.push(("extends".into(), extends.as_str().into()));
        }
        for layer in [TokenLayer::Shared, TokenLayer::Dark, TokenLayer::Light] {
            let map = document.layer(layer);
            if !map.is_empty() {
                entries.push((layer.key().into(), canonical_object(map, "")));
            }
        }
        for (key, value) in &document.extra {
            entries.push((key.clone(), value.clone().into()));
        }
        OrderedJson::Object(entries)
    }

    /// 规范键序、两空格缩进、末尾换行。
    #[must_use]
    pub fn to_json_pretty(&self, mode: ExportMode) -> String {
        self.to_ordered_json(mode).to_pretty()
    }

    /// 合法字面量改写为规范形式（颜色小写 `#rrggbbaa`、数字最短表示）；引用与非法值原样保留。
    fn normalize_literals(&mut self) {
        for layer in [TokenLayer::Shared, TokenLayer::Dark, TokenLayer::Light] {
            for spec in TOKENS {
                let Some(normalized) = self
                    .token_value(layer, spec.path)
                    .and_then(|value| normalized_literal(spec.kind, value))
                else {
                    continue;
                };
                self.set_token(layer, spec.path, normalized);
            }
        }
    }

    /// 反复去掉「删掉后两模式全部 token 的导出值都不变」的字面量覆盖，直到不动点；
    /// 引用、未知键与 [`ACCENT_TOKEN_PATHS`] 上的字面量保留（比较在无强调色下进行，
    /// 删掉它们会让运行时强调色覆盖用户显式固定的值）。
    fn minimized(&self) -> Self {
        let options = ResolveOptions::default();
        let snapshot = |document: &Self, mode: ThemeMode| {
            resolve_with(document, mode, &options).0.to_flat_json()
        };
        let mut document = self.clone();
        let mut current = [ThemeMode::Dark, ThemeMode::Light].map(|mode| snapshot(&document, mode));
        loop {
            let mut changed = false;
            for layer in [TokenLayer::Dark, TokenLayer::Light, TokenLayer::Shared] {
                for spec in TOKENS {
                    let Some(value) = document.token_value(layer, spec.path) else {
                        continue;
                    };
                    if matches!(value, Value::String(text) if text.starts_with('{'))
                        || ACCENT_TOKEN_PATHS.contains(&spec.path)
                    {
                        continue;
                    }
                    let mut without = document.clone();
                    without.remove_token(layer, spec.path);
                    let next =
                        [ThemeMode::Dark, ThemeMode::Light].map(|mode| snapshot(&without, mode));
                    if next == current {
                        document = without;
                        current = next;
                        changed = true;
                    }
                }
            }
            if !changed {
                return document;
            }
        }
    }

    /// 全部 token 解析为字面量写出。
    fn expanded(&self) -> Self {
        let mut document = self.clone();
        let options = ResolveOptions::default();
        let dark = resolve_with(self, ThemeMode::Dark, &options).0;
        let light = resolve_with(self, ThemeMode::Light, &options).0;
        for layer in [TokenLayer::Shared, TokenLayer::Dark, TokenLayer::Light] {
            for spec in TOKENS {
                document.remove_token(layer, spec.path);
            }
        }
        for spec in TOKENS.iter().filter(|spec| !spec.kit_bound) {
            let (Some(dark_value), Some(light_value)) = (dark.get(spec.path), light.get(spec.path))
            else {
                continue;
            };
            let (dark_json, light_json) = (dark_value.to_json(), light_value.to_json());
            if dark_json == light_json {
                document.set_token(TokenLayer::Shared, spec.path, dark_json);
            } else {
                document.set_token(TokenLayer::Dark, spec.path, dark_json);
                document.set_token(TokenLayer::Light, spec.path, light_json);
            }
        }
        document
    }
}

fn builtin_name(id: BuiltinThemeId) -> &'static str {
    match id {
        BuiltinThemeId::DefaultDark => "Default Dark",
        BuiltinThemeId::DefaultLight => "Default Light",
        BuiltinThemeId::MidnightBlue => "Midnight Blue",
        BuiltinThemeId::Nord => "Nord",
        BuiltinThemeId::WarmLight => "Warm Light",
        BuiltinThemeId::Graphite => "Graphite",
        BuiltinThemeId::GraphiteLight => "Graphite Light",
    }
}

fn string_field(key: &str, value: Value, diagnostics: &mut Vec<Diagnostic>) -> Option<String> {
    match value {
        Value::String(text) => Some(text),
        other => {
            diagnostics.push(Diagnostic::new(
                key,
                DiagnosticKind::InvalidValue,
                format!("须为字符串：{other}，已忽略"),
            ));
            None
        }
    }
}

fn parse_meta(value: Value, diagnostics: &mut Vec<Diagnostic>) -> Option<ThemeMeta> {
    let Value::Object(object) = value else {
        diagnostics.push(Diagnostic::new(
            "meta",
            DiagnosticKind::InvalidValue,
            "meta 须为对象，已忽略",
        ));
        return None;
    };
    let mut meta = ThemeMeta::default();
    for (key, value) in object {
        let path = format!("meta.{key}");
        if let Some(field) = meta.field(&key) {
            *field = string_field(&path, value, diagnostics);
        } else {
            diagnostics.push(Diagnostic::new(
                path,
                DiagnosticKind::UnknownKey,
                "未知 meta 键，已原样保留",
            ));
            meta.extra.insert(key, value);
        }
    }
    Some(meta)
}

/// 路径 `prefix` 是否是某个 token 的祖先（中间对象）。
fn is_token_prefix(prefix: &str) -> bool {
    TOKENS.iter().any(|spec| {
        spec.path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('.'))
    })
}

/// 未知键 / 结构错误诊断（值本身在解析时校验）。
fn check_layer(layer: &str, map: &Map<String, Value>, diagnostics: &mut Vec<Diagnostic>) {
    fn walk(
        layer: &str,
        prefix: &str,
        map: &Map<String, Value>,
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        for (key, value) in map {
            let path = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            if token(&path).is_some() {
                continue;
            }
            if is_token_prefix(&path) {
                match value {
                    Value::Object(child) => walk(layer, &path, child, diagnostics),
                    other => diagnostics.push(Diagnostic::new(
                        format!("{layer}.{path}"),
                        DiagnosticKind::InvalidValue,
                        format!("须为对象：{other}，已忽略"),
                    )),
                }
            } else {
                diagnostics.push(Diagnostic::new(
                    format!("{layer}.{path}"),
                    DiagnosticKind::UnknownKey,
                    "未知 token，已原样保留",
                ));
            }
        }
    }
    walk(layer, "", map, diagnostics);
}

pub(crate) fn get_path<'a>(map: &'a Map<String, Value>, path: &str) -> Option<&'a Value> {
    let mut segments = path.split('.');
    let mut current = map.get(segments.next()?)?;
    for segment in segments {
        current = current.as_object()?.get(segment)?;
    }
    Some(current)
}

fn set_path(map: &mut Map<String, Value>, path: &str, value: Value) {
    match path.split_once('.') {
        None => {
            map.insert(path.to_owned(), value);
        }
        Some((head, rest)) => {
            let child = map
                .entry(head.to_owned())
                .or_insert_with(|| Value::Object(Map::new()));
            if !child.is_object() {
                *child = Value::Object(Map::new());
            }
            if let Value::Object(child) = child {
                set_path(child, rest, value);
            }
        }
    }
}

pub(crate) fn remove_path(map: &mut Map<String, Value>, path: &str) -> Option<Value> {
    match path.split_once('.') {
        None => remove_key(map, path),
        Some((head, rest)) => {
            let Value::Object(child) = map.get_mut(head)? else {
                return None;
            };
            let removed = remove_path(child, rest);
            if removed.is_some() && child.is_empty() {
                remove_key(map, head);
            }
            removed
        }
    }
}

/// 删除键且保持其余键的相对顺序（`preserve_order` 下 `Map::remove` 会换位）。
fn remove_key(map: &mut Map<String, Value>, key: &str) -> Option<Value> {
    let mut removed = None;
    map.retain(|candidate, value| {
        if candidate == key {
            removed = Some(value.take());
            false
        } else {
            true
        }
    });
    removed
}

/// 子键按注册表顺序排列，未知键排在最后（按 `Map` 迭代顺序）。
fn canonical_object(map: &Map<String, Value>, prefix: &str) -> OrderedJson {
    let order = |path: &str| -> Option<usize> {
        if prefix.is_empty() {
            return GROUPS.iter().position(|group| *group == path);
        }
        TOKENS.iter().position(|spec| {
            spec.path == path
                || spec
                    .path
                    .strip_prefix(path)
                    .is_some_and(|rest| rest.starts_with('.'))
        })
    };
    let mut known: Vec<(usize, &String, String, &Value)> = Vec::new();
    let mut unknown: Vec<(String, OrderedJson)> = Vec::new();
    for (key, value) in map {
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        match order(&path) {
            Some(index) => known.push((index, key, path, value)),
            None => unknown.push((key.clone(), value.clone().into())),
        }
    }
    known.sort_by_key(|(index, ..)| *index);
    let mut entries: Vec<(String, OrderedJson)> = known
        .into_iter()
        .map(|(_, key, path, value)| {
            let value = match (token(&path), value) {
                (Some(spec), value) if spec.kind == TokenKind::Shadow => ordered_shadow_wire(value),
                (None, Value::Object(child)) => canonical_object(child, &path),
                (_, value) => value.clone().into(),
            };
            (key.clone(), value)
        })
        .collect();
    entries.append(&mut unknown);
    OrderedJson::Object(entries)
}
