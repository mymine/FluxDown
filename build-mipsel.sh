#!/usr/bin/env bash
# =============================================================================
# FluxDown  →  OpenWrt mipsel_24kc (MIPS32r2, 软浮点) 交叉编译脚本
# -----------------------------------------------------------------------------
# 产出：默认只编译 fluxdown-agent（含内嵌 Web UI）；需要 daemon 时用 BINS=agent,daemon
#
# 关键点（Tier-3 目标 mipsel-unknown-linux-musl 的固定"补洞"）：
#   * nightly + rust-src，用 -Z build-std 现场编译 std（官方不提供该目标的 std）
#   * 关闭 self-contained：crt/libc 由 OpenWrt 工具链提供
#   * libunwind：Tier-3 无自带 → 用工具链 libgcc/libgcc_eh 合成 libunwind.a
#   * libatomic：mips32 的 64 位原子（mimalloc C 代码）→ -l static=atomic
#   * 32 位原子：源码/依赖里无条件使用的 AtomicU64/I64 → 注入 crate 内 shim（见 patch-mips32.py）
#
# 用法：
#   ./build-mipsel.sh                       # 默认只编译 fluxdown-agent（更快）
#   BINS=agent,daemon ./build-mipsel.sh     # 同时编译 fluxdownd 并打 ipk
#   REPO_DIR=/root/FluxDown ./build-mipsel.sh
#   SKIP_WEB=1 ./build-mipsel.sh            # 跳过前端构建
#   UPX=0 ./build-mipsel.sh                 # 跳过 UPX 体积压缩（默认开启，mipsel-linux/musl 静态二进制）
#   STRIP=0 ./build-mipsel.sh               # 跳过压缩前的 strip 兜底（仓库 profile 已 strip=true）
# =============================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

# ------------------------- 自动定位仓库根 -----------------------------------
_is_repo() { [ -f "$1/Cargo.toml" ] && [ -d "$1/native" ] && [ -d "$1/packaging/openwrt" ]; }
if [ -n "${REPO_DIR:-}" ] && ! _is_repo "$REPO_DIR"; then
  echo "REPO_DIR=$REPO_DIR 看起来不是 FluxDown 仓库（缺 Cargo.toml/native/packaging/openwrt）" >&2
  exit 1
fi
if [ -z "${REPO_DIR:-}" ]; then
  for cand in "$PWD" "$SCRIPT_DIR" "$SCRIPT_DIR/.." "$SCRIPT_DIR/../.."; do
    if _is_repo "$cand"; then REPO_DIR="$(cd "$cand" && pwd)"; break; fi
  done
fi
if [ -z "${REPO_DIR:-}" ]; then
  echo "无法自动定位 FluxDown 仓库根。请显式指定，例如：REPO_DIR=/root/FluxDown $0" >&2
  exit 1
fi

# ------------------------------- 可调参数 -----------------------------------
WORK="${WORK:-$REPO_DIR/.mipsel-build}"
OUT_DIR="${OUT_DIR:-$REPO_DIR/dist-mipsel}"
TARGET="mipsel-unknown-linux-musl"
CC_TRIPLE="mipsel_unknown_linux_musl"   # cc-rs 环境变量用的下划线写法

OPENWRT_VER="${OPENWRT_VER:-23.05.5}"
SDK_BASE="openwrt-sdk-${OPENWRT_VER}-ramips-mt7621_gcc-12.3.0_musl.Linux-x86_64"
SDK_TGZ="${SDK_BASE}.tar.xz"
SDK_PATH="releases/${OPENWRT_VER}/targets/ramips/mt7621/${SDK_TGZ}"
SDK_MIRRORS=(
  "https://mirrors.aliyun.com/openwrt"
  "https://mirrors.tuna.tsinghua.edu.cn/openwrt"
  "https://downloads.openwrt.org"
)

# 若已有 mipsel-musl 交叉工具链，可用 CROSS_PREFIX 覆盖（例如 musl.cc 的 mipsel-linux-musl-）
CROSS_PREFIX="${CROSS_PREFIX:-}"
IPK_ARCH="${IPK_ARCH:-mipsel_24kc}"
JOBS="${JOBS:-$(nproc 2>/dev/null || echo 4)}"
# 要编译的二进制；默认只编译 agent（更快）。可设 BINS=agent,daemon 以同时编译 daemon 并打 ipk。
BINS="${BINS:-agent}"
# 体积压缩：默认 UPX=1 启用（mipsel 静态二进制可压到 30~40% 体积）。UPX_BIN 可指主机上已有的 upx。
UPX="${UPX:-1}"
UPX_LEVEL="${UPX_LEVEL:---best}"
UPX_BIN="${UPX_BIN:-}"
# 压缩前是否 strip 一次：默认 1（幂等；仓库 [profile.release] 已 strip=true，这里仅兜底）。STRIP=0 跳过。
STRIP="${STRIP:-1}"
STRIP_BIN=""

VERSION="${VERSION:-$(git -C "$REPO_DIR" describe --tags --always 2>/dev/null | sed 's/^v//' || true)}"
VERSION="${VERSION:-0.0.0}"

say() { printf '\033[1;32m[%s]\033[0m %s\n' "$(date +%T)" "$*"; }
_has_bin() { case ",$BINS," in *",$1,"*) return 0 ;; *) return 1 ;; esac; }

# ------------------------- 1) Rust nightly + rust-src -----------------------
ensure_rust() {
  export PATH="$HOME/.cargo/bin:$PATH"
  if ! command -v rustup >/dev/null 2>&1; then
    say "安装 rustup（nightly）"
    curl -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path --profile minimal --default-toolchain nightly
  fi
  if ! rustup run nightly rustc --version >/dev/null 2>&1 \
     || ! rustup component list --toolchain nightly 2>/dev/null | grep -q 'rust-src (installed)'; then
    say "安装 nightly + rust-src"
    rustup toolchain install nightly --profile minimal --component rust-src
  fi
  say "rustc: $(rustup run nightly rustc --version)"
}

# --------------------------- 2) 交叉工具链 ---------------------------------
ensure_toolchain() {
  if [ -n "$CROSS_PREFIX" ]; then
    say "使用外部 musl 工具链: ${CROSS_PREFIX}"
    GCC="${CROSS_PREFIX}gcc"; GXX="${CROSS_PREFIX}g++"; AR="${CROSS_PREFIX}ar"; RANLIB="${CROSS_PREFIX}ranlib"
    STRIP_BIN="${CROSS_PREFIX}strip"
    return
  fi
  local sdk_dir="$WORK/$SDK_BASE"
  local tc="$sdk_dir/staging_dir/toolchain-mipsel_24kc_gcc-12.3.0_musl"
  if [ ! -x "$tc/bin/mipsel-openwrt-linux-musl-gcc" ]; then
    mkdir -p "$WORK"
    if [ ! -f "$WORK/$SDK_TGZ" ]; then
      say "下载 OpenWrt SDK: $SDK_TGZ"
      local ok=0
      for m in "${SDK_MIRRORS[@]}"; do
        echo "  try $m"
        if curl -fL --retry 5 --connect-timeout 20 --max-time 1800 -o "$WORK/$SDK_TGZ" "$m/$SDK_PATH"; then ok=1; break; fi
      done
      [ "$ok" = 1 ] || { echo "SDK 下载失败" >&2; exit 1; }
    fi
    say "解压 SDK（含 host 工具）"
    tar -xJf "$WORK/$SDK_TGZ" -C "$WORK"
  fi
  export STAGING_DIR="$sdk_dir/staging_dir"
  export PATH="$tc/bin:$PATH"
  GCC="$tc/bin/mipsel-openwrt-linux-musl-gcc"
  GXX="$tc/bin/mipsel-openwrt-linux-musl-g++"
  AR="$tc/bin/mipsel-openwrt-linux-musl-ar"
  RANLIB="$tc/bin/mipsel-openwrt-linux-musl-ranlib"
  STRIP_BIN="$tc/bin/mipsel-openwrt-linux-musl-strip"
  say "工具链: $("$GCC" --version | head -1)"
}

# --------------------------- 2.5) UPX（体积压缩，host 端工具）--------------------
ensure_upx() {
  if [ "$UPX" != "1" ]; then say "UPX=0，跳过体积压缩"; UPX_BIN=""; return; fi
  if [ -n "$UPX_BIN" ] && [ -x "$UPX_BIN" ]; then say "使用已指定 UPX: $UPX_BIN"; return; fi
  if command -v upx >/dev/null 2>&1; then UPX_BIN="$(command -v upx)"; say "UPX: $( "$UPX_BIN" --version | head -1 )"; return; fi
  local ver="4.2.4" tarball="upx-${ver}-amd64_linux.tar.xz" dir="$WORK/upx-${ver}"
  mkdir -p "$WORK"
  if [ ! -f "$dir/upx" ]; then
    say "下载 UPX $ver（host x86_64 工具，用于压缩 mipsel 产物）"
    local tgz="$WORK/$tarball" ok=0
    for m in \
      "https://gh-proxy.com/https://github.com/upx/upx/releases/download/v${ver}/$tarball" \
      "https://ghfast.top/https://github.com/upx/upx/releases/download/v${ver}/$tarball" \
      "https://github.com/upx/upx/releases/download/v${ver}/$tarball"; do
      if curl -fsSL --retry 3 --connect-timeout 15 --max-time 600 -o "$tgz" "$m"; then ok=1; break; fi
    done
    if [ "$ok" = 1 ] && [ -s "$tgz" ]; then
      mkdir -p "$dir" && tar xJf "$tgz" -C "$dir" --strip-components=1
    else
      say "UPX 下载失败，跳过压缩（设 UPX=1 + UPX_BIN=<已安装路径>，或保持 UPX=0 不压）"
      UPX_BIN=""; return
    fi
  fi
  UPX_BIN="$dir/upx"
  chmod +x "$UPX_BIN" || true
  say "UPX: $( "$UPX_BIN" --version 2>/dev/null | head -1 )"
}

# --------------------------- 4.5) 压缩 agent/daemon ----------------------------
compress_bins() {
  local rel="$REPO_DIR/target/$TARGET/release" bin before after
  local do_upx=0
  [ "$UPX" = "1" ] && [ -n "$UPX_BIN" ] && [ -x "$UPX_BIN" ] && do_upx=1
  if [ "$STRIP" != "1" ] && [ "$do_upx" != "1" ]; then
    say "STRIP=0 且无 UPX，跳过体积处理"; return
  fi
  if [ "$UPX" = "1" ] && [ "$do_upx" != "1" ]; then say "未取得 UPX，仅做 strip 兜底"; fi
  for bin in fluxdown-agent fluxdownd; do
    [ -f "$rel/$bin" ] || continue
    # 兜底 strip：仓库 [profile.release] 已 strip=true，通常已经是 stripped，这里只在需要时执行
    if [ "$STRIP" = "1" ] && [ -n "$STRIP_BIN" ] && [ -x "$STRIP_BIN" ] \
       && file "$rel/$bin" 2>/dev/null | grep -q 'not stripped'; then
      say "strip --strip-all $rel/$bin"
      "$STRIP_BIN" --strip-all "$rel/$bin" 2>&1 | sed 's/^/    /' || say "  strip 失败（继续）"
    fi
    before=$(stat -c%s "$rel/$bin" 2>/dev/null || echo 0)
    if [ "$do_upx" = "1" ]; then
      say "upx $UPX_LEVEL -- $rel/$bin"
      if "$UPX_BIN" $UPX_LEVEL --no-backup -- "$rel/$bin" 2>&1 | sed 's/^/    /'; then
        after=$(stat -c%s "$rel/$bin" 2>/dev/null || echo 0)
        say "  $bin: $before → $after 字节"
      else
        say "  $bin: UPX 失败（已保留原文件）"
      fi
    fi
  done
}


# ------------------------------ 3) Web SPA ---------------------------------
build_web() {
  local webroot="$REPO_DIR/web/dist"
  if [ "${SKIP_WEB:-0}" = "1" ]; then say "SKIP_WEB=1，跳过前端"; export FLUXDOWN_EMBED_WEBROOT="$webroot"; return; fi
  if [ ! -f "$webroot/index.html" ]; then
    say "构建 Web SPA（需要 bun）"
    if ! command -v bun >/dev/null 2>&1; then
      curl -fsSL https://bun.sh/install | bash
      export PATH="$HOME/.bun/bin:$PATH"
    fi
    ( cd "$REPO_DIR/web" && bun install --frozen-lockfile && bun run build )
  fi
  export FLUXDOWN_EMBED_WEBROOT="$webroot"
}

# --------------------- 4) 仓库源码 mips32 补丁 -----------------------------
apply_patches() {
  if [ -f "$SCRIPT_DIR/patch-mips32.py" ]; then
    say "应用 mips32 补丁（仓库源码）"
    python3 "$SCRIPT_DIR/patch-mips32.py" "$REPO_DIR"
  else
    say "未找到 patch-mips32.py，跳过仓库补丁"
  fi
}

# --------------------------- 5) 交叉编译二进制 -----------------------------
build_bins() {
  export CC_${CC_TRIPLE}="$GCC"
  export CXX_${CC_TRIPLE}="$GXX"
  export AR_${CC_TRIPLE}="$AR"
  export RANLIB_${CC_TRIPLE}="$RANLIB"
  export CARGO_TARGET_MIPSEL_UNKNOWN_LINUX_MUSL_LINKER="$GCC"
  export CFLAGS_${CC_TRIPLE}="-msoft-float"
  export CXXFLAGS_${CC_TRIPLE}="-msoft-float"

  # libunwind shim：Tier-3 musl 无自带 libunwind；用工具链 libgcc/libgcc_eh 合成
  local unwind_shim="$WORK/unwind-shim"
  mkdir -p "$unwind_shim"
  local libgcc_dir; libgcc_dir="$(dirname "$("$GCC" -print-libgcc-file-name)")"
  printf 'create %s/libunwind.a\naddlib %s/libgcc.a\naddlib %s/libgcc_eh.a\nsave\nend\n' \
        "$unwind_shim" "$libgcc_dir" "$libgcc_dir" | "$AR" -M

  # libatomic.a 也放进 -L 目录：`-l static=atomic` 会在编译 std/core 阶段触发 rustc 本地库查找
  local libatomic
  libatomic="$("$GCC" -print-file-name=libatomic.a 2>/dev/null || true)"
  if [ ! -f "$libatomic" ]; then
    libatomic="$(cd "$(dirname "$GCC")/../mipsel-openwrt-linux-musl/lib" && pwd)/libatomic.a"
  fi
  [ -f "$libatomic" ] && cp -f "$libatomic" "$unwind_shim/"

  export RUSTFLAGS="${RUSTFLAGS:-} -C link-self-contained=no -C target-feature=+crt-static -L native=$unwind_shim -l static=atomic -C link-arg=-l:libatomic.a"

  cd "$REPO_DIR"

  # 清理依赖源码（确保补丁从干净源码开始）；cargo 会用本地缓存重新解压/检出
  local cargohome="${CARGO_HOME:-$HOME/.cargo}"
  rm -rf "$cargohome/registry/src" "$cargohome/git/checkouts"

  say "cargo fetch（预取依赖，便于给第三方源码打补丁）"
  cargo +nightly fetch --target "$TARGET" || true
  say "给 cargo 依赖源码打 mips32 补丁"
  python3 "$SCRIPT_DIR/patch-mips32.py" "$cargohome" || true

  # rquickjs-sys（JS 插件引擎）没有 mipsel 的预生成 FFI bindings：借用同为 32 位小端的
  # armv7（64 位类型 8 字节对齐，与 MIPS O32 一致）/ i686 bindings 充当之。
  for d in "$cargohome"/registry/src/*/rquickjs-sys-*; do
    [ -d "$d/src/bindings" ] || continue
    [ -f "$d/src/bindings/mipsel-unknown-linux-musl.rs" ] && continue
    for cand in armv7-unknown-linux-gnueabihf.rs i686-unknown-linux-gnu.rs; do
      if [ -f "$d/src/bindings/$cand" ]; then
        cp "$d/src/bindings/$cand" "$d/src/bindings/mipsel-unknown-linux-musl.rs"
        say "rquickjs-sys bindings: 复制 $cand → mipsel-unknown-linux-musl"
        break
      fi
    done
  done

  if _has_bin agent; then
    say "cargo build: fluxdown_agent (web-ui)"
    cargo +nightly build --release -p fluxdown_agent --features web-ui \
        --target "$TARGET" -Z build-std=std,panic_unwind -j "$JOBS"
  fi
  if _has_bin daemon; then
    say "cargo build: fluxdown_daemon"
    cargo +nightly build --release -p fluxdown_daemon \
        --target "$TARGET" -Z build-std=std,panic_unwind -j "$JOBS"
  fi
}

# ------------------------------ 6) 打包 ------------------------------------
package() {
  local rel="$REPO_DIR/target/$TARGET/release"
  mkdir -p "$OUT_DIR/bin"
  local built=()
  if _has_bin agent && [ -f "$rel/fluxdown-agent" ]; then
    cp -f "$rel/fluxdown-agent" "$OUT_DIR/bin/"; built+=(fluxdown-agent)
  fi
  if _has_bin daemon && [ -f "$rel/fluxdownd" ]; then
    cp -f "$rel/fluxdownd" "$OUT_DIR/bin/"; built+=(fluxdownd)
  fi
  [ -f "$OUT_DIR/bin/fluxdown-agent" ] && { file "$OUT_DIR/bin/fluxdown-agent" || true; }
  say "已产出二进制: ${built[*]:-（无）}"

  if _has_bin daemon; then
    say "打包 ipk（build_ipk.sh）"
    ( cd "$REPO_DIR" && ./packaging/openwrt/build_ipk.sh server "$VERSION" "$OUT_DIR/bin" "$IPK_ARCH" "$OUT_DIR" ) || \
      say "ipk 打包失败（可手动执行 packaging/openwrt/build_ipk.sh）"
  else
    say "未选择 daemon，跳过 ipk 打包（如需 ipk：BINS=agent,daemon）"
    say "注意：fluxdown-agent 运行时会拉起「同目录」的 fluxdownd；若只要 agent 单跑不了，请用 BINS=agent,daemon 编译（或设 FLUXDOWN_DAEMON_BIN 指向 fluxdownd）"
  fi

  if [ "${#built[@]}" -gt 0 ]; then
    say "打包 tar.gz"
    ( cd "$OUT_DIR" && tar czf "fluxdown-${VERSION}-mipsel_24kc.tar.gz" -C bin "${built[@]}" ) || true
  fi

  say "完成。产物目录: $OUT_DIR"
  ls -la "$OUT_DIR/bin" || true
}

say "FluxDown mipsel_24kc 交叉编译 | 版本=$VERSION 仓库=$REPO_DIR"
ensure_rust
ensure_toolchain
ensure_upx
build_web
apply_patches
build_bins
compress_bins
package
