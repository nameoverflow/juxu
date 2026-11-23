#!/bin/sh
# 句序 (Juxu) macOS 一键安装 / 升级脚本
# 用法:
#   curl -fsSL https://raw.githubusercontent.com/nameoverflow/juxu/main/Scripts/install-macos.sh | sh
set -eu

REPO="nameoverflow/juxu"
LSREGISTER="/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"

if [ "$(uname -s)" != "Darwin" ]; then
    echo "错误：此安装脚本仅支持 macOS。" >&2
    exit 1
fi

if [ "$(uname -m)" != "arm64" ]; then
    echo "错误：当前预编译包仅面向 Apple Silicon (arm64) Mac；Intel Mac 请克隆仓库运行 make install。" >&2
    exit 1
fi

TAG="${JUXU_VERSION:-}"
if [ -z "$TAG" ]; then
    # Resolve latest release tag via HTTP redirect first (zero GitHub API rate-limit consumption)
    FINAL_URL="$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/${REPO}/releases/latest" 2>/dev/null || true)"
    case "$FINAL_URL" in
        */releases/tag/*)
            TAG="${FINAL_URL##*/releases/tag/}"
            ;;
    esac
fi
if [ -z "$TAG" ]; then
    TAG="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" | sed -n 's/.*"tag_name":[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)"
fi
if [ -z "$TAG" ]; then
    echo "错误：无法获取最新版本号，可通过 JUXU_VERSION=v0.1.0 指定版本重试。" >&2
    exit 1
fi

case "$TAG" in
    v*) VERSION="${TAG#v}" ;;
    *)  VERSION="$TAG"; TAG="v${VERSION}" ;;
esac

ZIP_NAME="Juxu-${VERSION}-macOS-arm64.zip"
BASE_URL="https://github.com/${REPO}/releases/download/${TAG}"

TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/juxu-install.XXXXXX")"
trap 'rm -rf "$TMP_DIR"' EXIT

echo "==> 正在下载 句序 (Juxu) ${TAG} (${ZIP_NAME})..."
curl -fL --progress-bar "${BASE_URL}/${ZIP_NAME}" -o "$TMP_DIR/$ZIP_NAME"

if curl -fsSL "${BASE_URL}/SHA256SUMS.txt" -o "$TMP_DIR/SHA256SUMS.txt" 2>/dev/null; then
    EXPECTED_SHA="$(awk -v f="$ZIP_NAME" '$2 == f || $2 == "*"f { print $1; exit }' "$TMP_DIR/SHA256SUMS.txt")"
    if [ -n "$EXPECTED_SHA" ]; then
        ACTUAL_SHA="$(shasum -a 256 "$TMP_DIR/$ZIP_NAME" | awk '{print $1}')"
        if [ "$EXPECTED_SHA" != "$ACTUAL_SHA" ]; then
            echo "错误：SHA-256 校验不匹配（期望 ${EXPECTED_SHA}，实际 ${ACTUAL_SHA}）。" >&2
            exit 1
        fi
        echo "==> SHA-256 校验通过"
    fi
fi

ditto -x -k "$TMP_DIR/$ZIP_NAME" "$TMP_DIR/extracted"
SRC_APP="$TMP_DIR/extracted/Juxu.app"
if [ ! -d "$SRC_APP" ]; then
    echo "错误：压缩包内未找到 Juxu.app。" >&2
    exit 1
fi
xattr -cr "$SRC_APP" 2>/dev/null || true

SYS_APP="/Library/Input Methods/Juxu.app"
USER_DIR="$HOME/Library/Input Methods"
USER_APP="$USER_DIR/Juxu.app"

echo "==> 正在安装并注册句序输入法..."
pkill -TERM -x Juxu 2>/dev/null || true
pkill -TERM -x PrisWubi 2>/dev/null || true
sleep 0.3

if [ -d "$SYS_APP" ]; then
    if [ -w "$SYS_APP" ] && [ -w "/Library/Input Methods" ]; then
        rm -rf "$SYS_APP"
        cp -R "$SRC_APP" "$SYS_APP"
        DST_APP="$SYS_APP"
    else
        echo "==> 检测到旧的系统级副本 $SYS_APP，需要管理员权限更新..."
        sudo rm -rf "$SYS_APP"
        mkdir -p "$USER_DIR"
        rm -rf "$USER_APP"
        cp -R "$SRC_APP" "$USER_APP"
        DST_APP="$USER_APP"
    fi
else
    mkdir -p "$USER_DIR"
    rm -rf "$USER_APP"
    cp -R "$SRC_APP" "$USER_APP"
    DST_APP="$USER_APP"
fi

"$LSREGISTER" -f -R -trusted "$DST_APP" 2>/dev/null || true
"$DST_APP/Contents/MacOS/Juxu" --register-and-enable
killall imklaunchagent 2>/dev/null || true
sleep 0.3
open -g "$DST_APP"

echo ""
echo "✓ 句序 (Juxu) ${TAG} 已安装至 ${DST_APP} 并自动启用！"
echo "  无需注销或重启，按 Ctrl+Space（或地球键 🌐）即可直接切换到「句序」开始打字。"
