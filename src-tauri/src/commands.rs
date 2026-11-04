use std::path::{Path, PathBuf};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use tauri::{http, AppHandle, Emitter, Manager};
use uuid::Uuid;
use crate::apps::{self, AppContext, AppId, AppLocation, AppPathOverride};
use crate::config;
use crate::filecrypto;
use crate::packer;
use crate::platform::PlatformInfo;
use crate::models::*;

#[tauri::command]
pub fn get_config(app: AppHandle) -> AppConfig {
    config::load_config(&app)
}

#[tauri::command]
pub fn save_config(app: AppHandle, config: AppConfig) -> Result<(), String> {
    config::save_config(&app, &config)
}

#[tauri::command]
pub fn get_config_path(app: AppHandle) -> Result<String, String> {
    config::get_config_path(&app)
}

/// 本地存储信息（设置页展示用）
#[derive(serde::Serialize)]
pub struct StorageInfo {
    pub db_path: String,
    pub settings_path: String,
    pub ok: bool,
    pub error: String,
}

#[tauri::command]
pub fn storage_info(app: AppHandle) -> StorageInfo {
    let mut errors: Vec<String> = Vec::new();
    let db_path = match crate::db::check(&app) {
        Ok(path) => path.to_string_lossy().to_string(),
        Err(err) => {
            errors.push(format!("数据库: {}", err));
            "—".to_string()
        }
    };
    let settings_path = match crate::config::settings_path(&app) {
        Ok(path) => path,
        Err(err) => {
            errors.push(format!("设置: {}", err));
            "—".to_string()
        }
    };
    StorageInfo {
        db_path,
        settings_path,
        ok: errors.is_empty(),
        error: errors.join("; "),
    }
}

/// 加密任意字符串（供前端加密本地键值使用，复用后端主密钥）
#[tauri::command]
pub fn vault_encrypt(app: AppHandle, plain: String) -> Result<String, String> {
    crate::crypto::encrypt_str(&app, &plain)
}

/// 解密由 `vault_encrypt` 产生的密文
#[tauri::command]
pub fn vault_decrypt(app: AppHandle, cipher: String) -> Result<String, String> {
    crate::crypto::decrypt_str(&app, &cipher)
}

#[tauri::command]
pub fn get_default_exclude_rules() -> Vec<String> {
    default_exclude_rules()
}

#[tauri::command]
pub async fn pack_project(
    app: AppHandle,
    project: ProjectConfig,
    default_exclude: Vec<String>,
) -> Result<PackResult, String> {
    packer::pack_project(&app, &project, &default_exclude)
}

#[tauri::command]
pub async fn pack_to_zip(
    app: AppHandle,
    project: ProjectConfig,
    default_exclude: Vec<String>,
) -> Result<PackResult, String> {
    packer::pack_to_zip(&app, &project, &default_exclude)
}

#[tauri::command]
pub fn validate_project(project: ProjectConfig) -> ProjectValidation {
    packer::validate_project(&project)
}

#[tauri::command]
pub fn detect_project_type(source_dir: String) -> ProjectType {
    packer::detect_project_type(&source_dir)
}

#[tauri::command]
pub fn detect_vcs(source_dir: String) -> VcsInfo {
    packer::detect_vcs(&source_dir)
}

// ===== 目录打开相关 =====

/// 查找最近的存在目录：若路径不存在，则逐级向上查找直至找到存在的父目录
fn nearest_existing_dir(path: &str) -> PathBuf {
    let mut cur = PathBuf::from(path);
    loop {
        if cur.exists() {
            return cur;
        }
        match cur.parent() {
            // 已到根目录仍不存在，返回原路径交由调用方报错
            Some(p) if p.as_os_str().is_empty() => return PathBuf::from(path),
            Some(p) => {
                let parent = p.to_path_buf();
                if parent == cur {
                    return PathBuf::from(path);
                }
                cur = parent;
            }
            None => return PathBuf::from(path),
        }
    }
}

fn open_path(path: &str) -> Result<(), String> {
    if !Path::new(path).exists() {
        return Err(format!("路径不存在: {}", path));
    }
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(target_os = "windows")]
    let cmd = "explorer";
    #[cfg(all(unix, not(target_os = "macos")))]
    let cmd = "xdg-open";

    std::process::Command::new(cmd)
        .arg(path)
        .spawn()
        .map_err(|e| format!("打开失败: {}", e))?;
    Ok(())
}

/// 用指定应用打开目录。
///
/// 可执行文件的解析全部交给 [`apps::locate`]：
/// 设置页指定的路径 > 程序托管副本 > 内置资源 > Windows 注册表 > 常见安装目录 > PATH。
/// 六条路都找不到时，macOS 上再用 `open -a` 按应用名兜一次
/// （那里应用是 .app 包，很多没有可执行入口）。
pub fn open_with_app(app: &AppHandle, id: AppId, path: &str) -> Result<(), String> {
    if !Path::new(path).exists() {
        return Err(format!("路径不存在: {}", path));
    }

    let ctx = app_context(app);
    let location = apps::locate(id, &ctx, "");

    if location.available {
        return apps::finder::spawn_with_target(Path::new(&location.path), path);
    }

    #[cfg(target_os = "macos")]
    {
        for name in macos_app_names(id) {
            let result = std::process::Command::new("/usr/bin/open")
                .arg("-a")
                .arg(name)
                .arg(path)
                .output();
            if let Ok(output) = result {
                if output.status.success() {
                    return Ok(());
                }
            }
        }
    }

    Err(format!("未找到 {}。{}", id.display_name(), location.note))
}

/// 组装查找上下文：应用数据目录 / 资源目录 / 可执行文件目录 + 设置页里的路径覆盖。
///
/// 覆盖项放进来之后，项目打包页、VPN 页、设置页三处共享同一份设置。
pub fn app_context(app: &AppHandle) -> AppContext {
    let config = config::load_config(app);
    AppContext {
        data_dir: app.path().app_data_dir().ok(),
        resource_dir: app.path().resource_dir().ok(),
        exe_dir: std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(|parent| parent.to_path_buf())),
        overrides: config.app_paths,
    }
}

/// macOS 上用 open -a 时按顺序尝试的应用名
#[cfg(target_os = "macos")]
fn macos_app_names(id: AppId) -> Vec<&'static str> {
    match id {
        AppId::Vscode => vec!["Visual Studio Code", "Visual Studio Code - Insiders"],
        AppId::Idea => vec![
            "IntelliJ IDEA Ultimate",
            "IntelliJ IDEA",
            "IntelliJ IDEA CE",
        ],
        AppId::Trae => vec!["Trae CN", "TRAE SOLO CN", "Trae"],
        AppId::Openconnect => vec![],
    }
}

/// 打开目录；若目录不存在，自动降级打开最近存在的上级目录
/// 返回实际打开的路径，供前端提示
#[tauri::command]
pub fn open_dir(path: String) -> Result<String, String> {
    let target = nearest_existing_dir(&path);
    let target_str = target.to_string_lossy().to_string();
    // 若最终仍不存在，给出明确错误
    if !target.is_dir() {
        return Err(format!("路径不存在: {}", path));
    }
    open_path(&target_str)?;
    Ok(target_str)
}

#[tauri::command]
pub fn open_parent_dir(path: String) -> Result<(), String> {
    let parent = Path::new(&path)
        .parent()
        .ok_or_else(|| format!("无法获取上级目录: {}", path))?
        .to_string_lossy()
        .to_string();
    open_path(&parent)
}

#[tauri::command]
pub fn open_in_vscode(app: AppHandle, path: String) -> Result<(), String> {
    open_with_app(&app, AppId::Vscode, &path)
}

#[tauri::command]
pub fn open_in_idea(app: AppHandle, path: String) -> Result<(), String> {
    open_with_app(&app, AppId::Idea, &path)
}

#[tauri::command]
pub fn open_in_trae(app: AppHandle, path: String) -> Result<(), String> {
    open_with_app(&app, AppId::Trae, &path)
}

/// 通用入口：按 app_id 打开目录，设置页的「测试启动」用它
#[tauri::command]
pub fn open_with_app_id(app: AppHandle, app_id: String, path: String) -> Result<(), String> {
    let id = AppId::from_str(&app_id).ok_or_else(|| format!("未知的应用标识: {}", app_id))?;
    open_with_app(&app, id, &path)
}

// ============================== 应用定位与路径设置 ==============================

/// 平台信息：前端据此决定是否展示 Windows 专有的设置项
#[tauri::command]
pub fn get_platform_info() -> PlatformInfo {
    PlatformInfo::current()
}

/// 列出所有外部应用的定位结果
#[tauri::command]
pub fn list_app_locations(app: AppHandle) -> Vec<AppLocation> {
    apps::locate_all(&app_context(&app))
}

/// 保存某个应用的手动指定路径；传空字符串表示清除，回到自动查找
#[tauri::command]
pub fn set_app_path(
    app: AppHandle,
    app_id: String,
    path: String,
) -> Result<Vec<AppLocation>, String> {
    let id = AppId::from_str(&app_id).ok_or_else(|| format!("未知的应用标识: {}", app_id))?;
    let mut config = config::load_config(&app);
    let trimmed = path.trim().to_string();

    config.app_paths.retain(|item| item.id != id.as_str());
    if !trimmed.is_empty() {
        config
            .app_paths
            .push(AppPathOverride::new(id.as_str(), &trimmed));
    }
    config::save_config(&app, &config)?;
    Ok(apps::locate_all(&app_context(&app)))
}

/// 清除手动指定的路径
#[tauri::command]
pub fn clear_app_path(app: AppHandle, app_id: String) -> Result<Vec<AppLocation>, String> {
    set_app_path(app, app_id, String::new())
}

/// 把一个已有的应用目录复制到程序数据目录并托管
#[tauri::command]
pub fn import_app(app: AppHandle, app_id: String, source_dir: String) -> Result<String, String> {
    let id = AppId::from_str(&app_id).ok_or_else(|| format!("未知的应用标识: {}", app_id))?;
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("无法获取应用数据目录: {}", e))?;
    apps::finder::import_from_dir(id, Path::new(&source_dir), &data_dir)?;

    let location = apps::locate(id, &app_context(&app), "");
    if location.available {
        Ok(format!("已导入并托管 {}", id.display_name()))
    } else {
        Err(format!(
            "文件已复制，但仍未能定位到可执行文件，请在设置页手动指定。{}",
            location.note
        ))
    }
}

/// 移除程序托管的副本
#[tauri::command]
pub fn remove_managed_app(app: AppHandle, app_id: String) -> Result<String, String> {
    let id = AppId::from_str(&app_id).ok_or_else(|| format!("未知的应用标识: {}", app_id))?;
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("无法获取应用数据目录: {}", e))?;
    apps::finder::remove_managed(id, &data_dir)?;
    Ok(format!("已移除托管的 {}", id.display_name()))
}

/// 清空注册表缓存后重新查找。刚装完软件、注册表还没被缓存到时要调它。
#[tauri::command]
pub fn refresh_app_search(app: AppHandle) -> Vec<AppLocation> {
    apps::registry::clear_cache();
    list_app_locations(app)
}

/// 在终端中打开目录；若目录不存在，自动降级打开最近存在的上级目录
/// 返回实际打开的路径，供前端提示
#[tauri::command]
pub fn open_in_terminal(path: String) -> Result<String, String> {
    let target = nearest_existing_dir(&path);
    let target_str = target.to_string_lossy().to_string();
    // 若最终仍不存在，给出明确错误
    if !target.is_dir() {
        return Err(format!("路径不存在: {}", path));
    }

    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("open")
            .arg("-a")
            .arg("Terminal")
            .arg(&target_str)
            .output()
            .map_err(|e| format!("打开终端失败: {}", e))?;
        if !output.status.success() {
            return Err(format!(
                "打开终端失败: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .arg("/c")
            .arg("start")
            .arg("cmd")
            .arg("/K")
            .arg(format!("cd /d {}", target_str))
            .spawn()
            .map_err(|e| format!("打开终端失败: {}", e))?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let candidates = [
            "x-terminal-emulator",
            "gnome-terminal",
            "konsole",
            "xfce4-terminal",
            "xterm",
        ];
        let mut last_err = String::new();
        let mut opened = false;
        for cmd in candidates {
            match std::process::Command::new(cmd).current_dir(&target_str).spawn() {
                Ok(_) => {
                    opened = true;
                    break;
                }
                Err(e) => last_err = e.to_string(),
            }
        }
        if !opened {
            return Err(format!("打开终端失败: {}", last_err));
        }
    }

    Ok(target_str)
}

/// 发出附件加解密进度事件：`{ id, done, total }`
fn emit_attachment_progress(app: &AppHandle, id: &str, done: u64, total: u64) {
    let _ = app.emit(
        "attachment-progress",
        serde_json::json!({ "id": id, "done": done, "total": total }),
    );
}

/// 解析附件的实际上传目录：实体自定义 > 全局默认 > App 私有 `attachments` 目录
///
/// 由前端在展示「上传位置」行时调用，保证路径来源与落盘位置一致。
#[tauri::command]
pub fn resolve_upload_dir(
    app: AppHandle,
    entity_dir: Option<String>,
    default_dir: Option<String>,
) -> Result<String, String> {
    if let Some(dir) = entity_dir.as_deref() {
        if !dir.trim().is_empty() {
            return Ok(dir.to_string());
        }
    }
    if let Some(dir) = default_dir.as_deref() {
        if !dir.trim().is_empty() {
            return Ok(dir.to_string());
        }
    }
    let base = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("获取应用数据目录失败: {}", e))?;
    Ok(base.join("attachments").to_string_lossy().to_string())
}

/// 校验目录路径在当前系统下是否合法。
///
/// - Windows：必须为绝对路径（盘符 `C:\` 或 UNC `\\server\share`），且盘符之后
///   不得出现 `< > : " | ? *` 等保留字符。
/// - macOS / Linux：必须是以 `/` 开头的绝对路径。
/// - 通用：不允许空字符。
fn validate_dir_path(path: &str) -> Result<(), String> {
    if path.contains('\0') {
        return Err("地址包含非法字符".to_string());
    }

    #[cfg(target_os = "windows")]
    {
        let bytes = path.as_bytes();
        let is_drive = bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && (bytes[2] == b'\\' || bytes[2] == b'/');
        let is_unc = path.starts_with("\\\\") || path.starts_with("//");
        if !is_drive && !is_unc {
            return Err("请填写绝对路径，例如 C:\\work".to_string());
        }
        // 盘符之后不允许出现 Windows 保留字符
        let rest = if is_drive { &path[2..] } else { path };
        if rest.contains(|c| matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*')) {
            return Err("地址包含 Windows 不允许的字符：< > : \" | ? *".to_string());
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        if !path.starts_with('/') {
            return Err("请填写绝对路径，例如 /Users/you/work".to_string());
        }
    }

    Ok(())
}

/// 校验分组地址并确保目录存在：不存在则按需创建，返回最终使用的路径。
///
/// 供「新建分组」填写/选择地址时调用：路径不合法直接报错；已存在但不是
/// 目录则报错；合法且不存在则递归创建。
#[tauri::command]
pub fn ensure_dir(path: String) -> Result<String, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("请填写地址".to_string());
    }
    validate_dir_path(trimmed)?;
    let dir = PathBuf::from(trimmed);
    if dir.is_dir() {
        return Ok(trimmed.to_string());
    }
    if dir.exists() {
        return Err(format!("该地址已存在同名文件，无法作为目录: {}", trimmed));
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建目录失败（{}）: {}", trimmed, e))?;
    Ok(trimmed.to_string())
}

/// 上传并加密文件：原子写入 `<upload_dir>/<uuid>.wst`，返回附件元信息
#[tauri::command]
pub fn encrypt_upload_file(
    app: AppHandle,
    src_path: String,
    upload_dir: String,
    name: String,
) -> Result<AttachmentMeta, String> {
    let src = Path::new(&src_path);
    if !src.is_file() {
        return Err(format!("源文件不存在: {}", src_path));
    }
    let size = std::fs::metadata(src)
        .map_err(|e| format!("读取源文件信息失败: {}", e))?
        .len();

    let dir = PathBuf::from(&upload_dir);
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("创建上传目录失败（{}）: {}", upload_dir, e))?;

    let id = Uuid::new_v4().to_string();
    let tmp = dir.join(format!("{}.wst.tmp", id));
    let final_path = dir.join(format!("{}.wst", id));

    {
        let mut on_progress = |done: u64, total: u64| {
            emit_attachment_progress(&app, &id, done, total);
        };
        if let Err(e) = filecrypto::encrypt_file_stream(&app, src, &tmp, &mut on_progress) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
    }

    if let Err(e) = std::fs::rename(&tmp, &final_path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("写入附件失败: {}", e));
    }

    let ext = src
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    let created_at = chrono::Utc::now().timestamp_millis();

    Ok(AttachmentMeta {
        id,
        name,
        size,
        ext,
        created_at,
        dir: upload_dir,
    })
}

/// 上传内存字节：base64 解码 → 内存加密 → 原子写入 `<upload_dir>/<uuid>.wst`
///
/// 供笔记编辑器粘贴 / 拖拽 / 选择图片使用，全程不产生明文临时文件。
#[tauri::command]
pub fn encrypt_upload_bytes(
    app: AppHandle,
    upload_dir: String,
    name: String,
    ext: String,
    data_b64: String,
) -> Result<AttachmentMeta, String> {
    let data = B64
        .decode(data_b64.as_bytes())
        .map_err(|e| format!("图片数据解码失败: {}", e))?;
    if data.is_empty() {
        return Err("图片内容为空".to_string());
    }
    let size = data.len() as u64;

    let dir = PathBuf::from(&upload_dir);
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("创建上传目录失败（{}）: {}", upload_dir, e))?;

    let blob = filecrypto::encrypt_bytes(&app, &data)?;

    let id = Uuid::new_v4().to_string();
    let tmp = dir.join(format!("{}.wst.tmp", id));
    let final_path = dir.join(format!("{}.wst", id));

    if let Err(e) = std::fs::write(&tmp, &blob) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("写入附件失败: {}", e));
    }
    if let Err(e) = std::fs::rename(&tmp, &final_path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("写入附件失败: {}", e));
    }

    let ext = ext.trim().trim_start_matches('.').to_lowercase();
    let created_at = chrono::Utc::now().timestamp_millis();

    Ok(AttachmentMeta {
        id,
        name,
        size,
        ext,
        created_at,
        dir: upload_dir,
    })
}

/// 解密导出到用户指定的目标路径
#[tauri::command]
pub fn decrypt_export_file(
    app: AppHandle,
    blob_dir: String,
    id: String,
    target_path: String,
) -> Result<(), String> {
    let blob = PathBuf::from(&blob_dir).join(format!("{}.wst", id));
    if !blob.is_file() {
        return Err(format!("附件文件不存在: {}", blob.display()));
    }
    let target = PathBuf::from(&target_path);
    let mut on_progress = |done: u64, total: u64| {
        emit_attachment_progress(&app, &id, done, total);
    };
    filecrypto::decrypt_file_stream(&app, &blob, &target, &mut on_progress)
}

/// 解密到带原扩展名的临时文件，返回绝对路径（供系统默认程序打开预览）
#[tauri::command]
pub fn decrypt_to_temp(
    app: AppHandle,
    blob_dir: String,
    id: String,
    name: String,
) -> Result<String, String> {
    let blob = PathBuf::from(&blob_dir).join(format!("{}.wst", id));
    if !blob.is_file() {
        return Err(format!("附件文件不存在: {}", blob.display()));
    }

    let ext = Path::new(&name)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let suffix = if ext.is_empty() || ext.eq_ignore_ascii_case("wst") {
        String::new()
    } else {
        format!(".{}", ext)
    };

    let named = tempfile::Builder::new()
        .prefix("ws-workbench-")
        .suffix(&suffix)
        .tempfile()
        .map_err(|e| format!("创建临时文件失败: {}", e))?;
    let (_file, path) = named
        .keep()
        .map_err(|e| format!("创建临时文件失败: {}", e))?;

    let mut on_progress = |done: u64, total: u64| {
        emit_attachment_progress(&app, &id, done, total);
    };
    filecrypto::decrypt_file_stream(&app, &blob, &path, &mut on_progress)?;

    Ok(path.to_string_lossy().to_string())
}

/// 删除附件密文文件；文件不存在视为成功
#[tauri::command]
pub fn remove_attachment_file(blob_dir: String, id: String) -> Result<(), String> {
    let blob = PathBuf::from(&blob_dir).join(format!("{}.wst", id));
    match std::fs::remove_file(&blob) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("删除附件失败: {}", e)),
    }
}

/// 解析笔记内联图片请求并返回响应：
/// `<prefix>/img?id=<uuid>&ext=<png>&dir=<urlencoded 绝对目录>`
///
/// 仅服务本应用密文（`.wst`）；`id` 必须为 uuid 形态，防目录穿越。
pub fn serve_note_image(app: &AppHandle, uri: &http::Uri) -> http::Response<Vec<u8>> {
    let params = parse_query(uri.query().unwrap_or(""));
    let id = params.get("id").cloned().unwrap_or_default();
    let ext = params.get("ext").cloned().unwrap_or_default();
    let dir = params.get("dir").cloned().unwrap_or_default();

    if id.is_empty() || dir.is_empty() || Uuid::parse_str(&id).is_err() {
        return plain_response(400, "请求参数不合法");
    }

    let blob_path = PathBuf::from(&dir).join(format!("{}.wst", id));
    let blob = match std::fs::read(&blob_path) {
        Ok(bytes) => bytes,
        Err(_) => return plain_response(404, "图片数据缺失"),
    };

    match filecrypto::decrypt_bytes(app, &blob) {
        Ok(plain) => http::Response::builder()
            .status(200)
            .header(http::header::CONTENT_TYPE, content_type_for_ext(&ext))
            .header(http::header::CACHE_CONTROL, "no-store")
            .body(plain)
            .unwrap_or_else(|_| plain_response(500, "构建响应失败")),
        Err(_) => plain_response(500, "图片数据损坏"),
    }
}

/// 构造一个 `text/plain` 的简单响应
fn plain_response(status: u16, message: &str) -> http::Response<Vec<u8>> {
    http::Response::builder()
        .status(status)
        .header(http::header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(message.as_bytes().to_vec())
        .unwrap_or_else(|_| http::Response::new(Vec::new()))
}

/// 扩展名 → `Content-Type`
fn content_type_for_ext(ext: &str) -> &'static str {
    match ext.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "avif" => "image/avif",
        _ => "application/octet-stream",
    }
}

/// 解析 `a=1&b=2` 形式的查询串（含百分号解码）
fn parse_query(query: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        map.insert(percent_decode(k), percent_decode(v));
    }
    map
}

/// 最小百分号解码（`%XX` 与 `+`），避免为解析 URL 引入新依赖
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => match (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                (Some(hi), Some(lo)) => {
                    out.push((hi << 4) | lo);
                    i += 3;
                }
                _ => {
                    out.push(bytes[i]);
                    i += 1;
                }
            },
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
