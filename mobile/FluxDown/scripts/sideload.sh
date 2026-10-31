#!/usr/bin/env bash
# 打包原生 iOS App（mobile/FluxDown）的侧载（full flavor）**未签名** IPA，供 AltStore / SideStore / Sideloadly 等用户自签安装。
# 与 testflight.sh 的区别：full flavor（本机引擎含 BitTorrent / eD2K，App 定义 FULL_FLAVOR）、不签名、不上传。
# 本机与 CI（.github/workflows/ios-sideload.yml，由 release.yml 的 ios 组件调用）共用这一份流程。
#
# 用法：mobile/FluxDown/scripts/sideload.sh --version X.Y.Z --build N [--out DIR] [--skip-core]
#   --version    CFBundleShortVersionString（不带 -rc 后缀）
#   --build      CFBundleVersion（1–3 段整数，如 42 或 20261009.123045）
#   --out        输出目录，缺省 mobile/FluxDown/build/sideload；产物 FluxDown-<version>-<build>-unsigned.ipa
#   --skip-core  跳过 build-core.sh --release --flavor full（核心须已是 full + release，且以同一 FLUXDOWN_APP_VERSION 构建）
# 可选环境变量：FLUXCLOUD_BASE_URL（编译期写入核心，见 build-core.sh）；FLUXDOWN_APP_VERSION 缺省取 --version。
# 前置：rustup target add aarch64-apple-ios aarch64-apple-ios-sim；Xcode 命令行工具。
set -euo pipefail

VERSION=""
BUILD=""
OUT_DIR=""
SKIP_CORE=0
while [ $# -gt 0 ]; do
  case "$1" in
    --version) VERSION="${2:?--version needs a value}"; shift ;;
    --build) BUILD="${2:?--build needs a value}"; shift ;;
    --out) OUT_DIR="${2:?--out needs a value}"; shift ;;
    --skip-core) SKIP_CORE=1 ;;
    *) echo "unknown argument: $1 (expected --version, --build, --out, --skip-core)" >&2; exit 64 ;;
  esac
  shift
done
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "--version must be X.Y.Z (got '$VERSION')" >&2; exit 64; }
[[ "$BUILD" =~ ^[0-9]+(\.[0-9]+){0,2}$ ]] || { echo "--build must be 1-3 dot-separated integers (got '$BUILD')" >&2; exit 64; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IOS_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
OUT="${OUT_DIR:-$IOS_ROOT/build/sideload}"
ARCHIVE="$OUT/FluxDown.xcarchive"
IPA="$OUT/FluxDown-$VERSION-$BUILD-unsigned.ipa"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

if [ "$SKIP_CORE" = 0 ]; then
  FLUXDOWN_APP_VERSION="${FLUXDOWN_APP_VERSION:-$VERSION}" "$SCRIPT_DIR/build-core.sh" --release --flavor full
fi
CORE_FLAVOR="$(cat "$IOS_ROOT/FluxKit/Artifacts/flavor" 2>/dev/null || true)"
[ "$CORE_FLAVOR" = "full" ] \
  || { echo "core xcframework is flavor '${CORE_FLAVOR:-unknown}', expected 'full'; rerun without --skip-core" >&2; exit 1; }

rm -rf "$ARCHIVE" "$IPA"
mkdir -p "$OUT"

# 不签名归档：描述文件 / 证书由侧载工具在安装时重签。FLUX_FLAVOR_CONDITIONS 显式给 FULL_FLAVOR，不依赖工程默认值。
echo "==> xcodebuild archive $VERSION ($BUILD) unsigned"
xcodebuild archive -quiet \
  -project "$IOS_ROOT/FluxDown.xcodeproj" -scheme FluxDown -configuration Release \
  -destination "generic/platform=iOS" -archivePath "$ARCHIVE" \
  MARKETING_VERSION="$VERSION" CURRENT_PROJECT_VERSION="$BUILD" \
  FLUX_FLAVOR_CONDITIONS=FULL_FLAVOR \
  CODE_SIGNING_ALLOWED=NO

APP="$ARCHIVE/Products/Applications/FluxDown.app"
[ -d "$APP" ] || { echo "archive has no $APP" >&2; exit 1; }
# full 版必须带 P2P 入口声明（预处理 Info.plist 按 FLUX_FLAVOR_CONDITIONS 保留）。
plutil -p "$APP/Info.plist" | grep -q 'org.bittorrent.torrent' \
  || { echo "full archive Info.plist lacks P2P declarations (FLUX_FLAVOR_CONDITIONS not applied?)" >&2; exit 1; }

# IPA = Payload/<App>.app 的 zip；ditto 保留符号链接与文件属性。
mkdir -p "$WORK/Payload"
cp -R "$APP" "$WORK/Payload/FluxDown.app"
(cd "$WORK" && ditto -c -k --keepParent Payload "$IPA")
[ -f "$IPA" ] || { echo "ipa packaging produced no $IPA" >&2; exit 1; }
echo "==> unsigned IPA: $IPA"
