#!/usr/bin/env python3
"""校验 iOS 源码里 `L("key")` / `L("key", …)`（以及可注入本地化闭包的 `t("key", …)`、
`BuildFlavor.key(full: "key", store: "key")`）用到的文案键
都存在于 assets/i18n/en.json 与 zh.json。

Android 由构建期生成 R.string 在编译时发现漏键；iOS 运行期查表（缺键回退键名），用本脚本在提交前兜底。
用法：python3 mobile/FluxDown/scripts/check-i18n.py   （非零退出 = 有缺失键）
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
sys.exit(1 if missing else 0)
