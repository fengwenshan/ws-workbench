//! macOS 的主密钥托管：系统钥匙串（`security` 命令行，明文为十六进制）。
//!
//! 钥匙串只有一份、跟用户走，数据目录怎么回退都不会出现「密钥在 A、数据在 B」，
//! 所以这里只负责读取/新建，不关心数据落在哪个目录。

use tauri::AppHandle;

use super::{decode_master_hex, random_key};

/// 钥匙串服务名 / 账号
const VAULT_SERVICE: &str = "ws-workbench-vault";
const VAULT_ACCOUNT: &str = "ws-workbench";

/// 读取已有主密钥；没有就在钥匙串里新建一把
pub(super) fn load_or_create_master_key(_app: &AppHandle) -> Result<[u8; 32], String> {
    if let Some(hex) = keychain_read(VAULT_SERVICE, VAULT_ACCOUNT) {
        return decode_master_hex(&hex);
    }

    let master = random_key();
    let hex = crate::vpn::credential::to_hex(&master);
    keychain_write(VAULT_SERVICE, VAULT_ACCOUNT, &hex)?;
    Ok(master)
}

/// 读取指定服务名 / 账号下的钥匙串条目；不存在时返回 None
fn keychain_read(service: &str, account: &str) -> Option<String> {
    let output = std::process::Command::new("security")
        .args(["find-generic-password", "-a", account, "-s", service, "-w"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// 写入（覆盖）钥匙串条目
fn keychain_write(service: &str, account: &str, value: &str) -> Result<(), String> {
    let output = std::process::Command::new("security")
        .args([
            "add-generic-password",
            "-U",
            "-a",
            account,
            "-s",
            service,
            "-w",
            value,
        ])
        .output()
        .map_err(|e| format!("调用 security 失败: {}", e))?;
    if !output.status.success() {
        return Err(format!(
            "写入钥匙串失败: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}
