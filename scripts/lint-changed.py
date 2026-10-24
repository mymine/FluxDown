#!/usr/bin/env python3
"""lint-changed：只评判「本次改动行」的 clippy 棘轮。

用途
    工作区 clippy 已 deny unwrap/expect/忽略错误，但对解析不可信输入的代码
    （naming/ Content-Disposition、BT 元数据、HTTP 头、协议解码）还缺：
    `&s[a..b]` 非字符边界 panic、裸索引、`as` 截断、
    `String::truncate/split_off/insert_str/drain/replace_range` 与 `str::split_at` 的字符边界 panic。
    这些 lint 全量开启会要求改大量存量代码，所以只对「改动行」设闸：
    存量不动，新写的代码不再引入同类问题（棘轮只进不退）。

用法
    python3 scripts/lint-changed.py [--base REF] [--package P ...] [--all-lines] [-v]

    --base REF     对比基线，缺省 `git merge-base HEAD origin/main`，失败回退 `main`。
                   改动集 = REF 以来已提交 + 暂存 + 未暂存 + 未跟踪文件的 `.rs` 新增行。
    --package P    只检查指定 package（可重复）；缺省由改动文件映射（取最长 manifest_dir 前缀）。
    --all-lines    不按改动行过滤，报告所选 package 内全部命中（用于评估存量）。
    -v             打印 cargo 原始输出。

    阻断级（改动行命中则失败）：string_slice / panic / dbg_macro / mem_forget
    提示级（只打印）：indexing_slicing / disallowed_methods / cast_possible_truncation /
                      unreachable / missing_panics_doc
    测试代码豁免：路径含 /tests/ /benches/ /examples/，或 `#[cfg(test)] mod … { }` 块内。

    clippy 配置在 scripts/lint-changed/clippy.toml（经 CLIPPY_CONF_DIR 指定，会整体替代根
    clippy.toml，所以其中必须包含根配置的全部行；本脚本启动时校验，不一致退出 2）。

退出码
    0 干净（含无改动，此时不调用 cargo）
    1 改动行存在阻断级问题
    2 用法或环境错误（git/cargo 失败、rustc 编译错误、配置不同步）
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CONF_DIR = Path("scripts/lint-changed")

BLOCKING = ["string_slice", "panic", "dbg_macro", "mem_forget"]
ADVISORY = [
    "indexing_slicing",
    "disallowed_methods",
    "cast_possible_truncation",
    "unreachable",
    "missing_panics_doc",
]
BLOCK_SET = {f"clippy::{n}" for n in BLOCKING}
ADVISORY_SET = {f"clippy::{n}" for n in ADVISORY}
EXEMPT_DIRS = ("/tests/", "/benches/", "/examples/")


def die(msg: str, code: int = 2) -> "None":
    print(f"lint-changed: {msg}", file=sys.stderr)
    sys.exit(code)


def run(cmd: list[str], **kw) -> subprocess.CompletedProcess:
    return subprocess.run(
        cmd, cwd=ROOT, capture_output=True, text=True, encoding="utf-8", errors="replace", **kw
    )


def git(*args: str) -> str:
    r = run(["git", *args])
    if r.returncode != 0:
        die(f"git {' '.join(args)} 失败: {r.stderr.strip()}")
    return r.stdout


def resolve_base(base: str | None) -> str:
    if base:
        return base
    r = run(["git", "merge-base", "HEAD", "origin/main"])
    if r.returncode == 0 and r.stdout.strip():
        return r.stdout.strip()
    r = run(["git", "merge-base", "HEAD", "main"])
    if r.returncode == 0 and r.stdout.strip():
        return r.stdout.strip()
    die("无法确定基线：origin/main 与 main 均不可用，请用 --base 指定")
    return ""


def check_conf_sync() -> None:
    root_conf = ROOT / "clippy.toml"
    mine = ROOT / CONF_DIR / "clippy.toml"
    if not mine.is_file():
        die(f"缺少 {CONF_DIR}/clippy.toml")
    if not root_conf.is_file():
        return
    have = {l.strip() for l in mine.read_text(encoding="utf-8").splitlines()}
    for line in root_conf.read_text(encoding="utf-8").splitlines():
        s = line.strip()
        if s and not s.startswith("#") and s not in have:
            die(f"{CONF_DIR}/clippy.toml 未同步根 clippy.toml，缺少行: {s}")


def changed_lines(base: str) -> dict[str, set[int]]:
    """相对 base 的新增行：{相对路径: {行号}}（仅 .rs）。"""
    out: dict[str, set[int]] = {}
    diff = git("diff", "-U0", "--no-color", "--no-renames", base, "--", "*.rs")
    cur: str | None = None
    for line in diff.splitlines():
        if line.startswith("+++ "):
            p = line[4:]
            cur = p[2:] if p.startswith("b/") else None
            if cur is not None:
                out.setdefault(cur, set())
        elif line.startswith("@@") and cur is not None:
            m = re.match(r"@@ -\d+(?:,\d+)? \+(\d+)(?:,(\d+))? @@", line)
            if m:
                start = int(m.group(1))
                n = int(m.group(2)) if m.group(2) is not None else 1
                out[cur].update(range(start, start + n))
    for p in git("ls-files", "--others", "--exclude-standard", "-z", "--", "*.rs").split("\0"):
        if not p:
            continue
        try:
            text = (ROOT / p).read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        out[p] = set(range(1, text.count("\n") + 2))
    return {p: ls for p, ls in out.items() if ls and (ROOT / p).is_file()}


def cargo_packages() -> list[tuple[str, Path]]:
    r = run(["cargo", "metadata", "--no-deps", "--format-version", "1"])
    if r.returncode != 0:
        die(f"cargo metadata 失败: {r.stderr.strip()}")
    meta = json.loads(r.stdout)
    return [(p["name"], Path(p["manifest_path"]).parent) for p in meta["packages"]]


def package_of(path: str, pkgs: list[tuple[str, Path]]) -> str | None:
    abs_p = (ROOT / path).resolve()
    best: tuple[int, str] | None = None
    for name, d in pkgs:
        try:
            abs_p.relative_to(d)
        except ValueError:
            continue
        if best is None or len(str(d)) > best[0]:
            best = (len(str(d)), name)
    return best[1] if best else None


# ---------- 测试块范围 ----------

_CFG_TEST = re.compile(r"#\[cfg\(test\)\]")
_MOD_OPEN = re.compile(r"\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+\w+\s*\{")
_ATTR_OR_WS = re.compile(r"(?:\s+|#\[[^\]]*\]|//[^\n]*)*")


def _skip_non_code(src: str, i: int) -> int:
    """若 i 处是字符串/字符/注释起点，返回其末尾之后的位置，否则返回 i。"""
    n = len(src)
    if src.startswith("//", i):
        j = src.find("\n", i)
        return n if j < 0 else j
    if src.startswith("/*", i):
        depth, j = 1, i + 2
        while j < n and depth:
            if src.startswith("/*", j):
                depth, j = depth + 1, j + 2
            elif src.startswith("*/", j):
                depth, j = depth - 1, j + 2
            else:
                j += 1
        return j
    m = re.compile(r'b?r(#*)"').match(src, i)
    if m:
        end = src.find('"' + m.group(1), m.end())
        return n if end < 0 else end + 1 + len(m.group(1))
    if src[i] == '"':
        j = i + 1
        while j < n and src[j] != '"':
            j += 2 if src[j] == "\\" else 1
        return j + 1
    m = re.compile(r"'(?:\\[^']+|[^\\'])'").match(src, i)
    if m:
        return m.end()
    return i


def test_ranges(src: str) -> list[tuple[int, int]]:
    """`#[cfg(test)]` 后紧跟的 `mod … {` 块的行号范围（含首尾行）。"""
    ranges: list[tuple[int, int]] = []
    for m in _CFG_TEST.finditer(src):
        j = _ATTR_OR_WS.match(src, m.end()).end()
        mo = _MOD_OPEN.match(src, j)
        if not mo:
            continue
        depth, k, n = 1, mo.end(), len(src)
        while k < n and depth:
            nk = _skip_non_code(src, k)
            if nk != k:
                k = nk
                continue
            c = src[k]
            depth += (c == "{") - (c == "}")
            k += 1
        ranges.append((src.count("\n", 0, m.start()) + 1, src.count("\n", 0, k) + 1))
    return ranges


_range_cache: dict[str, list[tuple[int, int]]] = {}


def is_exempt(path: str, line: int) -> bool:
    if any(d in "/" + path for d in EXEMPT_DIRS):
        return True
    if path not in _range_cache:
        try:
            _range_cache[path] = test_ranges((ROOT / path).read_text(encoding="utf-8"))
        except OSError:
            _range_cache[path] = []
    return any(a <= line <= b for a, b in _range_cache[path])


# ---------- 主流程 ----------


def main() -> None:
    ap = argparse.ArgumentParser(description="只评判改动行的 clippy 棘轮")
    ap.add_argument("--base", metavar="REF")
    ap.add_argument("--package", "-p", action="append", default=[], metavar="P")
    ap.add_argument("--all-lines", action="store_true")
    ap.add_argument("-v", "--verbose", action="store_true")
    args = ap.parse_args()

    check_conf_sync()

    changed: dict[str, set[int]] = {}
    if not args.all_lines:
        changed = changed_lines(resolve_base(args.base))
        if not changed:
            print("lint-changed: 没有改动的 .rs 行，跳过")
            sys.exit(0)

    pkgs_meta = cargo_packages()
    if args.package:
        known = {n for n, _ in pkgs_meta}
        bad = [p for p in args.package if p not in known]
        if bad:
            die(f"未知 package: {', '.join(bad)}")
        packages = sorted(set(args.package))
    elif args.all_lines:
        die("--all-lines 需要配合 --package 指定范围")
        return
    else:
        packages = sorted(
            {p for f in changed if (p := package_of(f, pkgs_meta)) is not None}
        )
        if not packages:
            print("lint-changed: 改动文件不属于任何 workspace package，跳过")
            sys.exit(0)

    cmd = ["cargo", "clippy"]
    for p in packages:
        cmd += ["-p", p]
    cmd += ["--all-targets", "--message-format=json", "--"]
    for n in BLOCKING + ADVISORY:
        cmd += ["-W", f"clippy::{n}"]
    env = {**os.environ, "CLIPPY_CONF_DIR": str(ROOT / CONF_DIR)}
    print(f"lint-changed: cargo clippy 包: {', '.join(packages)}", flush=True)
    r = subprocess.run(
        cmd, cwd=ROOT, env=env, capture_output=True, text=True, encoding="utf-8", errors="replace"
    )

    if args.verbose:
        sys.stdout.write(r.stdout)
        sys.stderr.write(r.stderr)

    hits: dict[tuple[str, int, int, str], str] = {}
    errors: list[str] = []
    for raw in r.stdout.splitlines():
        try:
            obj = json.loads(raw)
        except json.JSONDecodeError:
            continue
        if obj.get("reason") != "compiler-message":
            continue
        msg = obj["message"]
        code = (msg.get("code") or {}).get("code")
        if msg.get("level") in ("error", "error: internal compiler error") and code not in (
            BLOCK_SET | ADVISORY_SET
        ):
            errors.append(msg.get("rendered") or msg.get("message", ""))
            continue
        if code not in BLOCK_SET | ADVISORY_SET:
            continue
        span = next((s for s in msg.get("spans", []) if s.get("is_primary")), None)
        # 宏展开里的命中回溯到最外层调用点
        while span is not None:
            f = span["file_name"]
            if not os.path.isabs(f) and not f.startswith(".."):
                if args.all_lines or span["line_start"] in changed.get(f, ()):
                    break
            exp = span.get("expansion")
            span = exp["span"] if exp else None
            if span is None:
                break
        if span is None:
            continue
        f, line, col = span["file_name"], span["line_start"], span["column_start"]
        if is_exempt(f, line):
            continue
        hits.setdefault((f, line, col, code), msg.get("message", "").splitlines()[0])

    if errors:
        print("lint-changed: 编译失败或触发 workspace 级 deny lint，请先修复:", file=sys.stderr)
        for e in errors:
            sys.stderr.write(e)
        sys.exit(2)
    if r.returncode != 0 and not hits and not r.stdout.strip():
        die(f"cargo clippy 失败: {r.stderr.strip()}")

    blocked = advisory = 0
    for (f, line, col, code), text in sorted(hits.items()):
        is_block = code in BLOCK_SET
        blocked += is_block
        advisory += not is_block
        print(f"{f}:{line}:{col} [{'阻断' if is_block else '提示'}] {code.removeprefix('clippy::')}  {text}")
    print(f"lint-changed: 阻断 {blocked} 条，提示 {advisory} 条")
    sys.exit(1 if blocked else 0)


if __name__ == "__main__":
    main()
