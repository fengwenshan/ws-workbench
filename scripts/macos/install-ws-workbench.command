#!/bin/bash
# ws-workbench 安装程序（macOS）
#
# 由 DMG 提供；双击运行即可完成：复制到 /Applications → 清除隔离属性 → 启动。
# 之所以要清除隔离属性：安装包没有走 Apple 公证，从网上下载的 .app 会被
# Gatekeeper 打上 com.apple.quarantine，双击时提示「已损坏」或「无法验证开发者」。
# 手动等价命令：sudo xattr -cr /Applications/ws-workbench.app
set -euo pipefail

DIR="$(cd "$(dirname "$0")" && pwd)"
APP_SRC="$(/usr/bin/find "$DIR" -maxdepth 1 -name '*.app' -print -quit)"
if [ -z "$APP_SRC" ]; then
    echo "在 $DIR 里没找到 .app，请从 DMG 窗口运行本脚本。" >&2
    /bin/read -r -n 1 -p "按任意键退出…" || true
    exit 1
fi
APP_NAME="$(basename "$APP_SRC")"
DEST="/Applications/$APP_NAME"
BUNDLE_ID="com.ws.workbench"

echo "== 安装 $APP_NAME 到 /Applications =="
if [ -e "$DEST" ]; then
    echo "发现旧版本，先退出再替换：$DEST"
    /usr/bin/osascript -e "tell application id \"$BUNDLE_ID\" to quit" >/dev/null 2>&1 || true
    /usr/bin/pkill -f "$APP_NAME/Contents/MacOS/" >/dev/null 2>&1 || true
    sleep 1
    rm -rf "$DEST" 2>/dev/null || sudo rm -rf "$DEST"
fi
/usr/bin/ditto "$APP_SRC" "$DEST"

echo "== 清除隔离属性（可能要求输入管理员密码）=="
sudo /usr/bin/xattr -cr "$DEST"

echo "== 启动 $APP_NAME =="
/usr/bin/open "$DEST"

echo "安装完成。以后从「应用程序」里打开 $APP_NAME 即可。"
