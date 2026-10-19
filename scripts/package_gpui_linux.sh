#!/usr/bin/env bash
# GPUI 客户端 Linux 打包（x64）：tar.gz / AppImage / deb / Arch（.pkg.tar.zst）。
#
# 四个可执行文件必须同目录：fluxdown-desktop 按 current_exe 同级找 fluxdown-agent
# （crates/app/src/service_bootstrap.rs），agent 同理找 fluxdownd（native/agent/src/supervisor.rs）
# 与 fluxdown_nmh（native/agent/src/nmh.rs）。所有 UI 资源已编译进二进制，无运行期资源目录。
#
# 产物名与 Flutter 时代一致（官网 /api/release 与旧客户端自动更新按后缀匹配）：
#   FluxDown-<ver>-linux-x64.{tar.gz,AppImage,deb,pkg.tar.zst}
# deb / Arch 安装到 /opt/fluxdown，包名恒为 fluxdown：agent 的更新检查按此路径与包名识别官方包，
# 其它包名（AUR 等）只提示经包管理器更新（native/agent/src/update/install/detect.rs 的 OFFICIAL_PACKAGE）。
#
# 用法：VERSION=0.5.0 BIN_DIR=target/x86_64-unknown-linux-gnu/release OUT_DIR=build/installer \
#       scripts/package_gpui_linux.sh
# 依赖：dpkg-deb、zstd、tar；APPIMAGETOOL 指向 appimagetool（缺省在 PATH 中找 appimagetool）。
set -euo pipefail

REPO=$(cd "$(dirname "$0")/.." && pwd)
VERSION=${VERSION:?VERSION required}
BIN_DIR=${BIN_DIR:?BIN_DIR required}
OUT_DIR=${OUT_DIR:-"$REPO/build/installer"}
APPIMAGETOOL=${APPIMAGETOOL:-appimagetool}
BINS=(fluxdown-desktop fluxdown-agent fluxdownd fluxdown_nmh)
DESKTOP_FILE="$REPO/packaging/linux/com.fluxdown.app.desktop"
ICON="$REPO/assets/logo/fluxdown_logo.png"
BASE="FluxDown-${VERSION}-linux-x64"

mkdir -p "$OUT_DIR"
OUT_DIR=$(cd "$OUT_DIR" && pwd)
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

# 复制四个二进制到目标目录（保留 0755）
stage_bins() {
  mkdir -p "$1"
  for bin in "${BINS[@]}"; do
    install -m 0755 "$BIN_DIR/$bin" "$1/$bin"
  done
}

# ── tar.gz（解压得 FluxDown-<ver>-linux-x64/）──
# flux_down：Flutter 自动更新器覆盖安装后固定重启 <dir>/flux_down（native/fluxdown_updater），
# 由它转交 fluxdown-desktop，保证从 Flutter 便携版升级后能直接拉起 GPUI。
TAR_DIR="$WORK/$BASE"
stage_bins "$TAR_DIR"
cat >"$TAR_DIR/flux_down" <<'EOF'
#!/bin/sh
exec "$(dirname "$(readlink -f "$0")")/fluxdown-desktop" "$@"
EOF
chmod 0755 "$TAR_DIR/flux_down"
install -m 0644 "$DESKTOP_FILE" "$TAR_DIR/com.fluxdown.app.desktop"
install -m 0644 "$ICON" "$TAR_DIR/com.fluxdown.app.png"
tar -C "$WORK" --owner=0 --group=0 -czf "$OUT_DIR/$BASE.tar.gz" "$BASE"

# ── AppImage ──
# AppRun 要点：
# - `--autostart` 直接交给 agent：agent 在 AppImage 内把自启条目写成 "$APPIMAGE" --autostart
#   （native/agent/src/platform/mod.rs::agent_executable），登录时由这里转交。
# - UI 退出后常驻的 agent / fluxdownd 仍从本次挂载点运行；AppRun 等它们退出后再返回，
#   让 AppImage 运行时保持挂载，否则 daemon 重启、浏览器中继会找不到可执行文件。
APPDIR="$WORK/AppDir"
stage_bins "$APPDIR"
install -m 0644 "$DESKTOP_FILE" "$APPDIR/com.fluxdown.app.desktop"
install -m 0644 "$ICON" "$APPDIR/com.fluxdown.app.png"
cat >"$APPDIR/AppRun" <<'EOF'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
if [ "${1:-}" = "--autostart" ]; then
  exec "$HERE/fluxdown-agent" "$@"
fi
"$HERE/fluxdown-desktop" "$@"
status=$?
resident() {
  for exe in /proc/[0-9]*/exe; do
    case "$(readlink "$exe" 2>/dev/null)" in
      "$HERE/fluxdown-agent" | "$HERE/fluxdownd") return 0 ;;
    esac
  done
  return 1
}
while resident; do sleep 5; done
exit "$status"
EOF
chmod 0755 "$APPDIR/AppRun"
ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$APPIMAGETOOL" "$APPDIR" "$OUT_DIR/$BASE.AppImage"

# ── deb / Arch 共用的文件系统布局 ──
# /opt/fluxdown/<bins>、/usr/bin 符号链接（Linux current_exe 解析真实路径，兄弟查找不受影响）、
# XDG .desktop（agent 用 xdg-mime default com.fluxdown.app.desktop 注册关联）与图标。
stage_root() {
  local root=$1
  stage_bins "$root/opt/fluxdown"
  mkdir -p "$root/usr/bin" "$root/usr/share/applications" "$root/usr/share/icons/hicolor/256x256/apps"
  ln -s /opt/fluxdown/fluxdown-desktop "$root/usr/bin/fluxdown-desktop"
  ln -s /opt/fluxdown/fluxdown-agent "$root/usr/bin/fluxdown-agent"
  install -m 0644 "$DESKTOP_FILE" "$root/usr/share/applications/com.fluxdown.app.desktop"
  install -m 0644 "$ICON" "$root/usr/share/icons/hicolor/256x256/apps/com.fluxdown.app.png"
  # Flutter 版自启条目（~/.config/autostart/FluxDown.desktop）固定执行 /opt/fluxdown/flux_down --silentStart；
  # 包升级会删除旧文件，保留转发器让旧条目继续生效（--silentStart 等价于 agent 的 --autostart）。
  cat >"$root/opt/fluxdown/flux_down" <<'FWD'
#!/bin/sh
dir="$(dirname "$(readlink -f "$0")")"
if [ "$1" = "--silentStart" ]; then
  shift
  exec "$dir/fluxdown-agent" --autostart "$@"
fi
exec "$dir/fluxdown-desktop" "$@"
FWD
  chmod 0755 "$root/opt/fluxdown/flux_down"
}

# ── deb ──
# dpkg 以 '~' 表示预发布（0.5.0~rc.2 < 0.5.0）；'-' 会被当成 Debian revision 而排在正式版之后。
DEB_VERSION=${VERSION/-/'~'}
DEB="$WORK/deb"
stage_root "$DEB"
mkdir -p "$DEB/DEBIAN"
INSTALLED_KB=$(du -sk "$DEB/opt" "$DEB/usr" | awk '{s+=$1} END {print s}')
cat >"$DEB/DEBIAN/control" <<EOF
Package: fluxdown
Version: ${VERSION/-/\~}
Architecture: amd64
Maintainer: FluxDown Team <contact@fluxdown.app>
Homepage: https://fluxdown.com
Section: net
Priority: optional
Installed-Size: ${INSTALLED_KB}
Depends: libc6 (>= 2.35), libgtk-3-0 | libgtk-3-0t64, libayatana-appindicator3-1, libxkbcommon0, libxkbcommon-x11-0, libwayland-client0, libx11-xcb1, libvulkan1, libfontconfig1, libfreetype6, libasound2 | libasound2t64, libdbus-1-3, libxdo3
Recommends: xdg-utils, mesa-vulkan-drivers
Description: Free IDM-alternative download manager
 FluxDown is a Rust-powered, open-source download manager.
 Supports HTTP/HTTPS/FTP, BitTorrent magnetic links, and HLS/DASH streaming
 with multi-threaded acceleration and seamless browser integration.
EOF
cat >"$DEB/DEBIAN/postinst" <<'EOF'
#!/bin/sh
set -e
update-desktop-database -q /usr/share/applications/ 2>/dev/null || true
gtk-update-icon-cache -q /usr/share/icons/hicolor/ 2>/dev/null || true
EOF
chmod 0755 "$DEB/DEBIAN/postinst"
dpkg-deb --build --root-owner-group "$DEB" "$OUT_DIR/$BASE.deb"

# ── Arch（.PKGINFO 须列在首位）──
ARCHPKG="$WORK/arch"
stage_root "$ARCHPKG"
INSTALLED_BYTES=$(du -sb "$ARCHPKG/opt" "$ARCHPKG/usr" | awk '{s+=$1} END {print s}')
{
  echo "# Generated by FluxDown CI"
  echo "pkgname = fluxdown"
  echo "pkgver = ${VERSION//-/_}-1"
  echo "pkgdesc = Free IDM-alternative download manager"
  echo "url = https://fluxdown.com"
  echo "builddate = $(date +%s)"
  echo "packager = FluxDown CI <ci@fluxdown.app>"
  echo "size = ${INSTALLED_BYTES}"
  echo "arch = x86_64"
  echo "license = custom"
  for dep in gtk3 libayatana-appindicator libxkbcommon libxkbcommon-x11 wayland libx11 \
    vulkan-icd-loader fontconfig freetype2 alsa-lib dbus xdotool; do
    echo "depend = $dep"
  done
  echo "optdepend = xdg-utils: file and URL associations"
} >"$ARCHPKG/.PKGINFO"
(cd "$ARCHPKG" && tar --zstd --owner=0 --group=0 -cf "$OUT_DIR/$BASE.pkg.tar.zst" .PKGINFO opt usr)

ls -la "$OUT_DIR"
