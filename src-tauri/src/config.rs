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

/// 使用 Store 插件加载配置。
///
/// - `Ok(Some((配置, 是否为历史明文)))`：读取成功
/// - `Ok(None)`：Store 中没有配置
/// - `Err`：Store 中存在数据但无法解密/解析，调用方绝不能覆盖回写
fn load_from_store(app: &AppHandle) -> Result<Option<(AppConfig, bool)>, String> {
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
                let config = serde_json::from_str::<AppConfig>(&json)
                    .map_err(|e| format!("解析配置失败: {}", e))?;
                Ok(Some((config, false)))
            } else {
                let config = serde_json::from_str::<AppConfig>(&s)
                    .map_err(|e| format!("解析配置失败: {}", e))?;
                Ok(Some((config, true)))
            }
        }
        other => {
            let config = serde_json::from_value::<AppConfig>(other)
                .map_err(|e| format!("解析配置失败: {}", e))?;
            Ok(Some((config, true)))
        }
    }
}

/// 使用 Store 插件保存配置（以 SM4 密文信封写入）
fn save_to_store(app: &AppHandle, config: &AppConfig) -> Result<(), String> {
    let store = app.store("config.json")
        .map_err(|e| format!("创建 store 失败: {}", e))?;
    let json = serde_json::to_string(config)
        .map_err(|e| format!("序列化失败: {}", e))?;
    let envelope = crate::crypto::encrypt_str(app, &json)?;
    store.set(STORE_KEY, serde_json::Value::String(envelope));
    store.save()
        .map_err(|e| format!("保存 store 失败: {}", e))?;
    Ok(())
}

/// 从文件系统加载配置（兜底）。
///
/// - `Ok(Some((配置, 是否为历史明文)))`：读取成功
/// - `Ok(None)`：所有候选目录都没有可读配置
/// - `Err`：存在密文但无法解密，调用方绝不能覆盖回写
fn load_from_file(app: &AppHandle) -> Result<Option<(AppConfig, bool)>, String> {
    use std::fs;

    let mut saw_unreadable = false;
    // 按优先级尝试多个路径（含历史遗留临时目录，仅用于迁移读取）
    let candidates = get_read_candidates(app);
    for (_, dir) in candidates {
        let path = dir.join("config.json");
        if !path.exists() {
            continue;
        }
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(_) => {
                // 文件存在却读不出（权限/非 UTF-8）：绝不能当作"无数据"被默认配置覆盖
                saw_unreadable = true;
                continue;
            }
        };
        let trimmed = content.trim();
        if crate::crypto::is_encrypted(trimmed) {
            if let Ok(json) = crate::crypto::decrypt_str(app, trimmed) {
                if let Ok(config) = serde_json::from_str::<AppConfig>(&json) {
                    return Ok(Some((config, false)));
                }
            }
            // 密文无法解密：记录后跳过该候选，避免被默认配置覆盖
            saw_unreadable = true;
            continue;
        }
        match serde_json::from_str::<AppConfig>(trimmed) {
            Ok(config) => return Ok(Some((config, true))),
            Err(_) => {
                // 明文存在但无法解析：同样不能视为"无数据"
                saw_unreadable = true;
                continue;
            }
        }
    }
    if saw_unreadable {
        return Err("配置文件存在但无法读取或解密".to_string());
    }
    Ok(None)
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

/// 读取候选目录：在持久化目录基础上，额外包含历史遗留的临时目录，
/// 用于把"曾经误落在临时目录里的数据"一次性迁移出来；
/// 临时目录只参与读取，绝不作为写入目标。
fn get_read_candidates(app: &AppHandle) -> Vec<(String, std::path::PathBuf)> {
    use std::env;
    use std::path::PathBuf;

    let mut candidates = get_write_candidates(app);
    if let Ok(temp) = env::var("TMPDIR") {
        push_candidate(
            &mut candidates,
            "TMPDIR/ws-tools(legacy)",
            PathBuf::from(temp).join("ws-tools"),
        );
    }
    push_candidate(
        &mut candidates,
        "/tmp/ws-tools(legacy)",
        PathBuf::from("/tmp/ws-tools"),
    );
    candidates
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
fn get_write_candidates(app: &AppHandle) -> Vec<(String, std::path::PathBuf)> {
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
        push_candidate(&mut candidates, "home/.ws-tools", home.join(".ws-tools"));
    }
    if let Ok(home) = env::var("HOME") {
        push_candidate(
            &mut candidates,
            "env_HOME/.ws-tools",
            PathBuf::from(home).join(".ws-tools"),
        );
    }
    push_project_candidates(&mut candidates);

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

    for (_, dir) in get_write_candidates(app) {
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

/// 保存到文件系统（兜底，以 SM4 密文信封写入）
fn save_to_file(app: &AppHandle, config: &AppConfig) -> Result<(), String> {
    use std::fs;

    let path = find_writable_file_path(app)
        .ok_or_else(|| "没有可写的文件目录".to_string())?;

    let json = serde_json::to_string_pretty(config)
        .map_err(|e| format!("序列化失败: {}", e))?;
    let envelope = crate::crypto::encrypt_str(app, &json)?;

    fs::write(&path, envelope)
        .map_err(|e| format!("写入文件失败: {} (路径: {})", e, path.display()))?;

    Ok(())
}

/// 加载配置
pub fn load_config(app: &AppHandle) -> AppConfig {
    // 1. 优先从 Store 插件加载
    match load_from_store(app) {
        Ok(Some((config, was_plaintext))) => {
            let migrated = migrate_config(app, config);
            // 历史明文首读即回写密文，完成升级
            if was_plaintext {
                let _ = save_config(app, &migrated);
            }
            return migrated;
        }
        // Store 中有数据但无法读取：绝不覆盖，退回内存/默认值
        Err(_) => return load_config_readonly(),
        Ok(None) => {}
    }

    // 2. 从文件系统加载（可能是旧数据迁移）
    match load_from_file(app) {
        Ok(Some((config, _was_plaintext))) => {
            let migrated = migrate_config(app, config);
            // 迁移到 Store（密文）；Store 不可用时才退回文件，
            // 避免二者写同一个 config.json 却用不同格式互相覆盖
            if save_to_store(app, &migrated).is_err() {
                let _ = save_to_file(app, &migrated);
            }
            return migrated;
        }
        // 文件中有密文但无法解密：绝不覆盖，退回内存/默认值
        Err(_) => return load_config_readonly(),
        Ok(None) => {}
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

/// 存在既有配置但无法读取（如钥匙串暂时不可用）时的兜底：
/// 只用内存缓存或默认值，绝不回写，避免覆盖既有数据
fn load_config_readonly() -> AppConfig {
    let mut cache = get_memory_cache();
    if !cache.groups.is_empty() || !cache.projects.is_empty() || !cache.default_exclude.is_empty() {
        mark_default_group(&mut cache);
        return cache;
    }
    AppConfig::default()
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
///
/// 这是一个只读探测：绝不能借"查看路径"之名写入任何配置。
/// 历史实现会 `save_to_store(default)` 做可写性探测，导致每次启动
/// 都把用户的真实配置覆盖成默认值（表现为"每次重启数据都没了"）。
pub fn get_config_path(app: &AppHandle) -> Result<String, String> {
    // 先看 Store 目标目录能否工作（只做 .write_test 探测，不写配置）
    if store_dir_writable(app) {
        return Ok("Store 插件 (config.json)".to_string());
    }

    // 再看文件系统
    if let Some(path) = find_writable_file_path(app) {
        return Ok(path.to_string_lossy().to_string());
    }

    // 都不行，返回内存模式
    Err("当前为内存模式（重启后数据丢失）".to_string())
}

/// 探测 Store 插件目标目录（app_data_dir）是否可写。
/// 只写 `.write_test` 探针，绝不触碰 config.json。
fn store_dir_writable(app: &AppHandle) -> bool {
    use std::fs;

    match app.path().app_data_dir() {
        Ok(dir) => {
            if !dir.exists() && fs::create_dir_all(&dir).is_err() {
                return false;
            }
            test_writable(&dir)
        }
        Err(_) => false,
    }
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

/// 迁移历史字段级加密的 VPN 密码：
/// 旧版 password 为 `keychain:` / `dpapi:` 密文且 password_encrypted=true，
/// 解出明文后清掉标记；解密失败则保留原值并记录日志。
/// 返回是否发生了变更。
fn migrate_vpn_passwords(config: &mut AppConfig) -> bool {
    let mut changed = false;
    for profile in config.vpn_profiles.iter_mut() {
        if !profile.password_encrypted {
            continue;
        }
        let looks_legacy = profile.password.starts_with("keychain:")
            || profile.password.starts_with("dpapi:");
        if !looks_legacy {
            // 已是明文（或空），只需清掉标记
            profile.password_encrypted = false;
            changed = true;
            continue;
        }
        match crate::vpn::credential::unprotect(&profile.password) {
            Ok(plain) => {
                profile.password = plain;
                profile.password_encrypted = false;
                changed = true;
            }
            Err(err) => {
                eprintln!(
                    "VPN 配置「{}」的旧密码解密失败，保留原值：{}",
                    profile.name, err
                );
            }
        }
    }
    changed
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

    // 4. 迁移历史字段级加密的 VPN 密码
    if migrate_vpn_passwords(&mut config) {
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
            default_exclude: rules.iter().map(|s| s.to_string()).collect(),
            default_exclude_version: version,
            ..Default::default()
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

    fn vpn_profile(password: &str, encrypted: bool) -> crate::vpn::models::VpnProfile {
        crate::vpn::models::VpnProfile {
            id: "p1".to_string(),
            name: "测试配置".to_string(),
            server: "vpn.example.com".to_string(),
            username: "user".to_string(),
            password: password.to_string(),
            password_encrypted: encrypted,
            protocol: "anyconnect".to_string(),
            connector: Default::default(),
            mode: Default::default(),
            rule_set_id: String::new(),
            use_vpn_dns: true,
            learn: false,
            extra_args: Vec::new(),
            connector_path: String::new(),
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    /// 标记为已加密但实际已是明文：只清标记，值保持不变
    #[test]
    fn migrate_vpn_passwords_clears_flag_for_plaintext() {
        let mut config = AppConfig {
            vpn_profiles: vec![vpn_profile("secret", true)],
            ..Default::default()
        };
        assert!(migrate_vpn_passwords(&mut config));
        assert_eq!(config.vpn_profiles[0].password, "secret");
        assert!(!config.vpn_profiles[0].password_encrypted);
    }

    /// 幂等：迁移过的配置再次迁移不再产生变更
    #[test]
    fn migrate_vpn_passwords_is_idempotent() {
        let mut config = AppConfig {
            vpn_profiles: vec![vpn_profile("secret", true)],
            ..Default::default()
        };
        assert!(migrate_vpn_passwords(&mut config));
        assert!(!migrate_vpn_passwords(&mut config));
    }

    /// 空密码同样只清标记
    #[test]
    fn migrate_vpn_passwords_clears_flag_for_empty() {
        let mut config = AppConfig {
            vpn_profiles: vec![vpn_profile("", true)],
            ..Default::default()
        };
        assert!(migrate_vpn_passwords(&mut config));
        assert_eq!(config.vpn_profiles[0].password, "");
        assert!(!config.vpn_profiles[0].password_encrypted);
    }

    /// 旧密文解不开时：保留原值与标记，且不视为变更（绝不覆盖既有数据）
    #[test]
    fn migrate_vpn_passwords_keeps_unreadable_legacy() {
        let mut config = AppConfig {
            vpn_profiles: vec![vpn_profile("keychain:__missing__", true)],
            ..Default::default()
        };
        assert!(!migrate_vpn_passwords(&mut config));
        assert_eq!(config.vpn_profiles[0].password, "keychain:__missing__");
        assert!(config.vpn_profiles[0].password_encrypted);
    }
}
