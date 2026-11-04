//! 配置读写、存储信息与本地键值加密命令。

use tauri::AppHandle;

use crate::config;
use crate::models::{default_include_rules, AppConfig};

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
pub fn get_default_include_rules() -> Vec<String> {
    default_include_rules()
}
