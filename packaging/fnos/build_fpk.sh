#!/bin/sh
# 飞牛 fnOS 应用包（.fpk）：拼装应用目录后交给官方 fnpack 打包与校验。
# 规范：https://developer.fnnas.com/docs/core-concepts/framework/
#
# 用法：build_fpk.sh <version> <x86|arm> <bindir> <out_fpk_path>
#   bindir: 含 fluxdown-agent 与 fluxdownd 两个二进制的目录（linux musl 静态产物）
#   x86 / arm 为 manifest 的 platform 取值；二进制含架构，不能用 all。
#
# 前置：fnpack（PATH 或 FNPACK 环境变量）、imagemagick（convert，生成图标）。
set -eu

[ $# -eq 4 ] || { echo "usage: $0 <version> <x86|arm> <bindir> <out_fpk>" >&2; exit 2; }
VERSION=$1 PLATFORM=$2 BINDIR=$3 OUT=$4
FNPACK=${FNPACK:-fnpack}

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/../.." && pwd)
LOGO="$REPO_ROOT/assets/logo/fluxdown_logo.png"
APPNAME=fluxdown

case "$PLATFORM" in
	x86|arm) ;;
	*) echo "invalid platform: $PLATFORM (expect x86|arm)" >&2; exit 2 ;;
esac
for b in fluxdown-agent fluxdownd; do
	[ -f "$BINDIR/$b" ] || { echo "binary not found: $BINDIR/$b" >&2; exit 1; }
done
command -v convert >/dev/null || { echo "imagemagick 'convert' not in PATH" >&2; exit 1; }
command -v "$FNPACK" >/dev/null || { echo "fnpack not found (set FNPACK or add to PATH)" >&2; exit 1; }

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
pkg="$work/$APPNAME"

# ── app/：安装后即 TRIM_APPDEST（/var/apps/fluxdown/target）。fluxdownd 必须与 agent
#    同目录（agent 按自身路径拉起它）；Web UI 已编译期内嵌进 agent，无静态资源目录。──
mkdir -p "$pkg/app/bin" "$pkg/app/ui/images" "$pkg/cmd" "$pkg/config" "$pkg/wizard"
cp "$BINDIR/fluxdown-agent" "$BINDIR/fluxdownd" "$pkg/app/bin/"
chmod 755 "$pkg/app/bin/fluxdown-agent" "$pkg/app/bin/fluxdownd"

# ── 图标：包图标 ICON.PNG 64px / ICON_256.PNG 256px，入口图标 icon_{64,256}.png。
#    官方要求主体不贴边，按 7/8 缩放后居中留透明边。──
icon() {
	inner=$(($1 * 7 / 8))
	convert "$LOGO" -resize "${inner}x${inner}" -background none -gravity center \
		-extent "${1}x${1}" "PNG32:$2"
}
icon 64 "$pkg/ICON.PNG"
icon 256 "$pkg/ICON_256.PNG"
icon 64 "$pkg/app/ui/images/icon_64.png"
icon 256 "$pkg/app/ui/images/icon_256.png"

# ── 桌面入口：端口服务（agent 自带访问密钥鉴权与 WebSocket /rpc，不走 index.cgi /
#    统一网关）。type=url 在新标签页打开 http://<NAS>:17800/。──
cat > "$pkg/app/ui/config" <<'UICONF'
{
  ".url": {
    "fluxdown.main": {
      "title": "FluxDown",
      "icon": "images/icon_{0}.png",
      "type": "url",
      "protocol": "http",
      "port": "17800",
      "url": "/",
      "allUsers": true
    }
  }
}
UICONF

# ── manifest：version 原样沿用 tag（文档允许 2.1.3-beta 这类后缀）。──
cat > "$pkg/manifest" <<EOF
appname=$APPNAME
version=$VERSION
display_name=FluxDown Server
desc=Blazing fast, multi-protocol download manager. Rust engine with HTTP/HTTPS/FTP/BitTorrent/HLS support, intelligent segmentation, and a full Web UI on port 17800.
source=thirdparty
platform=$PLATFORM
maintainer=zerx-lab
maintainer_url=https://fluxdown.com
distributor=zerx-lab
distributor_url=https://github.com/zerx-lab/FluxDown
desktop_uidir=ui
desktop_applaunchname=fluxdown.main
service_port=17800
checkport=true
ctl_stop=true
EOF

# ── 权限：专用应用用户 fluxdown 运行（非 root）。──
printf '{"defaults":{"run-as":"package"},"username":"fluxdown","groupname":"fluxdown"}\n' \
	> "$pkg/config/privilege"
# ── 资源：安装时系统创建共享目录 fluxdown 并为应用用户授予 ACL，作为默认保存目录；
#    其他共享文件夹由用户在应用设置「授权目录」里追加。──
printf '{"data-share":{"shares":[{"name":"fluxdown"}]}}\n' > "$pkg/config/resource"

# ── 生命周期脚本：main 负责启停与状态，其余为幂等空脚本（fnpack 要求 cmd/ 存在）。──
cp "$SCRIPT_DIR/cmd/main" "$pkg/cmd/main"
for s in install_init install_callback upgrade_init upgrade_callback \
	uninstall_init uninstall_callback config_init config_callback; do
	printf '#!/bin/bash\nexit 0\n' > "$pkg/cmd/$s"
done
chmod 755 "$pkg/cmd/"*

# ── fnpack 在当前目录产出 <appname>.fpk（打包前校验必需文件与 JSON 格式）。──
(cd "$work" && "$FNPACK" build --directory "$pkg")
mkdir -p "$(dirname "$OUT")"
mv "$work/$APPNAME.fpk" "$OUT"
echo "built: $OUT"
