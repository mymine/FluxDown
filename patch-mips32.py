#!/usr/bin/env python3
# =============================================================================
# patch-mips32.py —— 让 FluxDown（及其依赖）能在 32 位 MIPS (mipsel) 上编译
# -----------------------------------------------------------------------------
# 问题：mipsel-unknown-linux-musl 是 32 位目标，Rust 的 core/std 不提供
#       64 位原子类型（AtomicU64 / AtomicI64，cfg(target_has_atomic="64") 不成立），
#       而代码多处使用了它们 → E0432 unresolved import。
# 解决：改用 `portable-atomic` crate（无 64 位原生原子的平台上用锁回退实现），
#       把源码里对 std/core 原子的导入改到 `portable_atomic`。
#
# 覆盖三种写法：
#   1) use std::sync::atomic::{AtomicU64, Ordering};
#   2) use std::{ collections::VecDeque, sync::atomic::{AtomicU64, Ordering}, time::Instant };
#      （嵌套 use 树——需要把 sync::atomic::{...} 抽出来做成独立 use）
#   3) 完全限定路径 std::sync::atomic::AtomicU64 / core::sync::atomic::AtomicU64
#
# 用法： python3 patch-mips32.py <目录>
#   - 对 FluxDown 仓库根执行一次；
#   - 对 $CARGO_HOME 再执行一次（覆盖 git/checkouts 与 registry/src 里的第三方 crate，
#     例如 librqbit-core）。
# 幂等。
# =============================================================================
import re
import sys
from pathlib import Path

DEP_LINE = 'portable-atomic = { version = "1", features = ["fallback"] }'
SKIP_DIRS = {'.git', 'target', 'node_modules'}

PAT_STD = re.compile(r'\bstd::sync::atomic::')
PAT_CORE = re.compile(r'\bcore::sync::atomic::')
# 嵌套 use 树里的 sync::atomic::{...}（整行一项）
NESTED_LINE = re.compile(r'(?m)^[ \t]*sync::atomic::\{([^}]*)\}[ \t]*,?[ \t]*\r?\n')
# 同一行内联的 sync::atomic::{...}
NESTED_INLINE = re.compile(r'\bsync::atomic::\{([^}]*)\}[ \t]*,?')


def owning_crate(root: Path, f: Path):
    d = f.parent
    while True:
        if (d / 'Cargo.toml').exists():
            return d
        if d == root or d == d.parent:
            return None
        d = d.parent


def ensure_dep(manifest: Path) -> bool:
    s = manifest.read_text(encoding='utf-8')
    if re.search(r'^portable-atomic\s*=', s, re.M):
        return False
    lines = s.splitlines()
    out, done = [], False
    for l in lines:
        out.append(l)
        if not done and l.strip() == '[dependencies]':
            out.append(DEP_LINE)
            done = True
    if not done:
        out += ['', '[dependencies]', DEP_LINE]
    manifest.write_text('\n'.join(out) + '\n', encoding='utf-8')
    return True


def hoist_nested(text: str):
    """把 use 树里嵌套的 sync::atomic::{...} 抽出来，返回 (新文本, 被抽出的项列表)"""
    items = []

    def grab(m):
        items.append(m.group(1).strip())
        return ''

    text = NESTED_LINE.sub(grab, text)
    text = NESTED_INLINE.sub(grab, text)
    return text, items


def insert_use(text: str, line: str) -> str:
    """把一行 use 插到文件顶部（跳过 inner attributes / 前导注释）"""
    lines = text.split('\n')
    i = 0
    while i < len(lines):
        s = lines[i].strip()
        if s == '' or s.startswith('//') or s.startswith('/*') or s.startswith('*') \
           or s.startswith('*/') or s.startswith('#![') or s.startswith('# ['):
            i += 1
        else:
            break
    lines.insert(i, line)
    return '\n'.join(lines)


def transform(s: str):
    # 1) 简单前缀 / 完全限定路径
    s2 = PAT_CORE.sub('portable_atomic::', PAT_STD.sub('portable_atomic::', s))
    # 2) 嵌套 use 树
    s2, nested_items = hoist_nested(s2)
    changed = s2 != s
    if nested_items:
        merged = []
        for it in nested_items:
            for part in it.split(','):
                part = part.strip()
                if part and part not in merged:
                    merged.append(part)
        s2 = insert_use(s2, 'use portable_atomic::{%s};' % ', '.join(merged))
    return s2, changed


def main() -> int:
    root = Path(sys.argv[1] if len(sys.argv) > 1 else '.').resolve()
    if not root.is_dir():
        print(f'[patch-mips32] 目录不存在，跳过: {root}')
        return 0

    changed_rs, crates, deps_added = [], set(), []
    for f in root.rglob('*.rs'):
        if any(seg in f.parts for seg in SKIP_DIRS):
            continue
        try:
            s = f.read_text(encoding='utf-8')
        except (UnicodeDecodeError, OSError):
            continue
        if 'AtomicU64' not in s and 'AtomicI64' not in s:
            continue
        ns, changed = transform(s)
        if ns != s:
            f.write_text(ns, encoding='utf-8')
            changed_rs.append(str(f.relative_to(root)))
        c = owning_crate(root, f)
        if c:
            crates.add(c)

    for c in sorted(crates):
        if ensure_dep(c / 'Cargo.toml'):
            deps_added.append(str((c / 'Cargo.toml').relative_to(root)))

    if changed_rs:
        print(f'[patch-mips32] {root}: 改写 {len(changed_rs)} 个源文件')
        for p in changed_rs:
            print(f'    {p}')
    if deps_added:
        print(f'[patch-mips32] {root}: 添加 portable-atomic 依赖: {deps_added}')
    if not changed_rs and not deps_added:
        print(f'[patch-mips32] {root}: 无需改动')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
