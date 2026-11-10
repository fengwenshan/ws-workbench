#!/bin/bash
# ws-workbench 卸载程序（macOS）
# 由 DMG 提供；先确认所有进程退出，再展示数据/附件路径，默认保留数据。
set -u
set -o pipefail

APP_NAME="ws-workbench"
BUNDLE_ID="com.ws.workbench"
LOG_DIR="$HOME/Library/Logs"
LOG_FILE="$LOG_DIR/ws-workbench-uninstall.log"
DRY_RUN="${WS_UNINSTALL_DRY_RUN:-0}"
DRY_RUN_DATA="${WS_UNINSTALL_DRY_RUN_DATA:-keep}"
FAST="${WS_UNINSTALL_FAST:-0}"

mkdir -p "$LOG_DIR" 2>/dev/null || true
log() { printf '[%s] %s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$*" | tee -a "$LOG_FILE"; }

pause() {
    [ "$FAST" = "1" ] && return 0
    sleep "${1:-1}"
}

# 对话框：返回按钮文本；关闭/取消返回空串。dry-run 直接返回默认按钮。
# 用法：dialog 标题 内容 [按钮1] [按钮2] [按钮3] [默认按钮]
dialog() {
    local title="$1" message="$2" b1="${3:-好}" b2="${4:-}" b3="${5:-}" default="${6:-$b1}"
    if [ "$DRY_RUN" = "1" ]; then
        if [ -n "$b2" ]; then printf '%s' "$b2"; else printf '%s' "$b1"; fi
        return 0
    fi
    WS_TITLE="$title" WS_MSG="$message" WS_B1="$b1" WS_B2="$b2" WS_B3="$b3" WS_DEFAULT="$default" \
    osascript 2>/dev/null <<'APPLESCRIPT'
set t to system attribute "WS_TITLE"
set m to system attribute "WS_MSG"
set b1 to system attribute "WS_B1"
set b2 to system attribute "WS_B2"
set b3 to system attribute "WS_B3"
set db to system attribute "WS_DEFAULT"
if b2 is "" then
    display dialog m with title t buttons {b1} default button db with icon note
else if b3 is "" then
    display dialog m with title t buttons {b1, b2} default button db with icon caution
else
    display dialog m with title t buttons {b1, b2, b3} default button db with icon caution
end if
return button returned of result
APPLESCRIPT
}

fail_no_delete() {
    log "卸载失败：$*"
    if [ "$DRY_RUN" != "1" ]; then
        dialog "卸载失败" "$*" "好" >/dev/null
    fi
    exit 1
}

cancel_no_delete() {
    log "已取消：$*"
    if [ "$DRY_RUN" != "1" ]; then
        dialog "已取消" "$*" "好" >/dev/null
    fi
    exit 0
}

app_pids() {
    if [ -n "${WS_UNINSTALL_FAKE_PIDS:-}" ]; then
        printf '%s\n' $WS_UNINSTALL_FAKE_PIDS
        return 0
    fi
    /usr/bin/pgrep -f "/ws-workbench.app/Contents/MacOS/ws-workbench" 2>/dev/null || true
}

wait_for_exit() {
    local i=0
    while [ "$i" -lt 20 ]; do
        [ -z "$(app_pids)" ] && return 0
        pause 0.5
        i=$((i + 1))
    done
    return 1
}

force_quit() {
    local round=0 pids
    while [ "$round" -lt 3 ]; do
        pids=$(app_pids)
        [ -z "$pids" ] && return 0
        if [ "$DRY_RUN" != "1" ]; then
            kill -TERM $pids 2>/dev/null || true
            pause 1
            pids=$(app_pids)
            [ -n "$pids" ] && kill -KILL $pids 2>/dev/null || true
        fi
        round=$((round + 1))
        pause 1
        [ -z "$(app_pids)" ] && return 0
    done
    return 1
}

find_app() {
    local candidate
    for candidate in "/Applications/$APP_NAME.app" "$HOME/Applications/$APP_NAME.app"; do
        if [ -d "$candidate" ]; then
            printf '%s' "$candidate"
            return 0
        fi
    done
    return 1
}

load_info() {
    local file="" line
    for file in \
        "$HOME/Library/Application Support/com.ws.workbench/uninstall-info.txt" \
        "$HOME/Library/Application Support/ws-workbench/uninstall-info.txt" \
        "$HOME/.ws-workbench/uninstall-info.txt"
    do
        [ -f "$file" ] && break
        file=""
    done
    [ -z "$file" ] && return 0

    while IFS= read -r line || [ -n "$line" ]; do
        case "$line" in
            app_bundle=*) info_app="${line#app_bundle=}" ;;
            data_dir=*) data_dirs+=("${line#data_dir=}") ;;
            db=*) db_path="${line#db=}" ;;
            upload_dir=*) upload_dir="${line#upload_dir=}" ;;
            upload_custom=*) upload_custom="${line#upload_custom=}" ;;
            clipboard_dir=*) clipboard_dir="${line#clipboard_dir=}" ;;
            cache_dir=*) cache_dirs+=("${line#cache_dir=}") ;;
        esac
    done < "$file"
}

format_paths() {
    local path
    for path in "$@"; do
        [ -n "$path" ] && printf '  • %s\n' "$path"
    done
}

is_under() {
    case "$1/" in
        "$2"/*) return 0 ;;
        *) return 1 ;;
    esac
}

# ===== 开始 =====
log "开始卸载 $APP_NAME"

APP_PATH="$(find_app)" || fail_no_delete "未在 /Applications 或 ~/Applications 找到 $APP_NAME.app。"
actual_id="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$APP_PATH/Contents/Info.plist" 2>/dev/null || true)"
if [ "$actual_id" != "$BUNDLE_ID" ]; then
    fail_no_delete "应用标识不匹配（${actual_id}），为避免误删已中止。"
fi

start_choice="$(dialog "卸载 ws-workbench" \
    "将退出 ws-workbench 并把它移到废纸篓。是否继续？" \
    "取消" "继续" "" "继续")"
[ "$start_choice" = "继续" ] || cancel_no_delete "未卸载，未删除任何文件。"

# 1. 退出应用（先正常退出，退不掉询问强制退出）
if [ -n "$(app_pids)" ]; then
    if [ "$DRY_RUN" != "1" ]; then
        osascript -e "tell application id \"$BUNDLE_ID\" to quit" >/dev/null 2>&1 || true
    fi
    if ! wait_for_exit; then
        choice="$(dialog "ws-workbench 仍在运行" \
            "还有进程没有退出。强制退出可能导致正在编辑的数据丢失，是否强制退出？" \
            "取消" "强制退出" "" "取消")"
        [ "$choice" = "强制退出" ] || cancel_no_delete "未退出应用，未删除任何文件。"
        force_quit || fail_no_delete "无法退出所有 ws-workbench 进程，未删除任何文件。"
    fi
fi
log "所有进程已退出"

# 2. 读路径清单
data_dirs=()
cache_dirs=()
info_app=""
db_path=""
upload_dir=""
upload_custom="0"
clipboard_dir=""
load_info
if [ -z "$info_app" ] && [ -z "$upload_dir" ]; then
    log "未找到路径清单，回退默认路径（可能无法确认自定义附件目录）"
fi

[ -z "$upload_dir" ] && upload_dir="$HOME/Library/Application Support/com.ws.workbench/attachments"
if [ "${#data_dirs[@]}" -eq 0 ]; then
    data_dirs=(
        "$HOME/Library/Application Support/com.ws.workbench"
        "$HOME/Library/Application Support/ws-workbench"
        "$HOME/.ws-workbench"
    )
fi

owned_data="$(format_paths "${data_dirs[@]}")"
owned_cache=""
if [ "${#cache_dirs[@]}" -gt 0 ]; then
    owned_cache="$(format_paths "${cache_dirs[@]}")"
fi
upload_note=""
[ "$upload_custom" = "1" ] && upload_note="（自定义）"

# 3. 确认数据策略
choice="$(dialog "卸载 ws-workbench" \
"应用：$APP_PATH

以下为 App 自有数据，默认保留：
$owned_data$owned_cache
附件目录${upload_note}：$upload_dir

是否同时删除这些用户数据？删除后附件/数据库不可恢复。" \
"取消" "保留数据并卸载" "删除全部数据" "保留数据并卸载")"
if [ "$DRY_RUN" = "1" ] && [ "$DRY_RUN_DATA" = "all" ]; then
    choice="删除全部数据"
fi
case "$choice" in
    "删除全部数据")
        if [ "$DRY_RUN" = "1" ]; then
            confirm="确认删除"
        else
            confirm="$(dialog "确认删除全部数据" \
                "附件、数据库、剪贴板图片和缓存将被永久删除，无法恢复。确认继续？" \
                "取消" "确认删除" "" "取消")"
        fi
        [ "$confirm" = "确认删除" ] || cancel_no_delete "未删除数据，也未卸载应用。"
        delete_data=1
        ;;
    "保留数据并卸载") delete_data=0 ;;
    *) cancel_no_delete "未卸载，未删除任何文件。" ;;
esac

# 4. 删除前最后确认进程归零
[ -z "$(app_pids)" ] || fail_no_delete "确认期间应用被重新启动，未删除任何文件。"

# 5. 应用进废纸篓
TRASH_DIR="$HOME/.Trash"
mkdir -p "$TRASH_DIR" 2>/dev/null || true
DEST="$TRASH_DIR/$(basename "$APP_PATH")"
[ -e "$DEST" ] && DEST="$DEST.$(date +%s)"
if [ "$DRY_RUN" = "1" ]; then
    log "[dry-run] 移动 $APP_PATH -> $DEST"
else
    mv "$APP_PATH" "$DEST" || fail_no_delete "无法把应用移到废纸篓，数据未删除。请关闭占用后重试。"
    log "已移到废纸篓：$DEST"
fi

# 6. 可选删除 App 自有数据
custom_kept=""
data_failures=0
if [ "$delete_data" = "1" ]; then
    delete_one() {
        local path="$1"
        [ -z "$path" ] && return 0
        case "$path" in
            "$HOME"/*) ;;
            *) log "跳过不在用户目录的路径：$path"; return 0 ;;
        esac
        [ -e "$path" ] || return 0
        if [ "$DRY_RUN" = "1" ]; then
            log "[dry-run] 删除 $path"
        elif rm -rf -- "$path" 2>>"$LOG_FILE"; then
            log "已删除 $path"
        else
            log "删除失败：$path"
            data_failures=$((data_failures + 1))
        fi
    }

    for path in "${data_dirs[@]}"; do delete_one "$path"; done
    if [ "${#cache_dirs[@]}" -gt 0 ]; then
        for path in "${cache_dirs[@]}"; do delete_one "$path"; done
    fi
    delete_one "$clipboard_dir"

    if [ "$upload_custom" = "1" ]; then
        owned=0
        for path in "${data_dirs[@]}"; do
            is_under "$upload_dir" "$path" && owned=1
        done
        if [ "$owned" = "0" ] && [ -e "$upload_dir" ]; then
            custom_kept="$upload_dir"
            log "自定义附件目录不在 App 数据目录内，未自动删除：$upload_dir"
        fi
    fi
fi

# 7. 完成
if [ "$DRY_RUN" = "1" ]; then
    log "[dry-run] 卸载流程完成"
    exit 0
fi
tips="卸载完成。应用已移到废纸篓。"
if [ "$delete_data" = "1" ]; then
    if [ "$data_failures" -gt 0 ]; then
        tips="$tips\n有 $data_failures 个数据目录删除失败，请查看日志：$LOG_FILE"
    else
        tips="$tips\nApp 自有数据已删除。"
    fi
else
    tips="$tips\n用户数据已保留。"
fi
[ -n "$custom_kept" ] && tips="$tips\n\n自定义附件目录未删除，请按需手动处理：\n$custom_kept"
dialog "卸载 ws-workbench" "$tips" "好" >/dev/null
log "卸载完成"
exit 0
