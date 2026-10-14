#!/usr/bin/env python3
# =============================================================================
# patch-mips32.py —— 让 FluxDown（及其依赖）能在 32 位 MIPS (mipsel) 上编译
# -----------------------------------------------------------------------------
# 背景：mipsel-unknown-linux-musl 是 32 位目标，Rust 的 core/std 不提供 64 位原子
#       类型（AtomicU64 / AtomicI64，cfg(target_has_atomic="64") 不成立）。
#       但「有的代码是无条件使用它们（必须改）」，也「有的代码已经用 #[cfg] 为
#       无原子目标写了 portable_atomic 分支（不能动）」。本脚本区分二者。
#
# 规则：
#   * 只有当文件里存在「未被 #[cfg(...)] 门控的 AtomicU64/AtomicI64」时，才改写该文件：
#       - use std::sync::atomic::{AtomicU64, Ordering};                 → portable_atomic
#       - use std::{ …, sync::atomic::{AtomicU64, Ordering}, … };       （嵌套 use 树，抽出）
#       - 完全限定路径 std::sync::atomic::AtomicU64 / core::…
#   * 若某文件所有 AtomicU64/I64 用法都在 #[cfg] 门控下（例如 serde_core），跳过，
#     因为那些分支只会为「缺原子的目标」启用，本目标不会走。
#   * 只要 crate 里出现了对 `portable_atomic` 的引用（或被改写），就确保其
#     Cargo.toml 声明 `portable-atomic` 依赖（幂等）。
#
# 用法：
#   python3 patch-mips32.py <FluxDown 仓库根>
#   python3 patch-mips32.py "$CARGO_HOME"      # 覆盖 git/checkouts 与 registry/src
# 建议顺序：clean 依赖源码 → cargo fetch → patch(仓库) + patch($CARGO_HOME) → build
# =============================================================================
import re
import sys
from pathlib import Path

DEP_LINE = 'portable-atomic = { version = "1", features = ["fallback"] }'
SKIP_DIRS = {'.git', 'target', 'node_modules'}

PAT_STD = re.compile(r'\bstd::sync::atomic::')
PAT_CORE = re.compile(r'\bcore::sync::atomic::')
NESTED_LINE = re.compile(r'(?m)^[ \t]*sync::atomic::\{([^}]*)\}[ \t]*,?[ \t]*\r?\n')
NESTED_INLINE = re.compile(r'\bsync::atomic::\{([^}]*)\}[ \t]*,?')
ATOMIC_TYPES = ('AtomicU64', 'AtomicI64')


def line_gated(lines, i):
    """判断第 i 行是否处在 #[cfg(...)] 门控之下（向上找最近的属性/项起始行）"""
    j = i - 1
    depth = 0
    while j >= 0:
        s = lines[j].strip()
        if s == '' or s.startswith('//'):
            j -= 1
            continue
        if s.startswith('#'):
            # 属性行（可能是多行）；把该属性块向上合并
            block = [s]
            k = j - 1
            while k >= 0 and block[0].count('(') != block[0].count(')'):
                block.insert(0, lines[k].strip())
                k -= 1
            return any('cfg' in b for b in block)
        # 遇到普通代码行（上一项/上一字段）→ 未被属性门控
        return False
    return False


def has_ungated_atomic(text):
    lines = text.split('\n')
    for i, ln in enumerate(lines):
        if any(t in ln for t in ATOMIC_TYPES):
            if not line_gated(lines, i):
                return True
    return False


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


def hoist_nested(text):
    items = []

    def grab(m):
        items.append(m.group(1).strip())
        return ''

    text = NESTED_LINE.sub(grab, text)
    text = NESTED_INLINE.sub(grab, text)
    return text, items


def insert_use(text, line):
    lines = text.split('\n')
    i = 0
    while i < len(lines):
        s = lines[i].strip()
        if s == '' or s.startswith('//') or s.startswith('/*') or s.startswith('*') \
           or s.startswith('*/') or s.startswith('#!['):
            i += 1
        else:
            break
    lines.insert(i, line)
    return '\n'.join(lines)


def transform(s):
    s2 = PAT_CORE.sub('portable_atomic::', PAT_STD.sub('portable_atomic::', s))
    s2, nested = hoist_nested(s2)
    if nested:
        merged = []
        for it in nested:
            for part in it.split(','):
                part = part.strip()
                if part and part not in merged:
                    merged.append(part)
        s2 = insert_use(s2, 'use portable_atomic::{%s};' % ', '.join(merged))
    return s2


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
        has_64 = any(t in s for t in ATOMIC_TYPES)
        refs_pa = 'portable_atomic' in s
        if not has_64 and not refs_pa:
            continue

        if has_64 and has_ungated_atomic(s):
            ns = transform(s)
            if ns != s:
                f.write_text(ns, encoding='utf-8')
                changed_rs.append(str(f.relative_to(root)))
            c = owning_crate(root, f)
            if c:
                crates.add(c)
        elif refs_pa:
            # 文件引用了 portable_atomic（含被历史补丁改过的情况）→ 确保依赖存在
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
