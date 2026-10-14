#!/usr/bin/env python3
# =============================================================================
# patch-mips32.py —— 让 FluxDown（及其依赖）能在 32 位 MIPS (mipsel) 上编译
# -----------------------------------------------------------------------------
# 背景：mipsel-unknown-linux-musl 是 32 位目标，Rust 的 core/std 不提供 64 位原子
#       类型（AtomicU64 / AtomicI64，需要 target_has_atomic="64"）。但：
#         * 有的代码「无条件」从 std/core 取用它们 → 必须改；
#         * 有的代码已用 #[cfg] 为无原子目标写了分支（如 serde_core）→ 不能动；
#         * 有的 crate 用自定义 build.rs cfg + 宏式条件编译回退（如 radium 的 if_atomic!）→ 不能动；
#         * 有的 crate 用自己的抽象层提供回退（如 tokio 的 crate::loom::...；portable-atomic 自身）→ 不能动；
#         * 有的只是在「文档注释/字符串」里提到（如 nix 的 //! 例子）→ 绝不能动。
#
# 本脚本「精准」地只搬运「代码里」从 std/core 取用的 AtomicU64 / AtomicI64：
#   1) use std::sync::atomic::{AtomicU64, Ordering};         → Ordering 留 std，AtomicU64 走替身
#   2) use std::{ …, sync::atomic::{AtomicU64, Ordering}, …} → 仅从 std 树的嵌套组里摘出 AtomicU64
#   3) 完全限定路径 std::sync::atomic::AtomicU64             → 替身::AtomicU64
# 其它原子类型（Ordering / AtomicU32 / AtomicUsize…）原样保留；绝不触碰 crate::… 这种非 std/core 路径。
# 检测与改写前会先「屏蔽」注释与字符串字面量，因此注释里出现的 AtomicU64 不会触发/被改。
#
# 替身：**统一使用「注入到 crate 内部的 shim 模块」**（src/pd_atomic_shim.rs）。该 shim 只用 core，
# 以「自旋锁 + 两个 32 位原子」实现 AtomicU64/AtomicI64，API 与 std 一致（含 update/try_update/
# fetch_update）——因此 std / no_std crate 都能编译。好处：
#   * 不新增任何外部依赖（registry 依赖的 Cargo.toml 本来也改不动：cargo 用 registry 元数据里的
#     清单做依赖解析，不重读 $CARGO_HOME/registry/src/.../Cargo.toml）；
#   * std 较新加入的方法（AtomicU64::update / try_update）在 shim 里同样提供，避免「换成
#     portable-atomic 后缺 update」这类问题；portable-atomic 也会引入 std 依赖，对 no_std 不友好。
#   * 仅极少数「找不到 crate 根源文件（lib.rs/main.rs）」的 crate 才回退用 portable-atomic 依赖。
#
# 跳过条件：文件里（代码中）AtomicU64/I64 全部处在 #[cfg] 门控下；或属于 portable-atomic 自身；
#           registry 依赖若自带无 64 位原子目标的处理（源码含 target_has_atomic / portable_atomic /
#           if_atomic / radium_atomic，或 build.rs 提及 atomic）则整包跳过。
#
# 用法：
#   python3 patch-mips32.py <FluxDown 仓库根>
#   python3 patch-mips32.py "$CARGO_HOME"      # git/checkouts 与 registry/src
# 强制（判据误跳过时）改某个 registry crate：PATCH_REGISTRY_CRATES=<crate目录名,...> python3 patch-mips32.py "$CARGO_HOME"
# =============================================================================
import os
import re
import sys
from pathlib import Path

DEP_LINE = 'portable-atomic = { version = "1", features = ["fallback"] }'
SHIM_MOD = 'pd_atomic_shim'                 # 注入 crate 的 shim 模块名
SKIP_DIRS = {'.git', 'target', 'node_modules'}
SKIP_CRATE_PREFIX = 'portable-atomic'      # 排除 portable-atomic / portable-atomic-util 自身
SUB_CRATE_DIRS = {'examples', 'benches', 'tests'}   # registry crate 内的独立子 crate，不参与本目标构建
ATOMIC64 = ('AtomicU64', 'AtomicI64')
SELF_HANDLE_MARKERS = ('target_has_atomic', 'portable_atomic', 'if_atomic', 'radium_atomic')

SIMPLE_USE = re.compile(r'use\s+(std|core)::sync::atomic::\{([^}]*)\};')
QUALIFIED = re.compile(r'\b(?:std|core)::sync::atomic::(AtomicU64|AtomicI64)\b')
STD_BLOCK = re.compile(r'use\s+(std|core)::\{[^;]*\};', re.S)   # [^;]* 可跨任意层级嵌套
ATOMIC_GROUP = re.compile(r'atomic::\{([^}]*)\}')               # 含 sync::atomic::{…} 与 atomic::{…}
HAVE_DEP = re.compile(r'(?m)^\s*portable-atomic\s*=|^\s*\[[^\]]*dependencies\.portable-atomic\s*\]')
PORTABLE_PATH = re.compile(r'(?:::)?portable_atomic::')

# 注入 crate 内部的最小 64 位原子 shim（只用 core；自旋锁 + 两个 32 位原子；std/no_std 通用）
SHIM_SRC = '''//! 由 patch-mips32.py 自动注入：在缺少原生 64 位原子（无 target_has_atomic="64"）的
//! 32 位目标（如 mipsel）上，用「自旋锁 + 两个 32 位原子」提供 AtomicU64 / AtomicI64 的
//! 等价实现（含 std 较新加入的 update / try_update）。仅依赖 core，std / no_std crate 均可编译。
#![allow(dead_code, unused_imports)]
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

macro_rules! pd_atomic64 {
    ($name:ident, $int:ty) => {
        #[derive(Debug, Default)]
        pub struct $name {
            lock: AtomicBool,
            hi: AtomicU32,
            lo: AtomicU32,
        }

        impl $name {
            pub const fn new(v: $int) -> Self {
                Self {
                    lock: AtomicBool::new(false),
                    hi: AtomicU32::new(((v as u64) >> 32) as u32),
                    lo: AtomicU32::new((v as u64) as u32),
                }
            }

            #[inline]
            fn acquire(&self) {
                while self
                    .lock
                    .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                    .is_err()
                {
                    core::hint::spin_loop();
                }
            }

            #[inline]
            fn release(&self) {
                self.lock.store(false, Ordering::Release);
            }

            #[inline]
            fn get_raw(&self) -> $int {
                (((self.hi.load(Ordering::Relaxed) as u64) << 32)
                    | (self.lo.load(Ordering::Relaxed) as u64)) as $int
            }

            #[inline]
            fn set_raw(&self, v: $int) {
                let u = v as u64;
                self.lo.store(u as u32, Ordering::Relaxed);
                self.hi.store((u >> 32) as u32, Ordering::Relaxed);
            }

            pub fn load(&self, _o: Ordering) -> $int {
                self.acquire();
                let v = self.get_raw();
                self.release();
                v
            }

            pub fn store(&self, val: $int, _o: Ordering) {
                self.acquire();
                self.set_raw(val);
                self.release();
            }

            pub fn swap(&self, val: $int, _o: Ordering) -> $int {
                self.acquire();
                let old = self.get_raw();
                self.set_raw(val);
                self.release();
                old
            }

            pub fn fetch_add(&self, val: $int, _o: Ordering) -> $int {
                self.acquire();
                let old = self.get_raw();
                self.set_raw(old.wrapping_add(val));
                self.release();
                old
            }

            pub fn fetch_sub(&self, val: $int, _o: Ordering) -> $int {
                self.acquire();
                let old = self.get_raw();
                self.set_raw(old.wrapping_sub(val));
                self.release();
                old
            }

            pub fn fetch_and(&self, val: $int, _o: Ordering) -> $int {
                self.acquire();
                let old = self.get_raw();
                self.set_raw(old & val);
                self.release();
                old
            }

            pub fn fetch_or(&self, val: $int, _o: Ordering) -> $int {
                self.acquire();
                let old = self.get_raw();
                self.set_raw(old | val);
                self.release();
                old
            }

            pub fn fetch_xor(&self, val: $int, _o: Ordering) -> $int {
                self.acquire();
                let old = self.get_raw();
                self.set_raw(old ^ val);
                self.release();
                old
            }

            pub fn compare_exchange(
                &self,
                cur: $int,
                new: $int,
                _s: Ordering,
                _f: Ordering,
            ) -> Result<$int, $int> {
                self.acquire();
                let old = self.get_raw();
                let r = if old == cur {
                    self.set_raw(new);
                    Ok(old)
                } else {
                    Err(old)
                };
                self.release();
                r
            }

            pub fn compare_exchange_weak(
                &self,
                cur: $int,
                new: $int,
                s: Ordering,
                f: Ordering,
            ) -> Result<$int, $int> {
                self.compare_exchange(cur, new, s, f)
            }

            /// 应用 f 到当前值，写回新值，返回旧值（与 std 的 AtomicU64::update 语义一致）。
            pub fn update(
                &self,
                _set: Ordering,
                _fetch: Ordering,
                mut f: impl FnMut($int) -> $int,
            ) -> $int {
                self.acquire();
                let old = self.get_raw();
                self.set_raw(f(old));
                self.release();
                old
            }

            /// 应用 f，返回 Some(new) 则写回，否则不动（与 std 的 try_update/fetch_update 一致）。
            pub fn fetch_update(
                &self,
                _set: Ordering,
                _fetch: Ordering,
                mut f: impl FnMut($int) -> Option<$int>,
            ) -> Result<$int, $int> {
                self.acquire();
                let old = self.get_raw();
                let r = match f(old) {
                    Some(new) => {
                        self.set_raw(new);
                        Ok(old)
                    }
                    None => Err(old),
                };
                self.release();
                r
            }

            pub fn try_update(
                &self,
                s: Ordering,
                f: Ordering,
                fun: impl FnMut($int) -> Option<$int>,
            ) -> Result<$int, $int> {
                self.fetch_update(s, f, fun)
            }

            pub fn into_inner(self) -> $int {
                self.get_raw()
            }
        }
    };
}

pd_atomic64!(AtomicU64, u64);
pd_atomic64!(AtomicI64, i64);
'''


# ---------------------------------------------------------------------------
# 屏蔽注释与字符串字面量（同长度替换为空格），用于「只处理真正的代码」。
# ---------------------------------------------------------------------------
def mask_code(text):
    out = list(text)
    n = len(text)
    i = 0

    def blank(a, b):
        for k in range(a, b):
            if out[k] != '\n':
                out[k] = ' '

    while i < n:
        if text.startswith('//', i):
            j = text.find('\n', i)
            if j < 0:
                j = n
            blank(i, j)
            i = j
            continue
        if text.startswith('/*', i):
            depth = 1
            j = i + 2
            while j < n and depth > 0:
                if text.startswith('/*', j):
                    depth += 1
                    j += 2
                elif text.startswith('*/', j):
                    depth -= 1
                    j += 2
                else:
                    j += 1
            blank(i, j)
            i = j
            continue
        m = re.compile(r'(?:b|rb|br)?r(#*)"').match(text, i)
        if m:
            hashes = m.group(1)
            close = '"' + hashes
            j = text.find(close, m.end())
            j = n if j < 0 else j + len(close)
            blank(m.start(), j)
            i = j
            continue
        c = text[i]
        if c == '"' or (c == 'b' and i + 1 < n and text[i + 1] == '"'):
            start = i
            j = i + 1 if c == '"' else i + 2
            while j < n:
                if text[j] == '\\':
                    j += 2
                    continue
                if text[j] == '"':
                    j += 1
                    break
                j += 1
            blank(start, j)
            i = j
            continue
        if c == "'":
            if i + 1 < n and text[i + 1] == '\\':
                j = i + 3
                if j < n and text[j] == "'":
                    j += 1
                blank(i, j)
                i = j
                continue
            if i + 2 < n and text[i + 1] != "'" and text[i + 2] == "'":
                blank(i, i + 3)
                i += 3
                continue
        i += 1
    return ''.join(out)


def is_code(masked, start, end):
    return masked[start:end].strip() != ''


def inner_prefix_end(text):
    """返回「内部属性(#![…]，可跨多行) + 文档注释」前缀之后的第一个字符下标。"""
    i = 0
    n = len(text)
    while i < n:
        c = text[i]
        if c.isspace():
            i += 1
            continue
        if text.startswith('//', i):
            j = text.find('\n', i)
            i = n if j < 0 else j + 1
            continue
        if text.startswith('/*', i):
            depth = 1
            j = i + 2
            while j < n and depth > 0:
                if text.startswith('/*', j):
                    depth += 1
                    j += 2
                elif text.startswith('*/', j):
                    depth -= 1
                    j += 2
                else:
                    j += 1
            i = j
            continue
        if text.startswith('#!', i):
            k = text.find('[', i)
            if k < 0:
                break
            depth = 0
            p = k
            while p < n:
                ch = text[p]
                if ch == '"':
                    p += 1
                    while p < n:
                        if text[p] == '\\':
                            p += 2
                            continue
                        if text[p] == '"':
                            p += 1
                            break
                        p += 1
                    continue
                if ch == '[':
                    depth += 1
                elif ch == ']':
                    depth -= 1
                    if depth == 0:
                        p += 1
                        break
                p += 1
            i = p
            continue
        break
    return i


def line_gated(lines, i):
    j = i - 1
    while j >= 0:
        s = lines[j].strip()
        if s == '' or s.startswith('//'):
            j -= 1
            continue
        if s.startswith('#'):
            block = [s]
            k = j - 1
            while k >= 0 and block[0].count('(') != block[0].count(')'):
                block.insert(0, lines[k].strip())
                k -= 1
            return any('cfg' in b for b in block)
        return False
    return False


def has_ungated_atomic(masked):
    lines = masked.split('\n')
    for i, ln in enumerate(lines):
        if any(t in ln for t in ATOMIC64) and not line_gated(lines, i):
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
    """给 crate 的 Cargo.toml 补 portable-atomic 依赖（仅回退路径使用，幂等）。"""
    s = manifest.read_text(encoding='utf-8')
    if HAVE_DEP.search(s):
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


def crate_root_file(crate_dir: Path):
    """定位 crate 的根源文件（lib.rs / main.rs / Cargo.toml 里 [lib].path 指定的文件）。"""
    try:
        s = (crate_dir / 'Cargo.toml').read_text(encoding='utf-8')
    except OSError:
        s = ''
    m = re.search(r'(?ms)^\[lib\](.*?)(?=^\[|\Z)', s)
    if m:
        pm = re.search(r'(?m)^\s*path\s*=\s*"([^"]+)"', m.group(1))
        if pm:
            p = crate_dir / pm.group(1)
            if p.exists():
                return p
    for name in ('src/lib.rs', 'src/main.rs'):
        p = crate_dir / name
        if p.exists():
            return p
    return None


def ensure_shim(crate_dir: Path) -> bool:
    """在 crate 内注入 shim 模块并在根文件声明 mod（幂等）。返回是否有改动。"""
    root_file = crate_root_file(crate_dir)
    if root_file is None:
        return False
    changed = False
    shim = root_file.parent / (SHIM_MOD + '.rs')
    if (not shim.exists()) or shim.read_text(encoding='utf-8') != SHIM_SRC:
        shim.write_text(SHIM_SRC, encoding='utf-8')
        changed = True
    t = root_file.read_text(encoding='utf-8')
    if not re.search(r'(?m)^\s*(?:pub\s+)?mod\s+%s\s*;' % SHIM_MOD, t):
        i = inner_prefix_end(t)
        t = t[:i] + 'mod %s;\n' % SHIM_MOD + t[i:]
        root_file.write_text(t, encoding='utf-8')
        changed = True
    return changed


def split_items(s):
    return [x.strip() for x in s.split(',') if x.strip()]


def transform(text, masked):
    moved = []

    def repl_qual(m):
        if not is_code(masked, m.start(), m.end()):
            return m.group(0)
        moved.append(m.group(1))
        return 'portable_atomic::' + m.group(1)
    text = QUALIFIED.sub(repl_qual, text)

    def repl_simple(m):
        if not is_code(masked, m.start(), m.end()):
            return m.group(0)
        scope, items = m.group(1), split_items(m.group(2))
        keep = [x for x in items if x not in ATOMIC64]
        mv = [x for x in items if x in ATOMIC64]
        moved.extend(mv)
        out = []
        if keep:
            out.append('use %s::sync::atomic::{%s};' % (scope, ', '.join(keep)))
        if mv:
            out.append('use portable_atomic::{%s};' % ', '.join(mv))
        return '\n'.join(out)
    text = SIMPLE_USE.sub(repl_simple, text)

    def repl_block(m):
        if not is_code(masked, m.start(), m.end()):
            return m.group(0)
        block = m.group(0)
        dropped = [False]

        def repl_group(gm):
            items = split_items(gm.group(1))
            mv = [x for x in items if x in ATOMIC64]
            if not mv:
                return gm.group(0)
            keep = [x for x in items if x not in ATOMIC64]
            moved.extend(mv)
            if keep:
                return 'atomic::{%s}' % ', '.join(keep)
            dropped[0] = True
            return '\x00DROP\x00'

        nb = ATOMIC_GROUP.sub(repl_group, block)
        if dropped[0]:
            nb = re.sub(r'\x00DROP\x00[ \t]*,', '', nb)
            nb = re.sub(r',[ \t]*\x00DROP\x00', '', nb)
            nb = nb.replace('\x00DROP\x00', '')
            nb = re.sub(r'\{(?=[ \t]*[,}])', '{ ', nb)
            nb = re.sub(r'\{\s*,', '{', nb)
            nb = re.sub(r',(\s*)\}', r'\1}', nb)
            nb = re.sub(r',\s*,', ',', nb)
            nb = re.sub(r'\bsync::(?=[ \t]*[,}])', '', nb)
            nb = re.sub(r'\batomic::(?=[ \t]*[,}])', '', nb)
        return nb
    text = STD_BLOCK.sub(repl_block, text)

    if moved:
        uniq = []
        for x in moved:
            if x not in uniq:
                uniq.append(x)
        if 'use portable_atomic::' not in text:
            i = inner_prefix_end(text)
            text = text[:i] + ('#[allow(unused_imports)] use portable_atomic::{%s};\n' % ', '.join(uniq)) + text[i:]
    return text


def main() -> int:
    root = Path(sys.argv[1] if len(sys.argv) > 1 else '.').resolve()
    if not root.is_dir():
        print(f'[patch-mips32] 目录不存在，跳过: {root}')
        return 0

    changed_rs, deps_added, shim_crates, port_crates = [], [], set(), set()
    skipped_noroot = []
    extra_reg = {x for x in (os.environ.get('PATCH_REGISTRY_CRATES', '') or '').split(',') if x}
    self_cache = {}
    root_cache = {}

    def crate_self_handles(crate_dir):
        """crate 是否自带「无 64 位原子目标」的处理（应整包跳过）。"""
        if crate_dir in self_cache:
            return self_cache[crate_dir]
        res = False
        for rf in crate_dir.rglob('*.rs'):
            try:
                t = mask_code(rf.read_text(encoding='utf-8'))
            except (UnicodeDecodeError, OSError):
                continue
            if any(mk in t for mk in SELF_HANDLE_MARKERS):
                res = True
                break
        if not res:
            # 宏式条件编译（如 radium 的 if_atomic!）由 build.rs 设置自定义 cfg 决定
            try:
                br = (crate_dir / 'build.rs').read_text(encoding='utf-8')
            except OSError:
                br = ''
            if 'atomic' in br:
                res = True
        self_cache[crate_dir] = res
        return res

    def has_root(crate_dir):
        if crate_dir not in root_cache:
            root_cache[crate_dir] = crate_root_file(crate_dir)
        return root_cache[crate_dir] is not None

    for f in root.rglob('*.rs'):
        if any(seg in f.parts for seg in SKIP_DIRS):
            continue
        if any(seg.startswith(SKIP_CRATE_PREFIX) for seg in f.parts):
            continue    # 绝不改 portable-atomic 自身
        c0 = owning_crate(root, f)
        is_registry = 'registry' in f.parts
        if is_registry:
            if not any(c in f.parts for c in extra_reg) and c0 and crate_self_handles(c0):
                continue    # 自带处理（tokio/serde_core/async-task/radium…）→ 跳过
            if c0:
                try:
                    rel = f.relative_to(c0)
                except ValueError:
                    rel = None
                if rel and rel.parts and rel.parts[0] in SUB_CRATE_DIRS:
                    continue    # 独立子 crate（examples/benches/tests），不参与构建
        try:
            s = f.read_text(encoding='utf-8')
        except (UnicodeDecodeError, OSError):
            continue
        masked = mask_code(s)
        if not any(t in masked for t in ATOMIC64):
            continue    # 只出现在注释/字符串里（如 nix 的文档示例）→ 不改
        if not has_ungated_atomic(masked):
            continue    # 全部被 #[cfg] 门控 → 本目标不需要改（如 serde_core）

        # 选择替身：能找到 crate 根源文件 → 注入 shim；找不到：
        #   * registry 依赖（manifest 改不动）→ 无法注入也无法补依赖，只能跳过（避免产出坏代码）
        #   * 仓库/git 依赖 → 回退用 portable-atomic 依赖
        has_root_file = bool(c0) and has_root(c0)
        if has_root_file:
            mode = 'shim'
        elif is_registry:
            skipped_noroot.append(str(f.relative_to(root)))
            continue
        else:
            mode = 'portable'

        ns = transform(s, masked)
        if mode == 'shim':
            ns = PORTABLE_PATH.sub('crate::%s::' % SHIM_MOD, ns)   # 也迁移历史 portable_atomic 引用
        if ns == s:
            continue
        f.write_text(ns, encoding='utf-8')
        changed_rs.append(str(f.relative_to(root)))
        if c0:
            (shim_crates if mode == 'shim' else port_crates).add(c0)

    for c in sorted(port_crates):
        if ensure_dep(c / 'Cargo.toml'):
            deps_added.append(str((c / 'Cargo.toml').relative_to(root)))

    shims = []
    for c in sorted(shim_crates):
        if ensure_shim(c):
            shims.append(str(c.relative_to(root)))

    if changed_rs:
        print(f'[patch-mips32] {root}: 改写 {len(changed_rs)} 个源文件')
        for p in changed_rs:
            print(f'    {p}')
    if deps_added:
        print(f'[patch-mips32] {root}: 添加 portable-atomic 依赖: {deps_added}')
    if shims:
        print(f'[patch-mips32] {root}: 注入 pd_atomic_shim 模块: {shims}')
    if skipped_noroot:
        print(f'[patch-mips32] {root}: 跳过（registry crate 无根源文件，无法注入 shim）: {skipped_noroot}')
    if not changed_rs and not deps_added and not shims and not skipped_noroot:
        print(f'[patch-mips32] {root}: 无需改动')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
