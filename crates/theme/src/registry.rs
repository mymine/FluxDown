//! Token 注册表：主题系统唯一事实源。
//!
//! 每个 token 声明路径、类型、默认表达式、取值范围、引入版本、是否随明暗模式变化、
//! 是否绑定 gpui-component 库控件（`kit_bound`）以及是否随界面缩放。解析器、导出、
//! JSON Schema 与 `registry.json`（网站主题编辑器的镜像数据）都只从这里派生。

use gpui_component::ThemeMode;
use serde_json::Value;

use crate::builtin::{ACCENT_TOKEN_PATHS, BuiltinBase, base_values};
use crate::json::OrderedJson;
use crate::migrate::{MIGRATIONS, V1_KEY_RENAMES};
use crate::value::{color_hex, number_value, rgba_color};
use crate::{RADIUS_FULL_THRESHOLD, THEME_FORMAT, THEME_SCHEMA_URL, THEME_SCHEMA_VERSION};

/// token 值类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Color,
    /// 长度（px）。
    Length,
    /// 圆角（px）。
    Radius,
    FontFamily,
    /// 100 ~ 900。
    FontWeight,
    /// 无单位数值。
    Number,
    Shadow,
}

impl TokenKind {
    #[must_use]
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Color => "color",
            Self::Length => "length",
            Self::Radius => "radius",
            Self::FontFamily => "fontFamily",
            Self::FontWeight => "fontWeight",
            Self::Number => "number",
            Self::Shadow => "shadow",
        }
    }

    /// 数值类 token（引用可在这些 kind 之间互通）。
    #[must_use]
    pub fn is_numeric(self) -> bool {
        matches!(
            self,
            Self::Length | Self::Radius | Self::FontWeight | Self::Number
        )
    }
}

/// 按明暗模式区分的一对值。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ByMode<T> {
    pub dark: T,
    pub light: T,
}

impl<T: Copy> ByMode<T> {
    pub const fn same(value: T) -> Self {
        Self {
            dark: value,
            light: value,
        }
    }

    #[must_use]
    pub fn get(&self, mode: ThemeMode) -> T {
        match mode {
            ThemeMode::Dark => self.dark,
            ThemeMode::Light => self.light,
        }
    }
}

/// 注册表中的字面量默认值。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Literal {
    /// `0xRRGGBBAA`。
    Color(u32),
    Number(f32),
}

/// 默认值表达式；文件未给出该 token 时按此求值。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DefaultExpr {
    /// 取 `extends` 基底中同路径的值（只有 Base token 使用）。
    Base,
    /// 等于另一 token。
    Ref(&'static str),
    /// 颜色引用并把 alpha 乘以系数（0 ~ 1）。
    RefAlpha(&'static str, f32),
    /// 数值引用加偏移量（px）。
    RefOffset(&'static str, f32),
    /// 在 sRGB 空间把 `from` 向 `to` 混合 `amount`（0 = from，1 = to），结果不透明。
    Mix {
        from: &'static str,
        to: &'static str,
        amount: ByMode<f32>,
    },
    /// 保持 `color` 的色相与饱和度，只沿亮度远离 `against`，直到 WCAG 对比度不低于
    /// `min`（已达标则原样，alpha 不变）。任意强调色下都需要可读的文字 / 图标用它派生。
    Contrast {
        color: &'static str,
        against: &'static str,
        min: f32,
    },
    /// 字面量（可按模式不同）。
    Literal(ByMode<Literal>),
    /// 按模式选择不同表达式。
    Mode(&'static ByMode<DefaultExpr>),
}

/// 单个 token 的完整声明。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TokenSpec {
    /// `group.key`（组件为 `components.<name>.<prop>`，文字角色为 `typography.<role>.<prop>`）。
    pub path: &'static str,
    pub kind: TokenKind,
    pub default: DefaultExpr,
    /// 闭区间；越界值 clamp 并记诊断。
    pub range: Option<(f32, f32)>,
    /// 引入该 token 的 schema 版本。
    pub since: u32,
    pub group: &'static str,
    /// 默认值是否随明暗模式不同。
    pub mode_dependent: bool,
    /// gpui-component 库控件直接读 `radius.md` / `radius.lg`，文件中的覆盖被忽略（编辑器只读）。
    pub kit_bound: bool,
    /// 是否随界面缩放（长度类）；`radius.full` 这类「全圆」值（≥ [`RADIUS_FULL_THRESHOLD`]）始终不缩放。
    pub scales: bool,
}

/// 分组，顺序即规范导出顺序。
pub const GROUPS: [&str; 10] = [
    "colors",
    "radius",
    "spacing",
    "typography",
    "shadow",
    "stroke",
    "focusRing",
    "icon",
    "density",
    "components",
];

const RADIUS_RANGE: Option<(f32, f32)> = Some((0., 64.));
const FULL_RADIUS_RANGE: Option<(f32, f32)> = Some((0., 9999.));
const SPACING_RANGE: Option<(f32, f32)> = Some((0., 64.));
const FONT_SIZE_RANGE: Option<(f32, f32)> = Some((8., 40.));
/// 行高下限另受「不小于同角色字号」约束。
const LINE_HEIGHT_RANGE: Option<(f32, f32)> = Some((8., 96.));
const WEIGHT_RANGE: Option<(f32, f32)> = Some((100., 900.));
const STROKE_RANGE: Option<(f32, f32)> = Some((0., 4.));
const ICON_RANGE: Option<(f32, f32)> = Some((8., 32.));
const DENSITY_RANGE: Option<(f32, f32)> = Some((16., 96.));
const PROGRESS_HEIGHT_RANGE: Option<(f32, f32)> = Some((1., 16.));

const fn spec(
    path: &'static str,
    group: &'static str,
    kind: TokenKind,
    default: DefaultExpr,
    range: Option<(f32, f32)>,
    since: u32,
) -> TokenSpec {
    let scales = matches!(
        kind,
        TokenKind::Length | TokenKind::Radius | TokenKind::Shadow
    );
    TokenSpec {
        path,
        kind,
        default,
        range,
        since,
        group,
        mode_dependent: matches!(kind, TokenKind::Color),
        kit_bound: false,
        scales,
    }
}

const fn base_color(path: &'static str) -> TokenSpec {
    spec(path, "colors", TokenKind::Color, DefaultExpr::Base, None, 1)
}

const fn color(path: &'static str, default: DefaultExpr) -> TokenSpec {
    spec(path, "colors", TokenKind::Color, default, None, 2)
}

const fn literal_colors(dark: u32, light: u32) -> DefaultExpr {
    DefaultExpr::Literal(ByMode {
        dark: Literal::Color(dark),
        light: Literal::Color(light),
    })
}

const fn mix(from: &'static str, to: &'static str, dark: f32, light: f32) -> DefaultExpr {
    DefaultExpr::Mix {
        from,
        to,
        amount: ByMode { dark, light },
    }
}

const fn contrast(color: &'static str, against: &'static str, min: f32) -> DefaultExpr {
    DefaultExpr::Contrast {
        color,
        against,
        min,
    }
}

const fn number(value: f32) -> DefaultExpr {
    DefaultExpr::Literal(ByMode::same(Literal::Number(value)))
}

const fn base_px(
    path: &'static str,
    group: &'static str,
    kind: TokenKind,
    range: Option<(f32, f32)>,
) -> TokenSpec {
    spec(path, group, kind, DefaultExpr::Base, range, 1)
}

const fn text_size(path: &'static str, default: DefaultExpr, since: u32) -> TokenSpec {
    spec(
        path,
        "typography",
        TokenKind::Length,
        default,
        FONT_SIZE_RANGE,
        since,
    )
}

const fn line_height(path: &'static str, default: DefaultExpr, since: u32) -> TokenSpec {
    spec(
        path,
        "typography",
        TokenKind::Length,
        default,
        LINE_HEIGHT_RANGE,
        since,
    )
}

const fn weight(path: &'static str, default: DefaultExpr, since: u32) -> TokenSpec {
    spec(
        path,
        "typography",
        TokenKind::FontWeight,
        default,
        WEIGHT_RANGE,
        since,
    )
}

/// 线宽类：不随缩放（1px 描边缩放后会落在半像素上发虚）。
const fn stroke(path: &'static str, group: &'static str, value: f32) -> TokenSpec {
    TokenSpec {
        scales: false,
        ..spec(
            path,
            group,
            TokenKind::Length,
            number(value),
            STROKE_RANGE,
            2,
        )
    }
}

const fn icon(path: &'static str, value: f32) -> TokenSpec {
    spec(
        path,
        "icon",
        TokenKind::Length,
        number(value),
        ICON_RANGE,
        2,
    )
}

const fn density(path: &'static str, value: f32) -> TokenSpec {
    spec(
        path,
        "density",
        TokenKind::Length,
        number(value),
        DENSITY_RANGE,
        2,
    )
}

const fn component_radius(path: &'static str, default: DefaultExpr) -> TokenSpec {
    spec(
        path,
        "components",
        TokenKind::Radius,
        default,
        FULL_RADIUS_RANGE,
        2,
    )
}

const fn kit_radius(path: &'static str, default: DefaultExpr) -> TokenSpec {
    TokenSpec {
        kit_bound: true,
        ..component_radius(path, default)
    }
}

/// 全部 token；顺序即规范导出与解析求值顺序。
pub static TOKENS: &[TokenSpec] = &[
    // ── colors：Base 17 ──
    base_color("colors.background"),
    base_color("colors.foreground"),
    base_color("colors.surface"),
    base_color("colors.surfaceForeground"),
    base_color("colors.primary"),
    base_color("colors.primaryForeground"),
    base_color("colors.secondary"),
    base_color("colors.secondaryForeground"),
    base_color("colors.muted"),
    base_color("colors.mutedForeground"),
    base_color("colors.accent"),
    base_color("colors.accentForeground"),
    base_color("colors.destructive"),
    base_color("colors.destructiveForeground"),
    base_color("colors.border"),
    base_color("colors.input"),
    base_color("colors.ring"),
    // ── colors：扩展 ──
    color("colors.success", literal_colors(0x30D1_58FF, 0x1F9D_55FF)),
    color("colors.warning", literal_colors(0xFFB3_40FF, 0xC77C_02FF)),
    color("colors.info", DefaultExpr::Ref("colors.primary")),
    color(
        "colors.textTertiary",
        mix("colors.mutedForeground", "colors.surface", 0.38, 0.38),
    ),
    color(
        "colors.hairline",
        mix("colors.border", "colors.surface", 0.35, 0.4),
    ),
    color(
        "colors.rowHover",
        mix("colors.muted", "colors.surface", 0.45, 0.4),
    ),
    color(
        "colors.chrome",
        DefaultExpr::Mode(&ByMode {
            dark: mix("colors.background", "colors.surface", 0.35, 0.35),
            light: mix("colors.background", "colors.muted", 0.35, 0.35),
        }),
    ),
    color(
        "colors.navHover",
        mix("colors.chrome", "colors.navSelected", 0.55, 0.55),
    ),
    color(
        "colors.navSelected",
        DefaultExpr::Mode(&ByMode {
            dark: mix("colors.muted", "colors.surface", 0.15, 0.15),
            light: mix("colors.muted", "colors.border", 0.65, 0.65),
        }),
    ),
    // 选中导航项：文字默认保持正文色，图标取强调色并保证与选中底色 ≥ 3:1（WCAG 非文本）。
    color(
        "colors.navSelectedForeground",
        DefaultExpr::Ref("colors.foreground"),
    ),
    color(
        "colors.navSelectedIcon",
        contrast("colors.primary", "colors.navSelected", 3.),
    ),
    // ── colors：强调色派生 ──
    // 强调色文字（链接、生效中的状态、活跃计数）：与内容底色 ≥ 4.5:1（WCAG AA 正文）。
    color(
        "colors.accentText",
        contrast("colors.primary", "colors.surface", 4.5),
    ),
    color("colors.caret", DefaultExpr::Ref("colors.accentText")),
    color(
        "colors.textSelection",
        DefaultExpr::RefAlpha("colors.primary", 0.3),
    ),
    color(
        "colors.dragBorder",
        DefaultExpr::RefAlpha("colors.primary", 0.65),
    ),
    color(
        "colors.dropTarget",
        DefaultExpr::RefAlpha("colors.primary", 0.2),
    ),
    // ── colors：下载状态 ──
    color(
        "colors.statusDownloading",
        DefaultExpr::Ref("colors.primary"),
    ),
    color(
        "colors.statusCompleted",
        DefaultExpr::Ref("colors.textTertiary"),
    ),
    color(
        "colors.statusFailed",
        DefaultExpr::Ref("colors.destructive"),
    ),
    color(
        "colors.statusPaused",
        DefaultExpr::Ref("colors.mutedForeground"),
    ),
    color(
        "colors.statusQueued",
        DefaultExpr::Ref("colors.textTertiary"),
    ),
    // ── colors：进度 ──
    color(
        "colors.progressTrack",
        DefaultExpr::RefAlpha("colors.mutedForeground", 0.16),
    ),
    color("colors.progressFill", DefaultExpr::Ref("colors.primary")),
    // ── radius ──
    base_px("radius.none", "radius", TokenKind::Radius, RADIUS_RANGE),
    base_px("radius.sm", "radius", TokenKind::Radius, RADIUS_RANGE),
    base_px("radius.md", "radius", TokenKind::Radius, RADIUS_RANGE),
    base_px("radius.lg", "radius", TokenKind::Radius, RADIUS_RANGE),
    base_px("radius.xl", "radius", TokenKind::Radius, RADIUS_RANGE),
    base_px(
        "radius.full",
        "radius",
        TokenKind::Radius,
        FULL_RADIUS_RANGE,
    ),
    // ── spacing ──
    base_px("spacing.xxs", "spacing", TokenKind::Length, SPACING_RANGE),
    base_px("spacing.xs", "spacing", TokenKind::Length, SPACING_RANGE),
    base_px("spacing.sm", "spacing", TokenKind::Length, SPACING_RANGE),
    base_px("spacing.md", "spacing", TokenKind::Length, SPACING_RANGE),
    base_px("spacing.lg", "spacing", TokenKind::Length, SPACING_RANGE),
    base_px("spacing.xl", "spacing", TokenKind::Length, SPACING_RANGE),
    base_px("spacing.xxl", "spacing", TokenKind::Length, SPACING_RANGE),
    // ── typography ──
    spec(
        "typography.sans",
        "typography",
        TokenKind::FontFamily,
        DefaultExpr::Base,
        None,
        1,
    ),
    spec(
        "typography.mono",
        "typography",
        TokenKind::FontFamily,
        DefaultExpr::Base,
        None,
        1,
    ),
    text_size("typography.xs.size", DefaultExpr::Base, 1),
    line_height("typography.xs.lineHeight", DefaultExpr::Base, 1),
    weight("typography.xs.weight", DefaultExpr::Base, 1),
    text_size("typography.sm.size", DefaultExpr::Base, 1),
    line_height("typography.sm.lineHeight", DefaultExpr::Base, 1),
    weight("typography.sm.weight", DefaultExpr::Base, 1),
    text_size("typography.md.size", DefaultExpr::Base, 1),
    line_height("typography.md.lineHeight", DefaultExpr::Base, 1),
    weight("typography.md.weight", DefaultExpr::Base, 1),
    text_size("typography.lg.size", DefaultExpr::Base, 1),
    line_height("typography.lg.lineHeight", DefaultExpr::Base, 1),
    weight("typography.lg.weight", DefaultExpr::Base, 1),
    text_size("typography.xl.size", DefaultExpr::Base, 1),
    line_height("typography.xl.lineHeight", DefaultExpr::Base, 1),
    weight("typography.xl.weight", DefaultExpr::Base, 1),
    text_size("typography.monoMd.size", DefaultExpr::Base, 1),
    line_height("typography.monoMd.lineHeight", DefaultExpr::Base, 1),
    weight("typography.monoMd.weight", DefaultExpr::Base, 1),
    text_size("typography.caption.size", number(11.), 2),
    line_height("typography.caption.lineHeight", number(14.), 2),
    weight("typography.caption.weight", number(400.), 2),
    text_size("typography.title.size", number(15.), 2),
    line_height("typography.title.lineHeight", number(20.), 2),
    weight("typography.title.weight", number(600.), 2),
    // ── shadow ──
    spec(
        "shadow.sm",
        "shadow",
        TokenKind::Shadow,
        DefaultExpr::Base,
        None,
        1,
    ),
    spec(
        "shadow.md",
        "shadow",
        TokenKind::Shadow,
        DefaultExpr::Base,
        None,
        1,
    ),
    spec(
        "shadow.lg",
        "shadow",
        TokenKind::Shadow,
        DefaultExpr::Base,
        None,
        1,
    ),
    // ── stroke / focusRing ──
    stroke("stroke.thin", "stroke", 1.),
    stroke("stroke.strong", "stroke", 2.),
    stroke("focusRing.width", "focusRing", 2.),
    stroke("focusRing.offset", "focusRing", 1.),
    // ── icon ──
    icon("icon.sm", 12.),
    icon("icon.md", 14.),
    icon("icon.lg", 16.),
    // ── density ──
    density("density.control", 28.),
    density("density.navRow", 28.),
    density("density.toolbarButton", 28.),
    // 交通灯按标题栏高度垂直居中，标题栏不随界面缩放。
    TokenSpec {
        scales: false,
        ..density("density.titleBar", 40.)
    },
    density("density.statusBar", 28.),
    density("density.statusControl", 22.),
    density("density.sectionHeader", 24.),
    density("density.taskRow", 44.),
    density("density.taskRowCompact", 30.),
    density("density.checkMark", 16.),
    // ── components ──
    component_radius("components.button.radius", DefaultExpr::Ref("radius.md")),
    kit_radius("components.input.radius", DefaultExpr::Ref("radius.md")),
    kit_radius("components.dialog.radius", DefaultExpr::Ref("radius.lg")),
    component_radius("components.card.radius", DefaultExpr::Ref("radius.lg")),
    component_radius("components.badge.radius", DefaultExpr::Ref("radius.full")),
    component_radius("components.navItem.radius", DefaultExpr::Ref("radius.md")),
    component_radius(
        "components.progress.radius",
        DefaultExpr::Ref("radius.full"),
    ),
    spec(
        "components.progress.height",
        "components",
        TokenKind::Length,
        number(4.),
        PROGRESS_HEIGHT_RANGE,
        2,
    ),
    component_radius("components.taskRow.radius", DefaultExpr::Ref("radius.md")),
    component_radius(
        "components.checkbox.radius",
        DefaultExpr::RefOffset("radius.sm", 1.),
    ),
    component_radius("components.tab.radius", DefaultExpr::Ref("radius.md")),
    kit_radius("components.menu.radius", DefaultExpr::Ref("radius.md")),
    kit_radius("components.tooltip.radius", DefaultExpr::Ref("radius.md")),
];

/// 按路径查找 token。
#[must_use]
pub fn token(path: &str) -> Option<&'static TokenSpec> {
    TOKENS.iter().find(|spec| spec.path == path)
}

/// 默认表达式 → registry JSON。
fn default_json(expr: &DefaultExpr) -> OrderedJson {
    match expr {
        DefaultExpr::Base => ordered_json!({ "kind": "base" }),
        DefaultExpr::Ref(path) => ordered_json!({ "kind": "ref", "path": (*path) }),
        DefaultExpr::RefAlpha(path, alpha) => {
            ordered_json!({ "kind": "refAlpha", "path": (*path), "alpha": (number_value(*alpha)) })
        }
        DefaultExpr::RefOffset(path, offset) => {
            ordered_json!({ "kind": "refOffset", "path": (*path), "offset": (number_value(*offset)) })
        }
        DefaultExpr::Mix { from, to, amount } => ordered_json!({
            "kind": "mix",
            "from": (*from),
            "to": (*to),
            "amount": {
                "dark": (number_value(amount.dark)),
                "light": (number_value(amount.light)),
            },
        }),
        DefaultExpr::Contrast {
            color,
            against,
            min,
        } => ordered_json!({
            "kind": "contrast",
            "color": (*color),
            "against": (*against),
            "min": (number_value(*min)),
        }),
        DefaultExpr::Literal(values) => ordered_json!({
            "kind": "literal",
            "value": { "dark": (literal_json(values.dark)), "light": (literal_json(values.light)) },
        }),
        DefaultExpr::Mode(exprs) => ordered_json!({
            "kind": "byMode",
            "dark": (default_json(&exprs.dark)),
            "light": (default_json(&exprs.light)),
        }),
    }
}

fn literal_json(literal: Literal) -> Value {
    match literal {
        Literal::Color(value) => Value::String(color_hex(rgba_color(value))),
        Literal::Number(value) => number_value(value),
    }
}

fn range_json(range: Option<(f32, f32)>) -> OrderedJson {
    range.map_or(
        OrderedJson::Value(Value::Null),
        |(min, max)| ordered_json!({ "min": (number_value(min)), "max": (number_value(max)) }),
    )
}

fn renames_json(renames: &[(&str, &str)]) -> OrderedJson {
    OrderedJson::Array(
        renames
            .iter()
            .map(|(from, to)| ordered_json!({ "from": (*from), "to": (*to) }))
            .collect(),
    )
}

/// 注册表的 JSON 镜像：token 声明、默认表达式、范围、分组、since、kitBound、
/// 迁移表、Flutter 适配表与全部 builtin 基底的完整值。网站主题编辑器与 TS 解析器只消费它。
#[must_use]
pub fn registry_json() -> OrderedJson {
    let tokens: Vec<OrderedJson> = TOKENS
        .iter()
        .map(|spec| {
            ordered_json!({
                "path": (spec.path),
                "group": (spec.group),
                "kind": (spec.kind.wire_name()),
                "default": (default_json(&spec.default)),
                "range": (range_json(spec.range)),
                "since": (spec.since),
                "modeDependent": (spec.mode_dependent),
                "kitBound": (spec.kit_bound),
                "scales": (spec.scales),
            })
        })
        .collect();

    let builtins = BuiltinBase::ALL
        .iter()
        .map(|&base| {
            let modes = [("dark", ThemeMode::Dark), ("light", ThemeMode::Light)]
                .into_iter()
                .map(|(name, mode)| {
                    let values = base_values(base, mode, None)
                        .into_iter()
                        .map(|(path, value)| (path.to_owned(), value.to_ordered_json()))
                        .collect();
                    (name.to_owned(), OrderedJson::Object(values))
                })
                .collect();
            (base.extends_value().to_owned(), OrderedJson::Object(modes))
        })
        .collect();

    let steps: Vec<OrderedJson> = MIGRATIONS
        .iter()
        .map(|migration| {
            ordered_json!({
                "toVersion": (migration.to_version),
                "renames": (renames_json(migration.renames)),
            })
        })
        .collect();

    ordered_json!({
        "format": THEME_FORMAT,
        "schemaVersion": THEME_SCHEMA_VERSION,
        "schemaUrl": THEME_SCHEMA_URL,
        "groups": (GROUPS.as_slice()),
        "extends": (BuiltinBase::ALL.iter().map(|base| base.extends_value()).collect::<Vec<_>>()),
        "defaultExtends": (BuiltinBase::Default.extends_value()),
        "radiusFullThreshold": (number_value(RADIUS_FULL_THRESHOLD)),
        "fontSentinels": {
            "monospace": "平台默认等宽字体（macOS Menlo / Windows Consolas / Linux DejaVu Sans Mono）",
            "system-ui": "平台默认界面字体（macOS 系统字体 / Windows 系统界面字体如微软雅黑 UI、Segoe UI / Linux 系统无衬线字体）"
        },
        "resolution": {
            "layerOrder": ["<mode>", "tokens", "base|default"],
            "evaluationOrder": "registry",
            "refOpacity": "multiply",
            "mix": "srgb-opaque",
            "lineHeightAtLeastSize": true,
            "kitBoundIgnoresOverrides": true,
            "invalidOrCycleFallsThrough": true,
            "colorOutput": "#rrggbbaa lowercase, round(channel * 255)",
            "runtimeOnly": ["accent", "ensurePrimaryContrast", "uiScale"],
        },
        "accentTokenPaths": (ACCENT_TOKEN_PATHS.as_slice()),
        "shadowDefaults": {
            "x": 0, "y": 0, "blur": 0, "spread": 0,
            "color": (color_hex(rgba_color(crate::value::SHADOW_DEFAULT_COLOR))),
            "inset": false,
        },
        "tokens": tokens,
        "builtins": (OrderedJson::Object(builtins)),
        "migrations": {
            "v1KeyRenames": (renames_json(V1_KEY_RENAMES)),
            "steps": steps,
        },
        "flutterAdapter": (crate::flutter::adapter_json()),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use gpui_component::ThemeMode;

    use super::{DefaultExpr, GROUPS, TOKENS, token};
    use crate::builtin::{BuiltinBase, base_values};

    fn referenced(expr: &DefaultExpr, out: &mut Vec<&'static str>) {
        match expr {
            DefaultExpr::Base | DefaultExpr::Literal(_) => {}
            DefaultExpr::Ref(path)
            | DefaultExpr::RefAlpha(path, _)
            | DefaultExpr::RefOffset(path, _) => {
                out.push(path);
            }
            DefaultExpr::Mix { from, to, .. } => {
                out.push(from);
                out.push(to);
            }
            DefaultExpr::Contrast { color, against, .. } => {
                out.push(color);
                out.push(against);
            }
            DefaultExpr::Mode(exprs) => {
                referenced(&exprs.dark, out);
                referenced(&exprs.light, out);
            }
        }
    }

    #[test]
    fn paths_are_unique_grouped_and_references_resolve() {
        let mut seen = HashSet::new();
        for spec in TOKENS {
            assert!(seen.insert(spec.path), "重复路径 {}", spec.path);
            assert!(GROUPS.contains(&spec.group), "{} 分组未登记", spec.path);
            assert!(spec.path.starts_with(&format!("{}.", spec.group)));
            let mut refs = Vec::new();
            referenced(&spec.default, &mut refs);
            for target in refs {
                let target_spec = token(target);
                assert!(target_spec.is_some(), "{} 引用未知 {target}", spec.path);
            }
        }
    }

    #[test]
    fn every_base_token_has_builtin_value() {
        for base in BuiltinBase::ALL {
            for mode in [ThemeMode::Dark, ThemeMode::Light] {
                let values = base_values(base, mode, None);
                for spec in TOKENS
                    .iter()
                    .filter(|spec| spec.default == DefaultExpr::Base)
                {
                    let value = values.iter().find(|(path, _)| *path == spec.path);
                    assert!(
                        value.is_some_and(|(_, value)| value.fits(spec.kind)),
                        "{:?} {mode:?} 缺少 {}",
                        base,
                        spec.path
                    );
                }
                // 其余条目只能是预设固定的非 Base token，且同样类型合法。
                for (path, value) in &values {
                    let spec = token(path).unwrap_or_else(|| panic!("{base:?} 未知 {path}"));
                    assert!(value.fits(spec.kind), "{base:?} {mode:?} {path}");
                }
            }
        }
    }
}
