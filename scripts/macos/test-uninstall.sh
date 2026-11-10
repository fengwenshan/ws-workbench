#!/bin/bash
set -eu

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRIPT="$ROOT/scripts/macos/uninstall-ws-workbench.command"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

assert_contains() { grep -Fq "$2" "$1" || { echo "缺少: $2"; cat "$1"; exit 1; }; }
assert_not_contains() { ! grep -Fq "$2" "$1" || { echo "不应出现: $2"; cat "$1"; exit 1; }; }

make_fake_home() {
    mkdir -p "$TMP/home/Applications/ws-workbench.app/Contents/MacOS"
    cat > "$TMP/home/Applications/ws-workbench.app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>com.ws.workbench</string>
</dict></plist>
PLIST
    touch "$TMP/home/Applications/ws-workbench.app/Contents/MacOS/ws-workbench"
    mkdir -p "$TMP/home/Library/Application Support/com.ws.workbench/attachments"
    touch "$TMP/home/Library/Application Support/com.ws.workbench/ws-workbench.db"
    touch "$TMP/home/Library/Application Support/com.ws.workbench/attachments/a.wst"
    mkdir -p "$TMP/home/Library/WebKit/com.ws.workbench"
    touch "$TMP/home/Library/WebKit/com.ws.workbench/cache"
    mkdir -p "$TMP/custom-attachments"
    touch "$TMP/custom-attachments/user.wst"
    cat > "$TMP/home/Library/Application Support/com.ws.workbench/uninstall-info.txt" <<INFO
version=1
app_bundle=$TMP/home/Applications/ws-workbench.app
data_dir=$TMP/home/Library/Application Support/com.ws.workbench
db=$TMP/home/Library/Application Support/com.ws.workbench/ws-workbench.db
upload_dir=$TMP/custom-attachments
upload_custom=1
clipboard_dir=$TMP/home/Library/Application Support/com.ws.workbench/clipboard-attachments
cache_dir=$TMP/home/Library/WebKit/com.ws.workbench
INFO
}

run_case() {
    local name="$1" pids="$2" data="$3"
    local log="$TMP/$name.log"
    set +e
    HOME="$TMP/home" WS_UNINSTALL_DRY_RUN=1 WS_UNINSTALL_DRY_RUN_DATA="$data" \
        WS_UNINSTALL_FAST=1 WS_UNINSTALL_FAKE_PIDS="$pids" "$SCRIPT" >"$log" 2>&1
    echo "$? $log"
}

make_fake_home

read -r code log <<<"$(run_case keep "" keep)"
[ "$code" = "0" ] || { cat "$log"; exit 1; }
assert_contains "$log" "[dry-run] 移动"
assert_not_contains "$log" "[dry-run] 删除"
[ -f "$TMP/custom-attachments/user.wst" ]

read -r code log <<<"$(run_case all "" all)"
[ "$code" = "0" ] || { cat "$log"; exit 1; }
assert_contains "$log" "[dry-run] 删除"
assert_contains "$log" "自定义附件目录"
assert_contains "$log" "$TMP/custom-attachments"

read -r code log <<<"$(run_case running 999999 keep)"
[ "$code" = "1" ] || { cat "$log"; exit 1; }
assert_contains "$log" "卸载失败"
assert_contains "$log" "无法退出所有"
assert_not_contains "$log" "[dry-run] 移动"

echo "uninstall script tests passed"
