#!/usr/bin/env bash
# 下载预编译 PDFium 动态库（决策 5）。
#
# 源：bblanchon/pdfium-binaries（BSD-3-Clause 的 PDFium 预编译包）
# 版本必须与 crates/ingest/Cargo.toml 中 pdfium-render 的 pdfium_7881 feature 严格一致，
# 否则 pdfium-render bind() 时 missing-symbol 崩溃。
#
# 用法：
#   ./fetch-pdfium.sh            # 下载当前平台
#   ./fetch-pdfium.sh all        # 下载全部平台（CI 用）
#
# 依赖 curl + tar。github 直连慢时可用镜像：GH_PROXY 环境变量（如 https://ghfast.top/）

set -euo pipefail

VERSION="chromium/7881"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# 项目根：脚本位于 <root>/scripts/，资源位于 <root>/src-tauri/resources/pdfium
RES_DIR="$(cd "$SCRIPT_DIR/.." && pwd)/src-tauri/resources/pdfium"

# 镜像前缀（可选）
PROXY="${GH_PROXY:-}"
if [ -n "$PROXY" ]; then
  BASE="${PROXY}/https://github.com/bblanchon/pdfium-binaries/releases/download"
else
  BASE="https://github.com/bblanchon/pdfium-binaries/releases/download"
fi

download() {
  local target="$1" # 如 win-x64
  local pkg="$2"    # 如 pdfium-win-x64
  local out="$RES_DIR/$target"
  local tmp="$(mktemp -d)"
  echo "→ 下载 $pkg (chromium/7881) → $out"
  curl -L --fail --retry 2 -o "$tmp/pkg.tgz" "$BASE/$VERSION/$pkg.tgz"
  mkdir -p "$out"
  # bblanchon 打包布局：win 用 bin/，mac/linux 用 lib/（chromium/7881 起已变更）
  local src dst
  case "$target" in
  win-*)
    src="bin/pdfium.dll"
    dst="pdfium.dll"
    ;;
  mac-*)
    src="lib/libpdfium.dylib"
    dst="libpdfium.dylib"
    ;;
  linux-*)
    src="lib/libpdfium.so"
    dst="libpdfium.so"
    ;;
  esac
  tar xzf "$tmp/pkg.tgz" -C "$tmp" "$src"
  mv "$tmp/$src" "$out/$dst"
  rm -rf "$tmp"
  echo "  ✓ $out/$dst 已就绪"
}

os="$(uname -s)"
arch="$(uname -m)"
case "$os-$arch" in
MINGW*-x86_64 | MSYS*-x86_64 | CYGWIN*-x86_64) current="win-x64" ;;
Darwin-arm64) current="mac-arm64" ;;
Darwin-x86_64) current="mac-x64" ;;
Linux-x86_64) current="linux-x64" ;;
*)
  echo "未识别的平台: $os-$arch"
  exit 1
  ;;
esac

if [ "${1:-}" = "all" ]; then
  download win-x64 pdfium-win-x64
  download mac-arm64 pdfium-mac-arm64
  download mac-x64 pdfium-mac-x64
  download linux-x64 pdfium-linux-x64
else
  case "$current" in
  win-x64) download win-x64 pdfium-win-x64 ;;
  mac-arm64) download mac-arm64 pdfium-mac-arm64 ;;
  mac-x64) download mac-x64 pdfium-mac-x64 ;;
  linux-x64) download linux-x64 pdfium-linux-x64 ;;
  esac
fi

echo ""
echo "完成。资源位于 src-tauri/resources/pdfium/<platform>/"
echo "注意：tauri.conf.json 的 bundle.resources 已配置，构建时会打包进安装包。"
