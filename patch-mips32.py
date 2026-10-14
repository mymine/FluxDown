#!/usr/bin/env python3
# =============================================================================
# patch-mips32.py —— 让 FluxDown 能在 32 位 MIPS (mipsel) 上编译
# -----------------------------------------------------------------------------
# 问题：mipsel-unknown-linux-musl 是 32 位目标，Rust 的 core/std 不提供
#       64 位原子类型（AtomicU64 / AtomicI64，cfg(target_has_atomic="64") 不成立），
#       而 FluxDown 多处使用了它们 → E0432 unresolved import。
# 解决：改用 `portable-atomic` crate（在无 64 位原子的平台上用锁回退实现），
#       并把源码里 `std::sync::atomic::` 前缀替换为 `portable_atomic::`。
#       替换后所有原子类型（含 AtomicBool/U32/Ordering 等）都由 portable_atomic 提供，
#       在有原生 64 位原子的平台上与 std 等价、零开销。
#
# 用法： python3 patch-mips32.py <FluxDown 仓库根>
# 幂等：重复执行不会重复添加依赖 / 不会二次替换。
# =============================================================================
import re
import sys
from pathlib import Path

DEP_LINE = 'portable-atomic = { version = "1", features = ["fallback"] }'

PAT_STD = re.compile(r'\bstd::sync::atomic::')
PAT_CORE = re.compile(r'\bcore::sync::atomic::')
SKIP_DIRS = {'.git', 'target', 'node_modules', 'website', 'website-v2'}


def owning_crate(root: Path, f: Path) -> Path | None:
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
    if not done:                     # 没有 [dependencies] 段
        out += ['', '[dependencies]', DEP_LINE]
    manifest.write_text('\n'.join(out) + '\n', encoding='utf-8')
    return True


def main() -> int:
    root = Path(sys.argv[1] if len(sys.argv) > 1 else '.').resolve()
    assert (root / 'Cargo.toml').exists(), f'{root} 不是仓库根'

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
        ns = PAT_CORE.sub('portable_atomic::', PAT_STD.sub('portable_atomic::', s))
        if ns != s:
            f.write_text(ns, encoding='utf-8')
            changed_rs.append(str(f.relative_to(root)))
        c = owning_crate(root, f)
        if c:
            crates.add(c)

    for c in sorted(crates):
        if ensure_dep(c / 'Cargo.toml'):
            deps_added.append(str((c / 'Cargo.toml').relative_to(root)))

    print(f'[patch-mips32] 改写源文件 {len(changed_rs)} 个:')
    for p in changed_rs:
        print(f'    {p}')
    print(f'[patch-mips32] 添加 portable-atomic 依赖的 crate: {deps_added or "无"}')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
