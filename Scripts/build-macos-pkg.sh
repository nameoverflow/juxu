#!/bin/sh
set -eu

APP_PATH="${1:?usage: build-macos-pkg.sh <Juxu.app> <version> <output.pkg> [installer_sign_identity]}"
VERSION="${2:?missing version}"
OUT_PKG="${3:?missing output pkg path}"
INSTALLER_IDENTITY="${4:-}"

if [ ! -d "$APP_PATH" ]; then
    echo "error: app bundle not found at $APP_PATH" >&2
    exit 1
fi

STAGE_DIR="$(mktemp -d "${TMPDIR:-/tmp}/juxu-pkg.XXXXXX")"
trap 'rm -rf "$STAGE_DIR"' EXIT

PAYLOAD_DIR="$STAGE_DIR/payload"
SCRIPTS_DIR="$STAGE_DIR/scripts"
RES_DIR="$STAGE_DIR/resources/zh-Hans.lproj"
mkdir -p "$PAYLOAD_DIR/Library/Input Methods" "$SCRIPTS_DIR" "$RES_DIR" "$(dirname "$OUT_PKG")"

cp -R "$APP_PATH" "$PAYLOAD_DIR/Library/Input Methods/Juxu.app"
xattr -cr "$PAYLOAD_DIR/Library/Input Methods/Juxu.app" 2>/dev/null || true
chmod -R go+rX "$PAYLOAD_DIR/Library/Input Methods/Juxu.app"

cat <<'EOF' > "$SCRIPTS_DIR/preinstall"
#!/bin/sh
pkill -TERM -x Juxu 2>/dev/null || true
pkill -TERM -x PrisWubi 2>/dev/null || true
exit 0
EOF

cat <<'EOF' > "$SCRIPTS_DIR/postinstall"
#!/bin/sh
set -u

APP_DST="/Library/Input Methods/Juxu.app"
LSREGISTER="/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"

xattr -cr "$APP_DST" 2>/dev/null || true
chmod -R go+rX "$APP_DST" 2>/dev/null || true
"$LSREGISTER" -f -R -trusted "$APP_DST" 2>/dev/null || true

CONSOLE_USER="$(stat -f '%Su' /dev/console 2>/dev/null || true)"
if [ -n "$CONSOLE_USER" ] && [ "$CONSOLE_USER" != "root" ] && [ "$CONSOLE_USER" != "loginwindow" ]; then
    CONSOLE_UID="$(id -u "$CONSOLE_USER" 2>/dev/null || true)"
    USER_HOME="$(dscl . -read "/Users/$CONSOLE_USER" NFSHomeDirectory 2>/dev/null | awk '{print $2}' || true)"

    # Remove any older per-user bundle so macOS InputMethodKit sees a single canonical copy.
    # Personal lexicon & preferences live in ~/Library/Application Support/Juxu and ~/.juxu.toml and are preserved.
    if [ -n "$USER_HOME" ] && [ -d "$USER_HOME/Library/Input Methods/Juxu.app" ]; then
        "$LSREGISTER" -u "$USER_HOME/Library/Input Methods/Juxu.app" 2>/dev/null || true
        rm -rf "$USER_HOME/Library/Input Methods/Juxu.app" 2>/dev/null || true
    fi

    if [ -n "$CONSOLE_UID" ]; then
        /bin/launchctl asuser "$CONSOLE_UID" sudo -u "$CONSOLE_USER" \
            "$APP_DST/Contents/MacOS/Juxu" --register-and-enable 2>/dev/null || true
        killall -u "$CONSOLE_USER" imklaunchagent 2>/dev/null || true
        sleep 0.3
        /bin/launchctl asuser "$CONSOLE_UID" sudo -u "$CONSOLE_USER" \
            open -g "$APP_DST" 2>/dev/null || true
    fi
fi

exit 0
EOF

chmod 0755 "$SCRIPTS_DIR/preinstall" "$SCRIPTS_DIR/postinstall"

cat <<EOF > "$RES_DIR/welcome.html"
<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<style>
  :root { color-scheme: light dark; }
  body { font: -apple-system-body, "PingFang SC", sans-serif; line-height: 1.65; margin: 16px 20px; }
  h2 { font-size: 18px; margin: 0 0 10px; }
  ul { padding-left: 20px; margin: 10px 0; }
  li { margin: 5px 0; }
  code { font-family: ui-monospace, Menlo, monospace; font-size: 12px; }
</style>
</head>
<body>
  <h2>欢迎安装 句序 (Juxu) v${VERSION}</h2>
  <p>本安装向导将自动完成以下配置：</p>
  <ul>
    <li>安装应用至系统输入法目录 <code>/Library/Input Methods/Juxu.app</code></li>
    <li>自动向系统注册并启用「句序」输入法（支持全拼、小鹤双拼/音形、五笔 86/98/06、仓颉三/五/六代）</li>
    <li>刷新输入法服务，<strong>无需注销或重启系统</strong>，装完即可直接切换使用</li>
  </ul>
</body>
</html>
EOF

cat <<EOF > "$RES_DIR/conclusion.html"
<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<style>
  :root { color-scheme: light dark; }
  body { font: -apple-system-body, "PingFang SC", sans-serif; line-height: 1.65; margin: 16px 20px; }
  h2 { font-size: 18px; margin: 0 0 10px; }
  ul { padding-left: 20px; margin: 10px 0; }
  li { margin: 5px 0; }
</style>
</head>
<body>
  <h2>句序 (Juxu) v${VERSION} 已安装并自动启用</h2>
  <ul>
    <li>无需注销或重启，现在即可按快捷键（<code>Ctrl + Space</code> 或地球键 🌐）切换到<strong>句序</strong>开始输入。</li>
    <li>点击屏幕右上角输入法菜单图标可随时切换输入方案（拼音 / 五笔 / 仓颉）或打开「句序偏好设置…」。</li>
  </ul>
</body>
</html>
EOF

COMPONENT_PLIST="$STAGE_DIR/component.plist"
pkgbuild --analyze --root "$PAYLOAD_DIR" "$COMPONENT_PLIST" >/dev/null
# Prevent macOS Installer from relocating Juxu.app onto a build/ or Downloads/ copy!
if /usr/libexec/PlistBuddy -c "Print :0" "$COMPONENT_PLIST" >/dev/null 2>&1; then
    /usr/libexec/PlistBuddy -c "Set :0:BundleIsRelocatable false" "$COMPONENT_PLIST" 2>/dev/null \
        || /usr/libexec/PlistBuddy -c "Add :0:BundleIsRelocatable bool false" "$COMPONENT_PLIST" 2>/dev/null \
        || true
fi

COMPONENT_PKG="$STAGE_DIR/juxu-component.pkg"
pkgbuild \
    --root "$PAYLOAD_DIR" \
    --component-plist "$COMPONENT_PLIST" \
    --scripts "$SCRIPTS_DIR" \
    --identifier "com.hcyue.juxu.pkg" \
    --version "$VERSION" \
    --install-location "/" \
    "$COMPONENT_PKG" >/dev/null

DIST_XML="$STAGE_DIR/Distribution.xml"
cat <<EOF > "$DIST_XML"
<?xml version="1.0" encoding="utf-8"?>
<installer-gui-script minSpecVersion="2">
    <title>句序 (Juxu) ${VERSION}</title>
    <organization>com.hcyue</organization>
    <options customize="never" require-scripts="true" hostArchitectures="arm64"/>
    <domains enable_anywhere="false" enable_currentUserHome="false" enable_localSystem="true"/>
    <os-check script="pm_os_check();"/>
    <script><![CDATA[
    function pm_os_check() {
        return system.compareVersions(system.version.ProductVersion, '14.0') >= 0;
    }
    ]]></script>
    <welcome file="welcome.html" mime-type="text/html"/>
    <conclusion file="conclusion.html" mime-type="text/html"/>
    <choices-outline>
        <line choice="default">
            <line choice="com.hcyue.juxu.pkg"/>
        </line>
    </choices-outline>
    <choice id="default"/>
    <choice id="com.hcyue.juxu.pkg" visible="false">
        <pkg-ref id="com.hcyue.juxu.pkg"/>
    </choice>
    <pkg-ref id="com.hcyue.juxu.pkg" version="${VERSION}" onConclusion="none">juxu-component.pkg</pkg-ref>
</installer-gui-script>
EOF

UNSIGNED_PKG="$STAGE_DIR/Juxu-${VERSION}-unsigned.pkg"
productbuild \
    --distribution "$DIST_XML" \
    --resources "$STAGE_DIR/resources" \
    --package-path "$STAGE_DIR" \
    "$UNSIGNED_PKG" >/dev/null

if [ -n "$INSTALLER_IDENTITY" ] && [ "$INSTALLER_IDENTITY" != "-" ]; then
    productsign --sign "$INSTALLER_IDENTITY" "$UNSIGNED_PKG" "$OUT_PKG"
else
    cp "$UNSIGNED_PKG" "$OUT_PKG"
fi

echo "Built macOS installer package: $OUT_PKG"
