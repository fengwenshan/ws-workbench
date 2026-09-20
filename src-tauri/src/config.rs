use std::sync::Mutex;
use tauri::{AppHandle, Manager};
use tauri_plugin_store::StoreExt;
use crate::models::{AppConfig, ProjectGroup, DEFAULT_EXCLUDE_ADDITIONS, DEFAULT_EXCLUDE_VERSION};

const STORE_KEY: &str = "app_config";

/// 内存缓存（所有存储方式都失败时的最后兜底）
static MEMORY_CACHE: Mutex<Option<AppConfig>> = Mutex::new(None);

/// 获取内存缓存
fn get_memory_cache() -> AppConfig {
    let cache = MEMORY_CACHE.lock().unwrap();
    cache.clone().unwrap_or_default()
}

/// 设置内存缓存
fn set_memory_cache(config: &AppConfig) {
    let mut cache = MEMORY_CACHE.lock().unwrap();
    *cache = Some(config.clone());
}

/// 使用 Store 插件加载配置
fn load_from_store(app: &AppHandle) -> Option<AppConfig> {
    let store = app.store("config.json").ok()?;
    let value = store.get(STORE_KEY)?;
    serde_json::from_value::<AppConfig>(value).ok()
}

/// 使用 Store 插件保存配置
fn save_to_store(app: &AppHandle, config: &AppConfig) -> Result<(), String> {
    let store = app.store("config.json")
        .map_err(|e| format!("创建 store 失败: {}", e))?;
    let value = serde_json::to_value(config)
        .map_err(|e| format!("序列化失败: {}", e))?;
    store.set(STORE_KEY, value);
    store.save()
        .map_err(|e| format!("保存 store 失败: {}", e))?;
    Ok(())
}

/// 从文件系统加载配置（兜底）
fn load_from_file(app: &AppHandle) -> Option<AppConfig> {
    use std::fs;

    // 按优先级尝试多个路径
    let candidates = get_file_candidates(app);
    for (_, dir) in candidates {
        let path = dir.join("config.json");
        if path.exists() {
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(config) = serde_json::from_str::<AppConfig>(&content) {
                    return Some(config);
                }
            }
        }
    }
    None
}

/// 获取文件系统候选目录
fn get_file_candidates(app: &AppHandle) -> Vec<(String, std::path::PathBuf)> {
    use std::env;
    use std::path::PathBuf;

    let mut candidates: Vec<(String, PathBuf)> = Vec::new();

    if let Ok(dir) = app.path().app_data_dir() {
        candidates.push(("app_data_dir".to_string(), dir));
    }
    if let Ok(dir) = app.path().app_config_dir() {
        candidates.push(("app_config_dir".to_string(), dir));
    }
    if let Ok(home) = app.path().home_dir() {
        candidates.push(("home/.demo-tools".to_string(), home.join(".demo-tools")));
    }
    if let Ok(home) = env::var("HOME") {
        let path = PathBuf::from(home).join(".demo-tools");
        if !candidates.iter().any(|(_, p)| p == &path) {
            candidates.push(("env_HOME/.demo-tools".to_string(), path));
        }
    }
    if let Ok(temp) = env::var("TMPDIR") {
        candidates.push(("TMPDIR/demo-tools".to_string(), PathBuf::from(temp).join("demo-tools")));
    }
    candidates.push(("/tmp/demo-tools".to_string(), PathBuf::from("/tmp/demo-tools")));
    if let Ok(cwd) = env::current_dir() {
        candidates.push(("cwd/.config".to_string(), cwd.join(".config")));
    }

    candidates
}

/// 测试目录是否可写
fn test_writable(dir: &std::path::PathBuf) -> bool {
    use std::fs;
    let test_file = dir.join(".write_test");
    if fs::write(&test_file, b"test").is_ok() {
        let _ = fs::remove_file(&test_file);
        true
    } else {
        false
    }
}

/// 找到第一个可写的文件路径
fn find_writable_file_path(app: &AppHandle) -> Option<std::path::PathBuf> {
    use std::fs;

    for (_, dir) in get_file_candidates(app) {
        if !dir.exists() {
            if fs::create_dir_all(&dir).is_err() {
                continue;
            }
        }
        if test_writable(&dir) {
            return Some(dir.join("config.json"));
        }
    }
    None
}

/// 保存到文件系统（兜底）
fn save_to_file(app: &AppHandle, config: &AppConfig) -> Result<(), String> {
    use std::fs;

    let path = find_writable_file_path(app)
        .ok_or_else(|| "没有可写的文件目录".to_string())?;

    let json = serde_json::to_string_pretty(config)
        .map_err(|e| format!("序列化失败: {}", e))?;

    fs::write(&path, json)
        .map_err(|e| format!("写入文件失败: {} (路径: {})", e, path.display()))?;

    Ok(())
}

/// 加载配置
pub fn load_config(app: &AppHandle) -> AppConfig {
    // 1. 优先从 Store 插件加载
    if let Some(config) = load_from_store(app) {
        return migrate_config(app, config);
    }

    // 2. 从文件系统加载（可能是旧数据迁移）
    if let Some(config) = load_from_file(app) {
        let migrated = migrate_config(app, config);
        // 尝试迁移到 Store
        let _ = save_to_store(app, &migrated);
        return migrated;
    }

    // 3. 从内存缓存加载
    let mut cache = get_memory_cache();
    if !cache.groups.is_empty() || !cache.projects.is_empty() || !cache.default_exclude.is_empty() {
        // 同样兼容历史数据，标记默认分组
        mark_default_group(&mut cache);
        return cache;
    }

    // 4. 首次启动，空配置
    let config = AppConfig::default();
    let _ = save_config(app, &config);
    config
}

/// 保存配置
pub fn save_config(app: &AppHandle, config: &AppConfig) -> Result<(), String> {
    // 始终更新内存缓存
    set_memory_cache(config);

    let mut errors: Vec<String> = Vec::new();

    // 1. 优先用 Store 插件保存
    match save_to_store(app, config) {
        Ok(_) => return Ok(()),
        Err(e) => errors.push(format!("store: {}", e)),
    }

    // 2. 降级到文件系统
    match save_to_file(app, config) {
        Ok(_) => return Ok(()),
        Err(e) => errors.push(format!("file: {}", e)),
    }

    // 3. 都失败了，返回详细错误（但内存中已经保存了，运行时不影响）
    Err(format!(
        "所有存储方式均失败（数据已保存在内存中，重启后会丢失）：{}",
        errors.join("; ")
    ))
}

/// 获取配置文件路径（用于调试显示）
pub fn get_config_path(app: &AppHandle) -> Result<String, String> {
    // 先看 Store 能否工作
    if save_to_store(app, &AppConfig::default()).is_ok() {
        return Ok("Store 插件 (config.json)".to_string());
    }

    // 再看文件系统
    if let Some(path) = find_writable_file_path(app) {
        return Ok(path.to_string_lossy().to_string());
    }

    // 都不行，返回内存模式
    Err("当前为内存模式（重启后数据丢失）".to_string())
}

/// 标记默认分组（兼容历史数据：旧数据没有 is_default 字段）
/// 返回是否发生了变更
fn mark_default_group(config: &mut AppConfig) -> bool {
    // 已有标记则不处理
    if config.groups.iter().any(|g| g.is_default) {
        return false;
    }
    if let Some(group) = config.groups.iter_mut().find(|g| g.name == "默认分组") {
        group.is_default = true;
        return true;
    }
    false
}

/// 为旧配置补齐新增的默认排除规则
/// 返回是否发生了变更
fn migrate_default_exclude(config: &mut AppConfig) -> bool {
    if config.default_exclude_version >= DEFAULT_EXCLUDE_VERSION {
        return false;
    }
    for (version, additions) in DEFAULT_EXCLUDE_ADDITIONS {
        if *version <= config.default_exclude_version {
            continue;
        }
        for rule in additions.iter() {
            if !config.default_exclude.iter().any(|r| r == rule) {
                config.default_exclude.push(rule.to_string());
            }
        }
    }
    config.default_exclude_version = DEFAULT_EXCLUDE_VERSION;
    true
}

/// 迁移配置（旧版 projects 字段 -> 分组结构 + 标记默认分组）
fn migrate_config(app: &AppHandle, mut config: AppConfig) -> AppConfig {
    let mut changed = false;

    // 1. 旧版 projects 字段 -> 默认分组
    if config.groups.is_empty() && !config.projects.is_empty() {
        let now = chrono::Utc::now().to_rfc3339();
        let group = ProjectGroup {
            id: uuid::Uuid::new_v4().to_string(),
            name: "默认分组".to_string(),
            projects: std::mem::take(&mut config.projects),
            is_default: true,
            collapsed: false,
            created_at: now,
        };
        config.groups.push(group);
        changed = true;
    }

    // 2. 兼容历史数据，标记默认分组
    if mark_default_group(&mut config) {
        changed = true;
    }

    // 3. 补齐新增的默认排除规则
    if migrate_default_exclude(&mut config) {
        changed = true;
    }

    if changed {
        let _ = save_config(app, &config);
    }
    config
}

#[cfg(test)]
mod tests {
    use super::*;

    fn old_config(rules: &[&str], version: u32) -> AppConfig {
        AppConfig {
            groups: vec![],
            projects: vec![],
            default_exclude: rules.iter().map(|s| s.to_string()).collect(),
            default_exclude_version: version,
        }
    }

    /// 老配置（无版本号，视为 0）会被补齐新增的默认排除规则
    #[test]
    fn migrate_default_exclude_fills_new_rules() {
        let mut config = old_config(&[".DS_Store", "*.md"], 0);
        assert!(migrate_default_exclude(&mut config));
        assert!(config.default_exclude.contains(&".*".to_string()));
        assert!(config.default_exclude.contains(&".*/**".to_string()));
        assert_eq!(config.default_exclude_version, DEFAULT_EXCLUDE_VERSION);
    }

    /// 已是最新版本时不重复补齐
    #[test]
    fn migrate_default_exclude_skips_up_to_date_config() {
        let mut config = old_config(&["*.md"], DEFAULT_EXCLUDE_VERSION);
        assert!(!migrate_default_exclude(&mut config));
        assert_eq!(config.default_exclude, vec!["*.md".to_string()]);
    }

    /// 已存在的规则不会被重复添加
    #[test]
    fn migrate_default_exclude_does_not_duplicate() {
        let mut config = old_config(&[".*"], 0);
        migrate_default_exclude(&mut config);
        assert_eq!(config.default_exclude.iter().filter(|r| *r == ".*").count(), 1);
    }

    /// 新配置（首次启动）直接带上新增规则
    #[test]
    fn default_config_contains_new_rules() {
        let config = AppConfig::default();
        assert!(config.default_exclude.contains(&".*".to_string()));
        assert!(config.default_exclude.contains(&".*/**".to_string()));
        assert_eq!(config.default_exclude_version, DEFAULT_EXCLUDE_VERSION);
    }
}
