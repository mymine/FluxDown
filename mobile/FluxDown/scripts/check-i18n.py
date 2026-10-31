#!/usr/bin/env python3
"""校验 iOS 源码里 `L("key")` / `L("key", …)`（以及可注入本地化闭包的 `t("key", …)`、
`BuildFlavor.key(full: "key", store: "key")`）用到的文案键
都存在于 assets/i18n/en.json 与 zh.json；并校验 assets/i18n 下每种语言都已在
`Config/Info.plist` 的 CFBundleLocalizations 与工程 knownRegions 里声明（否则 iOS「设置 › App › 语言」不会列出）。

Android 由构建期生成 R.string 在编译时发现漏键；iOS 运行期查表（缺键回退键名），用本脚本在提交前兜底。
用法：python3 mobile/FluxDown/scripts/check-i18n.py   （非零退出 = 有缺失键或未声明的语言）
"""
import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[3]
IOS = ROOT / "mobile" / "FluxDown"
PATTERN = re.compile(r'\b(?:L|t)\(\s*"([A-Za-z0-9_]+)"')
FLAVOR_PATTERN = re.compile(r'\b(?:full|store):\s*"([A-Za-z0-9_]+)"')

tables = {name: json.loads((ROOT / "assets" / "i18n" / f"{name}.json").read_text("utf-8")) for name in ("en", "zh")}
missing: dict[str, set[str]] = {}
for source in sorted(IOS.rglob("*.swift")):
    if "/Generated/" in str(source) or "/.build/" in str(source):
        continue
    text = source.read_text("utf-8")
    for key in PATTERN.findall(text) + FLAVOR_PATTERN.findall(text):
        for name, table in tables.items():
            if key not in table:
                missing.setdefault(f"{name}:{key}", set()).add(str(source.relative_to(ROOT)))

for entry, files in sorted(missing.items()):
    print(f"missing {entry}  ({', '.join(sorted(files))})")


def ios_region(code: str) -> str:
    """assets/i18n 语言代码 → iOS 本地化标识（简繁中文按文字区分，与现有 zh-Hans / zh-Hant 一致）。"""
    parts = code.replace("_", "-").split("-")
    if parts[0].lower() == "zh":
        return "zh-Hant" if len(parts) > 1 and parts[1].lower() in ("tw", "hk", "mo", "hant") else "zh-Hans"
    return code


plist = (IOS / "Config" / "Info.plist").read_text("utf-8")
declared_plist = set(re.findall(
    r"<string>([^<]+)</string>",
    re.search(r"<key>CFBundleLocalizations</key>\s*<array>(.*?)</array>", plist, re.S).group(1),
))
pbx = (IOS / "FluxDown.xcodeproj" / "project.pbxproj").read_text("utf-8")
declared_pbx = {r.strip().strip('"') for r in re.search(r"knownRegions = \((.*?)\);", pbx, re.S).group(1).split(",") if r.strip()}
undeclared = []
for path in sorted((ROOT / "assets" / "i18n").glob("*.json")):
    region = ios_region(path.stem)
    for where, declared in (("Config/Info.plist CFBundleLocalizations", declared_plist), ("knownRegions", declared_pbx)):
        if region not in declared:
            undeclared.append(f"undeclared {path.stem} → {region}  ({where})")
for line in undeclared:
    print(line)
sys.exit(1 if missing or undeclared else 0)
