#!/usr/bin/env python3
"""校验 ci.yml 的 `dorny/paths-filter` 覆盖了各门禁实际读取的全部仓库路径。

门禁按路径触发：过滤器漏掉一个被读取的目录/文件，改它的 PR 就不会跑对应检查
（典型：只改 Rust 侧共享夹具，TS 镜像测试不跑，§5 镜像契约漂移进 main）。
本脚本从源码推导每个门禁的真实输入，逐一比对过滤器：

- Rust 门禁（rust / android / ios / docker）：入口 crate 经 path 依赖的闭包（`cargo metadata
  --no-deps`，不联网）的目录、这些 crate 里 `include_str!` / `include_bytes!` 引到 crate 外的
  文件、根 `Cargo.toml` 的 `[patch]` 路径、`Cargo.toml` / `Cargo.lock`。
- TS 门禁（web / extension / sdk / website）：项目内源码以相对路径（`from` / `import()` /
  `new URL(…, import.meta.url)` / `require`）引用到项目目录之外的文件或目录。

    python3 .github/scripts/check_ci_paths.py           # 校验，漏项退出 1
    python3 .github/scripts/check_ci_paths.py --print   # 列出每个门禁推导出的外部输入
"""

from __future__ import annotations

import json
import re
import subprocess
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CI = ROOT / ".github" / "workflows" / "ci.yml"

# 过滤器名 → 入口 crate（取 path 依赖闭包）；None = 整个 workspace。
RUST_GATES: dict[str, list[str] | None] = {
    "rust": None,
    "android": ["fluxdown_mobile"],
    "ios": ["fluxdown_mobile"],
    "docker": ["fluxdown_agent", "fluxdown_daemon"],
}
# 过滤器名 → 项目目录。
TS_GATES = {
    "web": "web",
    "extension": "fluxDown",
    "sdk": "packages/plugin-sdk",
    "website": "website-v2",
}
TS_SUFFIXES = {".ts", ".tsx", ".mts", ".cts", ".js", ".mjs", ".cjs", ".json"}
SKIP_DIRS = {"node_modules", "dist", ".output", ".wxt", ".astro", "build", "target", "coverage"}
TS_REF = re.compile(
    r"""(?:\bfrom\s*|\bimport\s*\(\s*|\brequire\s*\(\s*|\bnew\s+URL\s*\(\s*)(['"])(\.{1,2}/[^'"\n]*)\1"""
)
RS_INCLUDE = re.compile(r"""include_(?:str|bytes)!\(\s*"([^"\n]+)"\s*\)""")


def parse_filters(text: str) -> dict[str, list[str]]:
    """解析 `filters: |` 块（只支持本仓库使用的 `name:` + `- 'glob'` 形式）。"""
    lines = text.splitlines()
    start = next((i for i, line in enumerate(lines) if re.match(r"\s*filters:\s*\|\s*$", line)), None)
    if start is None:
        sys.exit("check_ci_paths: ci.yml 中找不到 `filters: |` 块")
    indent = len(lines[start]) - len(lines[start].lstrip())
    filters: dict[str, list[str]] = {}
    current = None
    for line in lines[start + 1 :]:
        if line.strip() and len(line) - len(line.lstrip()) <= indent:
            break
        if m := re.match(r"\s*([A-Za-z0-9_-]+):\s*$", line):
            current = filters.setdefault(m.group(1), [])
        elif (m := re.match(r"""\s*-\s*['"]?([^'"]+)['"]?\s*$""", line)) and current is not None:
            current.append(m.group(1))
    return filters


def glob_regex(glob: str) -> re.Pattern[str]:
    out, i = "", 0
    while i < len(glob):
        if glob.startswith("**", i):
            out += ".*"
            i += 2
        elif glob[i] == "*":
            out += "[^/]*"
            i += 1
        elif glob[i] == "?":
            out += "[^/]"
            i += 1
        else:
            out += re.escape(glob[i])
            i += 1
    return re.compile(out)


def covered(path: str, globs: list[str], is_dir: bool) -> bool:
    # 目录依赖：其下任意文件变化都必须触发，用一个代表性子路径判定。
    probe = f"{path}/__any__" if is_dir else path
    return any(glob_regex(g).fullmatch(probe) for g in globs)


def rel(path: Path) -> str | None:
    try:
        return path.resolve().relative_to(ROOT).as_posix()
    except ValueError:
        return None


def walk(base: Path, suffixes: set[str]):
    for path in sorted(base.rglob("*")):
        if path.is_file() and path.suffix in suffixes and not SKIP_DIRS.intersection(path.relative_to(base).parts):
            yield path


def rust_inputs() -> dict[str, dict[str, bool]]:
    meta = json.loads(
        subprocess.run(
            ["cargo", "metadata", "--no-deps", "--format-version", "1", "--offline"],
            cwd=ROOT, check=True, capture_output=True, text=True,
        ).stdout
    )
    packages = {p["name"]: p for p in meta["packages"]}
    dirs = {name: Path(p["manifest_path"]).parent for name, p in packages.items()}
    # 本地 path 依赖（含 dev / build）：改动它们会改变入口 crate 的构建与测试。
    deps = {
        name: {d["name"] for d in p["dependencies"] if d.get("path") and d["name"] in packages}
        for name, p in packages.items()
    }
    root_toml = tomllib.loads((ROOT / "Cargo.toml").read_text("utf-8"))
    patch_dirs = {
        str(spec["path"])
        for registry in root_toml.get("patch", {}).values()
        for spec in registry.values()
        if isinstance(spec, dict) and "path" in spec
    }

    result: dict[str, dict[str, bool]] = {}
    for gate, entries in RUST_GATES.items():
        todo = list(packages) if entries is None else list(entries)
        seen: set[str] = set()
        while todo:
            name = todo.pop()
            if name in seen:
                continue
            seen.add(name)
            todo.extend(deps.get(name, ()))
        inputs: dict[str, bool] = {"Cargo.toml": False, "Cargo.lock": False}
        inputs.update({p: True for p in patch_dirs})
        for name in sorted(seen):
            crate_dir = dirs[name]
            if (path := rel(crate_dir)) is not None:
                inputs[path] = True
            for src in walk(crate_dir, {".rs"}):
                for m in RS_INCLUDE.finditer(src.read_text("utf-8", errors="replace")):
                    target = (src.parent / m.group(1)).resolve()
                    path = rel(target)
                    # crate 目录内的文件已由目录依赖覆盖，只记越出 crate 的引用。
                    if path is not None and not target.is_relative_to(crate_dir.resolve()):
                        inputs[path] = target.is_dir()
        result[gate] = inputs
    return result


def ts_inputs() -> dict[str, dict[str, bool]]:
    result: dict[str, dict[str, bool]] = {}
    for gate, project in TS_GATES.items():
        base = (ROOT / project).resolve()
        inputs: dict[str, bool] = {}
        for src in walk(base, TS_SUFFIXES):
            for m in TS_REF.finditer(src.read_text("utf-8", errors="replace")):
                target = (src.parent / m.group(2)).resolve()
                if target.is_relative_to(base) or not target.exists():
                    continue
                if (path := rel(target)) is not None:
                    inputs[path] = target.is_dir()
        result[gate] = inputs
    return result


def main() -> int:
    filters = parse_filters(CI.read_text("utf-8"))
    gates = {**rust_inputs(), **ts_inputs()}
    if "--print" in sys.argv[1:]:
        for gate, inputs in gates.items():
            print(f"{gate}:")
            for path, is_dir in sorted(inputs.items()):
                print(f"  {path}{'/' if is_dir else ''}")
        return 0

    missing = []
    for gate, inputs in gates.items():
        globs = filters.get(gate)
        if globs is None:
            missing.append(f"{gate}: ci.yml 缺少同名过滤器")
            continue
        for path, is_dir in sorted(inputs.items()):
            if not covered(path, globs, is_dir):
                missing.append(f"{gate}: {path}{'/' if is_dir else ''}")
    if missing:
        print("ci.yml 的 paths-filter 漏掉了门禁实际读取的路径（改它们不会触发对应检查）：", file=sys.stderr)
        for item in missing:
            print(f"  {item}", file=sys.stderr)
        print("把对应 glob 加进 .github/workflows/ci.yml 的 changes.filters。", file=sys.stderr)
        return 1
    print(f"ci paths ok: {sum(len(v) for v in gates.values())} 项外部输入均被过滤器覆盖")
    return 0


if __name__ == "__main__":
    sys.exit(main())
