# ws-workbench 资源管理 + DeepSeek 点击流程 改造 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** ws-workbench 设置新增「资源」栏目（远端检查/下载/安装/卸载 DSH 工作区插件），点「打开方式 → DeepSeek」改为：先检测 DSH 与插件，缺插件或版本旧时引导去设置，不再内置/自动安装。

**Architecture:** 新增 Rust 模块 `commands/resources.rs` 承载资源模型、GitHub Release 版本检查（系统 `curl` 子进程，无新 Cargo 依赖）、zip 下载/解压；DSH profile 的文件写入与清单维护继续留在 `commands/deepseek.rs`（复用现有 `merge_profile_manifest` / `manifest_contains_plugin`），但输入从「`include_str!` 内置文件」改为「zip 解出的文件列表」。前端新增 `ResourcePanel.vue`，`OpenWithButton.vue` 按状态分支，`App.vue` 处理「去设置」事件。

**Tech Stack:** Rust（serde_json / zip / tempfile，均已有依赖）、Tauri 2、Vue 3 + ant-design-vue。

---

## 前置

- **依赖计划**：`2026-11-10-dsh-plugin-repo-release.md` 的 Task 3 必须先完成（GitHub Release `v0.1.0` 的 zip 可下载），否则删掉内置插件后用户没地方装。
- 设计文档：`docs/superpowers/specs/2026-11-10-dsh-resource-manager-design.md`。
- 网络事实：本机 `api.github.com` 可达；Release 资产要经
  `https://api.github.com/repos/<repo>/releases/assets/<id>`（302 到
  `release-assets.githubusercontent.com`，curl `-L` 跟随，`Accept: application/octet-stream`）。
- DSH profile 清单的 bundle 路径是 **`dsh.profile.bundles`**（不是 `dsh.bundles`），
  依赖项是 `dependencies.dsh-open-workspace = "link:../../plugins/dsh-open-workspace"`。
- **未提交改动**：用户工作区里 `src-tauri/Cargo.toml`、`src-tauri/build.rs`、`src-tauri/src/lib.rs`、
  `src/components/OpenWithButton.vue`、`src/components/settings/SettingsPanel.vue`、`README.md`
  等含用户未提交 WIP。提交这些文件时必须用「HEAD blob + 自己的改动」的方式（见文末「混合文件提交法」），
  绝不 `git add` 整个文件，也绝不 revert 用户改动。
- 插件源码删除（`dsh-plugin/`）依赖前置计划完成；本计划 Task 7 才删。

## File Structure

| 文件 | 责任 |
| --- | --- |
| `src-tauri/src/commands/resources.rs`（新建） | 资源模型、版本比较、curl 远端检查、zip 下载/解压、资源命令 |
| `src-tauri/src/commands/mod.rs` | 注册 `resources` 模块 |
| `src-tauri/src/commands/deepseek.rs`（改） | 删内置插件/指纹；新增按文件列表安装/卸载；保留 app/profile/open；加 `deepseek_plugin_update` |
| `src-tauri/src/lib.rs`（改，混合文件） | 注册新命令、去掉 `install_deepseek_plugin` |
| `src-tauri/build.rs`（改，混合文件） | 删 `dsh-plugin` 的 `rerun-if-changed` |
| `src-tauri/tests/fixtures/dsh-plugin/`（新建） | 测试用最小插件文件（打 zip 用） |
| `src/api/system.ts`（改） | 资源 API + 类型；去掉 `installDeepseekPlugin` |
| `src/components/settings/ResourcePanel.vue`（新建） | 设置-资源 UI |
| `src/components/settings/SettingsPanel.vue`（改，混合文件） | 末尾插入「资源」卡片 |
| `src/components/OpenWithButton.vue`（改，混合文件） | 点击 DeepSeek 新流程 |
| `src/App.vue`（改） | 监听 `ws-open-settings` 切到设置页 |
| `scripts/dsh-plugin-install.sh`（改） | 改为从本地 zip 安装（开发/离线用） |
| `README.md`（改，混合文件） | 文档同步 |

---

### Task 1: 资源模型 + 版本比较（TDD）

**Files:**
- Create: `src-tauri/src/commands/resources.rs`
- Modify: `src-tauri/src/commands/mod.rs`

- [ ] **Step 1: 写失败测试**

`resources.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_newer_compares_numeric_segments() {
        assert!(version_newer("0.2.0", "0.1.0"));
        assert!(version_newer("v0.2.0", "0.1.9"));
        assert!(version_newer("1.0", "0.9.9"));
        assert!(!version_newer("0.1.0", "0.1.0"));
        assert!(!version_newer("0.1.0", "0.2.0"));
        assert!(!version_newer("0.1.0-beta.1", "0.1.0")); // 预发布截掉
        assert!(!version_newer("abc", "0.1.0")); // 非法输入按 0 处理
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cd src-tauri && cargo test resources::tests::version_newer_compares_numeric_segments`

Expected: 编译失败 `cannot find function version_newer`。

- [ ] **Step 3: 写最小实现**

```rust
//! 「设置 → 资源」：可下载资源的远端版本检查、下载、解压；DSH 文件安装复用 deepseek 模块。
//! 本期实现 DeepSeek 工作区插件；Chrome 插件只占位。

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::commands::deepseek::{dsh_home, dsh_running};

pub const DSH_PLUGIN_ID: &str = "dsh-open-workspace";
pub const DSH_PLUGIN_REPO: &str = "fengwenshan/dsh-open-workspace";
const DEFAULT_API_BASE: &str = "https://api.github.com";
const REMOTE_TTL: Duration = Duration::from_secs(5 * 60);

/// 版本号按 `.` 拆成数字段比较；`v` 前缀与 `-` 之后的预发布都忽略。
pub(crate) fn version_segments(version: &str) -> Vec<u64> {
    let core = version
        .trim()
        .trim_start_matches('v')
        .split('-')
        .next()
        .unwrap_or("");
    core.split('.')
        .map(|part| part.parse::<u64>().unwrap_or(0))
        .collect()
}

/// 远端版本是否比本地新（版本回退不提示更新）。
pub(crate) fn version_newer(remote: &str, local: &str) -> bool {
    let remote = version_segments(remote);
    let local = version_segments(local);
    let len = remote.len().max(local.len());
    for index in 0..len {
        let a = remote.get(index).copied().unwrap_or(0);
        let b = local.get(index).copied().unwrap_or(0);
        if a != b {
            return a > b;
        }
    }
    false
}
```

`src-tauri/src/commands/mod.rs` 加：

```rust
// 「设置 → 资源」：DSH 插件等资源的检查/下载/安装/卸载
mod resources;
```

并在 `pub use deepseek::*;` 后加 `pub use resources::*;`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cd src-tauri && cargo test resources::tests::version_newer_compares_numeric_segments`

Expected: `test result: ok. 1 passed`。

- [ ] **Step 5: Commit**

```bash
cd /Users/fengwenshan/wenshan/demo/layui-admin-build
git add src-tauri/src/commands/resources.rs src-tauri/src/commands/mod.rs
git commit -m "feat(resources): 资源模型与版本比较"
```

---

### Task 2: GitHub Release 远端检查（curl + 缓存 + 假服务器测试）

**Files:**
- Modify: `src-tauri/src/commands/resources.rs`

- [ ] **Step 1: 写失败测试**

`resources.rs` 的 `mod tests` 里加：

```rust
    #[test]
    fn fetch_release_parses_tag_and_asset() {
        let release: ReleaseDto = serde_json::from_str(
            r#"{"tag_name":"v0.2.0","assets":[{"name":"dsh-open-workspace-0.2.0.zip","url":"https://api.github.com/repos/x/y/releases/assets/1"}]}"#,
        )
        .unwrap();
        assert_eq!(release.tag_name, "v0.2.0");
        assert_eq!(
            asset_for(&release, "0.2.0").unwrap().name,
            "dsh-open-workspace-0.2.0.zip"
        );
        assert!(asset_for(&release, "0.1.0").is_none());
    }

    #[test]
    fn remote_check_uses_fake_server() {
        let (base, _handle) = spawn_fake_github("0.2.0");
        let release = fetch_release(&base).unwrap();
        assert_eq!(release.tag_name, "v0.2.0");
        assert_eq!(latest_plugin_version_from(&base).unwrap(), "0.2.0");
    }
```

测试工具放 `mod tests` 内：

```rust
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread::JoinHandle;

    /// 极简假 GitHub：`/repos/fengwenshan/dsh-open-workspace/releases/latest` 返回固定 JSON。
    fn spawn_fake_github(version: &str) -> (String, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let base = format!("http://127.0.0.1:{port}");
        let body = format!(
            r#"{{"tag_name":"v{version}","assets":[{{"name":"dsh-open-workspace-{version}.zip","url":"{base}/asset.zip"}}]}}"#
        );
        let handle = std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut request = [0u8; 2048];
                let _ = stream.read(&mut request);
                let request = String::from_utf8_lossy(&request);
                let response = if request.starts_with("GET /repos/") {
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(),
                        body
                    )
                } else {
                    "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_string()
                };
                let _ = stream.write_all(response.as_bytes());
            }
        });
        (base, handle)
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cd src-tauri && cargo test resources::tests::`

Expected: 编译失败（缺 `ReleaseDto` / `asset_for` / `fetch_release` / `latest_plugin_version_from`）。

- [ ] **Step 3: 实现远端检查**

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct ReleaseDto {
    pub tag_name: String,
    #[serde(default)]
    pub assets: Vec<AssetDto>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssetDto {
    pub name: String,
    pub url: String,
}

pub(crate) fn asset_for<'a>(release: &'a ReleaseDto, version: &str) -> Option<&'a AssetDto> {
    let wanted = format!("{DSH_PLUGIN_ID}-{version}.zip");
    release.assets.iter().find(|asset| asset.name == wanted)
}

type RemoteResult = Result<ReleaseDto, String>;
static REMOTE_CACHE: Mutex<Option<(Instant, RemoteResult)>> = Mutex::new(None);

fn api_base() -> String {
    std::env::var("WS_RESOURCE_API_BASE").unwrap_or_else(|_| DEFAULT_API_BASE.to_string())
}

/// 用系统 curl 取文本；macOS / Windows 10+ 自带 curl。
fn curl_text(url: &str) -> Result<String, String> {
    let output = std::process::Command::new("curl")
        .args([
            "-sSfL",
            "--max-time",
            "8",
            "-H",
            "Accept: application/vnd.github+json",
            url,
        ])
        .output()
        .map_err(|err| format!("系统缺少 curl 或无法执行: {err}"))?;
    if !output.status.success() {
        return Err(format!("请求失败: {url}"));
    }
    String::from_utf8(output.stdout).map_err(|err| format!("响应不是 UTF-8: {err}"))
}

pub(crate) fn fetch_release(base: &str) -> Result<ReleaseDto, String> {
    let url = format!("{base}/repos/{DSH_PLUGIN_REPO}/releases/latest");
    let body = curl_text(&url)?;
    serde_json::from_str(&body).map_err(|err| format!("解析 Release 失败: {err}"))
}

pub(crate) fn latest_plugin_version_from(base: &str) -> Result<String, String> {
    let release = fetch_release(base)?;
    let version = release.tag_name.trim_start_matches('v').to_string();
    if version.is_empty() {
        return Err("Release 没有 tag".to_string());
    }
    Ok(version)
}

/// 取最新 Release（5 分钟缓存；force=true 忽略缓存）。
pub(crate) fn latest_release(force: bool) -> Result<ReleaseDto, String> {
    let mut guard = REMOTE_CACHE.lock().unwrap();
    if !force {
        if let Some((at, cached)) = guard.as_ref() {
            if at.elapsed() < REMOTE_TTL {
                return cached.clone();
            }
        }
    }
    let result = fetch_release(&api_base());
    *guard = Some((Instant::now(), result.clone()));
    result
}

/// 最新版本号（从缓存/远端 Release tag 取）。
pub(crate) fn latest_plugin_version(force: bool) -> Result<String, String> {
    latest_release(force).map(|release| release.tag_name.trim_start_matches('v').to_string())
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cd src-tauri && cargo test resources::tests::`

Expected: 3 passed。

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/commands/resources.rs
git commit -m "feat(resources): GitHub Release 版本检查（curl + 缓存）"
```

---

### Task 3: zip 解压 + DSH 安装/卸载（TDD）

**Files:**
- Modify: `src-tauri/src/commands/resources.rs`
- Modify: `src-tauri/src/commands/deepseek.rs`
- Create: `src-tauri/tests/fixtures/dsh-plugin/package.json`
- Create: `src-tauri/tests/fixtures/dsh-plugin/cordis.patch.yml`
- Create: `src-tauri/tests/fixtures/dsh-plugin/lib/index.mjs`
- Create: `src-tauri/tests/fixtures/dsh-plugin/lib/client.js`
- Create: `src-tauri/tests/fixtures/dsh-plugin/README.md`

- [ ] **Step 1: 写 fixture 文件**

`package.json`：

```json
{
  "name": "dsh-open-workspace",
  "version": "0.2.0",
  "description": "fixture",
  "type": "module"
}
```

`cordis.patch.yml`：

```yaml
- insert:
    - id: dsh-open-workspace
      name: dsh-open-workspace
```

`lib/index.mjs`：`export function apply() {}`
`lib/client.js`：`window.__fixture = true;`
`README.md`：`# fixture`

- [ ] **Step 2: 写失败测试**

`resources.rs` 的 `mod tests` 里加：

```rust
    fn fixture_zip(version: &str) -> Vec<u8> {
        use std::io::Write;
        use zip::write::SimpleFileOptions;
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dsh-plugin");
        let mut buffer = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buffer);
            let options = SimpleFileOptions::default();
            zip.start_file("package.json", options).unwrap();
            let pkg = std::fs::read_to_string(root.join("package.json"))
                .unwrap()
                .replace("0.2.0", version);
            zip.write_all(pkg.as_bytes()).unwrap();
            for name in ["cordis.patch.yml", "lib/index.mjs", "lib/client.js", "README.md"] {
                zip.start_file(name, options).unwrap();
                let bytes = std::fs::read(root.join(name)).unwrap();
                zip.write_all(&bytes).unwrap();
            }
            zip.finish().unwrap();
        }
        buffer.into_inner()
    }

    fn test_home(name: &str) -> std::path::PathBuf {
        let home = std::env::temp_dir().join(format!("ws-res-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join("profiles/desktop")).unwrap();
        std::fs::write(
            home.join("profiles/desktop/package.json"),
            r#"{"name":"dsh-profile-desktop","dsh":{"profile":{"bundles":["@deepseek-ai/dsh-base"]}},"dependencies":{}}"#,
        )
        .unwrap();
        home
    }

    #[test]
    fn install_extracts_files_and_merges_manifest() {
        let home = test_home("install");
        install_from_zip_bytes(&fixture_zip("0.2.0"), &home).unwrap();

        for name in ["package.json", "cordis.patch.yml", "lib/index.mjs", "lib/client.js", "README.md"] {
            assert!(home.join("plugins/dsh-open-workspace").join(name).exists(), "store 缺 {name}");
            assert!(
                home.join("profiles/desktop/node_modules/dsh-open-workspace").join(name).exists(),
                "profile 缺 {name}"
            );
        }
        let (installed, version) = crate::commands::deepseek::local_plugin_status(&home);
        assert!(installed);
        assert_eq!(version.as_deref(), Some("0.2.0"));

        let manifest =
            std::fs::read_to_string(home.join("profiles/desktop/package.json")).unwrap();
        let manifest: serde_json::Value = serde_json::from_str(&manifest).unwrap();
        assert_eq!(
            manifest["dependencies"]["dsh-open-workspace"],
            "link:../../plugins/dsh-open-workspace"
        );
        assert!(manifest["dsh"]["profile"]["bundles"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item == "dsh-open-workspace"));
        assert!(home.join("profiles/desktop/package.json.ws-workbench.bak").exists());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn install_rejects_broken_zip_without_touching_dsh() {
        let home = test_home("broken");
        let mut bytes = fixture_zip("0.2.0");
        bytes.truncate(bytes.len() / 2);
        assert!(install_from_zip_bytes(&bytes, &home).is_err());
        assert!(!home.join("plugins/dsh-open-workspace").exists());
        let manifest =
            std::fs::read_to_string(home.join("profiles/desktop/package.json")).unwrap();
        assert!(!manifest.contains("dsh-open-workspace"));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn uninstall_removes_files_and_manifest_entries() {
        let home = test_home("uninstall");
        install_from_zip_bytes(&fixture_zip("0.2.0"), &home).unwrap();
        uninstall_plugin(&home).unwrap();

        assert!(!home.join("plugins/dsh-open-workspace").exists());
        assert!(!home.join("profiles/desktop/node_modules/dsh-open-workspace").exists());
        assert!(!crate::commands::deepseek::local_plugin_status(&home).0);
        let manifest =
            std::fs::read_to_string(home.join("profiles/desktop/package.json")).unwrap();
        assert!(!manifest.contains("dsh-open-workspace"));
        let _ = std::fs::remove_dir_all(&home);
    }
```

- [ ] **Step 3: 跑测试确认失败**

Run: `cd src-tauri && cargo test resources::tests::install_`

Expected: 编译失败（缺 `install_from_zip_bytes` / `uninstall_plugin` /
`deepseek::local_plugin_status`）。

- [ ] **Step 4: resources.rs 解压实现**

```rust
pub(crate) const DSH_PLUGIN_FILES: &[&str] = &[
    "package.json",
    "cordis.patch.yml",
    "lib/index.mjs",
    "lib/client.js",
    "README.md",
];

/// 校验 zip 结构并解出文件；只接受约定清单里的文件，防止路径穿越。
pub(crate) fn read_plugin_zip(bytes: &[u8]) -> Result<(String, Vec<(String, Vec<u8>)>), String> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|err| format!("zip 包损坏: {err}"))?;
    let mut files: Vec<(String, Vec<u8>)> = Vec::with_capacity(DSH_PLUGIN_FILES.len());
    for name in DSH_PLUGIN_FILES {
        let mut file = archive
            .by_name(name)
            .map_err(|_| format!("插件包缺少文件: {name}"))?;
        let mut buffer = Vec::new();
        std::io::Read::read_to_end(&mut file, &mut buffer)
            .map_err(|err| format!("读取 {name} 失败: {err}"))?;
        files.push(((*name).to_string(), buffer));
    }

    let package = files
        .iter()
        .find(|(name, _)| name == "package.json")
        .map(|(_, bytes)| serde_json::from_slice::<serde_json::Value>(bytes))
        .unwrap()
        .map_err(|err| format!("插件 package.json 非法: {err}"))?;
    if package.get("name").and_then(|name| name.as_str()) != Some(DSH_PLUGIN_ID) {
        return Err("插件包 name 不是 dsh-open-workspace".to_string());
    }
    let version = package
        .get("version")
        .and_then(|version| version.as_str())
        .filter(|version| !version.is_empty())
        .ok_or_else(|| "插件包缺少 version".to_string())?
        .to_string();
    Ok((version, files))
}

pub(crate) fn install_from_zip_bytes(bytes: &[u8], home: &Path) -> Result<(), String> {
    let (_, files) = read_plugin_zip(bytes)?;
    crate::commands::deepseek::install_plugin_files(home, &files)
}

pub(crate) fn uninstall_plugin(home: &Path) -> Result<(), String> {
    crate::commands::deepseek::uninstall_plugin_files(home)
}
```

- [ ] **Step 5: deepseek.rs 改造安装/卸载入口**

把 `write_plugin_files(dir)` / `replace_plugin_dir(dir)` / `install_to_home(home, ...)` 改为
「文件列表驱动」，并加卸载：

```rust
fn write_files(dir: &Path, files: &[(String, Vec<u8>)]) -> Result<(), String> {
    for (name, bytes) in files {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("创建插件目录失败 {}: {err}", parent.display()))?;
        }
        std::fs::write(&path, bytes)
            .map_err(|err| format!("写插件文件失败 {}: {err}", path.display()))?;
    }
    Ok(())
}

/// 整目录替换：先写临时目录再换名，避免更新到一半把 DSH 启动路径写坏。
fn replace_plugin_dir(dir: &Path, files: &[(String, Vec<u8>)]) -> Result<(), String> {
    let parent = dir
        .parent()
        .ok_or_else(|| format!("插件目录没有父目录: {}", dir.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|err| format!("创建插件目录失败 {}: {err}", parent.display()))?;
    let name = dir
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| format!("插件目录名非法: {}", dir.display()))?;
    let staging = parent.join(format!(".{name}.new"));
    let old = parent.join(format!(".{name}.old"));
    let _ = std::fs::remove_dir_all(&staging);
    let _ = std::fs::remove_dir_all(&old);
    write_files(&staging, files)?;
    let had_old = dir.exists();
    if had_old {
        std::fs::rename(dir, &old)
            .map_err(|err| format!("替换插件目录失败 {}: {err}", dir.display()))?;
    }
    if let Err(err) = std::fs::rename(&staging, dir) {
        if had_old {
            let _ = std::fs::rename(&old, dir);
        }
        let _ = std::fs::remove_dir_all(&staging);
        return Err(format!("替换插件目录失败 {}: {err}", dir.display()));
    }
    let _ = std::fs::remove_dir_all(&old);
    Ok(())
}

/// 按 zip 解出的文件安装/更新插件；清单最后写，失败不动清单。
pub(crate) fn install_plugin_files(
    home: &Path,
    files: &[(String, Vec<u8>)],
) -> Result<(), String> {
    let profile = profile_dir(home);
    let manifest = profile.join("package.json");
    if !manifest.is_file() {
        return Err("还没初始化 DeepSeek 的 desktop profile：请先打开一次 DeepSeek Harness".to_string());
    }
    replace_plugin_dir(&plugin_store_dir(home), files)?;
    replace_plugin_dir(&plugin_active_dir(home), files)?;

    let raw = std::fs::read_to_string(&manifest)
        .map_err(|err| format!("读 profile 清单失败: {err}"))?;
    let (merged, changed) = merge_profile_manifest(&raw)?;
    if changed {
        let backup = PathBuf::from(format!("{}.ws-workbench.bak", manifest.display()));
        if !backup.exists() {
            std::fs::copy(&manifest, &backup)
                .map_err(|err| format!("备份 profile 清单失败: {err}"))?;
        }
        std::fs::write(&manifest, merged)
            .map_err(|err| format!("写 profile 清单失败: {err}"))?;
    }
    Ok(())
}

/// 卸载：删两个插件目录，并从 profile 清单移除依赖与 bundle 项（备份保留）。
pub(crate) fn uninstall_plugin_files(home: &Path) -> Result<(), String> {
    for dir in [plugin_store_dir(home), plugin_active_dir(home)] {
        if dir.exists() {
            std::fs::remove_dir_all(&dir)
                .map_err(|err| format!("删除插件目录失败 {}: {err}", dir.display()))?;
        }
    }
    let manifest = profile_dir(home).join("package.json");
    let Ok(raw) = std::fs::read_to_string(&manifest) else {
        return Ok(());
    };
    let mut value: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|err| format!("profile 清单不是合法 JSON: {err}"))?;
    if let Some(dependencies) = value
        .get_mut("dependencies")
        .and_then(|value| value.as_object_mut())
    {
        dependencies.remove(DSH_PLUGIN_NAME);
    }
    if let Some(bundles) = value
        .get_mut("dsh")
        .and_then(|dsh| dsh.get_mut("profile"))
        .and_then(|profile| profile.get_mut("bundles"))
        .and_then(|bundles| bundles.as_array_mut())
    {
        bundles.retain(|item| item.as_str() != Some(DSH_PLUGIN_NAME));
    }
    let text = serde_json::to_string_pretty(&value).map_err(|err| err.to_string())?;
    std::fs::write(&manifest, format!("{text}\n"))
        .map_err(|err| format!("写 profile 清单失败: {err}"))
}

/// 本地插件状态：profile 里登记了依赖 + bundle，且 node_modules 里有版本号。
pub(crate) fn local_plugin_status(home: &Path) -> (bool, Option<String>) {
    let manifest = profile_dir(home).join("package.json");
    let Ok(raw) = std::fs::read_to_string(&manifest) else {
        return (false, None);
    };
    if !manifest_contains_plugin(&raw) {
        return (false, None);
    }
    match installed_version(home) {
        Some(version) => (true, Some(version)),
        None => (false, None),
    }
}
```

- [ ] **Step 6: 跑测试确认通过**

Run: `cd src-tauri && cargo test resources::tests::`

Expected: 6 passed（版本 2 + Release 1 + install 2 + uninstall 1）。

- [ ] **Step 7: Commit**

```bash
git add src-tauri/src/commands/resources.rs src-tauri/src/commands/deepseek.rs src-tauri/tests/fixtures/dsh-plugin
git commit -m "feat(resources): zip 解压与 DSH 安装/卸载"
```

---

### Task 4: 资源命令（list/install/uninstall）+ 注册

**Files:**
- Modify: `src-tauri/src/commands/resources.rs`
- Modify: `src-tauri/src/commands/deepseek.rs`（`dsh_home` / `dsh_running` 改 `pub(crate)`）
- Modify: `src-tauri/src/lib.rs`（混合文件，改动留到 Task 5 一起提交）

- [ ] **Step 1: 写资源状态与命令**

`resources.rs` 加：

```rust
#[derive(Debug, Clone, Serialize)]
pub struct ResourceStatus {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub description: String,
    pub available: bool,
    pub installed: bool,
    pub installed_version: Option<String>,
    pub latest_version: Option<String>,
    pub update_available: bool,
    pub error: Option<String>,
    pub note: Option<String>,
}

pub(crate) fn dsh_plugin_status(home: &Path, force: bool) -> ResourceStatus {
    let (installed, installed_version) = crate::commands::deepseek::local_plugin_status(home);
    let (latest_version, error) = match latest_plugin_version(force) {
        Ok(version) => (Some(version), None),
        Err(err) => (None, Some(err)),
    };
    let update_available = match (&installed_version, &latest_version) {
        (Some(local), Some(remote)) => version_newer(remote, local),
        _ => false,
    };
    ResourceStatus {
        id: DSH_PLUGIN_ID.to_string(),
        name: "DeepSeek 工作区插件".to_string(),
        kind: "dsh-plugin".to_string(),
        description:
            "让「打开方式 → DeepSeek」把当前目录作为工作区打开（本地小插件，离线运行）".to_string(),
        available: true,
        installed,
        installed_version,
        latest_version,
        update_available,
        error,
        note: None,
    }
}

fn chrome_placeholder() -> ResourceStatus {
    ResourceStatus {
        id: "chrome-extensions".to_string(),
        name: "Chrome 插件".to_string(),
        kind: "chrome-extensions".to_string(),
        description: "Chrome 插件管理".to_string(),
        available: false,
        installed: false,
        installed_version: None,
        latest_version: None,
        update_available: false,
        error: None,
        note: Some("Chrome 插件管理规划中".to_string()),
    }
}

/// 下载 Release 里的 zip 资产（走 API asset URL，302 由 curl -L 跟随）。
fn download_asset(asset: &AssetDto) -> Result<Vec<u8>, String> {
    let path = std::env::temp_dir().join(format!(
        "ws-resource-{}-{}.zip",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_millis())
            .unwrap_or(0)
    ));
    let output = std::process::Command::new("curl")
        .args([
            "-sSfL",
            "--max-time",
            "120",
            "-H",
            "Accept: application/octet-stream",
            "-o",
            &path.display().to_string(),
            &asset.url,
        ])
        .output()
        .map_err(|err| format!("系统缺少 curl 或无法执行: {err}"))?;
    if !output.status.success() {
        return Err(format!("下载插件包失败: {}", asset.name));
    }
    let bytes = std::fs::read(&path).map_err(|err| format!("读下载文件失败: {err}"))?;
    let _ = std::fs::remove_file(&path);
    if bytes.is_empty() {
        return Err("下载到的插件包是空文件".to_string());
    }
    Ok(bytes)
}

#[tauri::command]
pub fn resource_list(force: bool) -> Result<Vec<ResourceStatus>, String> {
    let home = dsh_home()?;
    Ok(vec![dsh_plugin_status(&home, force), chrome_placeholder()])
}

#[tauri::command]
pub fn resource_install(id: String) -> Result<ResourceStatus, String> {
    if id != DSH_PLUGIN_ID {
        return Err(format!("资源 {id} 暂不支持安装"));
    }
    let home = dsh_home()?;
    let release = latest_release(true)?;
    let version = release.tag_name.trim_start_matches('v').to_string();
    let asset = asset_for(&release, &version)
        .ok_or_else(|| format!("Release v{version} 缺少 {DSH_PLUGIN_ID}-{version}.zip"))?
        .clone();
    let bytes = download_asset(&asset)?;
    install_from_zip_bytes(&bytes, &home)?;
    let mut status = dsh_plugin_status(&home, false);
    if dsh_running() {
        status.note = Some("已安装，需要重启 DeepSeek Harness 生效".to_string());
    }
    Ok(status)
}

#[tauri::command]
pub fn resource_uninstall(id: String) -> Result<ResourceStatus, String> {
    if id != DSH_PLUGIN_ID {
        return Err(format!("资源 {id} 暂不支持卸载"));
    }
    let home = dsh_home()?;
    uninstall_plugin(&home)?;
    let mut status = dsh_plugin_status(&home, false);
    if dsh_running() {
        status.note = Some("已卸载，需要重启 DeepSeek Harness 生效".to_string());
    }
    Ok(status)
}
```

- [ ] **Step 2: 可见性调整**

`deepseek.rs`：`pub(crate) fn dsh_home()`、`pub(crate) fn dsh_running()`。

- [ ] **Step 3: 注册命令（混合文件，与 Task 5 一起提交）**

`git show HEAD:src-tauri/src/lib.rs > /tmp/lib_head.rs`，把 handler 里：

```rust
            commands::deepseek_plugin_status,
            commands::install_deepseek_plugin,
            commands::open_deepseek_workspace,
            commands::open_deepseek_app,
```

改为：

```rust
            commands::deepseek_plugin_status,
            commands::deepseek_plugin_update,
            commands::open_deepseek_workspace,
            commands::open_deepseek_app,
            commands::resource_list,
            commands::resource_install,
            commands::resource_uninstall,
```

- [ ] **Step 4: 暂不 commit（等 Task 5 补齐 `deepseek_plugin_update` 后一起验证）**

---

### Task 5: 收缩 deepseek.rs：删内置插件/指纹，加 update 检查

**Files:**
- Modify: `src-tauri/src/commands/deepseek.rs`
- Modify: `src-tauri/build.rs`（混合文件）
- Modify: `src-tauri/src/lib.rs`（用 Task 4 的 /tmp/lib_head.rs）

- [ ] **Step 1: 删内置插件相关代码**

删除：

- `PLUGIN_FILES` 常量与 `include_str!`；
- `installed_version` 保留（zip 安装后的目录版本读取要用）；
- `bundled_version` / `needs_update` 字段；
- `files_fingerprint` / `bundled_fingerprint` / `installed_fingerprint`；
- `status_for_home` 里的 bundled/fingerprint 判断（只保留 app/profile/plugin/version/dsh_running）；
- `install_deepseek_plugin` 命令与 `install_to_home`（已被 `install_plugin_files` 取代）；
- 旧测试：`bundled_plugin_files_are_complete`、`install_*`、`backup_is_kept_*`、
  `merge_profile_manifest_*`（如已迁到 resources 测试则删旧）。

`DeepseekPluginStatus` 改为：

```rust
#[derive(Debug, Serialize)]
pub struct DeepseekPluginStatus {
    pub dsh_home: String,
    pub app_installed: bool,
    pub profile_ready: bool,
    pub plugin_installed: bool,
    pub installed_version: String,
    pub dsh_running: bool,
}

#[tauri::command]
pub fn deepseek_plugin_status(app: tauri::AppHandle) -> Result<DeepseekPluginStatus, String> {
    let home = dsh_home()?;
    let (plugin_installed, installed_version) = local_plugin_status(&home);
    Ok(DeepseekPluginStatus {
        dsh_home: home.display().to_string(),
        app_installed: deepseek_app_installed(&app),
        profile_ready: profile_dir(&home).join("package.json").is_file(),
        plugin_installed,
        installed_version: installed_version.unwrap_or_default(),
        dsh_running: dsh_running(),
    })
}
```

新增：

```rust
#[derive(Debug, Serialize)]
pub struct DeepseekPluginUpdate {
    pub latest_version: Option<String>,
    pub update_available: bool,
    pub error: Option<String>,
}

/// 点「DeepSeek」时查远端插件版本；失败不阻塞打开流程。
#[tauri::command]
pub fn deepseek_plugin_update() -> Result<DeepseekPluginUpdate, String> {
    let home = dsh_home()?;
    let (_, installed_version) = local_plugin_status(&home);
    match crate::commands::resources::latest_plugin_version(false) {
        Ok(latest) => {
            let update_available = installed_version
                .as_deref()
                .map(|local| crate::commands::resources::version_newer(&latest, local))
                .unwrap_or(false);
            Ok(DeepseekPluginUpdate {
                latest_version: Some(latest),
                update_available,
                error: None,
            })
        }
        Err(err) => Ok(DeepseekPluginUpdate {
            latest_version: None,
            update_available: false,
            error: Some(err),
        }),
    }
}
```

`open_dir_workspace` 里的旧状态检查改为：

```rust
let home = dsh_home()?;
let (plugin_installed, _) = local_plugin_status(&home);
if !plugin_installed {
    return Err("还没安装 DeepSeek 工作区插件：去「设置 → 资源」下载安装后再试".to_string());
}
```

- [ ] **Step 2: build.rs 删插件监听（混合文件）**

`git show HEAD:src-tauri/build.rs > /tmp/build_head.rs`，删掉这 5 行（若存在）：

```rust
    println!("cargo:rerun-if-changed=../dsh-plugin/dsh-open-workspace/package.json");
    println!("cargo:rerun-if-changed=../dsh-plugin/dsh-open-workspace/cordis.patch.yml");
    println!("cargo:rerun-if-changed=../dsh-plugin/dsh-open-workspace/lib/index.mjs");
    println!("cargo:rerun-if-changed=../dsh-plugin/dsh-open-workspace/lib/client.js");
    println!("cargo:rerun-if-changed=../dsh-plugin/dsh-open-workspace/README.md");
```

- [ ] **Step 3: 编译 + 测试**

Run: `cd src-tauri && cargo check && cargo test`

Expected: 编译通过；`deepseek` 与 `resources` 测试全过（用户 WIP 的既有 flaky 测试除外）。

- [ ] **Step 4: Commit（资源命令 + lib.rs/build.rs 一起）**

```bash
cd /Users/fengwenshan/wenshan/demo/layui-admin-build
git add src-tauri/src/commands/deepseek.rs src-tauri/src/commands/resources.rs
BLOB=$(git hash-object -w /tmp/lib_head.rs); git update-index --cacheinfo 100644,$BLOB,src-tauri/src/lib.rs
BLOB=$(git hash-object -w /tmp/build_head.rs); git update-index --cacheinfo 100644,$BLOB,src-tauri/build.rs
git commit -m "refactor(deepseek): 内置插件安装下线，改为资源下载安装 + 远端版本检查"
```

---

### Task 6: 前端 API 与设置-资源 UI

**Files:**
- Modify: `src/api/system.ts`
- Create: `src/components/settings/ResourcePanel.vue`
- Modify: `src/components/settings/SettingsPanel.vue`（混合文件）
- Modify: `src/App.vue`

- [ ] **Step 1: system.ts**

```ts
/** 设置 → 资源：可下载资源的安装状态与远端版本 */
export interface ResourceStatus {
  id: string
  name: string
  kind: string
  description: string
  available: boolean
  installed: boolean
  installed_version: string | null
  latest_version: string | null
  update_available: boolean
  error: string | null
  note: string | null
}

export function resourceList(force = false): Promise<ResourceStatus[]> {
  return invoke<ResourceStatus[]>('resource_list', { force })
}

export function resourceInstall(id: string): Promise<ResourceStatus> {
  return invoke<ResourceStatus>('resource_install', { id })
}

export function resourceUninstall(id: string): Promise<ResourceStatus> {
  return invoke<ResourceStatus>('resource_uninstall', { id })
}

/** 点 DeepSeek 时查插件是否有更新（远端检查，失败不阻塞） */
export interface DeepseekPluginUpdate {
  latest_version: string | null
  update_available: boolean
  error: string | null
}

export function deepseekPluginUpdate(): Promise<DeepseekPluginUpdate> {
  return invoke<DeepseekPluginUpdate>('deepseek_plugin_update')
}
```

同时从 `DeepseekPluginStatus` 删除 `bundled_version`、`needs_update`，删除
`installDeepseekPlugin()` 函数。

- [ ] **Step 2: ResourcePanel.vue**

`src/components/settings/ResourcePanel.vue`：

```vue
<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";
import { Button, Modal, message } from "ant-design-vue";
import {
  resourceInstall,
  resourceList,
  resourceUninstall,
  type ResourceStatus,
} from "@/api/system";

const resources = ref<ResourceStatus[]>([]);
const loading = ref(false);
const busyId = ref("");
const root = ref<HTMLElement | null>(null);

async function load(force: boolean) {
  loading.value = true;
  try {
    resources.value = await resourceList(force);
  } catch (err) {
    message.error(`资源检查失败: ${err}`);
  } finally {
    loading.value = false;
  }
}

async function install(resource: ResourceStatus) {
  busyId.value = resource.id;
  try {
    await resourceInstall(resource.id);
    message.success(resource.installed ? "插件已更新" : "插件已安装");
    await load(false);
  } catch (err) {
    message.error(`安装失败: ${err}`);
  } finally {
    busyId.value = "";
  }
}

function uninstall(resource: ResourceStatus) {
  Modal.confirm({
    title: `卸载 ${resource.name}？`,
    content: "会从 DeepSeek 配置里移除插件，重启 DeepSeek 后生效。",
    okText: "卸载",
    okButtonProps: { danger: true },
    cancelText: "取消",
    onOk: async () => {
      busyId.value = resource.id;
      try {
        await resourceUninstall(resource.id);
        message.success("已卸载，重启 DeepSeek 后生效");
        await load(false);
      } catch (err) {
        message.error(`卸载失败: ${err}`);
      } finally {
        busyId.value = "";
      }
    },
  });
}

function statusText(resource: ResourceStatus) {
  if (!resource.available) return resource.note || "暂不支持";
  if (resource.error) return "检查失败";
  if (resource.update_available) {
    return `可更新 ${resource.installed_version} → ${resource.latest_version}`;
  }
  if (resource.installed) return `已安装 ${resource.installed_version}`;
  return "未安装";
}

function focus() {
  root.value?.scrollIntoView({ behavior: "smooth", block: "center" });
}

onMounted(() => {
  void load(false);
  window.addEventListener("ws-focus-resources", focus);
});
onUnmounted(() => window.removeEventListener("ws-focus-resources", focus));
</script>

<template>
  <section ref="root" class="card">
    <div class="res-head">
      <div>
        <h3>资源</h3>
        <p class="res-sub">可下载的插件与扩展；装好后重启对应宿主应用生效。</p>
      </div>
      <Button size="small" :loading="loading" @click="load(true)">刷新</Button>
    </div>

    <div v-for="resource in resources" :key="resource.id" class="res-row">
      <div class="res-main">
        <div class="res-title">
          {{ resource.name }}
          <span class="res-state">{{ statusText(resource) }}</span>
        </div>
        <div class="res-desc">{{ resource.description }}</div>
        <div class="res-meta">
          本地 {{ resource.installed_version || "—" }} · 最新 {{ resource.latest_version || "—" }}
        </div>
        <div v-if="resource.error" class="res-error">{{ resource.error }}</div>
        <div v-if="resource.note" class="res-note">{{ resource.note }}</div>
      </div>
      <div class="res-actions">
        <Button
          v-if="resource.available && (!resource.installed || resource.update_available)"
          type="primary"
          size="small"
          :loading="busyId === resource.id"
          @click="install(resource)"
        >{{ resource.installed ? "更新" : "下载安装" }}</Button>
        <Button
          v-if="resource.available && resource.installed"
          size="small"
          danger
          :loading="busyId === resource.id"
          @click="uninstall(resource)"
        >卸载</Button>
        <Button v-if="!resource.available" size="small" disabled>敬请期待</Button>
      </div>
    </div>
  </section>
</template>

<style scoped>
.res-head { display: flex; justify-content: space-between; gap: 12px; align-items: flex-start; }
.res-sub { margin: 4px 0 0; font-size: 12px; color: var(--text-muted); }
.res-row { display: flex; gap: 16px; padding: 12px 0; border-top: 1px solid var(--border-color, #2a2a2a); }
.res-row:first-of-type { border-top: none; }
.res-main { flex: 1; min-width: 0; }
.res-title { font-size: 13px; color: var(--text-primary); display: flex; gap: 8px; align-items: center; flex-wrap: wrap; }
.res-state { font-size: 12px; color: var(--text-muted); }
.res-desc { margin-top: 4px; font-size: 12px; color: var(--text-secondary); }
.res-meta { margin-top: 4px; font-size: 12px; color: var(--text-muted); }
.res-error { margin-top: 4px; font-size: 12px; color: #ff7875; }
.res-note { margin-top: 4px; font-size: 12px; color: #d89614; }
.res-actions { display: flex; align-items: center; gap: 8px; flex-shrink: 0; }
</style>
```

- [ ] **Step 3: SettingsPanel 插入卡片（混合文件）**

`git show HEAD:src/components/settings/SettingsPanel.vue > /tmp/settings_head.vue`，在模板最后一张
`</section>`（「打包输出目录」或规则卡片）之后、最外层 `</div>` 之前插入：

```vue
    <ResourcePanel />
```

并在 `<script setup>` import 区加：

```ts
import ResourcePanel from "@/components/settings/ResourcePanel.vue";
```

- [ ] **Step 4: App.vue 监听事件**

在既有 `onMounted` / `onUnmounted` 旁边合并：

```ts
function onOpenSettings(event: Event) {
  const detail = (event as CustomEvent<string>).detail;
  selectTool("settings");
  if (detail === "resources") {
    setTimeout(() => window.dispatchEvent(new CustomEvent("ws-focus-resources")), 50);
  }
}

onMounted(() => window.addEventListener("ws-open-settings", onOpenSettings));
onUnmounted(() => window.removeEventListener("ws-open-settings", onOpenSettings));
```

- [ ] **Step 5: 构建验证并 commit**

Run: `pnpm build`

Expected: `✓ built in ...`，无 TS 错误。

```bash
git add src/api/system.ts src/components/settings/ResourcePanel.vue src/App.vue
BLOB=$(git hash-object -w /tmp/settings_head.vue); git update-index --cacheinfo 100644,$BLOB,src/components/settings/SettingsPanel.vue
git commit -m "feat(settings): 资源栏目（DSH 插件下载/更新/卸载）"
```

---

### Task 7: 点击 DeepSeek 流程改造 + 删除内置插件目录

**Files:**
- Modify: `src/components/OpenWithButton.vue`（混合文件）
- Delete: `dsh-plugin/` 整个目录

- [ ] **Step 1: OpenWithButton 新流程**

`git show HEAD:src/components/OpenWithButton.vue > /tmp/owb_head.vue`，将
`askInstall` / `askOpenFirst` / `runDeepseek` 三个函数整体替换为：

```ts
/** 切到设置页并定位到资源卡片 */
function openResourceSettings() {
  window.dispatchEvent(new CustomEvent("ws-open-settings", { detail: "resources" }))
}

/** 「打开方式 → DeepSeek」的完整流程 */
async function runDeepseek() {
  const status = await deepseekPluginStatus()
  if (!status.app_installed) {
    Modal.warning({
      title: "还没安装 DeepSeek Harness",
      content: "请先安装 DeepSeek Harness 桌面版，打开一次完成初始化，再回来点「DeepSeek」。",
    })
    return
  }
  if (!status.profile_ready) {
    const openIt = await new Promise<boolean>((resolve) => {
      Modal.confirm({
        title: "DeepSeek 还没初始化",
        content: "先打开一次 DeepSeek Harness，等它完成初始化后再回来点「DeepSeek」。",
        okText: "先打开 DeepSeek",
        cancelText: "取消",
        onOk: () => resolve(true),
        onCancel: () => resolve(false),
      })
    })
    if (openIt) await openDeepseekApp()
    return
  }
  if (!status.plugin_installed) {
    const toSettings = await new Promise<boolean>((resolve) => {
      Modal.confirm({
        title: "还没安装 DeepSeek 工作区插件",
        content: "打开「设置 → 资源」下载安装后，再点一次「DeepSeek」就能直接打开工作区。",
        okText: "打开设置",
        cancelText: "取消",
        onOk: () => resolve(true),
        onCancel: () => resolve(false),
      })
    })
    if (toSettings) openResourceSettings()
    return
  }

  let update: Awaited<ReturnType<typeof deepseekPluginUpdate>> | null = null
  try {
    update = await deepseekPluginUpdate()
  } catch (err) {
    emit("log", "warn", `检查 DeepSeek 插件更新失败: ${err}`)
  }
  if (update?.update_available) {
    const toSettings = await new Promise<boolean>((resolve) => {
      Modal.confirm({
        title: `插件有新版本 v${update?.latest_version}`,
        content: `当前安装 v${status.installed_version}。可以先去「设置 → 资源」更新（重启 DeepSeek 生效），也可以现在直接打开。`,
        okText: "去设置更新",
        cancelText: "直接打开",
        onOk: () => resolve(true),
        onCancel: () => resolve(false),
      })
    })
    if (toSettings) {
      openResourceSettings()
      return
    }
  } else if (update?.error) {
    emit("log", "warn", `查不到插件最新版本，按当前版本打开: ${update.error}`)
  }

  await openDeepseekWorkspace(props.path)
  emit("log", "info", `用 DeepSeek 打开: ${props.path}`)
}
```

import 增加 `deepseekPluginUpdate`，删除 `installDeepseekPlugin`（若 import 里有）。

- [ ] **Step 2: 删除内置插件目录**

```bash
cd /Users/fengwenshan/wenshan/demo/layui-admin-build
git rm -r dsh-plugin
```

（必须在插件仓库 Release 可用后执行。）

- [ ] **Step 3: 构建与测试**

Run: `pnpm build && cd src-tauri && cargo test`

Expected: 前端构建通过；Rust 测试全过（用户 WIP 的既有 flaky 测试除外）。

- [ ] **Step 4: Commit**

```bash
BLOB=$(git hash-object -w /tmp/owb_head.vue); git update-index --cacheinfo 100644,$BLOB,src/components/OpenWithButton.vue
git add dsh-plugin
git commit -m "feat(openwith): 点击 DeepSeek 先检测插件，缺插件/版本旧引导去设置；删除内置插件"
```

---

### Task 8: 脚本、文档、端到端验证

**Files:**
- Modify: `scripts/dsh-plugin-install.sh`
- Modify: `README.md`（混合文件）
- Modify: `docs/superpowers/specs/2026-11-10-deepseek-workspace-open-design.md`

- [ ] **Step 1: 脚本改成 zip 安装**

完整替换 `scripts/dsh-plugin-install.sh`：

```bash
#!/usr/bin/env bash
# 从本地 zip 安装 / 卸载 DeepSeek 工作区插件（开发 / 离线用；生产走 ws-workbench 设置 → 资源）。
# 用法：
#   scripts/dsh-plugin-install.sh --zip dsh-open-workspace-0.1.0.zip
#   scripts/dsh-plugin-install.sh --uninstall
# 环境变量：DSH_HOME（默认 ~/.dsh）
set -euo pipefail

DSH_HOME="${DSH_HOME:-$HOME/.dsh}"
PROFILE_DIR="$DSH_HOME/profiles/desktop"
PLUGIN_NAME="dsh-open-workspace"
STORE_DIR="$DSH_HOME/plugins/$PLUGIN_NAME"
ACTIVE_DIR="$PROFILE_DIR/node_modules/$PLUGIN_NAME"
MANIFEST="$PROFILE_DIR/package.json"

die() { echo "错误: $*" >&2; exit 1; }

uninstall() {
  rm -rf "$STORE_DIR" "$ACTIVE_DIR"
  python3 - "$MANIFEST" <<'PYEOF'
import json, sys
path = sys.argv[1]
with open(path) as f:
    data = json.load(f)
data.get("dependencies", {}).pop("dsh-open-workspace", None)
bundles = data.get("dsh", {}).get("profile", {}).get("bundles", [])
if "dsh-open-workspace" in bundles:
    bundles.remove("dsh-open-workspace")
with open(path, "w") as f:
    json.dump(data, f, ensure_ascii=False, indent=2)
PYEOF
  echo "已卸载。重启 DeepSeek Harness 生效。"
}

install_zip() {
  local zip="$1"
  [ -f "$zip" ] || die "找不到 zip: $zip"
  [ -f "$MANIFEST" ] || die "DeepSeek 还没初始化：先打开一次 DeepSeek Harness"
  local tmp
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' RETURN
  unzip -q "$zip" -d "$tmp/dsh-open-workspace"
  for f in package.json cordis.patch.yml lib/index.mjs lib/client.js README.md; do
    [ -f "$tmp/dsh-open-workspace/$f" ] || die "zip 缺少 $f"
  done
  mkdir -p "$STORE_DIR" "$ACTIVE_DIR"
  cp -R "$tmp/dsh-open-workspace/." "$STORE_DIR/"
  cp -R "$tmp/dsh-open-workspace/." "$ACTIVE_DIR/"
  [ -f "$MANIFEST.ws-workbench.bak" ] || cp "$MANIFEST" "$MANIFEST.ws-workbench.bak"
  python3 - "$MANIFEST" <<'PYEOF'
import json, sys
path = sys.argv[1]
with open(path) as f:
    data = json.load(f)
data.setdefault("dependencies", {})["dsh-open-workspace"] = "link:../../plugins/dsh-open-workspace"
bundles = data.setdefault("dsh", {}).setdefault("profile", {}).setdefault("bundles", [])
if "dsh-open-workspace" not in bundles:
    bundles.append("dsh-open-workspace")
with open(path, "w") as f:
    json.dump(data, f, ensure_ascii=False, indent=2)
PYEOF
  echo "已安装。重启 DeepSeek Harness 生效。"
}

case "${1:-}" in
  --zip) install_zip "${2:?--zip 需要 zip 路径}" ;;
  --uninstall) uninstall ;;
  *) die "用法: $0 --zip <path> | --uninstall" ;;
esac
```

Run: `chmod +x scripts/dsh-plugin-install.sh && bash -n scripts/dsh-plugin-install.sh`

Expected: 无输出（语法通过）。

- [ ] **Step 2: README 更新（混合文件）**

把 README 中插件相关段落改为：

```markdown
### DeepSeek 工作区插件（独立仓库）

- 插件是独立项目：`https://github.com/fengwenshan/dsh-open-workspace`，按 tag 发布 zip。
- ws-workbench 不再内置插件：点「打开方式 → DeepSeek」会先检测本机 DSH 与插件；
  没装插件或版本旧时引导到「设置 → 资源」下载/更新，再点一次即可打开工作区。
- 手动安装/卸载：`scripts/dsh-plugin-install.sh --zip <zip路径>`（开发/离线用）。
```

- [ ] **Step 3: 旧设计文档加取代说明**

在 `docs/superpowers/specs/2026-11-10-deepseek-workspace-open-design.md` 顶部加：

```markdown
> 注意：插件安装方式已被 `2026-11-10-dsh-resource-manager-design.md` 取代
> （插件独立仓库 + Release zip + 设置-资源）；本文档只保留「写请求文件 + 插件消费打开工作区」的机制说明。
```

- [ ] **Step 4: 端到端验证（手动）**

1. `pnpm tauri build`（或 `pnpm tauri dev`）构建 ws-workbench；
2. 设置 → 资源：显示插件「已安装 0.1.0」与最新版本一致（本机已装）；
3. 卸载插件 → 重启 DSH → 点 DeepSeek：应提示「还没安装 DeepSeek 工作区插件」；
4. 设置 → 资源：下载安装 → 重启 DSH → 点 DeepSeek：左侧选中/新建工作区并打开空白会话；
5. 更新流程：把插件改成 0.2.0 本地打 zip、用本地假 API（`python3 -m http.server` +
   `WS_RESOURCE_API_BASE`）验证「可更新」提示与更新安装。

- [ ] **Step 5: Commit**

```bash
git add scripts/dsh-plugin-install.sh docs/superpowers/specs/2026-11-10-deepseek-workspace-open-design.md
BLOB=$(git hash-object -w /tmp/readme_head.md); git update-index --cacheinfo 100644,$BLOB,README.md
git commit -m "docs: DeepSeek 插件改为独立仓库 + 设置资源，更新脚本与文档"
```

---

## 混合文件提交法（必须使用）

对同时含用户未提交 WIP 的文件（`src-tauri/src/lib.rs`、`src-tauri/build.rs`、
`src/components/OpenWithButton.vue`、`src/components/settings/SettingsPanel.vue`、`README.md`）：

```bash
# 1) 取 HEAD 版本到 /tmp
git show HEAD:src-tauri/src/lib.rs > /tmp/lib_head.rs

# 2) 在 /tmp 副本上做自己的改动，并核对 diff
diff -u <(git show HEAD:src-tauri/src/lib.rs) /tmp/lib_head.rs

# 3) 写入 blob 并只更新 index，不动工作区
git update-index --cacheinfo 100644,$(git hash-object -w /tmp/lib_head.rs),src-tauri/src/lib.rs

# 4) 只 add 干净文件，然后 commit（不要带路径）
git add src-tauri/src/commands/resources.rs
git commit -m "..."
```

工作区里的文件保持「用户 WIP + 自己的改动」的混合状态，不 `git add`、不 `checkout`。
提交后用 `git status --short` 与 `git diff <file> | grep '^-'` 核对：只应看到用户 WIP 的差异，
不应出现自己新代码被删。

## 实施记录（2026-11-10）

- 远端源改为「Gitee `dist` 分支 raw 优先、GitHub Release 兜底」（原计划只有 GitHub API）：
  国内直连 GitHub 慢/易失败；Gitee 侧只要 git 推送权限、不用 API 令牌，raw 直链实测 0.6s。
  环境变量换成 `WS_RESOURCE_GITEE_WEB/REPO/BRANCH` + `WS_RESOURCE_GITHUB_API_BASE/REPO`。
- 因此 Task 1-5 的 `WS_RESOURCE_API_BASE`、`releases/latest` 单源代码以实际实现为准。
