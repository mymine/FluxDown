//! 主题解析：`ThemeDocument` + 模式 → 每个注册 token 的最终值。
//!
//! 每个 token 依次尝试：`<mode>` 层 → `tokens` 共享层 → `extends` 基底（Base token）或
//! 注册表默认表达式。某层取值非法（类型错、语法错、悬空引用）或引用成环时记诊断并
//! 回退到下一层；数值最后按注册表范围 clamp，行高另受「不小于同角色字号」约束。
//! kitBound token 忽略文件覆盖。求值按注册表顺序进行，结果记忆化。

use std::collections::{HashMap, HashSet};

use gpui::{Hsla, Rgba};
use gpui_component::ThemeMode;
use serde_json::{Map, Value};

use crate::builtin::{BuiltinBase, base_values};
use crate::document::get_path;
use crate::extended::{primary_with_contrast, with_min_contrast};
use crate::json::OrderedJson;
use crate::registry::{ByMode, DefaultExpr, Literal, TOKENS, TokenKind, TokenSpec, token};
use crate::value::{
    ColorSource, Reference, ShadowValue, TokenValue, WireValue, parse_wire, rgba_color,
};
use crate::{Diagnostic, DiagnosticKind, ResolvedTheme, ThemeDocument};

/// 解析选项（运行时策略；网站预览与 `*.resolved.json` 均使用默认值）。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ResolveOptions {
    /// 用户强调色：作用于 `extends` 基底（文件显式值仍优先）。
    pub accent: Option<Hsla>,
    /// 亮色模式下把 `colors.primary` 压暗到与 `primaryForeground` 达到 WCAG AA；
    /// 原本等于 `primary` 的 `accentForeground` / `ring` 同步使用调整后的颜色。
    pub ensure_primary_contrast: bool,
}

/// 单个模式全部 token 的解析结果（未缩放），按注册表顺序。
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedModeTokens {
    values: Vec<(&'static str, TokenValue)>,
}

impl ResolvedModeTokens {
    #[must_use]
    pub fn get(&self, path: &str) -> Option<&TokenValue> {
        self.values
            .iter()
            .find(|(candidate, _)| *candidate == path)
            .map(|(_, value)| value)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&'static str, &TokenValue)> {
        self.values.iter().map(|(path, value)| (*path, value))
    }

    /// 按注册表路径扁平化：`{ "colors.primary": "#3b82f6ff", ... }`。
    #[must_use]
    pub fn to_flat_json(&self) -> Value {
        self.to_flat_ordered_json().to_value()
    }

    /// 同 [`Self::to_flat_json`]，键按注册表顺序。
    #[must_use]
    pub fn to_flat_ordered_json(&self) -> OrderedJson {
        OrderedJson::Object(
            self.values
                .iter()
                .map(|(path, value)| ((*path).to_owned(), value.to_ordered_json()))
                .collect(),
        )
    }

    /// 类型化运行时快照（按 `scale` 缩放）。
    #[must_use]
    pub fn to_theme(&self, scale: f32) -> ResolvedTheme {
        ResolvedTheme::from_tokens(self, scale)
    }
}

/// 以默认选项解析。
#[must_use]
pub fn resolve(document: &ThemeDocument, mode: ThemeMode) -> (ResolvedModeTokens, Vec<Diagnostic>) {
    resolve_with(document, mode, &ResolveOptions::default())
}

/// 解析指定模式的全部 token。
#[must_use]
pub fn resolve_with(
    document: &ThemeDocument,
    mode: ThemeMode,
    options: &ResolveOptions,
) -> (ResolvedModeTokens, Vec<Diagnostic>) {
    let mut diagnostics = Vec::new();
    let base = match document.extends.as_deref() {
        None => BuiltinBase::Default,
        Some(value) => BuiltinBase::parse(value).unwrap_or_else(|| {
            diagnostics.push(Diagnostic::new(
                "extends",
                DiagnosticKind::UnknownExtends,
                format!("未知基底 {value}，回退 builtin:default"),
            ));
            BuiltinBase::Default
        }),
    };
    let mode_key = match mode {
        ThemeMode::Dark => "dark",
        ThemeMode::Light => "light",
    };
    let mut resolver = Resolver {
        mode,
        layers: [
            (
                mode_key,
                match mode {
                    ThemeMode::Dark => &document.dark,
                    ThemeMode::Light => &document.light,
                },
            ),
            ("tokens", &document.tokens),
        ],
        base: base_values(base, mode, options.accent)
            .into_iter()
            .collect(),
        memo: HashMap::new(),
        stack: Vec::new(),
        diagnostics,
    };

    if options.ensure_primary_contrast && mode == ThemeMode::Light {
        resolver.adjust_primary_contrast();
    }
    let values = TOKENS
        .iter()
        .map(|spec| {
            let value = resolver
                .eval(spec)
                .unwrap_or_else(|_| resolver.fallback(spec));
            (spec.path, value)
        })
        .collect();

    let mut seen = HashSet::new();
    let diagnostics = resolver
        .diagnostics
        .into_iter()
        .filter(|diagnostic| seen.insert(diagnostic.clone()))
        .collect();
    (ResolvedModeTokens { values }, diagnostics)
}

/// fixture 期望值：解析文件后两个模式按注册表路径扁平化的结果与全部诊断
/// （`{ "diagnostics": [...], "dark": {...}, "light": {...} }`；整体拒绝时为
/// `{ "error": "..." }`）。网站端逐项比对以保证两端解析一致。
#[must_use]
pub fn resolved_snapshot(text: &str) -> OrderedJson {
    let (document, mut diagnostics) = match ThemeDocument::parse(text) {
        Ok(parsed) => parsed,
        Err(error) => return ordered_json!({ "error": (error.to_string()) }),
    };
    let (dark, dark_diagnostics) = resolve(&document, ThemeMode::Dark);
    let (light, light_diagnostics) = resolve(&document, ThemeMode::Light);
    for diagnostic in dark_diagnostics.into_iter().chain(light_diagnostics) {
        if !diagnostics.contains(&diagnostic) {
            diagnostics.push(diagnostic);
        }
    }
    ordered_json!({
        "diagnostics": (diagnostics.iter().map(Diagnostic::to_json).collect::<Vec<_>>()),
        "dark": (dark.to_flat_ordered_json()),
        "light": (light.to_flat_ordered_json()),
    })
}

/// 求值时遇到正在求值的 token（引用成环）。
struct Cycle;

struct Resolver<'a> {
    mode: ThemeMode,
    /// 按优先级：`<mode>` 层、`tokens` 层。
    layers: [(&'static str, &'a Map<String, Value>); 2],
    base: HashMap<&'static str, TokenValue>,
    memo: HashMap<&'static str, TokenValue>,
    stack: Vec<&'static str>,
    diagnostics: Vec<Diagnostic>,
}

impl Resolver<'_> {
    fn eval(&mut self, spec: &'static TokenSpec) -> Result<TokenValue, Cycle> {
        if let Some(value) = self.memo.get(spec.path) {
            return Ok(value.clone());
        }
        if self.stack.contains(&spec.path) {
            return Err(Cycle);
        }
        self.stack.push(spec.path);
        let result = self.eval_uncached(spec);
        self.stack.pop();
        let (value, source) = result?;
        let value = self.constrain(spec, value, source.as_deref());
        self.memo.insert(spec.path, value.clone());
        Ok(value)
    }

    /// 返回值及其来源层路径（来自文件时用于诊断定位）。
    fn eval_uncached(
        &mut self,
        spec: &'static TokenSpec,
    ) -> Result<(TokenValue, Option<String>), Cycle> {
        for index in 0..self.layers.len() {
            let (layer_key, layer) = self.layers[index];
            let Some(raw) = get_path(layer, spec.path) else {
                continue;
            };
            let layer_path = format!("{layer_key}.{}", spec.path);
            if spec.kit_bound {
                self.diagnose(
                    &layer_path,
                    DiagnosticKind::InvalidValue,
                    "该组件圆角由 gpui-component 跟随 radius.md / radius.lg，文件覆盖被忽略",
                );
                continue;
            }
            let wire = match parse_wire(spec.kind, raw) {
                Ok(wire) => wire,
                Err(message) => {
                    self.diagnose(&layer_path, DiagnosticKind::InvalidValue, &message);
                    continue;
                }
            };
            match self.eval_wire(spec, &wire, &layer_path) {
                Ok(Some(value)) => return Ok((value, Some(layer_path))),
                Ok(None) => {}
                Err(Cycle) => self.diagnose(
                    &layer_path,
                    DiagnosticKind::RefCycle,
                    "引用成环，回退到下一层",
                ),
            }
        }
        let value = self.eval_default(spec, &spec.default)?;
        Ok((value, None))
    }

    /// 文件值 → token 值；`Ok(None)` 表示非法（已记诊断）。
    fn eval_wire(
        &mut self,
        spec: &'static TokenSpec,
        wire: &WireValue,
        layer_path: &str,
    ) -> Result<Option<TokenValue>, Cycle> {
        match wire {
            WireValue::Literal(value) => Ok(Some(value.clone())),
            WireValue::Ref(reference) => {
                let Some(value) = self.eval_reference(reference, layer_path)? else {
                    return Ok(None);
                };
                if value.fits(spec.kind) {
                    Ok(Some(value))
                } else {
                    self.diagnose(
                        layer_path,
                        DiagnosticKind::InvalidValue,
                        &format!(
                            "引用 {} 的类型与 {} 不符",
                            reference.path,
                            spec.kind.wire_name()
                        ),
                    );
                    Ok(None)
                }
            }
            WireValue::Shadow(layers) => {
                let mut resolved = Vec::with_capacity(layers.len());
                for layer in layers {
                    let color = match &layer.color {
                        ColorSource::Literal(color) => *color,
                        ColorSource::Ref(reference) => {
                            match self.eval_reference(reference, layer_path)? {
                                Some(TokenValue::Color(color)) => color,
                                Some(_) => {
                                    self.diagnose(
                                        layer_path,
                                        DiagnosticKind::InvalidValue,
                                        &format!("阴影颜色引用 {} 不是颜色", reference.path),
                                    );
                                    return Ok(None);
                                }
                                None => return Ok(None),
                            }
                        }
                    };
                    resolved.push(ShadowValue {
                        x: layer.x,
                        y: layer.y,
                        blur: layer.blur,
                        spread: layer.spread,
                        color,
                        inset: layer.inset,
                    });
                }
                Ok(Some(TokenValue::Shadow(resolved)))
            }
        }
    }

    /// 解析引用（含透明度）；悬空引用记诊断返回 `None`。
    fn eval_reference(
        &mut self,
        reference: &Reference,
        layer_path: &str,
    ) -> Result<Option<TokenValue>, Cycle> {
        let Some(target) = token(&reference.path) else {
            self.diagnose(
                layer_path,
                DiagnosticKind::InvalidValue,
                &format!("悬空引用 {{{}}}", reference.path),
            );
            return Ok(None);
        };
        let value = self.eval(target)?;
        Ok(Some(match (value, reference.opacity) {
            (TokenValue::Color(color), Some(opacity)) => TokenValue::Color(color.opacity(opacity)),
            (value, _) => value,
        }))
    }

    fn eval_default(
        &mut self,
        spec: &'static TokenSpec,
        expr: &DefaultExpr,
    ) -> Result<TokenValue, Cycle> {
        // 预设基底的值优先于默认表达式（Base token 与预设固定的非 Base token）。
        if let Some(value) = self.base.get(spec.path) {
            return Ok(value.clone());
        }
        Ok(match expr {
            DefaultExpr::Base => self.fallback(spec),
            DefaultExpr::Ref(path) => self.eval_path(path)?,
            DefaultExpr::RefAlpha(path, alpha) => match self.eval_path(path)? {
                TokenValue::Color(color) => TokenValue::Color(color.opacity(*alpha)),
                other => other,
            },
            DefaultExpr::RefOffset(path, offset) => match self.eval_path(path)? {
                TokenValue::Number(number) => TokenValue::Number(number + offset),
                other => other,
            },
            DefaultExpr::Mix { from, to, amount } => {
                let from = self.eval_path(from)?.as_color().unwrap_or_default();
                let to = self.eval_path(to)?.as_color().unwrap_or_default();
                TokenValue::Color(mix(from, to, amount.get(self.mode)))
            }
            DefaultExpr::Contrast {
                color,
                against,
                min,
            } => {
                let color = self.eval_path(color)?.as_color().unwrap_or_default();
                let against = self.eval_path(against)?.as_color().unwrap_or_default();
                TokenValue::Color(with_min_contrast(color, against, *min))
            }
            DefaultExpr::Literal(values) => literal_value(*values, self.mode),
            DefaultExpr::Mode(exprs) => {
                let expr = match self.mode {
                    ThemeMode::Dark => &exprs.dark,
                    ThemeMode::Light => &exprs.light,
                };
                self.eval_default(spec, expr)?
            }
        })
    }

    fn eval_path(&mut self, path: &str) -> Result<TokenValue, Cycle> {
        match token(path) {
            Some(target) => self.eval(target),
            None => Ok(TokenValue::Number(0.)),
        }
    }

    /// 所有候选都因成环失败时的兜底：基底值或默认表达式中不含引用的部分。
    fn fallback(&self, spec: &TokenSpec) -> TokenValue {
        if let Some(value) = self.base.get(spec.path) {
            return value.clone();
        }
        match spec.default {
            DefaultExpr::Literal(values) => literal_value(values, self.mode),
            _ => match spec.kind {
                TokenKind::Color => TokenValue::Color(Hsla::default()),
                TokenKind::FontFamily => TokenValue::Font(String::new()),
                TokenKind::Shadow => TokenValue::Shadow(Vec::new()),
                _ => TokenValue::Number(0.),
            },
        }
    }

    /// 范围 clamp 与「行高不小于字号」。
    fn constrain(
        &mut self,
        spec: &'static TokenSpec,
        value: TokenValue,
        source: Option<&str>,
    ) -> TokenValue {
        let TokenValue::Number(mut number) = value else {
            return value;
        };
        let original = number;
        if let Some((min, max)) = spec.range {
            number = number.clamp(min, max);
        }
        if let Some(role) = spec.path.strip_suffix(".lineHeight") {
            let size_path = format!("{role}.size");
            if let Some(size_spec) = token(&size_path)
                && let Ok(TokenValue::Number(size)) = self.eval(size_spec)
            {
                number = number.max(size);
            }
        }
        if number != original {
            let path = source.map_or_else(|| spec.path.to_owned(), str::to_owned);
            self.diagnose(
                &path,
                DiagnosticKind::OutOfRange,
                &format!("{original} 越界，已 clamp 为 {number}"),
            );
        }
        TokenValue::Number(number)
    }

    /// 亮色主色对比度调整：先求出 primary 系列，再覆盖记忆值。
    fn adjust_primary_contrast(&mut self) {
        let mut get = |path: &str| {
            token(path)
                .and_then(|spec| self.eval(spec).ok())
                .and_then(|value| value.as_color())
        };
        let (Some(primary), Some(foreground)) =
            (get("colors.primary"), get("colors.primaryForeground"))
        else {
            return;
        };
        let accent_foreground = get("colors.accentForeground");
        let ring = get("colors.ring");
        let adjusted = primary_with_contrast(primary, foreground);
        self.memo
            .insert("colors.primary", TokenValue::Color(adjusted));
        if accent_foreground == Some(primary) {
            self.memo
                .insert("colors.accentForeground", TokenValue::Color(adjusted));
        }
        if ring == Some(primary) {
            self.memo.insert("colors.ring", TokenValue::Color(adjusted));
        }
    }

    fn diagnose(&mut self, path: &str, kind: DiagnosticKind, message: &str) {
        self.diagnostics.push(Diagnostic::new(path, kind, message));
    }
}

fn literal_value(values: ByMode<Literal>, mode: ThemeMode) -> TokenValue {
    match values.get(mode) {
        Literal::Color(color) => TokenValue::Color(rgba_color(color)),
        Literal::Number(number) => TokenValue::Number(number),
    }
}

/// 在 sRGB 空间把 `from` 向 `to` 混合 `amount`（0 = from，1 = to），结果不透明。
fn mix(from: Hsla, to: Hsla, amount: f32) -> Hsla {
    let a = from.to_rgb();
    let b = to.to_rgb();
    let t = amount.clamp(0., 1.);
    Hsla::from(Rgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: 1.,
    })
}
