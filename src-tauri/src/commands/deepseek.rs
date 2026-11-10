//! 「用 DeepSeek 打开」：工作区请求文件 + DSH 插件状态/安装。
//!
//! ws-workbench 与 DSH 只通过 `$DSH_HOME/open-workspace.json` 通信：点菜单时写请求，
//! DSH 里的 `dsh-open-workspace` 插件消费后调 `workspaces.create` +
//! `uiWorkspace.openWorkspace`，落到左侧工作区。插件源码在
//! `dsh-plugin/dsh-open-workspace/`，用 include_str! 编进二进制，安装包里一定有。

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

/// 插件包名，同时是 `dsh.profile.bundles` 里的 id
pub const DSH_PLUGIN_NAME: &str = "dsh-open-workspace";
/// 请求文件名（放在 `$DSH_HOME` 下）
pub const REQUEST_FILE: &str = "open-workspace.json";
/// 依赖项写进 profile 清单时的 spec
const PLUGIN_LINK_SPEC: &str = "link:../../plugins/dsh-open-workspace";

/// 随二进制分发的插件文件（相对插件目录）。include_str! 让 dev 和安装包行为一致。
pub(crate) const PLUGIN_FILES: &[(&str, &str)] = &[
    (
        "package.json",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../dsh-plugin/dsh-open-workspace/package.json"
        )),
    ),
    (
        "cordis.patch.yml",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../dsh-plugin/dsh-open-workspace/cordis.patch.yml"
        )),
    ),
    (
        "lib/index.mjs",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../dsh-plugin/dsh-open-workspace/lib/index.mjs"
        )),
    ),
    (
        "lib/client.js",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../dsh-plugin/dsh-open-workspace/lib/client.js"
        )),
    ),
    (
        "README.md",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../dsh-plugin/dsh-open-workspace/README.md"
        )),
    ),
];

/// 插件状态：前端据此决定是否弹安装确认、装完是否提示重启
#[derive(Debug, Clone, Serialize)]
pub struct DeepseekPluginStatus {
    pub dsh_home: String,
    pub profile_ready: bool,
    pub plugin_installed: bool,
    pub installed_version: String,
    pub bundled_version: String,
    pub needs_update: bool,
    pub dsh_running: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OpenWorkspaceRequest {
    pub request_id: String,
    pub path: String,
    pub created_at: String,
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        })
}

/// `$DSH_HOME` 优先，和 DSH 的 `resolveDshHome` 规则一致
pub(crate) fn dsh_home() -> Result<PathBuf, String> {
    if let Some(raw) = std::env::var_os("DSH_HOME") {
        if !raw.is_empty() {
            return Ok(PathBuf::from(raw));
        }
    }
    home_dir()
        .map(|home| home.join(".dsh"))
        .ok_or_else(|| "找不到用户主目录（HOME / USERPROFILE）".to_string())
}

pub(crate) fn profile_dir(home: &Path) -> PathBuf {
    home.join("profiles").join("desktop")
}

pub(crate) fn request_path(home: &Path) -> PathBuf {
    home.join(REQUEST_FILE)
}

fn plugin_store_dir(home: &Path) -> PathBuf {
    home.join("plugins").join(DSH_PLUGIN_NAME)
}

fn plugin_active_dir(home: &Path) -> PathBuf {
    profile_dir(home).join("node_modules").join(DSH_PLUGIN_NAME)
}

fn bundled_version() -> String {
    PLUGIN_FILES
        .iter()
        .find(|(name, _)| *name == "package.json")
        .and_then(|(_, content)| serde_json::from_str::<serde_json::Value>(content).ok())
        .and_then(|value| {
            value
                .get("version")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "0.0.0".to_string())
}

fn installed_version(home: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(plugin_active_dir(home).join("package.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    value
        .get("version")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// profile 清单里是否同时登记了依赖和 bundle 项
pub(crate) fn manifest_contains_plugin(raw: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return false;
    };
    let dependency_ok = value
        .get("dependencies")
        .and_then(|deps| deps.get(DSH_PLUGIN_NAME))
        .is_some();
    let bundle_ok = value
        .get("dsh")
        .and_then(|dsh| dsh.get("profile"))
        .and_then(|profile| profile.get("bundles"))
        .and_then(|bundles| bundles.as_array())
        .is_some_and(|bundles| {
            bundles
                .iter()
                .any(|item| item.as_str() == Some(DSH_PLUGIN_NAME))
        });
    dependency_ok && bundle_ok
}

/// 把插件依赖和 bundle 项合进 profile 清单；返回（新清单, 是否有改动）。
pub(crate) fn merge_profile_manifest(raw: &str) -> Result<(String, bool), String> {
    let mut value: serde_json::Value = serde_json::from_str(raw)
        .map_err(|err| format!("DeepSeek 的 profile 清单不是合法 JSON: {err}"))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| "DeepSeek 的 profile 清单不是 JSON 对象".to_string())?;
    let mut changed = false;

    let dependencies = object
        .entry("dependencies")
        .or_insert_with(|| serde_json::json!({}));
    let dependencies = dependencies
        .as_object_mut()
        .ok_or_else(|| "profile 清单里的 dependencies 不是对象".to_string())?;
    if !dependencies.contains_key(DSH_PLUGIN_NAME) {
        dependencies.insert(
            DSH_PLUGIN_NAME.to_string(),
            serde_json::json!(PLUGIN_LINK_SPEC),
        );
        changed = true;
    }

    let dsh = object
        .entry("dsh")
        .or_insert_with(|| serde_json::json!({}));
    let dsh = dsh
        .as_object_mut()
        .ok_or_else(|| "profile 清单里的 dsh 不是对象".to_string())?;
    let profile = dsh
        .entry("profile")
        .or_insert_with(|| serde_json::json!({}));
    let profile = profile
        .as_object_mut()
        .ok_or_else(|| "profile 清单里的 dsh.profile 不是对象".to_string())?;
    let bundles = profile
        .entry("bundles")
        .or_insert_with(|| serde_json::json!([]));
    let bundles = bundles
        .as_array_mut()
        .ok_or_else(|| "profile 清单里的 dsh.profile.bundles 不是数组".to_string())?;
    if !bundles
        .iter()
        .any(|item| item.as_str() == Some(DSH_PLUGIN_NAME))
    {
        bundles.push(serde_json::json!(DSH_PLUGIN_NAME));
        changed = true;
    }

    let mut text = serde_json::to_string_pretty(&value).map_err(|err| err.to_string())?;
    text.push('\n');
    Ok((text, changed))
}

pub(crate) fn status_for_home(home: &Path) -> DeepseekPluginStatus {
    let manifest = profile_dir(home).join("package.json");
    let profile_ready = manifest.is_file();
    let manifest_has_plugin = std::fs::read_to_string(&manifest)
        .map(|raw| manifest_contains_plugin(&raw))
        .unwrap_or(false);
    let installed = installed_version(home);
    let bundled = bundled_version();
    let plugin_installed = profile_ready && manifest_has_plugin && installed.is_some();
    let needs_update = plugin_installed && installed.as_deref() != Some(bundled.as_str());
    DeepseekPluginStatus {
        dsh_home: home.display().to_string(),
        profile_ready,
        plugin_installed,
        installed_version: installed.unwrap_or_default(),
        bundled_version: bundled,
        needs_update,
        dsh_running: dsh_running(),
    }
}

/// 原子写：同目录临时文件 + rename（Windows 上 rename 不覆盖，会先删除目标）
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file");
    let temp = path.with_file_name(format!(".{file_name}.tmp"));
    {
        let mut file = std::fs::File::create(&temp)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
        }
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    #[cfg(windows)]
    if path.exists() {
        let _ = std::fs::remove_file(path);
    }
    std::fs::rename(&temp, path)?;
    Ok(())
}

pub(crate) fn write_open_request(home: &Path, target: &Path) -> Result<(), String> {
    if !target.is_dir() {
        return Err(format!("路径不存在或不是目录: {}", target.display()));
    }
    let request = OpenWorkspaceRequest {
        request_id: uuid::Uuid::new_v4().to_string(),
        path: target.display().to_string(),
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    let text = serde_json::to_string_pretty(&request).map_err(|err| err.to_string())?;
    write_atomic(&request_path(home), text.as_bytes())
        .map_err(|err| format!("写 DeepSeek 打开请求失败: {err}"))
}


fn write_plugin_files(dir: &Path) -> Result<(), String> {
    for (name, content) in PLUGIN_FILES {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("创建插件目录失败 {}: {err}", parent.display()))?;
        }
        std::fs::write(&path, content)
            .map_err(|err| format!("写插件文件失败 {}: {err}", path.display()))?;
    }
    Ok(())
}

/// 整目录替换：先写临时目录再换名，避免更新到一半把 DSH 启动路径写坏。
fn replace_plugin_dir(dir: &Path) -> Result<(), String> {
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
    write_plugin_files(&staging)?;
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

/// 安装/更新插件；profile 清单最后写，失败只回滚清单（插件目录先换也不影响启动）。
pub(crate) fn install_to_home(home: &Path) -> Result<DeepseekPluginStatus, String> {
    let profile = profile_dir(home);
    let manifest = profile.join("package.json");
    if !manifest.is_file() {
        return Err(
            "还没有初始化 DeepSeek 的 desktop profile：请先打开一次 DeepSeek Harness，再回来点「DeepSeek」。"
                .to_string(),
        );
    }
    let raw = std::fs::read_to_string(&manifest)
        .map_err(|err| format!("读 DeepSeek profile 清单失败: {err}"))?;
    let (next, _) = merge_profile_manifest(&raw)?;

    let backup = profile.join("package.json.ws-workbench.bak");
    if !backup.exists() {
        std::fs::copy(&manifest, &backup)
            .map_err(|err| format!("备份 DeepSeek profile 清单失败: {err}"))?;
    }

    replace_plugin_dir(&plugin_store_dir(home))?;
    replace_plugin_dir(&plugin_active_dir(home))?;
    if let Err(err) = write_atomic(&manifest, next.as_bytes()) {
        let _ = std::fs::copy(&backup, &manifest);
        return Err(format!("写 DeepSeek profile 清单失败: {err}"));
    }
    Ok(status_for_home(home))
}

/// 唤起 / 前置 DeepSeek Harness（不带目录参数；工作区交给插件消费请求文件后打开）
pub(crate) fn launch_deepseek(app: &tauri::AppHandle) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let _ = app;
        let bundle = crate::commands::deepseek_bundle_path()
            .ok_or_else(|| "DeepSeek Harness 没有安装".to_string())?;
        let output = std::process::Command::new("/usr/bin/open")
            .arg("-a")
            .arg(&bundle)
            .output()
            .map_err(|err| format!("启动 DeepSeek 失败: {err}"))?;
        if output.status.success() {
            return Ok(());
        }
        let reason = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(format!(
            "启动 DeepSeek 失败{}",
            if reason.is_empty() {
                String::new()
            } else {
                format!(": {reason}")
            }
        ))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let ctx = crate::commands::app_context(app);
        let location = crate::apps::locate(crate::apps::AppId::DeepSeek, &ctx, "");
        if !location.available {
            return Err(format!("DeepSeek Harness 没有安装。{}", location.note));
        }
        #[cfg(target_os = "windows")]
        {
            return crate::process::shell_open(Path::new(&location.path))
                .map_err(|err| format!("启动 DeepSeek 失败: {err}"));
        }
        #[cfg(not(target_os = "windows"))]
        {
            std::process::Command::new(&location.path)
                .spawn()
                .map(|_| ())
                .map_err(|err| format!("启动 DeepSeek 失败: {err}"))
        }
    }
}

/// 菜单 / 通用打开路径的入口：校验插件后写请求并启动 DSH
pub(crate) fn open_dir_workspace(app: &tauri::AppHandle, dir: &str) -> Result<(), String> {
    let home = dsh_home()?;
    let status = status_for_home(&home);
    if !status.profile_ready {
        return Err(
            "还没有初始化 DeepSeek 的 desktop profile：请先打开一次 DeepSeek Harness，再回来点「DeepSeek」。"
                .to_string(),
        );
    }
    if !status.plugin_installed || status.needs_update {
        return Err(
            "DeepSeek 工作区插件还没安装或需要更新：请回到「打开方式」重新点「DeepSeek」并按提示安装（装完需重启 DeepSeek）。"
                .to_string(),
        );
    }
    let target = crate::commands::nearest_existing_dir(dir);
    if !target.is_dir() {
        return Err(format!("路径不存在: {dir}"));
    }
    write_open_request(&home, &target)?;
    launch_deepseek(app)
}

#[tauri::command]
pub fn deepseek_plugin_status() -> Result<DeepseekPluginStatus, String> {
    let home = dsh_home()?;
    Ok(status_for_home(&home))
}

#[tauri::command]
pub fn install_deepseek_plugin() -> Result<DeepseekPluginStatus, String> {
    let home = dsh_home()?;
    install_to_home(&home)
}

#[tauri::command]
pub fn open_deepseek_workspace(
    app: tauri::AppHandle,
    path: String,
) -> Result<DeepseekPluginStatus, String> {
    let home = dsh_home()?;
    let target = crate::commands::nearest_existing_dir(&path);
    if !target.is_dir() {
        return Err(format!("路径不存在: {path}"));
    }
    open_dir_workspace(&app, &target.to_string_lossy())?;
    Ok(status_for_home(&home))
}

/// DSH 是否在运行（只影响提示文案）：看进程名里有没有 DeepSeek Harness
pub(crate) fn dsh_running() -> bool {
    let Ok(system) = sysinfo::System::new_all() else {
        return false;
    };
    system.processes().values().any(|process| {
        process
            .name()
            .to_string_lossy()
            .to_ascii_lowercase()
            .contains("deepseek harness")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn test_home(name: &str) -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-tmp")
            .join("deepseek")
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("profiles/desktop")).unwrap();
        std::fs::write(
            dir.join("profiles/desktop/package.json"),
            r#"{
  "name": "dsh-profile-desktop",
  "private": true,
  "dsh": {
    "profile": {
      "bundles": ["@deepseek-ai/dsh-base", "dsh-chime"]
    }
  },
  "dependencies": {
    "dsh-chime": "^0.1.3"
  }
}
"#,
        )
        .unwrap();
        dir
    }

    #[test]
    fn merge_profile_manifest_adds_plugin_and_bundle() {
        let raw = r#"{"name":"dsh-profile-desktop","private":true,"dsh":{"profile":{"bundles":["@deepseek-ai/dsh-base"]}},"dependencies":{}}"#;
        let (next, changed) = merge_profile_manifest(raw).unwrap();
        assert!(changed);
        assert!(manifest_contains_plugin(&next));
        assert!(next.contains("link:../../plugins/dsh-open-workspace"));
    }

    #[test]
    fn merge_profile_manifest_is_idempotent() {
        let raw = r#"{"name":"x","dsh":{"profile":{"bundles":["a"]}},"dependencies":{}}"#;
        let (first, changed) = merge_profile_manifest(raw).unwrap();
        assert!(changed);
        let (second, changed_again) = merge_profile_manifest(&first).unwrap();
        assert!(!changed_again);
        assert_eq!(first, second);
    }

    #[test]
    fn merge_profile_manifest_keeps_other_entries() {
        let raw = r#"{"dsh":{"profile":{"bundles":["dsh-chime"]}},"dependencies":{"dsh-chime":"^0.1.3"}}"#;
        let (next, _) = merge_profile_manifest(raw).unwrap();
        let value: serde_json::Value = serde_json::from_str(&next).unwrap();
        assert_eq!(value["dependencies"]["dsh-chime"], "^0.1.3");
        assert_eq!(value["dsh"]["profile"]["bundles"][0], "dsh-chime");
        assert_eq!(value["dsh"]["profile"]["bundles"][1], "dsh-open-workspace");
    }

    #[test]
    fn merge_profile_manifest_rejects_broken_json() {
        assert!(merge_profile_manifest("{oops").is_err());
    }

    #[test]
    fn bundled_plugin_files_are_complete() {
        assert!(!PLUGIN_FILES.is_empty());
        for (name, content) in PLUGIN_FILES {
            assert!(!content.trim().is_empty(), "{} 是空的", name);
        }
        let package = PLUGIN_FILES
            .iter()
            .find(|(name, _)| *name == "package.json")
            .map(|(_, content)| *content)
            .expect("插件包必须有 package.json");
        let value: serde_json::Value = serde_json::from_str(package).unwrap();
        assert_eq!(value["name"], "dsh-open-workspace");
        assert!(value["dsh"]["bundle"]["patch"].is_string());
        assert_eq!(value["dsh"]["client"]["platform"], "web");
    }

    #[test]
    fn status_reports_missing_plugin() {
        let home = test_home("status-missing");
        let status = status_for_home(&home);
        assert!(status.profile_ready);
        assert!(!status.plugin_installed);
        assert_eq!(status.installed_version, "");
        assert!(!status.needs_update);
        assert_eq!(status.dsh_home, home.display().to_string());
    }

    #[test]
    fn write_open_request_writes_fresh_json() {
        let home = test_home("request");
        let target = home.join("project");
        std::fs::create_dir_all(&target).unwrap();
        write_open_request(&home, &target).unwrap();
        let raw = std::fs::read_to_string(request_path(&home)).unwrap();
        let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(value["path"], target.display().to_string());
        assert!(value["requestId"].as_str().is_some_and(|id| !id.is_empty()));
        assert!(value["createdAt"].as_str().is_some_and(|t| !t.is_empty()));
    }

    #[test]
    fn write_open_request_rejects_missing_dir() {
        let home = test_home("request-missing");
        assert!(write_open_request(&home, &home.join("nope")).is_err());
    }

    #[test]
    fn install_creates_plugin_files_and_manifest() {
        let home = test_home("install");
        let status = install_to_home(&home).unwrap();
        assert!(status.plugin_installed);
        assert_eq!(status.installed_version, status.bundled_version);
        assert!(!status.needs_update);
        for (name, content) in PLUGIN_FILES {
            let installed = std::fs::read_to_string(plugin_active_dir(&home).join(name)).unwrap();
            assert_eq!(&installed, content, "{} 内容不一致", name);
        }
        assert!(plugin_store_dir(&home).join("package.json").is_file());
        let manifest = std::fs::read_to_string(profile_dir(&home).join("package.json")).unwrap();
        assert!(manifest_contains_plugin(&manifest));
        assert!(profile_dir(&home)
            .join("package.json.ws-workbench.bak")
            .is_file());
    }

    #[test]
    fn install_twice_is_idempotent() {
        let home = test_home("install-twice");
        install_to_home(&home).unwrap();
        install_to_home(&home).unwrap();
        let manifest = std::fs::read_to_string(profile_dir(&home).join("package.json")).unwrap();
        let value: serde_json::Value = serde_json::from_str(&manifest).unwrap();
        let bundles = value["dsh"]["profile"]["bundles"].as_array().unwrap();
        assert_eq!(
            bundles
                .iter()
                .filter(|item| item.as_str() == Some(DSH_PLUGIN_NAME))
                .count(),
            1
        );
        let deps = value["dependencies"].as_object().unwrap();
        assert_eq!(deps.len(), 2);
    }

    #[test]
    fn install_refuses_without_profile() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-tmp")
            .join("deepseek")
            .join("no-profile");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let error = install_to_home(&dir).unwrap_err();
        assert!(error.contains("先打开一次 DeepSeek"));
    }

    #[test]
    fn install_leaves_manifest_untouched_on_broken_json() {
        let home = test_home("broken-manifest");
        let manifest = profile_dir(&home).join("package.json");
        std::fs::write(&manifest, "{oops").unwrap();
        assert!(install_to_home(&home).is_err());
        assert_eq!(std::fs::read_to_string(&manifest).unwrap(), "{oops");
        assert!(!plugin_store_dir(&home).exists());
    }

    #[test]
    fn backup_is_kept_from_first_install() {
        let home = test_home("backup");
        let manifest = profile_dir(&home).join("package.json");
        let original = std::fs::read_to_string(&manifest).unwrap();
        install_to_home(&home).unwrap();
        let backup = profile_dir(&home).join("package.json.ws-workbench.bak");
        assert_eq!(std::fs::read_to_string(backup).unwrap(), original);
    }

}
