//! 主题文件 fixtures：加载、迁移、诊断、导出往返，以及生成物漂移守卫。

use std::fs;
use std::path::{Path, PathBuf};

use fluxdown_ui_theme::{
    DiagnosticKind, ExportMode, ThemeDocument, ThemeMode, ThemeParseError, TokenLayer, TokenValue,
    json_schema, parse_hex_color, registry_json, resolve, resolved_snapshot, to_pretty_json,
};
use serde_json::{Value, json};

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn fixture(name: &str) -> String {
    fs::read_to_string(fixtures_dir().join(name)).unwrap_or_else(|error| panic!("{name}: {error}"))
}

fn parse(name: &str) -> (ThemeDocument, Vec<fluxdown_ui_theme::Diagnostic>) {
    ThemeDocument::parse(&fixture(name)).unwrap_or_else(|error| panic!("{name}: {error}"))
}

fn kinds(diagnostics: &[fluxdown_ui_theme::Diagnostic]) -> Vec<(DiagnosticKind, &str)> {
    diagnostics
        .iter()
        .map(|diagnostic| (diagnostic.kind, diagnostic.path.as_str()))
        .collect()
}

fn value(document: &ThemeDocument, mode: ThemeMode, path: &str) -> TokenValue {
    resolve(document, mode)
        .0
        .get(path)
        .cloned()
        .unwrap_or_else(|| panic!("{path} 未解析"))
}

fn color(hex: &str) -> TokenValue {
    TokenValue::Color(parse_hex_color(hex).unwrap_or_else(|| panic!("{hex}")))
}

// ── 漂移守卫 ──

fn assert_committed(path: &Path, generated: &str) {
    let committed = fs::read_to_string(path).unwrap_or_default();
    assert!(
        committed == generated,
        "{} 与生成结果不一致；运行 `cargo run -p fluxdown_ui_theme --example gen_theme_registry` 重新生成",
        path.display()
    );
}

#[test]
fn committed_registry_and_schema_match_generated() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    // website-v2/ 被仓库 .gitignore 排除，只在本机存在；不存在时无镜像可比较。
    if !root.join("website-v2/package.json").is_file() {
        eprintln!("website-v2 不存在，跳过 registry / schema 漂移检查");
        return;
    }
    assert_committed(
        &root.join("website-v2/src/lib/gpui-theme/registry.json"),
        &to_pretty_json(&registry_json()),
    );
    assert_committed(
        &root.join("website-v2/public/schemas/gpui-theme.v2.json"),
        &to_pretty_json(&json_schema()),
    );
}

#[test]
fn committed_resolved_fixtures_match_generated() {
    let mut sources: Vec<PathBuf> = fs::read_dir(fixtures_dir())
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .collect()
        })
        .unwrap_or_default();
    sources.retain(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".json") && !name.ends_with(".resolved.json"))
    });
    assert!(sources.len() >= 11, "fixtures 缺失：{sources:?}");
    for source in sources {
        let text = fs::read_to_string(&source).unwrap_or_default();
        assert_committed(
            &source.with_extension("resolved.json"),
            &to_pretty_json(&resolved_snapshot(&text)),
        );
    }
}

// ── 加载 ──

#[test]
fn v1_internal_format_migrates_full_snapshot() {
    let (document, diagnostics) = parse("v1-internal.json");
    assert_eq!(
        kinds(&diagnostics),
        [(DiagnosticKind::Migrated, "schemaVersion")]
    );
    assert_eq!(document.schema_version, Some(2));
    assert_eq!(document.extends.as_deref(), Some("builtin:default"));
    let meta = document.meta.clone().unwrap_or_default();
    assert_eq!(meta.name.as_deref(), Some("My Nord Tweak"));
    assert_eq!(meta.author.as_deref(), Some("tester"));

    assert_eq!(
        value(&document, ThemeMode::Dark, "colors.background"),
        color("#2E3440")
    );
    assert_eq!(
        value(&document, ThemeMode::Dark, "colors.mutedForeground"),
        color("#D8DEE9")
    );
    assert_eq!(
        value(&document, ThemeMode::Dark, "radius.md"),
        TokenValue::Number(8.)
    );
    assert_eq!(
        value(&document, ThemeMode::Dark, "spacing.xxl"),
        TokenValue::Number(40.)
    );
    assert_eq!(
        value(&document, ThemeMode::Light, "spacing.xxl"),
        TokenValue::Number(32.)
    );
    assert_eq!(
        value(&document, ThemeMode::Light, "typography.sans"),
        TokenValue::Font("Inter".into())
    );
    assert_eq!(
        value(&document, ThemeMode::Dark, "typography.sm.lineHeight"),
        TokenValue::Number(18.)
    );
    let TokenValue::Shadow(shadow) = value(&document, ThemeMode::Dark, "shadow.md") else {
        panic!("shadow.md 不是阴影");
    };
    assert_eq!(
        (shadow[0].y, shadow[0].blur, shadow[0].spread),
        (4., 8., -2.)
    );

    // diff 导出去掉与默认基底相同的快照值：亮色槽位只剩字体改动与强调色路径
    // （强调色路径上的字面量固定了颜色，不让运行时强调色覆盖，永不精简）。
    let exported = document.to_json_value(ExportMode::Diff);
    assert_eq!(
        exported["light"],
        json!({
            "colors": {
                "primary": "#3b82f6ff",
                "primaryForeground": "#ffffffff",
                "accent": "#3b82f61a",
                "accentForeground": "#3b82f6ff",
                "ring": "#3b82f6ff",
            },
            "typography": { "sans": "Inter", "mono": "JetBrains Mono" },
        })
    );
    assert_eq!(exported["dark"]["radius"], json!({ "md": 8 }));
    assert_eq!(exported["dark"]["colors"]["background"], json!("#2e3440ff"));
}

#[test]
fn minimal_diff_layers_shared_and_mode_overrides_over_builtin() {
    let (document, diagnostics) = parse("v2-minimal-diff.json");
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(
        value(&document, ThemeMode::Dark, "colors.primary"),
        color("#5E81AC")
    );
    // Nord 只作用于暗色；亮色取默认主题。
    assert_eq!(
        value(&document, ThemeMode::Dark, "colors.background"),
        color("#2E3440")
    );
    assert_eq!(
        value(&document, ThemeMode::Light, "colors.background"),
        color("#F8F9FA")
    );
    assert_eq!(
        value(&document, ThemeMode::Light, "colors.primary"),
        color("#3B82F6")
    );
    for mode in [ThemeMode::Dark, ThemeMode::Light] {
        assert_eq!(value(&document, mode, "radius.md"), TokenValue::Number(2.));
        // 引用默认值跟随被覆盖的 radius。
        assert_eq!(
            value(&document, mode, "components.button.radius"),
            TokenValue::Number(2.)
        );
        assert_eq!(
            value(&document, mode, "components.checkbox.radius"),
            TokenValue::Number(4.)
        );
    }
    // 暗色 statusDownloading 默认引用 primary。
    assert_eq!(
        value(&document, ThemeMode::Dark, "colors.statusDownloading"),
        color("#5E81AC")
    );
}

#[test]
fn full_export_is_canonical_and_diff_reduces_to_minimal() {
    let text = fixture("v2-full.json");
    let (full, diagnostics) = parse("v2-full.json");
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(full.to_json_pretty(ExportMode::Full), text);

    let (minimal, _) = parse("v2-minimal-diff.json");
    let mut full_diff = full.to_json_value(ExportMode::Diff);
    let mut minimal_diff = minimal.to_json_value(ExportMode::Diff);
    // 完整导出把强调色路径写成字面量（固定颜色），diff 精简保留它们，其余与最小文件一致。
    assert_eq!(
        full_diff["dark"],
        json!({ "colors": {
            "primary": "#5e81acff",
            "primaryForeground": "#2e3440ff",
            "accent": "#88c0d026",
            "accentForeground": "#88c0d0ff",
            "ring": "#88c0d0ff",
        } })
    );
    assert_eq!(
        full_diff["light"],
        json!({ "colors": {
            "primary": "#3b82f6ff",
            "primaryForeground": "#ffffffff",
            "accent": "#3b82f61a",
            "accentForeground": "#3b82f6ff",
            "ring": "#3b82f6ff",
        } })
    );
    assert_eq!(
        minimal_diff["dark"],
        json!({ "colors": { "primary": "#5e81acff" } })
    );
    for diff in [&mut full_diff, &mut minimal_diff] {
        if let Some(object) = diff.as_object_mut() {
            for key in ["meta", "dark", "light"] {
                object.remove(key);
            }
        }
    }
    assert_eq!(full_diff, minimal_diff);
    for mode in [ThemeMode::Dark, ThemeMode::Light] {
        assert_eq!(
            resolve(&full, mode).0.to_flat_json(),
            resolve(&minimal, mode).0.to_flat_json()
        );
    }
}

#[test]
fn missing_fields_default_to_builtin_default() {
    let (document, diagnostics) = parse("missing-fields.json");
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(document.extends, None);
    assert_eq!(
        value(&document, ThemeMode::Light, "colors.primary"),
        color("#10B981")
    );
    assert_eq!(
        value(&document, ThemeMode::Dark, "colors.primary"),
        color("#10B981")
    );
    assert_eq!(
        value(&document, ThemeMode::Light, "colors.progressFill"),
        color("#10B981")
    );
    assert_eq!(
        resolve(&document, ThemeMode::Dark)
            .0
            .get("colors.background"),
        resolve(&ThemeDocument::fluxdown_default(), ThemeMode::Dark)
            .0
            .get("colors.background")
    );
    let exported = document.to_json_value(ExportMode::Diff);
    assert_eq!(exported["format"], json!("fluxdown.gpui-theme"));
    assert_eq!(exported["schemaVersion"], json!(2));
    assert_eq!(
        exported["tokens"],
        json!({ "colors": { "primary": "#10b981ff" } })
    );
    assert!(exported.get("light").is_none());
}

#[test]
fn diff_export_keeps_accent_literals_equal_to_base() {
    // 显式固定的强调色路径即使与基底相同也要保留：否则运行时强调色会覆盖它们。
    let text = r##"{
  "format": "fluxdown.gpui-theme",
  "schemaVersion": 2,
  "extends": "builtin:default",
  "tokens": { "colors": { "primary": "#3B82F6", "ring": "#3b82f6ff" } },
  "dark": { "colors": { "background": "#1c1c1e", "primaryForeground": "#ffffff", "accentForeground": "#3b82f6" } }
}"##;
    let (document, diagnostics) =
        ThemeDocument::parse(text).unwrap_or_else(|error| panic!("{error}"));
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let exported = document.to_json_value(ExportMode::Diff);
    // 非强调色路径上与基底相同的字面量照常精简。
    assert_eq!(
        exported["tokens"],
        json!({ "colors": { "primary": "#3b82f6ff", "ring": "#3b82f6ff" } })
    );
    assert_eq!(
        exported["dark"],
        json!({ "colors": { "primaryForeground": "#ffffffff", "accentForeground": "#3b82f6ff" } })
    );
}

#[test]
fn unknown_keys_survive_round_trip() {
    let (document, diagnostics) = parse("unknown-keys.json");
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.kind == DiagnosticKind::UnknownKey),
        "{diagnostics:?}"
    );
    assert_eq!(diagnostics.len(), 6);
    // 已知键仍然生效。
    assert_eq!(
        value(&document, ThemeMode::Light, "typography.sm.size"),
        TokenValue::Number(14.)
    );

    for mode in [ExportMode::Diff, ExportMode::Full] {
        let exported = document.to_json_value(mode);
        assert_eq!(
            exported["x-editor"],
            json!({ "lastOpened": "colors", "zoom": 2 })
        );
        assert_eq!(exported["meta"]["homepage"], json!("https://example.com"));
        assert_eq!(exported["tokens"]["motion"], json!({ "fast": 120 }));
        assert_eq!(exported["tokens"]["colors"]["sparkle"], json!("#FFFFFF"));
        assert_eq!(
            exported["tokens"]["typography"]["sm"]["letterSpacing"],
            json!(0.2)
        );
        assert_eq!(exported["dark"]["x-notes"], json!("keep me"));
    }
    let text = document.to_json_pretty(ExportMode::Diff);
    let (reparsed, _) = ThemeDocument::parse(&text).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(reparsed.to_json_pretty(ExportMode::Diff), text);
    // 规范键序：已知键按注册表顺序在前，未知键在后。
    let exported = reparsed.to_ordered_json(ExportMode::Diff);
    let Some(tokens) = exported.get("tokens") else {
        panic!("导出缺少 tokens 层");
    };
    assert_eq!(
        tokens.keys().collect::<Vec<_>>(),
        ["colors", "typography", "motion"]
    );
    let Some(colors) = tokens.get("colors") else {
        panic!("导出缺少 tokens.colors");
    };
    assert_eq!(colors.keys().collect::<Vec<_>>(), ["primary", "sparkle"]);
}

#[test]
fn invalid_values_fall_back_with_diagnostics() {
    let (document, parse_diagnostics) = parse("invalid-values.json");
    let (resolved, diagnostics) = resolve(&document, ThemeMode::Dark);
    let all: Vec<_> = parse_diagnostics
        .iter()
        .chain(&diagnostics)
        .cloned()
        .collect();
    let found = kinds(&all);
    for expected in [
        (DiagnosticKind::InvalidValue, "meta.name"),
        (DiagnosticKind::InvalidValue, "tokens.spacing"),
        (DiagnosticKind::UnknownExtends, "extends"),
        (DiagnosticKind::InvalidValue, "dark.colors.primary"),
        (DiagnosticKind::InvalidValue, "tokens.colors.primary"),
        (DiagnosticKind::InvalidValue, "tokens.colors.success"),
        (DiagnosticKind::InvalidValue, "tokens.colors.warning"),
        (DiagnosticKind::InvalidValue, "tokens.radius.md"),
        (DiagnosticKind::InvalidValue, "tokens.typography.sans"),
        (DiagnosticKind::InvalidValue, "tokens.shadow.lg"),
        (DiagnosticKind::InvalidValue, "tokens.stroke.thin"),
        (
            DiagnosticKind::InvalidValue,
            "tokens.components.input.radius",
        ),
    ] {
        assert!(found.contains(&expected), "缺少 {expected:?}：{found:?}");
    }
    let defaults = resolve(&ThemeDocument::fluxdown_default(), ThemeMode::Dark).0;
    for path in [
        "colors.primary",
        "colors.success",
        "colors.warning",
        "radius.md",
        "spacing.sm",
        "typography.sans",
        "shadow.md",
        "stroke.thin",
        "components.input.radius",
    ] {
        assert_eq!(resolved.get(path), defaults.get(path), "{path}");
    }
    // 其他合法值照常生效。
    assert_eq!(resolved.get("radius.lg"), Some(&TokenValue::Number(10.)));
    assert_eq!(
        resolved.get("components.input.radius"),
        Some(&TokenValue::Number(6.))
    );
}

#[test]
fn out_of_range_values_are_clamped() {
    let (document, _) = parse("out-of-range.json");
    let (resolved, diagnostics) = resolve(&document, ThemeMode::Light);
    let number = |path: &str| resolved.get(path).cloned();
    assert_eq!(number("radius.md"), Some(TokenValue::Number(64.)));
    assert_eq!(number("spacing.xxl"), Some(TokenValue::Number(64.)));
    assert_eq!(number("typography.sm.size"), Some(TokenValue::Number(8.)));
    assert_eq!(
        number("typography.md.lineHeight"),
        Some(TokenValue::Number(16.))
    );
    assert_eq!(
        number("typography.lg.weight"),
        Some(TokenValue::Number(900.))
    );
    assert_eq!(number("stroke.thin"), Some(TokenValue::Number(0.)));
    assert_eq!(number("icon.lg"), Some(TokenValue::Number(8.)));
    assert_eq!(number("density.navRow"), Some(TokenValue::Number(96.)));
    assert_eq!(
        number("components.button.radius"),
        Some(TokenValue::Number(64.))
    );
    assert_eq!(
        diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.kind == DiagnosticKind::OutOfRange)
            .count(),
        8
    );
}

#[test]
fn reference_cycles_fall_back_to_next_layer() {
    let (document, _) = parse("ref-cycle.json");
    let (resolved, diagnostics) = resolve(&document, ThemeMode::Dark);
    let defaults = resolve(&ThemeDocument::fluxdown_default(), ThemeMode::Dark).0;
    for path in [
        "colors.primary",
        "colors.ring",
        "colors.muted",
        "colors.rowHover",
        "spacing.md",
    ] {
        assert_eq!(resolved.get(path), defaults.get(path), "{path}");
    }
    let cycles: Vec<&str> = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.kind == DiagnosticKind::RefCycle)
        .map(|diagnostic| diagnostic.path.as_str())
        .collect();
    assert_eq!(
        cycles,
        [
            "dark.colors.ring",
            "tokens.colors.muted",
            "tokens.spacing.md"
        ]
    );
}

#[test]
fn newer_schema_version_loads_best_effort() {
    let (document, diagnostics) = parse("newer-version.json");
    assert!(kinds(&diagnostics).contains(&(DiagnosticKind::NewerVersion, "schemaVersion")));
    assert_eq!(
        value(&document, ThemeMode::Dark, "colors.accent"),
        color("#60A5FA33")
    );
    assert_eq!(
        value(&document, ThemeMode::Dark, "colors.background"),
        color("#0F172A")
    );
    let exported = document.to_json_value(ExportMode::Diff);
    assert_eq!(exported["schemaVersion"], json!(3));
    assert_eq!(exported["surfaces"], json!({ "glass": true }));
    assert_eq!(exported["tokens"]["motion"], json!({ "fast": 90 }));
}

#[test]
fn flutter_theme_json_converts_to_mode_layer() {
    let (document, diagnostics) = parse("flutter.json");
    assert!(kinds(&diagnostics).contains(&(DiagnosticKind::Migrated, "format")));
    assert!(kinds(&diagnostics).contains(&(DiagnosticKind::InvalidValue, "colors.status.error")));
    assert_eq!(
        document.meta.as_ref().and_then(|meta| meta.name.as_deref()),
        Some("Dracula")
    );
    assert_eq!(
        value(&document, ThemeMode::Dark, "colors.background"),
        color("#282A36")
    );
    assert_eq!(
        value(&document, ThemeMode::Dark, "colors.primary"),
        color("#BD93F9")
    );
    assert_eq!(
        value(&document, ThemeMode::Dark, "colors.primaryForeground"),
        color("#282A36")
    );
    assert_eq!(
        value(&document, ThemeMode::Dark, "colors.accent"),
        color("#BD93F92E")
    );
    assert_eq!(
        value(&document, ThemeMode::Dark, "colors.success"),
        color("#50FA7B")
    );
    // 非法 status.error 不转换，保持基底。
    assert_eq!(
        value(&document, ThemeMode::Dark, "colors.destructive"),
        color("#EF4444")
    );
    // Flutter metrics 不转换；亮色保持默认。
    assert_eq!(
        value(&document, ThemeMode::Dark, "radius.md"),
        TokenValue::Number(6.)
    );
    assert_eq!(
        value(&document, ThemeMode::Light, "colors.primary"),
        color("#3B82F6")
    );
    assert!(
        document
            .token_value(TokenLayer::Light, "colors.primary")
            .is_none()
    );
}

#[test]
fn whole_file_rejections() {
    assert!(matches!(
        ThemeDocument::parse("{"),
        Err(ThemeParseError::InvalidJson(_))
    ));
    assert_eq!(
        ThemeDocument::parse("[1]").err(),
        Some(ThemeParseError::NotAnObject)
    );
    assert!(matches!(
        ThemeDocument::parse(r#"{ "format": "vscode-theme", "colors": {} }"#),
        Err(ThemeParseError::UnsupportedFormat(_))
    ));
}

#[test]
fn editing_api_sets_and_removes_nested_tokens() {
    let mut document = ThemeDocument::fluxdown_default();
    document.set_token(TokenLayer::Light, "components.button.radius", json!(10));
    assert_eq!(
        value(&document, ThemeMode::Light, "components.button.radius"),
        TokenValue::Number(10.)
    );
    assert_eq!(
        value(&document, ThemeMode::Dark, "components.button.radius"),
        TokenValue::Number(6.)
    );
    assert_eq!(
        document.remove_token(TokenLayer::Light, "components.button.radius"),
        Some(json!(10))
    );
    assert!(document.light.is_empty());
    let exported: Value = document.to_json_value(ExportMode::Diff);
    assert!(exported.get("light").is_none());
}
