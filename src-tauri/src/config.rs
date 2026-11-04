use std::sync::Mutex;

use tauri::{AppHandle, Manager};
use tauri_plugin_store::StoreExt;

use crate::db::DbData;
use crate::models::{AppConfig, AppSettings};

const STORE_KEY: &str = "app_config";

/// 设置内存缓存（config.json 不可用时的兜底）
static SETTINGS_CACHE: Mutex<Option<AppSettings>> = Mutex::new(None);

/// 上次从数据库加载的业务快照，作为保存时 diff 的基线
static DB_SNAPSHOT: Mutex<Option<DbData>> = Mutex::new(None);

/// 设置读取失败只提示一次：load_config 启动时会被多处调用，避免刷屏
static SETTINGS_WARNED: std::sync::Once = std::sync::Once::new();

fn get_settings_cache() -> AppSettings {
    SETTINGS_CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .unwrap_or_default()
}

fn set_settings_cache(settings: &AppSettings) {
    *SETTINGS_CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some(settings.clone());
}

fn set_db_snapshot(snapshot: Option<DbData>) {
    *DB_SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner()) = snapshot;
}

fn db_snapshot() -> Option<DbData> {
    DB_SNAPSHOT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

impl AppSettings {
    /// 从完整配置中抽出设置字段
    pub fn from_config(config: &AppConfig) -> Self {
        AppSettings {
            default_include: config.default_include.clone(),
            default_include_version: config.default_include_version,
            vpn_active_profile_id: config.vpn_active_profile_id.clone(),
            vpn_worker_session_dir: config.vpn_worker_session_dir.clone(),
            default_upload_dir: config.default_upload_dir.clone(),
        }
    }

    /// 把设置与数据库实体合并成前端使用的完整配置
    pub fn into_config(self, data: DbData) -> AppConfig {
        AppConfig {
            groups: data.groups,
            projects: vec![],
            default_include: self.default_include,
            default_include_version: self.default_include_version,
            vpn_profiles: data.vpn_profiles,
            vpn_rule_sets: data.vpn_rule_sets,
            vpn_active_profile_id: self.vpn_active_profile_id,
            vpn_worker_session_dir: self.vpn_worker_session_dir,
            default_upload_dir: self.default_upload_dir,
        }
    }
}

/// 使用 Store 插件加载设置。
///
/// - `Ok(Some((设置, 是否为历史明文)))`：读取成功
/// - `Ok(None)`：Store 中没有设置
/// - `Err`：Store 中存在数据但无法解密/解析，调用方绝不能覆盖回写
fn load_settings_from_store(app: &AppHandle) -> Result<Option<(AppSettings, bool)>, String> {
    let store = match app.store("config.json") {
        Ok(store) => store,
        Err(_) => return Ok(None),
    };
    let value = match store.get(STORE_KEY) {
        Some(value) => value,
        None => return Ok(None),
    };
    match value {
        serde_json::Value::String(s) => {
            if crate::crypto::is_encrypted(&s) {
                let json = crate::crypto::decrypt_str(app, &s)?;
                let settings = serde_json::from_str::<AppSettings>(&json)
                    .map_err(|e| format!("解析设置失败: {}", e))?;
                Ok(Some((settings, false)))
            } else {
                let settings = serde_json::from_str::<AppSettings>(&s)
                    .map_err(|e| format!("解析设置失败: {}", e))?;
                Ok(Some((settings, true)))
            }
        }
        other => {
            let settings = serde_json::from_value::<AppSettings>(other)
                .map_err(|e| format!("解析设置失败: {}", e))?;
            Ok(Some((settings, true)))
        }
    }
}

/// 使用 Store 插件保存设置（以 SM4 密文信封写入）
fn save_settings_to_store(app: &AppHandle, settings: &AppSettings) -> Result<(), String> {
    let store = app
        .store("config.json")
        .map_err(|e| format!("创建 store 失败: {}", e))?;
    let json = serde_json::to_string(settings).map_err(|e| format!("序列化失败: {}", e))?;
    let envelope = crate::crypto::encrypt_str(app, &json)?;
    store.set(STORE_KEY, serde_json::Value::String(envelope));
    store.save().map_err(|e| format!("保存 store 失败: {}", e))?;
    Ok(())
}

/// 从文件系统加载设置（兜底）。
///
/// - `Ok(Some((设置, 是否为历史明文)))`：读取成功
/// - `Ok(None)`：所有候选目录都没有可读设置
/// - `Err`：存在密文但无法解密，调用方绝不能覆盖回写
fn load_settings_from_file(app: &AppHandle) -> Result<Option<(AppSettings, bool)>, String> {
    let mut unreadable: Vec<String> = Vec::new();
    for (_, dir) in storage_candidates(app) {
        let path = dir.join("config.json");
        let content = match crate::util::fs::read_text_if_exists(&path) {
            Ok(Some(content)) => content,
            Ok(None) => continue,
            Err(err) => {
                unreadable.push(format!("{}（{}）", path.display(), err));
                continue;
            }
        };
        let trimmed = content.trim();
        if crate::crypto::is_encrypted(trimmed) {
            if let Ok(json) = crate::crypto::decrypt_str(app, trimmed) {
                if let Ok(settings) = serde_json::from_str::<AppSettings>(&json) {
                    return Ok(Some((settings, false)));
                }
            }
            unreadable.push(path.display().to_string());
            continue;
        }
        match serde_json::from_str::<AppSettings>(trimmed) {
            Ok(settings) => return Ok(Some((settings, true))),
            Err(_) => {
                unreadable.push(path.display().to_string());
                continue;
            }
        }
    }
    if !unreadable.is_empty() {
        return Err(format!(
            "设置文件存在但无法解密（可能是旧版本主密钥加密，不会覆盖原文件）：{}",
            unreadable.join("; ")
        ));
    }
    Ok(None)
}

/// 把设置写到文件系统（兜底，以 SM4 密文信封写入）
fn save_settings_to_file(app: &AppHandle, settings: &AppSettings) -> Result<(), String> {
    let path = find_writable_file_path(app).ok_or_else(|| "没有可写的设置目录".to_string())?;
    let json = serde_json::to_string(settings).map_err(|e| format!("序列化失败: {}", e))?;
    let envelope = crate::crypto::encrypt_str(app, &json)?;
    crate::util::fs::write_text(&path, &envelope)
}

/// 加载设置：Store → 文件 → 默认值
/// 老版本配置补上新增的默认排除规则。
///
/// 包含规则是按版本维护的（见 [`crate::models::DEFAULT_INCLUDE`]）：老配置
/// （没有 include 字段，版本 0）加载时补上默认集；用户之后自己删掉的规则
/// 不会因为版本没变被重新塞回去。
fn upgrade_default_include(app: &AppHandle, settings: &mut AppSettings) -> bool {
    let mut version = settings.default_include_version;
    let changed = crate::models::migrate_default_include(&mut settings.default_include, &mut version);
    settings.default_include_version = version;
    if changed {
        if let Err(err) = save_settings(app, settings) {
            eprintln!("[settings] 打包包含规则升级后回写失败: {}", err);
        }
    }
    changed
}

pub fn load_settings(app: &AppHandle) -> AppSettings {
    match load_settings_from_store(app) {
        Ok(Some((mut settings, was_plaintext))) => {
            let upgraded = upgrade_default_include(app, &mut settings);
            if was_plaintext && !upgraded {
                let _ = save_settings_to_store(app, &settings);
            }
            set_settings_cache(&settings);
            return settings;
        }
        Err(err) => {
            SETTINGS_WARNED.call_once(|| {
                eprintln!("[settings] 读取失败，使用内存/默认设置: {}", err);
            });
            return get_settings_cache();
        }
        Ok(None) => {}
    }

    match load_settings_from_file(app) {
        Ok(Some((mut settings, _was_plaintext))) => {
            let upgraded = upgrade_default_include(app, &mut settings);
            if !upgraded && save_settings_to_store(app, &settings).is_err() {
                let _ = save_settings_to_file(app, &settings);
            }
            set_settings_cache(&settings);
            return settings;
        }
        Err(err) => {
            SETTINGS_WARNED.call_once(|| {
                eprintln!("[settings] 读取失败，使用内存/默认设置: {}", err);
            });
            return get_settings_cache();
        }
        Ok(None) => {}
    }

    let settings = AppSettings::default();
    if let Err(err) = save_settings(app, &settings) {
        eprintln!("[settings] 首次写入默认设置失败，仅保存在内存中: {}", err);
    }
    settings
}

/// 保存设置：Store → 文件
pub fn save_settings(app: &AppHandle, settings: &AppSettings) -> Result<(), String> {
    set_settings_cache(settings);
    let mut errors: Vec<String> = Vec::new();
    match save_settings_to_store(app, settings) {
        Ok(()) => return Ok(()),
        Err(e) => errors.push(format!("store: {}", e)),
    }
    match save_settings_to_file(app, settings) {
        Ok(()) => return Ok(()),
        Err(e) => errors.push(format!("file: {}", e)),
    }
    Err(format!(
        "设置保存失败（已保存在内存中，重启后会丢失）：{}",
        errors.join("; ")
    ))
}

/// 追加候选目录（去重）
fn push_candidate(
    candidates: &mut Vec<(String, std::path::PathBuf)>,
    name: &str,
    path: std::path::PathBuf,
) {
    if !candidates.iter().any(|(_, p)| p == &path) {
        candidates.push((name.to_string(), path));
    }
}

/// 追加项目内兜底候选（`<crate>/.config`、`<cwd>/.config`）。
///
/// 仅用于开发场景：`tauri dev` 时应用可能运行在受限沙箱里，写不了用户数据目录，
/// 而项目目录通常是允许写入的。`src-tauri/.config/` 已在 .gitignore 中忽略，
/// 不会污染版本库；发布版用户数据目录可写，永远不会走到这里。
fn push_project_candidates(candidates: &mut Vec<(String, std::path::PathBuf)>) {
    use std::env;

    fn is_rust_crate(dir: &std::path::Path) -> bool {
        dir.join("Cargo.toml").is_file()
    }

    let mut roots: Vec<(String, std::path::PathBuf)> = Vec::new();
    if let Ok(exe) = env::current_exe() {
        for anc in exe.ancestors().skip(1).take(8) {
            if is_rust_crate(anc) {
                roots.push(("crate/.config".to_string(), anc.join(".config")));
                break;
            }
        }
    }
    if let Ok(cwd) = env::current_dir() {
        roots.push(("cwd/.config".to_string(), cwd.join(".config")));
    }
    for (name, path) in roots {
        push_candidate(candidates, &name, path);
    }
}

/// 写入候选目录：优先可持久化的用户位置（应用数据/配置目录、用户主目录），
/// 最后才退到项目内目录（开发专用）。
///
/// 这里刻意不含 TMPDIR / tmp：临时目录重启即失，一旦静默落盘，
/// 用户就会看到"每次重启数据都没了"。
pub(crate) fn storage_candidates(app: &AppHandle) -> Vec<(String, std::path::PathBuf)> {
    use std::env;
    use std::path::PathBuf;

    let mut candidates: Vec<(String, PathBuf)> = Vec::new();

    if let Ok(dir) = app.path().app_data_dir() {
        push_candidate(&mut candidates, "app_data_dir", dir);
    }
    if let Ok(dir) = app.path().app_config_dir() {
        push_candidate(&mut candidates, "app_config_dir", dir);
    }
    if let Ok(home) = app.path().home_dir() {
        push_candidate(&mut candidates, "home/.ws-workbench", home.join(".ws-workbench"));
    }
    if let Ok(home) = env::var("HOME") {
        push_candidate(
            &mut candidates,
            "env_HOME/.ws-workbench",
            PathBuf::from(home).join(".ws-workbench"),
        );
    }
    push_project_candidates(&mut candidates);

    candidates
}

/// 测试目录是否可写
fn test_writable(dir: &std::path::PathBuf) -> bool {
    crate::util::fs::is_writable_dir(dir)
}

/// 找到第一个可写的文件路径
fn find_writable_file_path(app: &AppHandle) -> Option<std::path::PathBuf> {
    for (_, dir) in storage_candidates(app) {
        if !dir.exists() {
            if crate::util::fs::create_dir_all(&dir).is_err() {
                continue;
            }
        }
        if test_writable(&dir) {
            return Some(dir.join("config.json"));
        }
    }
    None
}

/// 加载完整配置：设置 + 数据库业务实体
pub fn load_config(app: &AppHandle) -> AppConfig {
    let settings = load_settings(app);
    let data = match crate::db::load_for_app(app) {
        Ok(data) => {
            set_db_snapshot(Some(data.clone()));
            data
        }
        Err(err) => {
            eprintln!("[db] 读取失败，本次不写回数据库: {}", err);
            set_db_snapshot(None);
            DbData::default()
        }
    };
    let mut config = settings.into_config(data);
    mark_default_group(&mut config);
    config
}

/// 保存完整配置：设置写 config.json，业务实体按快照 diff 写数据库
pub fn save_config(app: &AppHandle, config: &AppConfig) -> Result<(), String> {
    let mut errors: Vec<String> = Vec::new();

    let settings = AppSettings::from_config(config);
    if let Err(err) = save_settings(app, &settings) {
        errors.push(format!("settings: {}", err));
    }

    match db_snapshot() {
        None => errors.push("本地数据库不可用，业务数据未写入".to_string()),
        Some(prev) => {
            let next = DbData::from_config(config);
            match crate::db::save_for_app(app, &prev, &next) {
                Ok(()) => set_db_snapshot(Some(next)),
                Err(err) => errors.push(format!("db: {}", err)),
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

/// 获取数据库文件路径（用于设置页展示）
pub fn get_config_path(app: &AppHandle) -> Result<String, String> {
    crate::db::db_path(app).map(|path| path.to_string_lossy().to_string())
}

/// 获取设置文件路径（用于设置页展示）
pub fn settings_path(app: &AppHandle) -> Result<String, String> {
    find_writable_file_path(app)
        .map(|path| path.to_string_lossy().to_string())
        .ok_or_else(|| "设置文件当前不可写（内存模式）".to_string())
}

/// 标记默认分组（兼容历史数据：旧数据没有 is_default 字段）
fn mark_default_group(config: &mut AppConfig) -> bool {
    if config.groups.iter().any(|g| g.is_default) {
        return false;
    }
    if let Some(group) = config.groups.iter_mut().find(|g| g.name == "默认分组") {
        group.is_default = true;
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ProjectGroup;

    fn group_from_json(json: &str) -> ProjectGroup {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn app_settings_default_uses_latest_include_rules() {
        let settings = AppSettings::default();
        assert_eq!(
            settings.default_include_version,
            crate::models::DEFAULT_INCLUDE_VERSION
        );
        assert!(settings.default_include.iter().any(|r| r == "*.js"));
    }

    #[test]
    fn app_settings_roundtrip_from_config() {
        let config = AppConfig {
            default_include: vec!["*.js".to_string()],
            default_include_version: 7,
            vpn_active_profile_id: "p1".to_string(),
            vpn_worker_session_dir: "/tmp/session".to_string(),
            default_upload_dir: Some("/tmp/upload".to_string()),
            ..Default::default()
        };
        let settings = AppSettings::from_config(&config);
        let merged = settings.into_config(crate::db::DbData::default());
        assert_eq!(merged.default_include, config.default_include);
        assert_eq!(merged.default_include_version, 7);
        assert_eq!(merged.vpn_active_profile_id, "p1");
        assert_eq!(merged.vpn_worker_session_dir, "/tmp/session");
        assert_eq!(merged.default_upload_dir, config.default_upload_dir);
        assert!(merged.groups.is_empty());
    }

    #[test]
    fn mark_default_group_marks_named_group() {
        let mut config = AppConfig::default();
        assert!(!mark_default_group(&mut config));

        config
            .groups
            .push(group_from_json(r#"{"id":"g1","name":"默认分组"}"#));
        assert!(mark_default_group(&mut config));
        assert!(config.groups[0].is_default);
        assert!(!mark_default_group(&mut config));
    }

    #[test]
    fn mark_default_group_leaves_other_groups_alone() {
        let mut config = AppConfig::default();
        config
            .groups
            .push(group_from_json(r#"{"id":"g1","name":"工作"}"#));
        assert!(!mark_default_group(&mut config));
        assert!(!config.groups[0].is_default);
    }
}
