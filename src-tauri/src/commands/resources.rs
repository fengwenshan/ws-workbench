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
        description: "让「打开方式 → DeepSeek」把当前目录作为工作区打开（本地小插件，离线运行）"
            .to_string(),
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

    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::PathBuf;
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

    fn fixture_zip(version: &str) -> Vec<u8> {
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

    fn test_home(name: &str) -> PathBuf {
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

        for name in [
            "package.json",
            "cordis.patch.yml",
            "lib/index.mjs",
            "lib/client.js",
            "README.md",
        ] {
            assert!(
                home.join("plugins/dsh-open-workspace").join(name).exists(),
                "store 缺 {name}"
            );
            assert!(
                home
                    .join("profiles/desktop/node_modules/dsh-open-workspace")
                    .join(name)
                    .exists(),
                "profile 缺 {name}"
            );
        }
        let (installed, version) = crate::commands::deepseek::local_plugin_status(&home);
        assert!(installed);
        assert_eq!(version.as_deref(), Some("0.2.0"));

        let manifest = std::fs::read_to_string(home.join("profiles/desktop/package.json")).unwrap();
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
        assert!(home
            .join("profiles/desktop/package.json.ws-workbench.bak")
            .exists());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn install_rejects_broken_zip_without_touching_dsh() {
        let home = test_home("broken");
        let mut bytes = fixture_zip("0.2.0");
        bytes.truncate(bytes.len() / 2);
        assert!(install_from_zip_bytes(&bytes, &home).is_err());
        assert!(!home.join("plugins/dsh-open-workspace").exists());
        let manifest = std::fs::read_to_string(home.join("profiles/desktop/package.json")).unwrap();
        assert!(!manifest.contains("dsh-open-workspace"));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn uninstall_removes_files_and_manifest_entries() {
        let home = test_home("uninstall");
        install_from_zip_bytes(&fixture_zip("0.2.0"), &home).unwrap();
        uninstall_plugin(&home).unwrap();

        assert!(!home.join("plugins/dsh-open-workspace").exists());
        assert!(!home
            .join("profiles/desktop/node_modules/dsh-open-workspace")
            .exists());
        assert!(!crate::commands::deepseek::local_plugin_status(&home).0);
        let manifest = std::fs::read_to_string(home.join("profiles/desktop/package.json")).unwrap();
        assert!(!manifest.contains("dsh-open-workspace"));
        let _ = std::fs::remove_dir_all(&home);
    }
}
