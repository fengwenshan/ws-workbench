# ws-workbench 安装包自带卸载程序 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 给 ws-workbench 增加一个随安装包分发的卸载程序：先确认所有进程退出，再展示真实数据/附件路径，默认保留数据，用户可选彻底删除；macOS 用 DMG 内的 `.command`，Windows 后续用 NSIS 卸载钩子。

**Architecture:** App 在启动/保存设置时写一份纯路径清单 `uninstall-info.txt`（无密钥）；macOS DMG 里的 bash + osascript 卸载脚本读取清单，负责退出进程、确认、移动 app、可选删除 App 自有数据。Windows 复用 Tauri NSIS 标准卸载器，后续在 Windows 主机用 `NSIS_HOOK_PREUNINSTALL` 增加进程检查与数据选择。

**Tech Stack:** Rust/Tauri 2（AppHandle、路径 API）、Vue 3（卸载窗口守卫）、Bash + osascript（macOS 卸载程序）、NSIS（Windows 后续）、Node（release.mjs DMG 打包）。

**当前状态（2026-11-10）：** Phase 1（macOS）已实现并通过 `cargo test --lib`、`cargo check --bins`、`pnpm build`、卸载脚本 dry-run 测试和 DMG 可执行位验证；Phase W（Windows）尚未执行，需在 Windows 主机上按任务继续。

---

## 文件结构

| 文件 | 职责 |
|---|---|
| `src-tauri/src/uninstall_info.rs` | 汇总真实数据/附件/缓存路径，渲染并原子写入 `uninstall-info.txt`；纯函数便于单测 |
| `src-tauri/src/config.rs` | `save_settings` 成功后刷新清单（加一个内部 helper，避免递归） |
| `src-tauri/src/lib.rs` | `mod uninstall_info;`；setup 末尾首次写清单 |
| `src-tauri/src/commands/mod.rs` | `attachments` 改成 `pub(crate) mod`，供 uninstall_info 复用上传目录解析 |
| `scripts/macos/uninstall-ws-workbench.command` | macOS 卸载程序（DMG 内显示为「卸载 ws-workbench.command」） |
| `scripts/macos/test-uninstall.sh` | 卸载脚本的 dry-run 断言测试（临时 HOME + 假 app/假数据） |
| `scripts/release.mjs` | 生成 DMG 时复制卸载脚本并 `chmod +x` |
| `src/uninstall/UninstallWindow.vue` | 通用卸载窗口：进程状态查询失败/强杀后仍有进程时中止，不删任何文件 |
| `README.md` | 安装包卸载说明、数据路径、Windows 标准卸载器说明 |
| `docs/superpowers/specs/2026-11-10-installer-uninstall-design.md` | 设计（已完成） |

---

## Phase 1（本期，macOS）

### Task 1: 路径清单模块 `uninstall_info`

**Files:**
- Create: `src-tauri/src/uninstall_info.rs`
- Modify: `src-tauri/src/lib.rs`（只加 `mod uninstall_info;`）
- Test: `src-tauri/src/uninstall_info.rs` 内 `#[cfg(test)]`

- [ ] **Step 1: 写失败测试**

在 `src-tauri/src/uninstall_info.rs` 末尾先写：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn app_bundle_is_found_from_macos_executable() {
        let exe = PathBuf::from("/Applications/ws-workbench.app/Contents/MacOS/ws-workbench");
        assert_eq!(
            app_bundle_from_exe(&exe),
            Some(PathBuf::from("/Applications/ws-workbench.app"))
        );
    }

    #[test]
    fn app_bundle_is_none_outside_dot_app() {
        let exe = PathBuf::from("/Users/me/project/src-tauri/target/debug/ws-workbench");
        assert_eq!(app_bundle_from_exe(&exe), None);
    }

    #[test]
    fn render_skips_newline_paths_and_keeps_flags() {
        let info = UninstallInfo {
            app_bundle: "/Applications/ws-workbench.app".to_string(),
            data_dirs: vec![
                "/Users/me/Library/Application Support/ws-workbench".to_string(),
                "/Users/me/bad\npath".to_string(),
            ],
            upload_dir: "/Users/me/attachments".to_string(),
            upload_custom: true,
            db_path: "/Users/me/db/ws-workbench.db".to_string(),
            clipboard_dir: "/Users/me/clipboard-attachments".to_string(),
            cache_dirs: vec!["/Users/me/Library/WebKit/com.ws.workbench".to_string()],
        };
        let text = render(&info);
        assert!(text.contains("version=1\n"));
        assert!(text.contains("upload_custom=1\n"));
        assert!(text.contains("data_dir=/Users/me/Library/Application Support/ws-workbench\n"));
        assert!(!text.contains("bad\npath"));
        assert_eq!(text.lines().filter(|l| l.starts_with("data_dir=")).count(), 1);
    }

    #[test]
    fn cache_candidates_cover_known_app_paths() {
        let home = PathBuf::from("/Users/me");
        let paths: Vec<String> = cache_candidates(&home)
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        assert!(paths.iter().any(|p| p.ends_with("Library/WebKit/com.ws.workbench")));
        assert!(paths.iter().any(|p| p.ends_with("Library/Caches/com.ws.workbench")));
        assert!(paths.iter().any(|p| p.ends_with("Library/Preferences/com.ws.workbench.plist")));
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd src-tauri && cargo test --lib uninstall_info`
Expected: FAIL，`app_bundle_from_exe` / `UninstallInfo` 等未定义。

- [ ] **Step 3: 实现模块**

`src-tauri/src/uninstall_info.rs` 完整内容：

```rust
//! 安装包卸载程序使用的路径清单。
//!
//! 只写本地路径，不含密钥/配置内容；卸载脚本按行读取，不做 `eval`。
//! 目的是让卸载程序能准确显示用户自定义的附件目录，而不是猜默认路径。

use std::path::{Path, PathBuf};
use std::fmt::Write as _;

use tauri::{AppHandle, Manager};

use crate::models::AppSettings;

/// 清单文件名（各数据目录各放一份）
pub(crate) const FILE_NAME: &str = "uninstall-info.txt";
const VERSION: &str = "1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UninstallInfo {
    pub app_bundle: String,
    pub data_dirs: Vec<String>,
    pub upload_dir: String,
    pub upload_custom: bool,
    pub db_path: String,
    pub clipboard_dir: String,
    pub cache_dirs: Vec<String>,
}

/// 从 `.../Foo.app/Contents/MacOS/Foo` 找到 `.../Foo.app`。
pub(crate) fn app_bundle_from_exe(exe: &Path) -> Option<PathBuf> {
    exe.ancestors()
        .find(|path| path.extension().is_some_and(|ext| ext == "app"))
        .map(Path::to_path_buf)
}

/// App 在 macOS 上可能用到的缓存/WebView 目录（只返回候选，不判断存在）。
fn cache_candidates(home: &Path) -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let library = home.join("Library");
        vec![
            library.join("Caches").join("com.ws.workbench"),
            library.join("Caches").join("ws-workbench"),
            library.join("WebKit").join("com.ws.workbench"),
            library.join("WebKit").join("ws-workbench"),
            library.join("HTTPStorages").join("com.ws.workbench"),
            library
                .join("Saved Application State")
                .join("com.ws.workbench.savedState"),
            library.join("Preferences").join("com.ws.workbench.plist"),
            library.join("Logs").join("ws-workbench"),
        ]
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = home;
        Vec::new()
    }
}

fn push_unique(paths: &mut Vec<String>, path: PathBuf) {
    if path.as_os_str().is_empty() {
        return;
    }
    let text = path.to_string_lossy().to_string();
    if !paths.contains(&text) {
        paths.push(text);
    }
}

fn push_line(out: &mut String, key: &str, value: &str) {
    // 路径含换行会破坏按行解析；这种极端路径直接跳过。
    if value.contains('\n') || value.contains('\r') {
        return;
    }
    let _ = writeln!(out, "{key}={value}");
}

pub(crate) fn render(info: &UninstallInfo) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "version={VERSION}");
    push_line(&mut out, "app_bundle", &info.app_bundle);
    for dir in &info.data_dirs {
        push_line(&mut out, "data_dir", dir);
    }
    push_line(&mut out, "db", &info.db_path);
    push_line(&mut out, "upload_dir", &info.upload_dir);
    push_line(&mut out, "upload_custom", if info.upload_custom { "1" } else { "0" });
    push_line(&mut out, "clipboard_dir", &info.clipboard_dir);
    for dir in &info.cache_dirs {
        push_line(&mut out, "cache_dir", dir);
    }
    out
}

fn collect(app: &AppHandle, settings: &AppSettings) -> UninstallInfo {
    let home = crate::cleaner::home_dir();
    let mut data_dirs: Vec<String> = Vec::new();

    // Tauri 的 app data / config / local data 目录（macOS 上通常重合）
    for dir in [
        app.path().app_data_dir().ok(),
        app.path().app_config_dir().ok(),
        app.path().app_local_data_dir().ok(),
    ]
    .into_iter()
    .flatten()
    {
        if dir.exists() {
            push_unique(&mut data_dirs, dir);
        }
    }
    // 配置实际写入的候选目录（含 ~/.ws-workbench）
    for (_, dir) in crate::config::storage_candidates(app) {
        if dir.exists() {
            push_unique(&mut data_dirs, dir);
        }
    }
    // cleaner 模块自己的数据目录（popup / sleep 规则等）
    if let Ok(dir) = crate::cleaner::app_data_dir() {
        push_unique(&mut data_dirs, dir);
    }

    let app_bundle = std::env::current_exe()
        .ok()
        .and_then(|exe| app_bundle_from_exe(&exe))
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_default();

    let upload_custom = settings
        .default_upload_dir
        .as_deref()
        .map(str::trim)
        .is_some_and(|value| !value.is_empty());
    let upload_dir = crate::commands::attachments::resolve_upload_dir(
        app.clone(),
        settings.default_upload_dir.clone(),
    )
    .unwrap_or_default();

    let db_path = crate::db::db_path(app)
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_default();
    let clipboard_dir = crate::db::storage_dir(app)
        .ok()
        .map(|dir| dir.join(crate::clipboard::store::ATTACHMENTS_DIR))
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_default();

    let cache_dirs = home
        .as_deref()
        .map(cache_candidates)
        .unwrap_or_default()
        .into_iter()
        .filter(|path| path.exists())
        .map(|path| path.to_string_lossy().to_string())
        .collect();

    UninstallInfo {
        app_bundle,
        data_dirs,
        upload_dir,
        upload_custom,
        db_path,
        clipboard_dir,
        cache_dirs,
    }
}

#[cfg(unix)]
fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(".{FILE_NAME}.tmp"));
    std::fs::write(&tmp, text)?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    std::fs::rename(&tmp, path)
}

#[cfg(not(unix))]
fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, text)
}

/// 刷新清单：写到所有数据目录 + app_data_dir + cleaner 数据目录。
pub(crate) fn refresh(app: &AppHandle, settings: &AppSettings) {
    let info = collect(app, settings);
    let text = render(&info);

    let mut targets: Vec<PathBuf> = info.data_dirs.iter().map(PathBuf::from).collect();
    if let Ok(dir) = app.path().app_data_dir() {
        targets.push(dir);
    }
    if let Ok(dir) = crate::cleaner::app_data_dir() {
        targets.push(dir);
    }

    for dir in targets {
        if std::fs::create_dir_all(&dir).is_err() {
            continue;
        }
        let path = dir.join(FILE_NAME);
        if let Err(err) = write_private(&path, &text) {
            eprintln!("[uninstall-info] 写入 {} 失败: {}", path.display(), err);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn app_bundle_is_found_from_macos_executable() {
        let exe = PathBuf::from("/Applications/ws-workbench.app/Contents/MacOS/ws-workbench");
        assert_eq!(
            app_bundle_from_exe(&exe),
            Some(PathBuf::from("/Applications/ws-workbench.app"))
        );
    }

    #[test]
    fn app_bundle_is_none_outside_dot_app() {
        let exe = PathBuf::from("/Users/me/project/src-tauri/target/debug/ws-workbench");
        assert_eq!(app_bundle_from_exe(&exe), None);
    }

    #[test]
    fn render_skips_newline_paths_and_keeps_flags() {
        let info = UninstallInfo {
            app_bundle: "/Applications/ws-workbench.app".to_string(),
            data_dirs: vec![
                "/Users/me/Library/Application Support/ws-workbench".to_string(),
                "/Users/me/bad\npath".to_string(),
            ],
            upload_dir: "/Users/me/attachments".to_string(),
            upload_custom: true,
            db_path: "/Users/me/db/ws-workbench.db".to_string(),
            clipboard_dir: "/Users/me/clipboard-attachments".to_string(),
            cache_dirs: vec!["/Users/me/Library/WebKit/com.ws.workbench".to_string()],
        };
        let text = render(&info);
        assert!(text.contains("version=1\n"));
        assert!(text.contains("upload_custom=1\n"));
        assert!(text.contains("data_dir=/Users/me/Library/Application Support/ws-workbench\n"));
        assert!(!text.contains("bad\npath"));
        assert_eq!(text.lines().filter(|l| l.starts_with("data_dir=")).count(), 1);
    }

    #[test]
    fn cache_candidates_cover_known_app_paths() {
        let home = PathBuf::from("/Users/me");
        let paths: Vec<String> = cache_candidates(&home)
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        assert!(paths.iter().any(|p| p.ends_with("Library/WebKit/com.ws.workbench")));
        assert!(paths.iter().any(|p| p.ends_with("Library/Caches/com.ws.workbench")));
        assert!(paths.iter().any(|p| p.ends_with("Library/Preferences/com.ws.workbench.plist")));
    }
}
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cd src-tauri && cargo test --lib uninstall_info`
Expected: 4 passed。

- [ ] **Step 5: 注册模块**

`src-tauri/src/lib.rs` 顶部模块声明处加：

```rust
mod uninstall_info;
```

Run: `cd src-tauri && cargo check --lib`
Expected: 通过（`commands::attachments` 暂时会报私有模块，下一步修）。

> 注：本任务先改 `src-tauri/src/commands/mod.rs` 的 `mod attachments;` 为 `pub(crate) mod attachments;`，否则 `resolve_upload_dir` 不可见。

### Task 2: 启动/保存设置时刷新清单

**Files:**
- Modify: `src-tauri/src/commands/mod.rs:6`（`mod attachments;` → `pub(crate) mod attachments;`）
- Modify: `src-tauri/src/config.rs:233-249`（`save_settings` 尾部刷新）
- Modify: `src-tauri/src/lib.rs`（setup 末尾首次刷新）

- [ ] **Step 1: 写失败测试**

在 `src-tauri/src/uninstall_info.rs` 增加一个渲染测试（不依赖 AppHandle）：

```rust
#[test]
fn render_contains_all_sections() {
    let info = UninstallInfo {
        app_bundle: "/Applications/ws-workbench.app".to_string(),
        data_dirs: vec!["/Users/me/Library/Application Support/com.ws.workbench".to_string()],
        upload_dir: "/Users/me/attachments".to_string(),
        upload_custom: false,
        db_path: "/Users/me/db/ws-workbench.db".to_string(),
        clipboard_dir: "/Users/me/clipboard-attachments".to_string(),
        cache_dirs: vec!["/Users/me/Library/Caches/com.ws.workbench".to_string()],
    };
    let text = render(&info);
    for key in ["app_bundle=", "data_dir=", "db=", "upload_dir=", "clipboard_dir=", "cache_dir="] {
        assert!(text.contains(key), "missing {key}");
    }
}
```

- [ ] **Step 2: 运行确认现状**

Run: `cd src-tauri && cargo test --lib uninstall_info::tests::render_contains_all_sections`
Expected: PASS（这是防回归测试；刷新钩子本身由后续编译和手工验证覆盖）。

- [ ] **Step 3: 改可见性 + 保存钩子**

`src-tauri/src/commands/mod.rs` 第 6 行：

```rust
pub(crate) mod attachments;
```

`src-tauri/src/config.rs` 的 `save_settings` 改为：

```rust
/// 保存设置：Store → 文件
pub fn save_settings(app: &AppHandle, settings: &AppSettings) -> Result<(), String> {
    set_settings_cache(settings);
    let mut errors: Vec<String> = Vec::new();
    let result = match save_settings_to_store(app, settings) {
        Ok(()) => Ok(()),
        Err(e) => {
            errors.push(format!("store: {}", e));
            match save_settings_to_file(app, settings) {
                Ok(()) => Ok(()),
                Err(e) => {
                    errors.push(format!("file: {}", e));
                    Err(format!(
                        "设置保存失败（已保存在内存中，重启后会丢失）：{}",
                        errors.join("; ")
                    ))
                }
            }
        }
    };
    // 路径清单写入失败不影响设置保存；卸载时找不到会回退默认路径。
    crate::uninstall_info::refresh(app, settings);
    result
}
```

- [ ] **Step 4: setup 首次写入**

`src-tauri/src/lib.rs` 的 `setup(|app| { ... })` 末尾（`Ok(())` 之前）加：

```rust
            // 安装包卸载程序读取的路径清单：启动时先写一份，设置变化时 save_settings 会再写。
            let settings = crate::config::load_settings(app.handle());
            crate::uninstall_info::refresh(app.handle(), &settings);
```

- [ ] **Step 5: 编译 + 测试**

Run: `cd src-tauri && cargo check --lib && cargo test --lib uninstall_info`
Expected: 全部通过。

### Task 3: macOS 卸载脚本

**Files:**
- Create: `scripts/macos/uninstall-ws-workbench.command`
- Create: `scripts/macos/test-uninstall.sh`

- [ ] **Step 1: 写脚本**

完整内容：

```bash
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

mkdir -p "$LOG_DIR" 2>/dev/null || true
log() { printf '[%s] %s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$*" | tee -a "$LOG_FILE"; }

# 对话框：返回按钮文本；关闭/取消返回空串。dry-run 直接返回默认按钮。
dialog() {
    local title="$1" message="$2" b1="$3" b2="$4" b3="${5:-}"
    if [ "$DRY_RUN" = "1" ]; then
        printf '%s' "$b2"
        return 0
    fi
    WS_TITLE="$title" WS_MSG="$message" WS_B1="$b1" WS_B2="$b2" WS_B3="$b3" \
    osascript 2>/dev/null <<'APPLESCRIPT'
set t to system attribute "WS_TITLE"
set m to system attribute "WS_MSG"
set b1 to system attribute "WS_B1"
set b2 to system attribute "WS_B2"
set b3 to system attribute "WS_B3"
if b3 is "" then
    display dialog m with title t buttons {b1, b2} default button b2 with icon caution
else
    display dialog m with title t buttons {b1, b2, b3} default button b2 with icon caution
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
        sleep 0.5
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
            sleep 1
            pids=$(app_pids)
            [ -n "$pids" ] && kill -KILL $pids 2>/dev/null || true
        fi
        round=$((round + 1))
        sleep 1
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
plutil -extract CFBundleIdentifier raw "$APP_PATH/Contents/Info.plist" >/dev/null 2>&1
actual_id="$(plutil -extract CFBundleIdentifier raw "$APP_PATH/Contents/Info.plist" 2>/dev/null || true)"
if [ "$actual_id" != "$BUNDLE_ID" ]; then
    fail_no_delete "应用标识不匹配（$actual_id），为避免误删已中止。"
fi

# 1. 退出应用（先正常退出，退不掉询问强制退出）
if [ -n "$(app_pids)" ]; then
    if [ "$DRY_RUN" != "1" ]; then
        osascript -e "tell application id \"$BUNDLE_ID\" to quit" >/dev/null 2>&1 || true
    fi
    if ! wait_for_exit; then
        choice="$(dialog "ws-workbench 仍在运行" \
            "还有进程没有退出。强制退出可能导致正在编辑的数据丢失，是否强制退出？" \
            "取消" "强制退出")"
        [ "$choice" = "强制退出" ] || fail_no_delete "已取消，未删除任何文件。"
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

owned_data="$(format_paths ${data_dirs[@]+"${data_dirs[@]}"})"
owned_cache="$(format_paths ${cache_dirs[@]+"${cache_dirs[@]}"})"
upload_note=""
[ "$upload_custom" = "1" ] && upload_note="（自定义）"

# 3. 确认数据策略
choice="$(dialog "卸载 ws-workbench" \
"应用：$APP_PATH

以下为 App 自有数据，默认保留：
$owned_data$owned_cache
附件目录$upload_note：$upload_dir

是否同时删除这些用户数据？删除后附件/数据库不可恢复。" \
"取消" "保留数据并卸载" "删除全部数据")"
if [ "$DRY_RUN" = "1" ] && [ "$DRY_RUN_DATA" = "all" ]; then
    choice="删除全部数据"
fi
case "$choice" in
    "删除全部数据")
        confirm="$(dialog "确认删除全部数据" \
            "附件、数据库、剪贴板图片和缓存将被永久删除，无法恢复。确认继续？" \
            "取消" "确认删除")"
        [ "$confirm" = "确认删除" ] || fail_no_delete "已取消删除数据。"
        delete_data=1
        ;;
    "保留数据并卸载") delete_data=0 ;;
    *) fail_no_delete "已取消，未删除任何文件。" ;;
esac

# 4. 删除前最后确认进程归零
[ -z "$(app_pids)" ] || fail_no_delete "确认期间应用被重新启动，未删除任何文件。"

# 5. 应用进废纸篓
TRASH_DIR="$HOME/.Trash"
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
if [ "$delete_data" = "1" ]; then
    for path in ${data_dirs[@]+"${data_dirs[@]}"} ${cache_dirs[@]+"${cache_dirs[@]}"} "$clipboard_dir"; do
        [ -z "$path" ] && continue
        case "$path" in
            "$HOME"/*) ;;
            *) log "跳过不在用户目录的路径：$path"; continue ;;
        esac
        if [ ! -e "$path" ]; then continue; fi
        if [ "$DRY_RUN" = "1" ]; then
            log "[dry-run] 删除 $path"
        elif rm -rf -- "$path" 2>>"$LOG_FILE"; then
            log "已删除 $path"
        else
            log "删除失败：$path"
        fi
    done
    if [ "$upload_custom" = "1" ]; then
        owned=0
        for path in ${data_dirs[@]+"${data_dirs[@]}"}; do
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
    tips="$tips\nApp 自有数据已删除。"
else
    tips="$tips\n用户数据已保留。"
fi
[ -n "$custom_kept" ] && tips="$tips\n\n自定义附件目录未删除，请按需手动处理：\n$custom_kept"
dialog "卸载 ws-workbench" "$tips" "好" >/dev/null
log "卸载完成"
exit 0
```

- [ ] **Step 2: 写 dry-run 测试**

`scripts/macos/test-uninstall.sh`：

```bash
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
    mkdir -p "$TMP/home/Applications/ws-workbench.app/Contents"
    cat > "$TMP/home/Applications/ws-workbench.app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>com.ws.workbench</string>
</dict></plist>
PLIST
    touch "$TMP/home/Applications/ws-workbench.app/Contents/MacOS/ws-workbench"
    mkdir -p "$TMP/home/Library/Application Support/com.ws.workbench"
    touch "$TMP/home/Library/Application Support/com.ws.workbench/ws-workbench.db"
    mkdir -p "$TMP/home/Library/Application Support/com.ws.workbench/attachments"
    touch "$TMP/home/Library/Application Support/com.ws.workbench/attachments/a.wst"
    mkdir -p "$TMP/home/Library/WebKit/com.ws.workbench"
    touch "$TMP/home/Library/WebKit/com.ws.workbench/cache"
    mkdir -p "$TMP/custom-attachments"
    touch "$TMP/custom-attachments/user.wst"
    cat > "$TMP/home/Library/Application Support/com.ws.workbench/uninstall-info.txt" <<EOF
version=1
app_bundle=$TMP/home/Applications/ws-workbench.app
data_dir=$TMP/home/Library/Application Support/com.ws.workbench
db=$TMP/home/Library/Application Support/com.ws.workbench/ws-workbench.db
upload_dir=$TMP/custom-attachments
upload_custom=1
clipboard_dir=$TMP/home/Library/Application Support/com.ws.workbench/clipboard-attachments
cache_dir=$TMP/home/Library/WebKit/com.ws.workbench
EOF
}

run_case() {
    local name="$1" pids="$2" data="$3"
    local log="$TMP/$name.log"
    HOME="$TMP/home" WS_UNINSTALL_DRY_RUN=1 WS_UNINSTALL_DRY_RUN_DATA="$data" \
        WS_UNINSTALL_FAKE_PIDS="$pids" "$SCRIPT" >"$log" 2>&1 || true
    echo "$log"
}

make_fake_home
log="$(run_case keep "" keep)"
assert_contains "$log" "[dry-run] 移动"
assert_contains "$log" "已移到废纸篓" || true
assert_not_contains "$log" "[dry-run] 删除"
[ -f "$TMP/custom-attachments/user.wst" ]

log="$(run_case all "" all)"
assert_contains "$log" "[dry-run] 删除"
assert_contains "$log" "自定义附件目录"
assert_contains "$log" "$TMP/custom-attachments"

log="$(run_case running 999999 keep)"
assert_contains "$log" "卸载失败"
assert_contains "$log" "无法退出所有"
assert_not_contains "$log" "[dry-run] 移动"

echo "uninstall script tests passed"
```

- [ ] **Step 3: 运行测试**

Run: `bash scripts/macos/test-uninstall.sh`
Expected: `uninstall script tests passed`。若失败，按日志修脚本。

### Task 4: DMG 带上卸载脚本

**Files:**
- Modify: `scripts/release.mjs` 的 `createDmg`

- [ ] **Step 1: 修改 createDmg**

在 `execFileSync('cp', ['-R', join(dir, appName), join(stage, appName)], ...)` 之后加：

```js
    const uninstallerSource = join(ROOT, 'scripts/macos/uninstall-ws-workbench.command')
    const uninstallerName = '卸载 ws-workbench.command'
    if (!existsSync(uninstallerSource)) {
      fail(`缺少卸载脚本: ${uninstallerSource}`)
    }
    copyFileSync(uninstallerSource, join(stage, uninstallerName))
    chmodSync(join(stage, uninstallerName), 0o755)
```

顶部 `node:fs` 导入补 `chmodSync`。

- [ ] **Step 2: 语法检查**

Run: `node --check scripts/release.mjs`
Expected: 无输出、exit 0。

- [ ] **Step 3: 本地验证 DMG（如环境允许）**

Run（只构建 app + dmg；耗时较长）：
`TAURI_BUNDLES=app node scripts/release.mjs --no-publish --dmg`
Expected: 生成 `release/ws-workbench_*_*.dmg`，日志含「已生成 dmg」。

挂载验证：
```bash
hdiutil attach -readonly -nobrowse release/ws-workbench_*_*.dmg
ls -l "/Volumes/ws-workbench/卸载 ws-workbench.command"
test -x "/Volumes/ws-workbench/卸载 ws-workbench.command" && echo executable
hdiutil detach "/Volumes/ws-workbench"
```
Expected: 文件存在且可执行。

### Task 5: 通用卸载窗口的进程守卫

**Files:**
- Modify: `src/uninstall/UninstallWindow.vue`（`requestDelete` / `runUninstall` / error 区域）

- [ ] **Step 1: 改请求编排**

`requestDelete` 中把 `catch { running = false }` 改为中止：

```ts
  try {
    const state = await appRunning(app.value.id)
    running = !!state?.running
  } catch (err) {
    phase.value = 'ready'
    actionErrorSource.value = ''
    actionError.value = errorText(err, '无法确认应用运行状态，已中止卸载，未删除任何文件')
    return
  }
```

新增函数：

```ts
/** 系统卸载前再确认一次进程已归零；查询失败也视为不安全。 */
async function ensureNotRunning(): Promise<boolean> {
  const target = app.value
  if (!target) return false
  try {
    const state = await appRunning(target.id)
    if (state?.running) {
      phase.value = 'ready'
      actionErrorSource.value = ''
      actionError.value = `应用仍在运行（${state.processes ?? 0} 个进程），已中止卸载，未删除任何文件`
      return false
    }
    return true
  } catch (err) {
    phase.value = 'ready'
    actionErrorSource.value = ''
    actionError.value = errorText(err, '无法确认应用运行状态，已中止卸载，未删除任何文件')
    return false
  }
}
```

`runUninstall` 开头加：

```ts
  if (!(await ensureNotRunning())) return
```

- [ ] **Step 2: 错误条只在可重试时显示「重试」**

模板里给「重试」按钮加 `v-if="actionErrorSource"`（保持「关闭」始终可用）。搜索 `actionErrorSource` 对应的按钮。

- [ ] **Step 3: 构建验证**

Run: `pnpm build`
Expected: exit 0。

### Task 6: README

**Files:**
- Modify: `README.md`

- [ ] **Step 1: 在「软件卸载」说明后新增「卸载 ws-workbench 自己」小节**

内容：

```markdown
- **卸载 ws-workbench 自己**：安装包（macOS 为 DMG）里带一个「卸载 ws-workbench.command」。
  双击后先正常退出应用，退不掉会询问是否强制退出；**所有进程归零后才继续**，否则直接报
  「卸载失败」且不删除任何文件。继续后会列出应用本体、数据目录、附件上传目录、剪贴板图片
  和 WebView 缓存，默认**保留用户数据**，也可以选择「删除全部数据」（二次确认，附件/数据库
  删除后不可恢复）。自定义到应用数据目录之外的附件目录只提醒、不自动删。
  Windows 使用安装器自带的标准卸载器（设置 → 应用 → 卸载），数据目录位置见下。
```

- [ ] **Step 2: 更新已知限制**

在「已知限制」补一条：Windows 的 NSIS 数据清理页待 Windows 实机实现。

### Task 7: 全量验证

- [ ] **Step 1:** `cd src-tauri && cargo test --lib` → 期望全部通过
- [ ] **Step 2:** `cd src-tauri && cargo check --bins` → 期望通过
- [ ] **Step 3:** `pnpm build` → 期望 exit 0
- [ ] **Step 4:** `bash scripts/macos/test-uninstall.sh` → 期望 passed
- [ ] **Step 5:** 若有条件，按 Task 4 挂载 DMG 验证

---

## Phase W（Windows，后续在 Windows 主机执行）

> 本阶段不在当前 macOS 环境执行；在 Windows 主机上按顺序做，每步验收后再进入下一步。

### Task W1: 确认 NSIS 钩子能力

**Files:** 无（只读检查）

- [ ] 在 Windows 检出仓库，确认 `node_modules/@tauri-apps/cli/config.schema.json` 的
  `bundle.windows.nsis.installerHooks` 支持 `NSIS_HOOK_PREUNINSTALL` / `NSIS_HOOK_POSTUNINSTALL`（当前 schema 已确认支持）。
- [ ] 确认 `installMode` 默认/显式值；计划显式设 `currentUser`，保证 `$APPDATA` 指向当前用户。

### Task W2: 增加 `src-tauri/nsis/uninstall-hooks.nsh`

- [ ] 创建文件，内容：

```nsh
!include "LogicLib.nsh"
!include "FileFunc.nsh"

!macro NSIS_HOOK_PREUNINSTALL
  ; 1) 检查并退出 ws-workbench
  nsExec::ExecToStack 'tasklist /FI "IMAGENAME eq ws-workbench.exe" /NH'
  Pop $0
  Pop $1
  ${If} $1 != ""
    MessageBox MB_YESNO|MB_ICONEXCLAMATION "ws-workbench 正在运行，是否强制退出后继续卸载？" IDYES ws_kill IDNO ws_abort
    ws_kill:
      nsExec::Exec 'taskkill /IM ws-workbench.exe /F'
      Sleep 1500
      nsExec::ExecToStack 'tasklist /FI "IMAGENAME eq ws-workbench.exe" /NH'
      Pop $0
      Pop $1
      ${If} $1 != ""
        MessageBox MB_OK|MB_ICONSTOP "卸载失败：仍有 ws-workbench 进程未退出，未删除任何文件。"
        Abort
      ${EndIf}
      Goto ws_continue
    ws_abort:
      Abort
  ${EndIf}
  ws_continue:

  ; 2) 显示数据/附件路径（PowerShell 读 sidecar）
  nsExec::ExecToStack 'powershell -NoProfile -ExecutionPolicy Bypass -Command "$p=Join-Path $env:APPDATA ''com.ws.workbench\uninstall-info.txt''; if (Test-Path $p) { (Get-Content $p | Where-Object { $_ -match ''^(data_dir|upload_dir|cache_dir)='' }) -join \"`r`n\" }"'
  Pop $0
  Pop $1
  ${If} $1 == ""
    StrCpy $1 "未找到路径清单，将只删除程序文件。"
  ${EndIf}

  ; 3) 询问是否删除用户数据，默认保留
  MessageBox MB_YESNO|MB_DEFBUTTON2|MB_ICONQUESTION "以下为 ws-workbench 用户数据：$\r$\n$1$\r$\n$\r$\n是否同时删除这些数据？删除后附件/数据库不可恢复。" IDYES ws_delete IDNO ws_done
  ws_delete:
    RMDir /r "$APPDATA\com.ws.workbench"
    RMDir /r "$APPDATA\ws-workbench"
    RMDir /r "$LOCALAPPDATA\com.ws.workbench"
    RMDir /r "$LOCALAPPDATA\ws-workbench"
    RMDir /r "$PROFILE\.ws-workbench"
  ws_done:
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ; 清理可能残留的空目录；不碰自定义附件目录
  RMDir "$APPDATA\com.ws.workbench"
  RMDir "$APPDATA\ws-workbench"
!macroend
```

- [ ] 在 `tauri.conf.json` 的 `bundle.windows.nsis` 增加：

```json
"nsis": {
  "installMode": "currentUser",
  "installerHooks": "nsis/uninstall-hooks.nsh"
}
```

（路径相对 `src-tauri`，按 Tauri 实际解析规则调整；若相对 `tauri.conf.json` 则写 `./nsis/...`。）

### Task W3: Windows 验收

- [ ] `pnpm tauri build` 生成 NSIS 安装包；在 Windows 上安装。
- [ ] 运行 ws-workbench，直接去「设置 → 应用 → 卸载」：应先提示退出/强制退出；进程不退时卸载中止且文件仍在。
- [ ] 选择「保留数据」：程序删除，`%APPDATA%\com.ws.workbench` 保留。
- [ ] 选择「删除数据」：数据目录删除；自定义附件目录仍在并有提示。
- [ ] 检查 Apps & Features 中没有残留条目。

---

## Self-review

- **Spec coverage：** 路径清单（Task 1/2）、DMG 脚本（Task 3/4）、进程归零约束（Task 3/5）、数据默认保留/自定义目录不删（Task 3）、Windows 后续（Phase W）、README（Task 6）均有对应任务。
- **Placeholder scan：** 无 TBD/TODO；所有代码步骤给出完整可粘贴内容。
- **Type consistency：** `UninstallInfo` 字段在 render/collect/tests 中一致；`refresh(app, settings)` 两处调用一致；脚本键名 `data_dir` / `upload_dir` / `upload_custom` / `clipboard_dir` / `cache_dir` 与 Rust 渲染一致。


---

## 实现偏差记录（macOS，2026-11-10）

实际代码在以下细节上比任务里的示例更保守，后续按计划生成时以这一节为准：

1. **卸载脚本单按钮对话框**：`dialog` 支持 2～4 个按钮，并允许第 6 个参数单独指定默认按钮；只有 3 个参数时按单按钮弹窗处理（避免 `buttons {"好","好"}` 这种非法 AppleScript）。删除数据的二次确认默认按钮是「取消」，不是「确认删除」。
2. **脚本测试加速**：加入 `WS_UNINSTALL_FAST=1`，dry-run 测试时跳过 `sleep`；生产环境不设置，保持正常 10 秒/3 轮等待。
3. **读取应用标识**：用 `/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier'`，不用 `plutil -extract`（兼容更老的 macOS）。
4. **变量与中文相邻**：脚本里 `$upload_note`、`$actual_id` 后面紧跟中文标点时写成 `${upload_note}` / `${actual_id}`，否则 bash 会把 UTF-8 字节当成变量名的一部分。
5. **路径过滤**：`uninstall_info::collect` 只把 `$HOME` 下的目录写进 `data_dir`，并跳过 `storage_candidates` 里 `crate/.config` / `cwd/.config` 这些开发兜底目录；避免开发期把项目配置当成用户数据。
6. **后端兜底**：`cleaner::installer::uninstall` 在 `quit_running=false` 时也会再检测一次进程；仍有进程则返回 `ok:false`，不进入平台删除逻辑。前端 `UninstallWindow.vue` 同时增加了「查询失败/强杀后仍有进程就中止」的守卫。
7. **计划里的 `save_settings` 结构**：实际用 `result` 变量承接 Store/文件两条保存路径，最后统一刷新清单并返回结果；行为与计划一致。
8. **DMG 验证**：用临时 stage + `hdiutil makehybrid/convert/attach` 验证过 `卸载 ws-workbench.command` 的 UTF-8 文件名和可执行位；完整 release 构建仍需在有签名私钥的机器上跑一次 `TAURI_BUNDLES=app node scripts/release.mjs --no-publish --dmg`。
