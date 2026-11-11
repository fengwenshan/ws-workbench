//! 「设置 → 资源」：可下载资源的远端版本检查、下载、解压；DSH 文件安装复用 deepseek 模块。
//! 本期实现 DeepSeek 工作区插件；Chrome 插件只占位。
//!
//! 版本检查与下载走多源：默认 **Gitee 优先、GitHub 兜底**（国内直连 GitHub 容易超时）。
//! Gitee 走 `raw/dist` 分支上的 `latest.json` + zip 直链（不用 API 令牌、下载快）；
//! GitHub 走 Release API。两边都按同名 zip 约定，版本取各源里最高的那个。

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::commands::deepseek::{dsh_home, dsh_running};

pub const DSH_PLUGIN_ID: &str = "dsh-open-workspace";
/// GitHub 仓库（兜底源）
pub const DSH_PLUGIN_REPO: &str = "fengwenshan/dsh-open-workspace";
/// Gitee 镜像仓库（优先源，国内下载快）
pub const DSH_PLUGIN_GITEE_REPO: &str = "feng_wenshan/dsh-open-workspace";
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// Gitee 仓库的 `dist` 分支：`latest.json` + `<id>-<version>.zip` 走 raw 直链
    Gitee,
    /// GitHub Release API（asset 要 Accept: application/octet-stream）
    Github,
}

/// 一个 Release 来源：两个源取版本/下载的方式不同。
#[derive(Debug, Clone)]
pub struct Source {
    pub kind: SourceKind,
    /// Gitee: 网站根（拼 raw 直链）；GitHub: API 根
    pub base: String,
    pub repo: String,
    /// Gitee raw 分支（GitHub 忽略）
    pub branch: String,
}

impl Source {
    pub fn label(&self) -> &'static str {
        match self.kind {
            SourceKind::Gitee => "Gitee",
            SourceKind::Github => "GitHub",
        }
    }

    /// Gitee 的 raw 文件直链（`.../raw/<branch>/<path>`）。
    fn raw_url(&self, path: &str) -> String {
        format!(
            "{}/{}/raw/{}/{}",
            self.base.trim_end_matches('/'),
            self.repo,
            self.branch,
            path
        )
    }
}

/// 默认源顺序：`WS_RESOURCE_SOURCES` 可覆盖（逗号分隔，如 `github`），测试与排障用。
fn default_sources() -> Vec<Source> {
    let gitee_web = std::env::var("WS_RESOURCE_GITEE_WEB")
        .unwrap_or_else(|_| "https://gitee.com".to_string());
    let github_api = std::env::var("WS_RESOURCE_GITHUB_API_BASE")
        .unwrap_or_else(|_| "https://api.github.com".to_string());
    let gitee_repo = std::env::var("WS_RESOURCE_GITEE_REPO")
        .unwrap_or_else(|_| DSH_PLUGIN_GITEE_REPO.to_string());
    let github_repo = std::env::var("WS_RESOURCE_GITHUB_REPO")
        .unwrap_or_else(|_| DSH_PLUGIN_REPO.to_string());
    let gitee_branch = std::env::var("WS_RESOURCE_GITEE_BRANCH").unwrap_or_else(|_| "dist".to_string());
    let order =
        std::env::var("WS_RESOURCE_SOURCES").unwrap_or_else(|_| "gitee,github".to_string());
    let mut sources = Vec::new();
    for part in order.split(',').map(str::trim) {
        match part {
            "gitee" => sources.push(Source {
                kind: SourceKind::Gitee,
                base: gitee_web.clone(),
                repo: gitee_repo.clone(),
                branch: gitee_branch.clone(),
            }),
            "github" => sources.push(Source {
                kind: SourceKind::Github,
                base: github_api.clone(),
                repo: github_repo.clone(),
                branch: String::new(),
            }),
            _ => {}
        }
    }
    sources
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReleaseDto {
    pub tag_name: String,
    #[serde(default)]
    pub assets: Vec<AssetDto>,
}

/// 两个源的附件字段不一样：GitHub 给 API asset 地址（要 Accept: octet-stream），
/// Gitee 直接给可下载的 `browser_download_url`。
#[derive(Debug, Clone, Deserialize)]
pub struct AssetDto {
    pub name: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub browser_download_url: Option<String>,
}

impl AssetDto {
    fn download_url(&self, kind: SourceKind) -> Option<String> {
        match kind {
            SourceKind::Gitee => self
                .browser_download_url
                .clone()
                .or_else(|| self.url.clone()),
            SourceKind::Github => self.url.clone(),
        }
    }
}

pub(crate) fn asset_for<'a>(release: &'a ReleaseDto, version: &str) -> Option<&'a AssetDto> {
    let wanted = format!("{DSH_PLUGIN_ID}-{version}.zip");
    release.assets.iter().find(|asset| asset.name == wanted)
}

/// 真实可用的下载候选：`(来源, 附件)`，按来源顺序排（Gitee 在前）。
#[derive(Debug, Clone)]
pub struct Remote {
    pub version: String,
    pub candidates: Vec<(Source, AssetDto)>,
}

type RemoteResult = Result<Remote, String>;
static REMOTE_CACHE: Mutex<Option<(Instant, RemoteResult)>> = Mutex::new(None);

/// 用系统 curl 取文本；macOS / Windows 10+ 自带 curl。
fn curl_text(url: &str) -> Result<String, String> {
    let output = crate::process::hidden("curl")
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

pub(crate) fn fetch_release(source: &Source) -> Result<ReleaseDto, String> {
    match source.kind {
        SourceKind::Gitee => fetch_gitee_manifest(source),
        SourceKind::Github => fetch_github_release(source),
    }
}

/// Gitee 源：读 `dist` 分支的 `latest.json`，版本和 zip 直链都从里面推。
fn fetch_gitee_manifest(source: &Source) -> Result<ReleaseDto, String> {
    let body = curl_text(&source.raw_url("latest.json"))?;
    let manifest: serde_json::Value = serde_json::from_str(&body)
        .map_err(|err| format!("解析 Gitee latest.json 失败: {err}"))?;
    let version = manifest
        .get("version")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .trim_start_matches('v')
        .to_string();
    if version.is_empty() {
        return Err("Gitee latest.json 缺少 version".to_string());
    }
    let name = format!("{DSH_PLUGIN_ID}-{version}.zip");
    Ok(ReleaseDto {
        tag_name: version,
        assets: vec![AssetDto {
            name: name.clone(),
            url: None,
            browser_download_url: Some(source.raw_url(&name)),
        }],
    })
}

/// GitHub 源：`releases/latest`，asset 地址要带 Accept: octet-stream 才能下到二进制。
fn fetch_github_release(source: &Source) -> Result<ReleaseDto, String> {
    let url = format!(
        "{}/repos/{}/releases/latest",
        source.base.trim_end_matches('/'),
        source.repo
    );
    let body = curl_text(&url)?;
    serde_json::from_str(&body).map_err(|err| format!("解析 GitHub Release 失败: {err}"))
}

/// 查所有源：版本取最高的那个；下载候选按来源顺序（Gitee 优先）。
pub(crate) fn resolve_remote_at(sources: &[Source]) -> Result<Remote, String> {
    let mut found: Vec<(Source, String, Option<AssetDto>)> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    for source in sources {
        match fetch_release(source) {
            Ok(release) => {
                let version = release.tag_name.trim_start_matches('v').to_string();
                if version.is_empty() {
                    errors.push(format!("{}: Release 没有 tag", source.label()));
                    continue;
                }
                let asset = asset_for(&release, &version).cloned();
                found.push((source.clone(), version, asset));
            }
            Err(err) => errors.push(format!("{}: {err}", source.label())),
        }
    }
    if found.is_empty() {
        return Err(format!(
            "检查插件最新版本失败（{}）",
            errors.join("；")
        ));
    }
    let latest = found
        .iter()
        .map(|(_, version, _)| version.clone())
        .reduce(|current, next| {
            if version_newer(&next, &current) {
                next
            } else {
                current
            }
        })
        .unwrap_or_default();
    let mut candidates = Vec::new();
    for (source, version, asset) in found {
        if version == latest {
            if let Some(asset) = asset {
                candidates.push((source, asset));
            }
        }
    }
    if candidates.is_empty() {
        return Err(format!(
            "Release v{latest} 里没有插件 zip 附件（Gitee / GitHub 都没有）"
        ));
    }
    Ok(Remote {
        version: latest,
        candidates,
    })
}

/// 取最新版本（5 分钟缓存；force=true 忽略缓存）。
pub(crate) fn resolve_remote(force: bool) -> Result<Remote, String> {
    let mut guard = REMOTE_CACHE.lock().unwrap();
    if !force {
        if let Some((at, cached)) = guard.as_ref() {
            if at.elapsed() < REMOTE_TTL {
                return cached.clone();
            }
        }
    }
    let result = resolve_remote_at(&default_sources());
    *guard = Some((Instant::now(), result.clone()));
    result
}

/// 最新版本号（从缓存/远端 Release tag 取）。
pub(crate) fn latest_plugin_version(force: bool) -> Result<String, String> {
    resolve_remote(force).map(|remote| remote.version)
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

/// 下载一个候选附件；Gitee 是普通直链，GitHub 的 API asset 地址要带 Accept 头。
///
/// 直接读 curl 的 stdout，不落临时文件：Windows 上 `%TEMP%` 可能被受控文件夹
/// 访问、企业策略或沙箱设成不可写，`curl -o %TEMP%\...` 会以退出码 23 失败，
/// 但网络其实是通的（版本检查能成功、下载却两个源都报错，现场就是这个现象）。
fn download_asset(source: &Source, asset: &AssetDto) -> Result<Vec<u8>, String> {
    let url = asset
        .download_url(source.kind)
        .ok_or_else(|| format!("{} 附件缺少下载地址", asset.name))?;
    let mut command = crate::process::hidden("curl");
    command.args(["-sSfL", "--max-time", "120"]);
    if source.kind == SourceKind::Github {
        command.args(["-H", "Accept: application/octet-stream"]);
    }
    let output = command
        .arg(&url)
        .output()
        .map_err(|err| format!("系统缺少 curl 或无法执行: {err}"))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        let detail = detail.trim();
        return Err(if detail.is_empty() {
            format!("下载 {} 失败: {}", asset.name, source.label())
        } else {
            format!("下载 {} 失败: {}（{}）", asset.name, source.label(), detail)
        });
    }
    let bytes = output.stdout;
    if bytes.is_empty() {
        return Err(format!(
            "从 {} 下载到的插件包是空文件",
            source.label()
        ));
    }
    Ok(bytes)
}

/// 按候选顺序下载：Gitee 失败自动退到 GitHub，全部失败才报错。
fn download_plugin_zip(remote: &Remote) -> Result<Vec<u8>, String> {
    let mut errors = Vec::new();
    for (source, asset) in &remote.candidates {
        match download_asset(source, asset) {
            Ok(bytes) => return Ok(bytes),
            Err(err) => errors.push(err),
        }
    }
    Err(format!("下载插件包失败（{}）", errors.join("；")))
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

/// 资源清单：DeepSeek 插件那一条要联网查最新版本（`curl`，超时 8 秒 ×2 个源），
/// 所以必须离开主线程，否则打开「设置 → 资源」时窗口直接卡住。
#[tauri::command]
pub async fn resource_list(force: bool) -> Result<Vec<ResourceStatus>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let home = dsh_home()?;
        Ok(vec![dsh_plugin_status(&home, force), chrome_placeholder()])
    })
    .await
    .map_err(|err| format!("读取资源列表失败: {err}"))?
}

/// 安装要下载 zip（可能十几秒），同样放后台线程。
#[tauri::command]
pub async fn resource_install(id: String) -> Result<ResourceStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        if id != DSH_PLUGIN_ID {
            return Err(format!("资源 {id} 暂不支持安装"));
        }
        let home = dsh_home()?;
        let remote = resolve_remote(true)?;
        let bytes = download_plugin_zip(&remote)?;
        install_from_zip_bytes(&bytes, &home)?;
        let mut status = dsh_plugin_status(&home, false);
        if dsh_running() {
            status.note = Some("已安装，需要重启 DeepSeek Harness 生效".to_string());
        }
        Ok(status)
    })
    .await
    .map_err(|err| format!("安装资源失败: {err}"))?
}

#[tauri::command]
pub async fn resource_uninstall(id: String) -> Result<ResourceStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
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
    })
    .await
    .map_err(|err| format!("卸载资源失败: {err}"))?
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

    fn gitee_source(base: &str) -> Source {
        Source {
            kind: SourceKind::Gitee,
            base: base.to_string(),
            repo: DSH_PLUGIN_GITEE_REPO.to_string(),
            branch: "dist".to_string(),
        }
    }

    fn github_source(base: &str) -> Source {
        Source {
            kind: SourceKind::Github,
            base: base.to_string(),
            repo: DSH_PLUGIN_REPO.to_string(),
            branch: String::new(),
        }
    }

    #[test]
    fn github_asset_uses_api_url() {
        let github: ReleaseDto = serde_json::from_str(
            r#"{"tag_name":"v0.2.0","assets":[{"name":"dsh-open-workspace-0.2.0.zip","url":"https://api.github.com/repos/x/y/releases/assets/1"}]}"#,
        )
        .unwrap();
        let asset = asset_for(&github, "0.2.0").unwrap();
        assert_eq!(
            asset.download_url(SourceKind::Github).unwrap(),
            "https://api.github.com/repos/x/y/releases/assets/1"
        );
        assert!(asset_for(&github, "0.1.0").is_none());
    }

    #[test]
    fn gitee_manifest_synthesizes_raw_download_url() {
        let zip = fixture_zip("0.2.0");
        let (base, _handle) = spawn_gitee_server("0.2.0", zip);
        let release = fetch_release(&gitee_source(&base)).unwrap();
        assert_eq!(release.tag_name, "0.2.0");
        let asset = asset_for(&release, "0.2.0").unwrap();
        assert_eq!(
            asset.download_url(SourceKind::Gitee).unwrap(),
            format!(
                "{base}/{}/raw/dist/dsh-open-workspace-0.2.0.zip",
                DSH_PLUGIN_GITEE_REPO
            )
        );
    }

    #[test]
    fn resolve_prefers_gitee_for_same_version() {
        let zip = fixture_zip("0.2.0");
        let (gitee_base, _gitee) = spawn_gitee_server("0.2.0", zip);
        let (github_base, _github) = spawn_github_server("0.2.0", true);
        let sources = vec![gitee_source(&gitee_base), github_source(&github_base)];
        let remote = resolve_remote_at(&sources).unwrap();
        assert_eq!(remote.version, "0.2.0");
        assert_eq!(remote.candidates.len(), 2);
        assert_eq!(remote.candidates[0].0.kind, SourceKind::Gitee);
    }

    #[test]
    fn resolve_takes_newer_version_and_falls_back() {
        // Gitee 还停在 0.1.0（没镜像完），GitHub 已经是 0.2.0
        let (gitee_base, _gitee) = spawn_gitee_server("0.1.0", fixture_zip("0.1.0"));
        let (github_base, _github) = spawn_github_server("0.2.0", true);
        let sources = vec![gitee_source(&gitee_base), github_source(&github_base)];
        let remote = resolve_remote_at(&sources).unwrap();
        assert_eq!(remote.version, "0.2.0");
        assert_eq!(remote.candidates.len(), 1);
        assert_eq!(remote.candidates[0].0.kind, SourceKind::Github);

        // Gitee 不可用：只用 GitHub 也能查
        let (github_base, _github) = spawn_github_server("0.2.0", true);
        let dead = "http://127.0.0.1:1";
        let sources = vec![gitee_source(dead), github_source(&github_base)];
        let remote = resolve_remote_at(&sources).unwrap();
        assert_eq!(remote.version, "0.2.0");
        assert_eq!(remote.candidates[0].0.kind, SourceKind::Github);
    }

    #[test]
    fn resolve_reports_missing_zip_asset() {
        let (github_base, _github) = spawn_github_server("0.2.0", false);
        let sources = vec![github_source(&github_base)];
        let error = resolve_remote_at(&sources).unwrap_err();
        assert!(error.contains("没有插件 zip 附件"), "{error}");
    }

    #[test]
    fn download_and_install_from_gitee_raw() {
        let zip = fixture_zip("0.2.0");
        let (base, _handle) = spawn_gitee_server("0.2.0", zip.clone());
        let sources = vec![gitee_source(&base)];
        let remote = resolve_remote_at(&sources).unwrap();
        let bytes = download_plugin_zip(&remote).unwrap();
        assert_eq!(bytes, zip);

        let (version, _files) = read_plugin_zip(&bytes).unwrap();
        assert_eq!(version, "0.2.0");
        let home = test_home("gitee-install");
        install_from_zip_bytes(&bytes, &home).unwrap();
        let (installed, installed_version) = crate::commands::deepseek::local_plugin_status(&home);
        assert!(installed);
        assert_eq!(installed_version.as_deref(), Some("0.2.0"));
        let _ = std::fs::remove_dir_all(&home);
    }

    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::PathBuf;
    use std::thread::JoinHandle;

    /// 假 Gitee：`.../raw/dist/latest.json` 返回版本，`.../<zip>` 返回 zip 字节，其他 404。
    fn spawn_gitee_server(version: &str, zip: Vec<u8>) -> (String, JoinHandle<()>) {
        let version = version.to_string();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let base = format!("http://127.0.0.1:{port}");
        let handle = std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut request = [0u8; 4096];
                let read = stream.read(&mut request).unwrap_or(0);
                let request = String::from_utf8_lossy(&request[..read]).to_string();
                let path = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("")
                    .to_string();
                let (status, body, ctype): (&str, Vec<u8>, &str) = if path.ends_with("latest.json")
                {
                    (
                        "200 OK",
                        format!(r#"{{"version":"{version}","file":"dsh-open-workspace-{version}.zip"}}"#)
                            .into_bytes(),
                        "application/json",
                    )
                } else if path.ends_with(".zip") {
                    ("200 OK", zip.clone(), "application/zip")
                } else {
                    ("404 Not Found", Vec::new(), "text/plain")
                };
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        (base, handle)
    }

    /// 假 GitHub Release API：返回 tag 与（可选）zip 附件。
    fn spawn_github_server(version: &str, with_asset: bool) -> (String, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let base = format!("http://127.0.0.1:{port}");
        let assets = if with_asset {
            format!(
                r#"[{{"name":"dsh-open-workspace-{version}.zip","url":"{base}/asset/{version}"}}]"#
            )
        } else {
            "[]".to_string()
        };
        let body = format!(r#"{{"tag_name":"v{version}","assets":{assets}}}"#);
        let handle = std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut request = [0u8; 2048];
                let _ = stream.read(&mut request);
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(body.as_bytes());
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
