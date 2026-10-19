#!/bin/sh
# 纯脚本手打群晖 SPK（无需官方 toolkit / chroot 环境，做法同 SynoCommunity spksrc）。
# .spk = 顶层 tar（INFO + package.tgz + scripts/ + conf/ + 图标），仅在 Linux CI 上运行。
#
# 用法：build_spk.sh <version> <dsm6|dsm7> <x86_64|armv8> <bindir> <out_spk_path>
#   bindir: 含 fluxdown-agent 与 fluxdownd 两个二进制的目录
#   dsm7: os_min_ver=7.0，conf/privilege 以套件专属用户运行（DSM 7 禁止 root）
#   dsm6: os_min_ver=6.0 + os_max_ver=7.0 上界，root 运行（DSM 6 默认）
#   arch 为群晖架构家族值（官方 Appendix A）：x86_64 覆盖全部 Intel/AMD 机型
#   （二进制按基线 x86-64 编译，不要求 SSE4.2/POPCNT，老 Atom/AMD K10 机型可运行），
#   armv8 覆盖 rtd1296/rtd1619b/armada37xx 等 ARM64 机型。
#
# 前置：imagemagick（convert）用于从 assets/logo/fluxdown_logo.png 生成套件图标。
set -eu

[ $# -eq 5 ] || { echo "usage: $0 <version> <dsm6|dsm7> <x86_64|armv8> <bindir> <out_spk>" >&2; exit 2; }
VERSION=$1 DSM=$2 ARCH=$3 BINDIR=$4 OUT=$5

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/../.." && pwd)
LOGO="$REPO_ROOT/assets/logo/fluxdown_logo.png"

for b in fluxdown-agent fluxdownd; do
	[ -f "$BINDIR/$b" ] || { echo "binary not found: $BINDIR/$b" >&2; exit 1; }
done
command -v convert >/dev/null || { echo "imagemagick 'convert' not in PATH" >&2; exit 1; }

case "$DSM" in
	dsm6|dsm7) ;;
	*) echo "invalid dsm generation: $DSM (expect dsm6|dsm7)" >&2; exit 2 ;;
esac
case "$ARCH" in
	x86_64|armv8) ;;
	*) echo "invalid arch: $ARCH (expect x86_64|armv8)" >&2; exit 2 ;;
esac

# ── DSM 版本规范化：INFO 的 version 只允许数字/./-（含字母如 "0.2.5-rc.1" 会被
#    DSM 上传时直接判"套件文件格式不正确"）。映射为 base-build 形式并保序：
#    alpha.N→-10NN, beta.N→-20NN, rc.N→-30NN，正式版→-9000，
#    保证 rc < 正式版 < 下一版本，可原地升级。──
BASE=${VERSION%%-*}
PRE=${VERSION#"$BASE"}
case "$PRE" in
	"")        SPK_VERSION="$BASE-9000" ;;
	-alpha*) N=${PRE#-alpha}; N=${N#.}; SPK_VERSION="$BASE-10$(printf %02d "${N:-0}")" ;;
	-beta*)  N=${PRE#-beta};  N=${N#.}; SPK_VERSION="$BASE-20$(printf %02d "${N:-0}")" ;;
	-rc*)    N=${PRE#-rc};    N=${N#.}; SPK_VERSION="$BASE-30$(printf %02d "${N:-0}")" ;;
	*) echo "unsupported prerelease suffix in version: $VERSION" >&2; exit 2 ;;
esac

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
stage="$work/stage"
payload="$work/payload"

# ── package.tgz 载荷：bin/fluxdown-agent + bin/fluxdownd（同目录；Web UI 已编译期内嵌进 agent，无 web/ 目录）──
mkdir -p "$payload/bin"
cp "$BINDIR/fluxdown-agent" "$BINDIR/fluxdownd" "$payload/bin/"
chmod 755 "$payload/bin/fluxdown-agent" "$payload/bin/fluxdownd"

# ── ui/：DSM 桌面应用入口。官方要求该目录在 package.tgz 内（安装后位于
#    /var/packages/FluxDown/target/ui），DSM 依 INFO 的 dsmuidir 将其软链到
#    /usr/syno/synoman/webman/3rdparty/FluxDown，主菜单/导航窗格才会出图标。
#    .url 用 protocol+port+url 组合（DSM 以访问 NAS 的主机名自动拼 URL，spksrc 同款）；
#    图标 {0} 会被 DSM 按 16/24/32/48/64/72/256 逐尺寸请求，须全量生成。──
mkdir -p "$payload/ui/images"
for size in 16 24 32 48 64 72 256; do
	convert "$LOGO" -resize "${size}x${size}" "$payload/ui/images/icon_${size}.png"
done
cat > "$payload/ui/config" <<'UICONF'
{
  ".url": {
    "com.fluxdown.server": {
      "type": "url",
      "title": "FluxDown",
      "desc": "Blazing fast, multi-protocol download manager",
      "icon": "images/icon_{0}.png",
      "protocol": "http",
      "port": "17800",
      "url": "/",
      "allUsers": true,
      "grantPrivilege": "all",
      "advanceGrantPrivilege": true
    }
  }
}
UICONF

mkdir -p "$stage"
tar -czf "$stage/package.tgz" --owner=0 --group=0 --numeric-owner -C "$payload" bin ui

# ── INFO ──
EXTRACT_KB=$(du -sk "$payload" | cut -f1)
CHECKSUM=$(md5sum "$stage/package.tgz" | cut -d' ' -f1)
{
	echo 'package="FluxDown"'
	echo "version=\"$SPK_VERSION\""
	echo 'displayname="FluxDown Server"'
	echo 'description="Blazing fast, multi-protocol download manager. Rust engine with HTTP/HTTPS/FTP/BitTorrent/HLS support, intelligent segmentation, and a full Web UI on port 17800."'
	echo 'maintainer="zerx-lab"'
	echo 'maintainer_url="https://fluxdown.com"'
	echo 'support_url="https://github.com/zerx-lab/FluxDown/issues"'
	echo "arch=\"$ARCH\""
	echo 'thirdparty="yes"'
	echo 'startable="yes"'
	echo 'adminport="17800"'
	echo 'dsmuidir="ui"'
	echo 'dsmappname="com.fluxdown.server"'
	echo "extractsize=\"$EXTRACT_KB\""
	echo "checksum=\"$CHECKSUM\""
	if [ "$DSM" = "dsm7" ]; then
		echo 'os_min_ver="7.0-40000"'
	else
		echo 'os_min_ver="6.0-7321"'
		echo 'os_max_ver="7.0-40000"'
	fi
} > "$stage/INFO"

# ── conf/privilege：DSM 7 强制非 root，以套件专属用户运行；DSM 6 维持 root ──
#    DSM 7 显式命名用户 sc-fluxdown（与 SynoCommunity 同款前缀），共享文件夹
#    权限页「系统内部用户」里就是这个名字，文档 / Web 端提示都引用它。
mkdir -p "$stage/conf"
if [ "$DSM" = "dsm7" ]; then
	printf '{"defaults":{"run-as":"package"},"username":"sc-fluxdown","groupname":"sc-fluxdown"}\n' > "$stage/conf/privilege"
else
	printf '{"defaults":{"run-as":"root"}}\n' > "$stage/conf/privilege"
fi

# ── DSM 7：受限用户默认对任何共享文件夹都无权限，目录选择器里进 /volume1/<share>
#    只会得到 EACCES（表现为「看不到共享文件夹」）。用 DSM 的 data-share 资源
#    worker 在每次启动时创建/授权向导里选定的共享文件夹（rw → sc-fluxdown）；
#    conf/resource 支持 {{wizard_key}} 模板（官方 docker 示例同款）。
#    安装 / 升级向导都带同一个键：升级时 resource 会重新求值，缺键会变成空名。──
if [ "$DSM" = "dsm7" ]; then
	printf '{"data-share":{"shares":[{"name":"{{wizard_share_name}}","permission":{"rw":["sc-fluxdown"]}}]}}\n' > "$stage/conf/resource"
	mkdir -p "$stage/WIZARD_UIFILES"
	cp "$SCRIPT_DIR/wizard/install_uifile.sh" "$stage/WIZARD_UIFILES/install_uifile.sh"
	cp "$SCRIPT_DIR/wizard/install_uifile.sh" "$stage/WIZARD_UIFILES/upgrade_uifile.sh"
	chmod 755 "$stage/WIZARD_UIFILES/"*
fi

# ── scripts/（生命周期脚本）──
#    postinst / postupgrade 把向导选的共享文件夹名落到 var/share_name，
#    start-stop-status 据此解析路径传给 FLUXDOWN_SAVE_DIR（向导变量只在
#    安装脚本环境里可见，启动脚本拿不到）。其余为幂等空脚本。
mkdir -p "$stage/scripts"
cp "$SCRIPT_DIR/scripts/start-stop-status" "$stage/scripts/start-stop-status"
for s in preinst preuninst postuninst preupgrade; do
	printf '#!/bin/sh\nexit 0\n' > "$stage/scripts/$s"
done
for s in postinst postupgrade; do
	if [ "$DSM" = "dsm7" ]; then
		cp "$SCRIPT_DIR/scripts/save-share-name" "$stage/scripts/$s"
	else
		printf '#!/bin/sh\nexit 0\n' > "$stage/scripts/$s"
	fi
done
chmod 755 "$stage/scripts/"*

# ── 图标（DSM 7 要求 64x64，DSM 6 要求 72x72；256 两代通用） ──
if [ "$DSM" = "dsm7" ]; then
	convert "$LOGO" -resize 64x64 "$stage/PACKAGE_ICON.PNG"
else
	convert "$LOGO" -resize 72x72 "$stage/PACKAGE_ICON.PNG"
fi
convert "$LOGO" -resize 256x256 "$stage/PACKAGE_ICON_256.PNG"

# ── 顶层 tar 即 .spk（WIZARD_UIFILES 仅 DSM 7 存在）──
mkdir -p "$(dirname "$OUT")"
extra=""
[ -d "$stage/WIZARD_UIFILES" ] && extra="WIZARD_UIFILES"
# shellcheck disable=SC2086
tar -cf "$OUT" --owner=0 --group=0 --numeric-owner -C "$stage" \
	INFO PACKAGE_ICON.PNG PACKAGE_ICON_256.PNG package.tgz scripts conf $extra
echo "built: $OUT"
