# 软件卸载：运行中检测 + 二次确认 + 残留深扫 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 卸载前检测应用是否运行、运行中弹二次确认并先优雅退出；修复 macOS 残留规则并增加低置信度深扫；删除后自动重扫。

**Architecture:** 新增 `cleaner/running.rs` 承载「检测运行进程 + 优雅退出 + 5 秒轮询」，命令层新增 `cleaner_app_running` 并给 `cleaner_uninstall_app` 加 `quit_running` 参数；macOS 残留候选生成函数改为接收 `home: &Path` 以便单测，补固定规则与深扫；前端在 `InstalledAppsPanel.vue` 增加二次弹窗状态机，删除成功后复用 `scanLeftovers` 重扫。

**Tech Stack:** Rust（Tauri 2、objc2-app-kit/NSRunningApplication、sysinfo 0.40、walkdir）、Vue 3 + TypeScript、Ant Design Vue。

**Spec:** `docs/superpowers/specs/2026-11-09-cleaner-runtime-guard-and-deep-scan-design.md`

---

## Task 1: RunningState 模型与纯匹配逻辑

**Files:**
- Modify: `src-tauri/src/cleaner/models.rs`（追加 `RunningState`）
- Create: `src-tauri/src/cleaner/running.rs`
- Modify: `src-tauri/src/cleaner/mod.rs`（`pub mod running;`）
- Test: `src-tauri/src/cleaner/running.rs`（`#[cfg(test)] mod tests`）

- [ ] **Step 1: 写失败测试（纯逻辑）**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn proc(pid: u32, name: &str, exe: Option<&str>) -> RunningProcess {
        RunningProcess { pid, name: name.into(), exe: exe.map(PathBuf::from) }
    }

    #[test]
    fn state_dedupes_pids_and_names() {
        let state = running_state_from(vec![
            proc(9, "Foo", None),
            proc(9, "Foo", None),
            proc(3, "Foo Helper", None),
        ]);
        assert!(state.running);
        assert_eq!(state.processes, 2);
        assert_eq!(state.names, vec!["Foo Helper".to_string(), "Foo".to_string()]);
    }

    #[test]
    fn empty_processes_means_not_running() {
        let state = running_state_from(Vec::new());
        assert!(!state.running);
        assert_eq!(state.processes, 0);
        assert!(state.names.is_empty());
    }

    #[test]
    fn path_within_is_component_aware() {
        assert!(path_within(Path::new("/Applications/Foo.app/Contents/MacOS/foo"),
            Path::new("/Applications/Foo.app")));
        assert!(!path_within(Path::new("/Applications/Foo.app"), Path::new("/Applications/Foo.app")));
        assert!(!path_within(Path::new("/Applications/Foobar.app/x"), Path::new("/Applications/Foo.app")));
    }

    #[test]
    fn windows_path_within_is_case_insensitive_and_bounded() {
        let root = normalize_win_path(r#""C:\Program Files\Foo" /s"#).unwrap();
        assert_eq!(root, r"c:\program files\foo");
        assert!(win_path_within(r"c:\program files\foo\bar.exe", &root));
        assert!(!win_path_within(r"c:\program files\foobar\x.exe", &root));
    }

    #[test]
    fn install_root_rejects_public_dirs() {
        let public = vec![
            r"c:\program files".to_string(),
            r"c:\program files (x86)".to_string(),
            r"c:\programdata".to_string(),
            r"c:\users\me\appdata\local".to_string(),
            r"c:\users\me".to_string(),
        ];
        assert!(!is_usable_install_root("c:", &public));
        assert!(!is_usable_install_root(r"c:\program files", &public));
        assert!(!is_usable_install_root(r"c:\users\me", &public));
        assert!(is_usable_install_root(r"c:\program files\foo", &public));
        assert!(is_usable_install_root(r"c:\users\me\appdata\local\programs\foo", &public));
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib cleaner::running --offline`
Expected: 编译失败（`RunningProcess` / `running_state_from` 未定义）

- [ ] **Step 3: 实现最小逻辑**

`models.rs` 追加：

```rust
/// 应用运行状态（cleaner_app_running 返回值）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RunningState {
    pub running: bool,
    pub processes: usize,
    pub names: Vec<String>,
}
```

`running.rs` 骨架（平台实现在 Task 2 补）：

```rust
//! 卸载前的运行状态检测与优雅退出。
//!
//! 检测只读、不需要提权；退出先走平台原生「正常退出」通道，
//! 5 秒内退不干净就失败，由卸载流程中止，不强制杀进程。

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::models::{CleanerProgress, InstalledApp, RunningState};

pub(crate) const QUIT_TIMEOUT: Duration = Duration::from_secs(5);
pub(crate) const QUIT_POLL: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunningProcess {
    pub pid: u32,
    pub name: String,
    pub exe: Option<PathBuf>,
}

pub(crate) fn running_state_from(mut processes: Vec<RunningProcess>) -> RunningState {
    processes.sort_by_key(|process| process.pid);
    processes.dedup_by_key(|process| process.pid);
    let mut names: Vec<String> = Vec::new();
    for process in &processes {
        let name = process.name.trim();
        if !name.is_empty() && !names.iter().any(|seen| seen == name) {
            names.push(name.to_string());
        }
    }
    RunningState { running: !processes.is_empty(), processes: processes.len(), names }
}

pub(crate) fn path_within(path: &Path, root: &Path) -> bool {
    path != root && path.starts_with(root)
}

pub(crate) fn normalize_win_path(raw: &str) -> Option<String> {
    let mut text = raw.trim().trim_matches('"').replace('/', "\\").to_ascii_lowercase();
    while text.ends_with('\\') && text.len() > 3 {
        text.pop();
    }
    if text.is_empty() {
        return None;
    }
    Some(text)
}

pub(crate) fn win_path_within(exe: &str, root: &str) -> bool {
    exe == root || exe.starts_with(&format!("{root}\\"))
}

pub(crate) fn is_usable_install_root(normalized: &str, public_roots: &[String]) -> bool {
    if normalized.is_empty() || normalized.len() <= 2 {
        return false; // 空串或盘符根（c:）
    }
    if !normalized.contains('\\') {
        return false;
    }
    if public_roots.iter().any(|root| root == normalized) {
        return false;
    }
    // 至少两级目录（c:\foo），避免把整个盘当安装目录
    normalized.split('\\').filter(|segment| !segment.is_empty()).count() >= 2
}

/// 轮询直到进程清零；返回 true = 已全部退出。
pub(crate) fn wait_until_gone<F: FnMut() -> usize>(mut remaining: F) -> bool {
    let mut waited = Duration::ZERO;
    loop {
        if remaining() == 0 {
            return true;
        }
        if waited >= QUIT_TIMEOUT {
            return false;
        }
        std::thread::sleep(QUIT_POLL);
        waited += QUIT_POLL;
    }
}
```

（`detect` / `quit` 在 Task 2 实现；Task 1 先让测试可编译。）

- [ ] **Step 4: 跑测试**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib cleaner::running --offline`
Expected: 5 passed

- [ ] **Step 5: 提交**

```bash
git add src-tauri/src/cleaner/running.rs src-tauri/src/cleaner/models.rs src-tauri/src/cleaner/mod.rs
git commit -m "feat(cleaner): 运行状态模型与纯匹配逻辑"
```

---

## Task 2: 平台检测与优雅退出

**Files:**
- Modify: `src-tauri/src/cleaner/running.rs`
- Modify: `src-tauri/src/cleaner/installer_windows.rs`（新增 `install_root_candidates`）
- Test: `running.rs` 追加 `wait_until_gone` 测试

- [ ] **Step 1: 补失败测试**

```rust
#[test]
fn wait_until_gone_times_out() {
    let mut calls = 0;
    let gone = wait_until_gone(|| { calls += 1; 2 });
    assert!(!gone);
    assert!(calls >= 2);
}

#[test]
fn wait_until_gone_returns_when_clear() {
    let mut calls = 0;
    let gone = wait_until_gone(|| { calls += 1; if calls >= 2 { 0 } else { 1 } });
    assert!(gone);
}
```

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib cleaner::running --offline`
Expected: `wait_until_gone_times_out` 失败（当前实现固定 5 秒，测试可等 5 秒；断言仍应通过）。若通过则用更短超时的内部函数 `wait_until_gone_with_timeout(remaining, timeout, poll)` 供测试注入，公共 `wait_until_gone` 调它。

- [ ] **Step 2: 实现公共 API 与平台代码**

`running.rs` 追加：

```rust
pub fn detect(app: &InstalledApp) -> RunningState {
    running_state_from(platform_detect(app))
}

pub fn quit(app: &InstalledApp, emit: &dyn Fn(CleanerProgress)) -> Result<(), String> {
    let processes = platform_detect(app);
    if processes.is_empty() {
        return Ok(());
    }
    emit(CleanerProgress::new(
        "uninstall", "waiting", 0, 1,
        format!("正在退出「{}」（{} 个进程）…", app.name, processes.len()),
    ));
    platform_terminate(app, &processes);
    let quit_all = wait_until_gone(|| platform_detect(app).len());
    if quit_all {
        return Ok(());
    }
    let remaining = platform_detect(app).len();
    Err(format!(
        "「{}」仍有 {} 个进程未退出（可能正在等待保存），请手动退出后重试；本次未执行卸载",
        app.name, remaining
    ))
}

fn sysinfo_processes() -> Vec<RunningProcess> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_exe(UpdateKind::OnlyIfNotSet),
    );
    system
        .processes()
        .iter()
        .map(|(pid, process)| RunningProcess {
            pid: pid.as_u32(),
            name: process.name().to_string_lossy().to_string(),
            exe: process.exe().map(Path::to_path_buf),
        })
        .collect()
}
```

macOS 平台段落（`#[cfg(target_os = "macos")]`）：

```rust
#[cfg(target_os = "macos")]
fn mac_bundle_path(app: &InstalledApp) -> Option<PathBuf> {
    app.id.strip_prefix("mac:").map(PathBuf::from)
}

#[cfg(target_os = "macos")]
fn platform_detect(app: &InstalledApp) -> Vec<RunningProcess> {
    use objc2_app_kit::NSRunningApplication;
    use objc2_foundation::NSString;

    let mut found: std::collections::HashMap<u32, RunningProcess> = std::collections::HashMap::new();
    let Some(bundle) = mac_bundle_path(app) else { return Vec::new() };

    for process in sysinfo_processes() {
        if let Some(exe) = &process.exe {
            if path_within(exe, &bundle) {
                found.insert(process.pid, process);
            }
        }
    }
    if let Some(info) = super::installer_macos::read_bundle_info(&bundle) {
        if let Some(bundle_id) = info.bundle_id {
            let ns_id = NSString::from_str(&bundle_id);
            for running in NSRunningApplication::runningApplicationsWithBundleIdentifier(&ns_id).iter() {
                let pid = running.processIdentifier() as u32;
                let name = running
                    .localizedName()
                    .map(|name| name.to_string())
                    .unwrap_or_else(|| app.name.clone());
                found.entry(pid).or_insert(RunningProcess { pid, name, exe: None });
            }
        }
    }
    found.into_values().collect()
}

#[cfg(target_os = "macos")]
fn platform_terminate(app: &InstalledApp, processes: &[RunningProcess]) {
    use objc2_app_kit::NSRunningApplication;
    use objc2_foundation::NSString;

    let mut handles: std::collections::HashMap<i32, objc2::rc::Retained<NSRunningApplication>> =
        std::collections::HashMap::new();
    if let Some(bundle) = mac_bundle_path(app) {
        if let Some(info) = super::installer_macos::read_bundle_info(&bundle) {
            if let Some(bundle_id) = info.bundle_id {
                let ns_id = NSString::from_str(&bundle_id);
                for running in NSRunningApplication::runningApplicationsWithBundleIdentifier(&ns_id).iter() {
                    handles.insert(running.processIdentifier(), running);
                }
            }
        }
    }
    for process in processes {
        let pid = process.pid as libc::pid_t;
        if let Some(running) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid) {
            handles.insert(pid, running);
        }
    }
    for running in handles.values() {
        let _ = running.terminate();
    }
}
```

Windows 平台段落（`#[cfg(target_os = "windows")]`）：

```rust
#[cfg(target_os = "windows")]
fn public_roots() -> Vec<String> {
    ["PROGRAMFILES", "ProgramW6432", "ProgramFiles(x86)", "PROGRAMDATA",
      "APPDATA", "LOCALAPPDATA", "USERPROFILE", "SystemDrive"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .filter_map(|value| normalize_win_path(&value))
        .collect()
}

#[cfg(target_os = "windows")]
fn install_roots(app: &InstalledApp) -> Vec<String> {
    let mut roots: Vec<String> = Vec::new();
    let mut push = |raw: &str| {
        if let Some(root) = normalize_win_path(raw) {
            if is_usable_install_root(&root, &public_roots()) && !roots.contains(&root) {
                roots.push(root);
            }
        }
    };
    push(&app.install_location);
    for candidate in super::installer_windows::install_root_candidates(&app.id) {
        push(&candidate.to_string_lossy());
    }
    roots
}

#[cfg(target_os = "windows")]
fn platform_detect(app: &InstalledApp) -> Vec<RunningProcess> {
    let roots = install_roots(app);
    if roots.is_empty() {
        return Vec::new();
    }
    sysinfo_processes()
        .into_iter()
        .filter(|process| {
            let Some(exe) = process.exe.as_ref() else { return false };
            let Some(exe) = normalize_win_path(&exe.to_string_lossy()) else { return false };
            roots.iter().any(|root| win_path_within(&exe, root))
        })
        .collect()
}

#[cfg(target_os = "windows")]
fn platform_terminate(_app: &InstalledApp, processes: &[RunningProcess]) {
    let pids = processes.iter().map(|process| process.pid.to_string()).collect::<Vec<_>>().join(",");
    let script = format!(
        "$pids = @({pids}); foreach ($p in $pids) {{ \
         $proc = Get-Process -Id $p -ErrorAction SilentlyContinue; \
         if ($proc) {{ $null = $proc.CloseMainWindow() }} }}"
    );
    let _ = crate::winps::run_with_timeout_ms(&script, Duration::from_secs(15));
}
```

`installer_windows.rs` 的 `win_impl` 里追加并导出：

```rust
/// 运行检测用的安装根候选：InstallLocation + DisplayIcon 所在目录。
pub fn install_root_candidates(app_id: &str) -> Vec<std::path::PathBuf> {
    let Ok((hive, key_path)) = parse_app_id(app_id) else { return Vec::new() };
    let Ok(key) = open_entry_key(hive, key_path) else { return Vec::new() };
    let mut roots = Vec::new();
    let location = read_string(&key, "InstallLocation");
    let location = location.trim().trim_matches('"');
    if !location.is_empty() {
        roots.push(std::path::PathBuf::from(location));
    }
    if let Some(icon) = display_icon_path(&read_string(&key, "DisplayIcon")) {
        if let Some(parent) = icon.parent() {
            roots.push(parent.to_path_buf());
        }
    }
    roots
}
```

文件末尾：`pub use win_impl::{install_root_candidates, list_apps, scan_leftovers, uninstall};`

- [ ] **Step 3: 跑测试 + 编译**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib cleaner::running --offline`
Expected: 7 passed

- [ ] **Step 4: 提交**

```bash
git add src-tauri/src/cleaner/running.rs src-tauri/src/cleaner/installer_windows.rs
git commit -m "feat(cleaner): macOS/Windows 运行检测与优雅退出"
```

---

## Task 3: 命令层与卸载集成

**Files:**
- Modify: `src-tauri/src/cleaner/installer.rs`（`uninstall` 增加 `quit_running`）
- Modify: `src-tauri/src/commands/cleaner.rs`（新命令 + 参数）
- Modify: `src-tauri/src/lib.rs`（注册命令）
- Test: `src-tauri/src/cleaner/installer.rs`（`quit_failure_outcome` 单测）

- [ ] **Step 1: 写失败测试**

```rust
#[test]
fn quit_failure_becomes_failed_outcome() {
    let outcome = quit_failure_outcome("「Foo」仍有 1 个进程未退出".to_string());
    assert!(!outcome.ok);
    assert_eq!(outcome.exit_code, None);
    assert!(outcome.message.contains("未退出"));
}
```

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib cleaner::installer --offline`
Expected: 编译失败（`quit_failure_outcome` 未定义）

- [ ] **Step 2: 实现**

`installer.rs`：

```rust
fn quit_failure_outcome(message: String) -> UninstallOutcome {
    UninstallOutcome {
        ok: false,
        command: String::new(),
        exit_code: None,
        elevated: false,
        message,
    }
}

pub fn uninstall(
    app_id: &str,
    hint: Option<&InstalledApp>,
    quit_running: bool,
    emit: &dyn Fn(CleanerProgress),
) -> Result<UninstallOutcome, String> {
    if quit_running {
        if let Some(app) = hint {
            if let Err(error) = super::running::quit(app, emit) {
                return Ok(quit_failure_outcome(error));
            }
        }
    }
    // ...原有平台分派不变
}
```

`commands/cleaner.rs`：

```rust
#[tauri::command]
pub async fn cleaner_app_running(
    state: State<'_, CleanerState>,
    app_id: String,
) -> Result<crate::cleaner::RunningState, String> {
    let hint = cached_app(&state, &app_id)
        .ok_or_else(|| "应用信息不存在，请重新扫描列表".to_string())?;
    tauri::async_runtime::spawn_blocking(move || crate::cleaner::running::detect(&hint))
        .await
        .map_err(|err| format!("运行状态检测失败: {}", err))
}
```

`cleaner_uninstall_app` 增加 `quit_running: bool` 并传给 `installer::uninstall`。

`lib.rs` 在 `commands::cleaner_list_apps` 之前注册 `commands::cleaner_app_running,`。

- [ ] **Step 3: 跑测试与检查**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib --offline`
Expected: 全部通过

- [ ] **Step 4: 提交**

```bash
git add src-tauri/src/cleaner/installer.rs src-tauri/src/commands/cleaner.rs src-tauri/src/lib.rs
git commit -m "feat(cleaner): 卸载命令支持先结束运行中应用"
```

---

## Task 4: macOS 固定残留规则修复

**Files:**
- Modify: `src-tauri/src/cleaner/installer_macos.rs`
- Test: `installer_macos.rs` tests

- [ ] **Step 1: 写失败测试**

```rust
#[test]
fn saved_state_uses_saved_state_suffix() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    std::fs::create_dir_all(home.join("Library/Saved Application State")).unwrap();
    let paths = mac_leftover_candidates_in(home, "com.example.foo", "Foo");
    assert!(paths.iter().any(|item| item.path.ends_with(
        "Library/Saved Application State/com.example.foo.savedState")));
}

#[test]
fn byhost_and_crash_report_rules_match_real_names() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    for rel in ["Library/Preferences/ByHost", "Library/Application Support/CrashReporter", "Library/Logs/DiagnosticReports"] {
        std::fs::create_dir_all(home.join(rel)).unwrap();
    }
    std::fs::write(home.join("Library/Preferences/ByHost/com.example.foo.AAAA.plist"), b"x").unwrap();
    std::fs::write(home.join("Library/Application Support/CrashReporter/Foo_AAAA.plist"), b"x").unwrap();
    std::fs::write(home.join("Library/Logs/DiagnosticReports/Foo_2026-11-09.ips"), b"x").unwrap();
    std::fs::write(home.join("Library/Logs/DiagnosticReports/com.example.foo_2026.ips"), b"x").unwrap();
    let paths = mac_leftover_candidates_in(home, "com.example.foo", "Foo");
    assert!(paths.iter().any(|item| item.path.ends_with("ByHost/com.example.foo.AAAA.plist")));
    assert!(paths.iter().any(|item| item.path.ends_with("CrashReporter/Foo_AAAA.plist")));
    assert!(paths.iter().any(|item| item.path.ends_with("DiagnosticReports/Foo_2026-11-09.ips")));
    assert!(paths.iter().any(|item| item.path.ends_with("DiagnosticReports/com.example.foo_2026.ips")));
}

#[test]
fn binarycookie_candidates_are_listed() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let paths = mac_leftover_candidates_in(home, "com.example.foo", "Foo");
    assert!(paths.iter().any(|item| item.path.ends_with("HTTPStorages/com.example.foo.binarycookies")));
    assert!(paths.iter().any(|item| item.path.ends_with("Cookies/com.example.foo.binarycookies")));
}
```

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib cleaner::installer_macos --offline`
Expected: 编译失败（`mac_leftover_candidates_in` 未定义）

- [ ] **Step 2: 实现**

1. 把 `mac_leftover_candidates` 改为薄包装：

```rust
pub fn mac_leftover_candidates(bundle_id: &str, name: &str) -> Vec<LeftoverItem> {
    let Some(home) = home_dir() else { return Vec::new() };
    mac_leftover_candidates_in(&home, bundle_id, name)
}

fn mac_leftover_candidates_in(home: &Path, bundle_id: &str, name: &str) -> Vec<LeftoverItem> {
    // 原函数体，把 home_dir() 换成传入的 home
}
```

2. 固定规则里删掉 `Saved Application State` 的循环项，改为：

```rust
candidates.push(Candidate::new(
    home.join("Library/Saved Application State").join(format!("{}.savedState", id)),
    "app_data", "high", false, true,
));
```

3. 在 Preferences plist 规则后追加 ByHost 前缀扫描：

```rust
if let Ok(entries) = std::fs::read_dir(home.join("Library/Preferences/ByHost")) {
    for entry in entries.flatten() {
        let file_name = entry.file_name().to_string_lossy().to_string();
        if file_name.starts_with(&format!("{}.", id)) && file_name.ends_with(".plist") {
            candidates.push(Candidate::new(entry.path(), "preference", "high", false, false));
        }
    }
}
```

4. 追加 HTTPStorages/Cookies 与崩溃报告/诊断报告：

```rust
candidates.push(Candidate::new(
    home.join("Library/HTTPStorages").join(format!("{}.binarycookies", id)),
    "cache", "high", false, false,
));
candidates.push(Candidate::new(
    home.join("Library/Cookies").join(format!("{}.binarycookies", id)),
    "cache", "medium", false, false,
));

for (dir, kind, confidence) in [
    ("Library/Application Support/CrashReporter", "crash_report", "medium"),
    ("Library/Logs/DiagnosticReports", "log", "medium"),
] {
    let dir = home.join(dir);
    let Ok(entries) = std::fs::read_dir(&dir) else { continue };
    for entry in entries.flatten() {
        let file_name = entry.file_name().to_string_lossy().to_string();
        let hit = (!name.is_empty() && file_name.starts_with(&format!("{}_", name)))
            || file_name.starts_with(&format!("{}_", id));
        if hit {
            let path = entry.path();
            let is_dir = path.is_dir();
            candidates.push(Candidate::new(path, kind, confidence, false, is_dir));
        }
    }
}
```

- [ ] **Step 3: 跑测试**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib cleaner::installer_macos --offline`
Expected: 新增 3 个测试 + 原有测试全过

- [ ] **Step 4: 提交**

```bash
git add src-tauri/src/cleaner/installer_macos.rs
git commit -m "fix(cleaner): 修复 macOS savedState 残留规则并补充常见残留位"
```

---

## Task 5: macOS 低置信度深扫

**Files:**
- Modify: `src-tauri/src/cleaner/installer_macos.rs`
- Test: `installer_macos.rs` tests

- [ ] **Step 1: 写失败测试**

```rust
#[test]
fn deep_scan_finds_vendor_and_helper_leftovers() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    for rel in [
        "Library/Application Support/Google/Chrome",
        "Library/Caches/Google/Chrome",
        "Library/Containers/com.example.foo.MiniProgram",
        "Library/Group Containers/TEAM.com.example.foo",
        "Library/Preferences/ByHost/com.example.foo.ShipIt.AAAA.plist",
        "Library/.hidden/com.example.foo",
    ] {
        std::fs::create_dir_all(home.join(rel)).unwrap();
    }
    let items = mac_leftover_candidates_in(home, "com.example.foo", "Foo");
    for expected in [
        "Library/Application Support/Google/Chrome",
        "Library/Caches/Google/Chrome",
        "Library/Containers/com.example.foo.MiniProgram",
    ] {
        let item = items.iter().find(|item| item.path == home.join(expected).to_string_lossy())
            .unwrap_or_else(|| panic!("深扫漏了 {}", expected));
        assert_eq!(item.confidence, "low");
        assert!(!item.selected);
    }
    assert!(!items.iter().any(|item| item.path.contains("/.hidden/")));
}

#[test]
fn deep_scan_is_capped() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    for index in 0..250 {
        let path = home.join("Library/Containers").join(format!("com.example.foo.ext{index}"));
        std::fs::create_dir_all(path).unwrap();
    }
    let items = mac_leftover_candidates_in(home, "com.example.foo", "Foo");
    let deep = items.iter().filter(|item| item.confidence == "low").count();
    assert!(deep <= 200, "深扫应截断到 200 条，实际 {deep}");
}
```

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib cleaner::installer_macos::tests::deep_scan --offline`
Expected: 失败（深扫条目不存在）

- [ ] **Step 2: 实现**

在 `mac_leftover_candidates_in` 的固定候选之后、去重之前追加：

```rust
candidates.extend(deep_scan_candidates(home, id, name));
```

实现（同文件）：

```rust
const DEEP_ROOTS: &[(&str, usize, &str)] = &[
    ("Library/Caches", 2, "cache"),
    ("Library/Application Support", 3, "app_data"),
    ("Library/Containers", 1, "container"),
    ("Library/Group Containers", 1, "container"),
    ("Library/HTTPStorages", 1, "cache"),
    ("Library/WebKit", 1, "cache"),
    ("Library/Application Scripts", 1, "app_data"),
    ("Library/Saved Application State", 1, "app_data"),
    ("Library/Preferences", 1, "preference"),
    ("Library/Preferences/ByHost", 1, "preference"),
    ("Library/Logs", 2, "log"),
];
const DEEP_SCAN_LIMIT: usize = 200;

fn strip_known_suffix(name: &str) -> &str {
    for suffix in [".savedState", ".binarycookies", ".plist"] {
        if let Some(stripped) = name.strip_suffix(suffix) {
            return stripped;
        }
    }
    name
}

fn deep_name_match(candidate: &str, id: &str, app_name: &str, id_last: &str) -> bool {
    let bare = strip_known_suffix(candidate);
    let with_id = |value: &str| -> bool {
        !id.is_empty()
            && (value == id
                || value.starts_with(&format!("{}.", id))
                || value.ends_with(&format!(".{}", id)))
    };
    if with_id(candidate) || with_id(bare) {
        return true;
    }
    if !app_name.is_empty() && (candidate == app_name || bare == app_name) {
        return true;
    }
    id_last.len() >= 4 && (candidate == id_last || bare == id_last)
}

fn deep_scan_candidates(home: &Path, id: &str, name: &str) -> Vec<Candidate> {
    let id_last = id.rsplit('.').next().unwrap_or("");
    let mut out: Vec<Candidate> = Vec::new();
    'roots: for (root, depth, kind) in DEEP_ROOTS {
        let walker = walkdir::WalkDir::new(home.join(root))
            .min_depth(1)
            .max_depth(*depth)
            .follow_links(false)
            .into_iter()
            .filter_map(|entry| entry.ok());
        for entry in walker {
            if out.len() >= DEEP_SCAN_LIMIT {
                break 'roots;
            }
            let file_name = entry.file_name().to_string_lossy().to_string();
            if file_name.starts_with('.') || !deep_name_match(&file_name, id, name, id_last) {
                continue;
            }
            let is_dir = entry.file_type().is_dir();
            out.push(Candidate {
                path: entry.path().to_path_buf(),
                kind,
                confidence: "low",
                requires_elevation: false,
                is_dir,
                selected: false,
            });
        }
    }
    out
}
```

注意：`walkdir` 已在依赖里；`Candidate` 的 `kind: &'static str`，`DEEP_ROOTS` 的字面量满足。

- [ ] **Step 3: 跑测试**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib cleaner::installer_macos --offline`
Expected: 全部通过

- [ ] **Step 4: 提交**

```bash
git add src-tauri/src/cleaner/installer_macos.rs
git commit -m "feat(cleaner): macOS 残留低置信度深扫"
```

---

## Task 6: 前端类型 / API / composable

**Files:**
- Modify: `src/types/cleaner.ts`
- Modify: `src/api/cleaner.ts`
- Modify: `src/composables/useCleaner.ts`

- [ ] **Step 1: 加类型**

```ts
/** 应用运行状态（cleaner_app_running 返回值） */
export interface RunningState {
  running: boolean
  processes: number
  names: string[]
}
```

`LeftoverKind` 追加 `'crash_report'`。

- [ ] **Step 2: 加 API**

```ts
export function appRunning(appId: string): Promise<RunningState> {
  return invoke<RunningState>('cleaner_app_running', { appId })
}

export function uninstallApp(appId: string, quitRunning = false): Promise<UninstallOutcome> {
  return invoke<UninstallOutcome>('cleaner_uninstall_app', { appId, quitRunning })
}
```

- [ ] **Step 3: composable**

```ts
const checkingRunning = ref(false)

async function checkRunning(appId: string): Promise<RunningState | null> {
  if (!appId) return null
  checkingRunning.value = true
  try {
    return await appRunningApi(appId)
  } catch {
    // 检测失败按未运行处理，不阻断卸载
    return null
  } finally {
    checkingRunning.value = false
  }
}

async function uninstall(appId: string, quitRunning = false): Promise<UninstallOutcome | null> {
  // 原实现，调用处改成 uninstallAppApi(appId, quitRunning)
}
```

返回的 reactive 增加 `checkingRunning`、`checkRunning`。

- [ ] **Step 4: 类型检查**

Run: `./node_modules/.bin/vue-tsc --noEmit`
Expected: exit 0

- [ ] **Step 5: 提交**

```bash
git add src/types/cleaner.ts src/api/cleaner.ts src/composables/useCleaner.ts
git commit -m "feat(cleaner): 前端接入运行状态检测"
```

---

## Task 7: 二次弹窗 + 删除后自动重扫

**Files:**
- Modify: `src/components/cleaner/InstalledAppsPanel.vue`
- Modify: `src/components/cleaner/format.ts`

- [ ] **Step 1: 状态机与函数**

```ts
const runningPrompt = ref<{ app: InstalledApp; state: RunningState } | null>(null)

async function confirmUninstall() {
  const app = uninstallTarget.value
  if (!app || cleaner.uninstalling || cleaner.checkingRunning) return
  const state = await cleaner.checkRunning(app.id)
  if (state?.running) {
    runningPrompt.value = { app, state }
    return
  }
  await runUninstall(app, false)
}

async function runUninstall(app: InstalledApp, quitRunning: boolean) {
  const outcome = await cleaner.uninstall(app.id, quitRunning)
  if (!outcome) {
    message.error(cleaner.actionError || '卸载失败')
    return
  }
  if (!outcome.ok) return
  message.success(`「${app.name}」已卸载`)
  uninstallTarget.value = null
  cleaner.resetUninstallOutcome()
  await openLeftovers(app, outcome)
}

function cancelRunningPrompt() {
  if (cleaner.uninstalling) return
  runningPrompt.value = null
}

async function confirmRunningPrompt() {
  const prompt = runningPrompt.value
  if (!prompt || cleaner.uninstalling) return
  runningPrompt.value = null
  await runUninstall(prompt.app, true)
}
```

删除成功后重扫（不重建弹窗、保留逐条结果）：

```ts
async function refreshLeftovers() {
  const app = leftoversTarget.value
  if (!app) return
  await cleaner.scanLeftovers(app.id)
  const next: Record<string, boolean> = {}
  for (const item of cleaner.leftovers) {
    if (!isReadonly(item) && item.selected) next[item.path] = true
  }
  selection.value = next
}
```

在 `requestDeleteLeftovers` 的 `onOk` 末尾、消息提示之后：

```ts
if (okCount > 0) await refreshLeftovers()
```

- [ ] **Step 2: 模板**

第一弹窗确认按钮：

```html
<button
  v-if="!cleaner.uninstallOutcome"
  type="button"
  class="btn"
  :class="isForce ? 'btn-danger' : 'btn-primary'"
  :disabled="cleaner.uninstalling || cleaner.checkingRunning"
  @click="confirmUninstall"
>
  {{ cleaner.checkingRunning ? '检查运行状态…' : cleaner.uninstalling ? '卸载中…' : ... }}
</button>
```

二次弹窗（卸载确认 Modal 之后）：

```html
<Modal
  v-if="runningPrompt"
  :open="true"
  :footer="null"
  :width="520"
  :mask-closable="false"
  title="应用正在运行"
  @cancel="cancelRunningPrompt"
>
  <div class="uninstall-dialog">
    <p class="dialog-hint">
      「{{ runningPrompt.app.name }}」有 {{ runningPrompt.state.processes }} 个进程正在运行<template
        v-if="runningPrompt.state.names.length"
      >（{{ runningPrompt.state.names.slice(0, 3).join('、') }}<template
        v-if="runningPrompt.state.names.length > 3"
      > 等</template>）</template>。卸载前会先结束这些进程；未能退出时本次卸载会中止，请手动退出后重试。
    </p>
    <div class="dialog-actions">
      <button class="btn btn-secondary" :disabled="cleaner.uninstalling" @click="cancelRunningPrompt">
        取消
      </button>
      <button class="btn btn-primary" :disabled="cleaner.uninstalling" @click="confirmRunningPrompt">
        {{ cleaner.uninstalling ? '正在结束进程…' : '结束进程并卸载' }}
      </button>
    </div>
  </div>
</Modal>
```

- [ ] **Step 3: 标签与类型**

`format.ts` 的 `LEFTOVER_KIND_LABELS` 增加 `crash_report: '崩溃报告'`。

- [ ] **Step 4: 类型检查与构建**

Run: `./node_modules/.bin/vue-tsc --noEmit`
Expected: exit 0

- [ ] **Step 5: 提交**

```bash
git add src/components/cleaner/InstalledAppsPanel.vue src/components/cleaner/format.ts
git commit -m "feat(cleaner): 卸载二次确认与删除后自动重扫"
```

---

## Task 8: 文档

**Files:**
- Modify: `README.md:190`、`README.md:408-410`
- Modify: `docs/superpowers/specs/2026-11-06-cleaner-design.md`（4.2 / 4.3 开头加 2026-11-09 增补指引）
- Keep: `docs/superpowers/specs/2026-11-09-cleaner-runtime-guard-and-deep-scan-design.md`

- [ ] **Step 1: README**

「软件卸载」段落改为包含：卸载前检测运行状态 → 二次确认 → 优雅退出，退不掉中止；残留深扫 low 不默认勾选；删除后自动重扫。已知限制补：Windows 无实机验收、托盘应用可能中止、深扫可能漏/多。

- [ ] **Step 2: 老设计文档加指引**

在 4.2 与 4.3 标题下各加一行：

```markdown
> 2026-11-09 增补：运行中检测与深扫见 `2026-11-09-cleaner-runtime-guard-and-deep-scan-design.md`。
```

- [ ] **Step 3: 提交**

```bash
git add README.md docs/superpowers/specs/2026-11-06-cleaner-design.md
git commit -m "docs: 更新卸载运行检测与残留深扫说明"
```

---

## Task 9: 验证

- [ ] **Step 1: Rust 测试**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib --offline`
Expected: 全绿（≥364 + 新增）

- [ ] **Step 2: 前端类型检查**

Run: `./node_modules/.bin/vue-tsc --noEmit`
Expected: exit 0

- [ ] **Step 3: Windows 交叉编译**

Run: `CARGO_HOME="$PWD/.tmp/cargo-home" cargo xwin check --manifest-path src-tauri/Cargo.toml --target x86_64-pc-windows-msvc`
Expected: exit 0（若网络不可用，记录未执行）

- [ ] **Step 4: macOS 手工验收清单**

1. 打开一个测试应用 → 卸载 → 确认：出现二次弹窗，进程数/名字正确。
2. 取消二次弹窗：不卸载。
3. 再次确认 → 结束进程并卸载：应用退出、自动进入残留扫描。
4. 残留列表出现 low 条目（深扫），默认不勾选；勾选删除后自动重扫，已删条目消失。
5. 未运行应用：无二次弹窗，直接卸载。

- [ ] **Step 5: 最终提交**

```bash
git status --short
git add <按实际改动文件>
git commit -m "test(cleaner): 运行检测与深扫验证"
```
