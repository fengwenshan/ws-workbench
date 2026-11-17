//! 其它平台：没有系统托管的密钥存储。
//!
//! 明确报错，不用明文或固定密钥兜底 —— 那等于把「本地数据一律加密」的约定作废。

use tauri::AppHandle;

pub(super) fn load_or_create_master_key(_app: &AppHandle) -> Result<[u8; 32], String> {
    Err("当前平台没有可用的主密钥存储（仅支持 macOS 钥匙串 / Windows DPAPI）".to_string())
}
