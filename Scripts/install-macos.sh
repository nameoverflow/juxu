#!/bin/sh
# 句序 (Juxu) macOS 一键安装 / 升级脚本
# 用法:
#   1. 本地构建安装:   sh Scripts/install-macos.sh --local-app build/Juxu.app
#   2. DMG 双击安装:   自动检测脚本同级目录下的 Juxu.app
#   3. 在线一键安装:   curl -fsSL https://raw.githubusercontent.com/nameoverflow/juxu/main/Scripts/install-macos.sh | sh
set -eu
export COPYFILE_DISABLE=1

REPO="nameoverflow/juxu"
LSREGISTER="/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"
SYS_DIR="/Library/Input Methods"
SYS_APP="$SYS_DIR/Juxu.app"
USER_APP="$HOME/Library/Input Methods/Juxu.app"

if [ "$(uname -s)" != "Darwin" ]; then
    echo "错误：此安装脚本仅支持 macOS。" >&2
    exit 1
fi

LOCAL_APP=""
TAG="${JUXU_VERSION:-}"

while [ "$#" -gt 0 ]; do
    case "$1" in
        --local-app)
            if [ "$#" -lt 2 ]; then
                echo "错误：--local-app 需要提供 Juxu.app 路径。" >&2
                exit 1
            fi
            LOCAL_APP="$2"
            shift 2
            ;;
        --local-app=*)
            LOCAL_APP="${1#--local-app=}"
            shift
            ;;
        --version)
            if [ "$#" -lt 2 ]; then
                echo "错误：--version 需要提供版本号（如 v0.1.0）。" >&2
                exit 1
            fi
            TAG="$2"
            shift 2
            ;;
        --version=*)
            TAG="${1#--version=}"
            shift
            ;;
        -h|--help)
            echo "用法: $0 [--local-app <path/to/Juxu.app>] [--version <tag>]"
            exit 0
            ;;
        *)
            echo "错误：未知参数 $1" >&2
            exit 1
            ;;
    esac
done

# Auto-detect sibling Juxu.app when launched from a mounted DMG (e.g. 双击安装句序输入法.command)
if [ -z "$LOCAL_APP" ] && [ -f "$0" ]; then
    SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" 2>/dev/null && pwd || true)"
    if [ -n "$SCRIPT_DIR" ] \
        && [ "$SCRIPT_DIR" != "$SYS_DIR" ] \
        && [ "$SCRIPT_DIR" != "$HOME/Library/Input Methods" ] \
        && [ -d "$SCRIPT_DIR/Juxu.app/Contents/MacOS" ]; then
        LOCAL_APP="$SCRIPT_DIR/Juxu.app"
    fi
fi

TMP_DIR=""
DMG_MOUNT=""
cleanup() {
    if [ -n "$DMG_MOUNT" ] && [ -d "$DMG_MOUNT" ]; then
        hdiutil detach "$DMG_MOUNT" -quiet 2>/dev/null || hdiutil detach "$DMG_MOUNT" -force -quiet 2>/dev/null || true
    fi
    if [ -n "$TMP_DIR" ] && [ -d "$TMP_DIR" ]; then
        rm -rf "$TMP_DIR"
    fi
}
trap cleanup EXIT

verify_sha256_if_present() {
    sums_file="$1"
    asset_name="$2"
    asset_path="$3"
    if [ ! -f "$sums_file" ]; then
        return 0
    fi
    expected_sha="$(awk -v f="$asset_name" '$2 == f || $2 == "*"f { print $1; exit }' "$sums_file")"
    if [ -n "$expected_sha" ]; then
        actual_sha="$(shasum -a 256 "$asset_path" | awk '{print $1}')"
        if [ "$expected_sha" != "$actual_sha" ]; then
            echo "错误：${asset_name} SHA-256 校验不匹配（期望 ${expected_sha}，实际 ${actual_sha}）。" >&2
            exit 1
        fi
        echo "==> SHA-256 校验通过 (${asset_name})"
    fi
}

VERSION_LABEL=""
if [ -n "$LOCAL_APP" ]; then
    if [ ! -d "$LOCAL_APP/Contents/MacOS" ]; then
        echo "错误：未找到有效的本地应用包：$LOCAL_APP" >&2
        exit 1
    fi
    SRC_APP="$(CDPATH= cd -- "$LOCAL_APP" && pwd)"
    if [ -f "$SRC_APP/Contents/Info.plist" ]; then
        VER="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$SRC_APP/Contents/Info.plist" 2>/dev/null || true)"
        if [ -n "$VER" ]; then
            VERSION_LABEL="v${VER}"
        fi
    fi
else
    if [ "$(uname -m)" != "arm64" ]; then
        echo "错误：当前预编译包仅面向 Apple Silicon (arm64) Mac；Intel Mac 请克隆仓库运行 make install。" >&2
        exit 1
    fi

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
    VERSION_LABEL="$TAG"

    ZIP_NAME="Juxu-${VERSION}-macOS-arm64.zip"
    DMG_NAME="Juxu-${VERSION}-macOS-arm64.dmg"
    PKG_NAME="Juxu-${VERSION}-macOS-arm64.pkg"
    BASE_URL="https://github.com/${REPO}/releases/download/${TAG}"

    TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/juxu-install.XXXXXX")"
    curl -fsSL "${BASE_URL}/SHA256SUMS.txt" -o "$TMP_DIR/SHA256SUMS.txt" 2>/dev/null || true

    echo "==> 正在下载 句序 (Juxu) ${TAG}..."
    if curl -fL --progress-bar "${BASE_URL}/${ZIP_NAME}" -o "$TMP_DIR/$ZIP_NAME"; then
        verify_sha256_if_present "$TMP_DIR/SHA256SUMS.txt" "$ZIP_NAME" "$TMP_DIR/$ZIP_NAME"
        ditto -x -k "$TMP_DIR/$ZIP_NAME" "$TMP_DIR/extracted"
        SRC_APP="$TMP_DIR/extracted/Juxu.app"
    elif curl -fL --progress-bar "${BASE_URL}/${DMG_NAME}" -o "$TMP_DIR/$DMG_NAME"; then
        verify_sha256_if_present "$TMP_DIR/SHA256SUMS.txt" "$DMG_NAME" "$TMP_DIR/$DMG_NAME"
        DMG_MOUNT="$TMP_DIR/mnt"
        mkdir -p "$DMG_MOUNT"
        hdiutil attach "$TMP_DIR/$DMG_NAME" -nobrowse -readonly -mountpoint "$DMG_MOUNT" -quiet
        mkdir -p "$TMP_DIR/extracted"
        cp -R "$DMG_MOUNT/Juxu.app" "$TMP_DIR/extracted/Juxu.app"
        hdiutil detach "$DMG_MOUNT" -quiet 2>/dev/null || true
        DMG_MOUNT=""
        SRC_APP="$TMP_DIR/extracted/Juxu.app"
    elif curl -fL --progress-bar "${BASE_URL}/${PKG_NAME}" -o "$TMP_DIR/$PKG_NAME"; then
        verify_sha256_if_present "$TMP_DIR/SHA256SUMS.txt" "$PKG_NAME" "$TMP_DIR/$PKG_NAME"
        pkgutil --expand-full "$TMP_DIR/$PKG_NAME" "$TMP_DIR/pkg-expanded"
        SRC_APP="$(find "$TMP_DIR/pkg-expanded" -type d -name "Juxu.app" | head -n 1)"
    else
        echo "错误：无法从 ${BASE_URL} 下载安装包（.zip / .dmg / .pkg）。" >&2
        exit 1
    fi

    if [ -z "${SRC_APP:-}" ] || [ ! -d "$SRC_APP/Contents/MacOS" ]; then
        echo "错误：安装包内未找到有效的 Juxu.app。" >&2
        exit 1
    fi
    xattr -cr "$SRC_APP" 2>/dev/null || true
fi

# IMPORTANT (macOS 27+):
# 1. `KeyboardSettings.appex` runs sandboxed without access to `~/Library/Input Methods` and reads
#    `/System/Library/Caches/com.apple.IntlDataCache.le.kbdx`. Third-party input methods MUST live in
#    `/Library/Input Methods/Juxu.app` (`root:staff`, `0775` = `ug+rwX,o+rX`).
# 2. First-time install MUST invoke `Juxu --register-system-cache` as `root` (`geteuid() == 0`) so
#    `CarbonCore` rebuilds the global `IntlDataCache.le.kbdx` immediately without logout/reboot.
echo "==> 正在安装句序输入法至 ${SYS_APP}..."
pkill -TERM -x Juxu 2>/dev/null || true
pkill -TERM -x PrisWubi 2>/dev/null || true
sleep 0.3

if [ -d "$USER_APP" ]; then
    "$LSREGISTER" -u "$USER_APP" 2>/dev/null || true
    rm -rf "$USER_APP" 2>/dev/null || true
fi

NEEDS_ROOT_CACHE=0
if ! LC_ALL=C grep -q -a "im.juxu.inputmethod.Juxu" /System/Library/Caches/com.apple.IntlDataCache.le.kbdx 2>/dev/null; then
    NEEDS_ROOT_CACHE=1
fi

ROOT_INSTALL_CMD='rm -rf "$2" && mkdir -p "$(dirname "$2")" && cp -R "$1" "$2" && xattr -cr "$2" && chown -R root:staff "$2" && chmod -R ug+rwX,o+rX "$2" && "$3" -f -R -trusted "$2" && "$2/Contents/MacOS/Juxu" --register-system-cache'

if [ "$NEEDS_ROOT_CACHE" = "0" ] && [ -d "$SYS_APP" ] && [ -w "$SYS_APP" ]; then
    rm -rf "$SYS_APP/Contents"
    cp -R "$SRC_APP/Contents" "$SYS_APP/Contents"
    xattr -cr "$SYS_APP/Contents" 2>/dev/null || true
    chmod -R ug+rwX,o+rX "$SYS_APP/Contents"
    "$LSREGISTER" -f -R -trusted "$SYS_APP" 2>/dev/null || true
elif [ "$(id -u)" -eq 0 ]; then
    sh -c "$ROOT_INSTALL_CMD" _ "$SRC_APP" "$SYS_APP" "$LSREGISTER"
else
    echo "==> 首次安装正在写入 ${SYS_APP} 并刷新系统输入法全局缓存（需验证管理员权限，后续升级免密）..."
    if [ -t 0 ] && sudo -v 2>/dev/null; then
        sudo sh -c "$ROOT_INSTALL_CMD" _ "$SRC_APP" "$SYS_APP" "$LSREGISTER"
    elif [ -t 1 ] && [ -r /dev/tty ] && [ -w /dev/tty ] && sudo -v </dev/tty 2>/dev/null; then
        sudo sh -c "$ROOT_INSTALL_CMD" _ "$SRC_APP" "$SYS_APP" "$LSREGISTER" </dev/tty
    else
        osascript - "$SRC_APP" "$SYS_APP" "$LSREGISTER" <<'APPLESCRIPT'
on run argv
    set srcApp to item 1 of argv
    set dstApp to item 2 of argv
    set lsReg to item 3 of argv
    set dstDir to "/Library/Input Methods"
    set juxuBin to dstApp & "/Contents/MacOS/Juxu"
    do shell script "rm -rf " & quoted form of dstApp & " && mkdir -p " & quoted form of dstDir & " && cp -R " & quoted form of srcApp & " " & quoted form of dstApp & " && xattr -cr " & quoted form of dstApp & " && chown -R root:staff " & quoted form of dstApp & " && chmod -R ug+rwX,o+rX " & quoted form of dstApp & " && " & quoted form of lsReg & " -f -R -trusted " & quoted form of dstApp & " && " & quoted form of juxuBin & " --register-system-cache" with administrator privileges
end run
APPLESCRIPT
    fi
fi

USER_CACHE_DIR="$(getconf DARWIN_USER_CACHE_DIR 2>/dev/null || true)"
if [ -n "$USER_CACHE_DIR" ] && [ -d "$USER_CACHE_DIR" ]; then
    rm -f "${USER_CACHE_DIR}"com.apple.IntlDataCache* "${USER_CACHE_DIR}"*/com.apple.IntlDataCache* 2>/dev/null || true
fi
killall KeyboardSettings TextInputMenuAgent TextInputSwitcher imklaunchagent 2>/dev/null || true
sleep 0.3
"$SYS_APP/Contents/MacOS/Juxu" --register-and-enable
open -g "$SYS_APP"

echo ""
echo "✓ 句序 (Juxu)${VERSION_LABEL:+ ${VERSION_LABEL}} 已安装至 ${SYS_APP}！"
echo "  • 若系统弹出「系统设置 → 键盘 → 输入法」，点一次添加/启用「句序」即可（macOS 安全限制首次需用户确认一次，后续升级自动保持启用）。"
echo "  • 无需注销或重启，按 Ctrl+Space（或地球键 🌐）即可切换到「句序」开始打字。"
