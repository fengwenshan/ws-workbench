//! 局域网分享设置的持久化（`lan_share.json`）。
//!
//! 单独一个 store 文件、明文 JSON：设备名 / 接收目录 / 指纹都不属于机密，
//! 没必要混进加密的 `config.json`。

use tauri::AppHandle;
use tauri_plugin_store::StoreExt;

use super::{LanShareSettings, STORE_FILE, STORE_KEY};

/// 读取设置；store 无法打开或内容损坏时返回错误，调用方决定用默认值。
pub(crate) fn load(app: &AppHandle) -> Result<LanShareSettings, String> {
    let store = app
        .store(STORE_FILE)
        .map_err(|err| format!("打开设置文件失败：{err}"))?;
    match store.get(STORE_KEY) {
        Some(value) => serde_json::from_value::<LanShareSettings>(value.clone())
            .map_err(|err| format!("解析设置失败：{err}")),
        None => Ok(LanShareSettings::default()),
    }
}

/// 写入设置（store 插件会把文件落在应用数据目录）。
pub(crate) fn save(app: &AppHandle, settings: &LanShareSettings) -> Result<(), String> {
    let store = app
        .store(STORE_FILE)
        .map_err(|err| format!("打开设置文件失败：{err}"))?;
    let value = serde_json::to_value(settings).map_err(|err| format!("序列化设置失败：{err}"))?;
    store.set(STORE_KEY, value);
    store.save().map_err(|err| format!("保存设置失败：{err}"))
}
