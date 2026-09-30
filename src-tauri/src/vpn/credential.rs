//! 密码的安全存储
//!
//! - Windows：走 DPAPI（CryptProtectData / CryptUnprotectData），密文与当前用户绑定；
//!   提权 worker 以同一用户运行，因此能正常解密。
//! - macOS：走系统钥匙串（security 命令行），配置里只留一个引用标记。
//!
//! 密文统一以十六进制字符串保存，避免额外引入 base64 依赖。

/// 十六进制编码
pub(crate) fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{:02x}", b));
    }
    out
}

/// 十六进制解码
pub(crate) fn from_hex(text: &str) -> Result<Vec<u8>, String> {
    let trimmed = text.trim();
    if trimmed.len() % 2 != 0 {
        return Err("密文长度不合法".to_string());
    }
    let mut out = Vec::with_capacity(trimmed.len() / 2);
    let bytes = trimmed.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let hi = (bytes[i] as char)
            .to_digit(16)
            .ok_or_else(|| "密文包含非十六进制字符".to_string())?;
        let lo = (bytes[i + 1] as char)
            .to_digit(16)
            .ok_or_else(|| "密文包含非十六进制字符".to_string())?;
        out.push(((hi << 4) | lo) as u8);
        i += 2;
    }
    Ok(out)
}

/// 加密密码。
///
/// `key` 用于在钥匙串里区分不同配置项（Windows 的 DPAPI 不需要，会忽略它），
/// 返回可直接写入配置的字符串。
pub fn protect(key: &str, plain: &str) -> Result<String, String> {
    if plain.is_empty() {
        return Ok(String::new());
    }
    platform_protect(key, plain)
}

/// 解密密码
pub fn unprotect(stored: &str) -> Result<String, String> {
    if stored.is_empty() {
        return Ok(String::new());
    }
    platform_unprotect(stored)
}

/// 删除配置项对应的凭据（目前只有 macOS 钥匙串需要）
pub fn forget(stored: &str) {
    platform_forget(stored)
}

// ============================ Windows / DPAPI ============================

#[cfg(target_os = "windows")]
fn platform_protect(_key: &str, plain: &str) -> Result<String, String> {
    let cipher = dpapi::protect(plain.as_bytes())?;
    Ok(format!("dpapi:{}", to_hex(&cipher)))
}

#[cfg(target_os = "windows")]
fn platform_unprotect(stored: &str) -> Result<String, String> {
    let hex = stored
        .strip_prefix("dpapi:")
        .ok_or_else(|| "密文格式不是 DPAPI".to_string())?;
    let cipher = from_hex(hex)?;
    let plain = dpapi::unprotect(&cipher)?;
    String::from_utf8(plain).map_err(|e| format!("密文解码失败: {}", e))
}

#[cfg(target_os = "windows")]
fn platform_forget(_stored: &str) {}

#[cfg(target_os = "windows")]
pub(crate) mod dpapi {
    use std::ffi::c_void;
    use std::ptr;

    /// Win32 CRYPT_INTEGER_BLOB（即 DATA_BLOB）
    #[repr(C)]
    struct DataBlob {
        cb_data: u32,
        pb_data: *mut u8,
    }

    /// CRYPTPROTECT_UI_FORBIDDEN：禁止弹出任何 UI，失败直接返回错误
    const CRYPTPROTECT_UI_FORBIDDEN: u32 = 0x1;

    #[link(name = "crypt32")]
    extern "system" {
        fn CryptProtectData(
            p_data_in: *const DataBlob,
            sz_data_descr: *const u16,
            p_optional_entropy: *const DataBlob,
            pv_reserved: *const c_void,
            p_prompt_struct: *const c_void,
            dw_flags: u32,
            p_data_out: *mut DataBlob,
        ) -> i32;

        fn CryptUnprotectData(
            p_data_in: *const DataBlob,
            ppsz_data_descr: *mut *mut u16,
            p_optional_entropy: *const DataBlob,
            pv_reserved: *const c_void,
            p_prompt_struct: *const c_void,
            dw_flags: u32,
            p_data_out: *mut DataBlob,
        ) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn LocalFree(h_mem: *mut c_void) -> *mut c_void;
    }

    /// 把 Crypt* 输出的 DATA_BLOB 复制成 Rust Vec 并释放系统内存
    unsafe fn take_blob(blob: DataBlob) -> Vec<u8> {
        let mut out = Vec::new();
        if !blob.pb_data.is_null() && blob.cb_data > 0 {
            out.extend_from_slice(std::slice::from_raw_parts(blob.pb_data, blob.cb_data as usize));
        }
        if !blob.pb_data.is_null() {
            LocalFree(blob.pb_data as *mut c_void);
        }
        out
    }

    pub fn protect(plain: &[u8]) -> Result<Vec<u8>, String> {
        unsafe {
            let input = DataBlob {
                cb_data: plain.len() as u32,
                pb_data: plain.as_ptr() as *mut u8,
            };
            let mut output = DataBlob {
                cb_data: 0,
                pb_data: ptr::null_mut(),
            };
            let ok = CryptProtectData(
                &input,
                ptr::null(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            );
            if ok == 0 {
                return Err("DPAPI 加密失败（CryptProtectData 返回 0）".to_string());
            }
            Ok(take_blob(output))
        }
    }

    pub fn unprotect(cipher: &[u8]) -> Result<Vec<u8>, String> {
        unsafe {
            let input = DataBlob {
                cb_data: cipher.len() as u32,
                pb_data: cipher.as_ptr() as *mut u8,
            };
            let mut output = DataBlob {
                cb_data: 0,
                pb_data: ptr::null_mut(),
            };
            let ok = CryptUnprotectData(
                &input,
                ptr::null_mut(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            );
            if ok == 0 {
                return Err("DPAPI 解密失败（换过系统账号，或配置是从别的机器复制来的）".to_string());
            }
            Ok(take_blob(output))
        }
    }
}

// ============================== macOS / 钥匙串 ==============================

#[cfg(target_os = "macos")]
const KEYCHAIN_SERVICE: &str = "ws-tools-vpn";

#[cfg(target_os = "macos")]
fn keychain_service(key: &str) -> String {
    format!("{}-{}", KEYCHAIN_SERVICE, key)
}

#[cfg(target_os = "macos")]
fn platform_protect(key: &str, plain: &str) -> Result<String, String> {
    let service = keychain_service(key);
    let output = std::process::Command::new("security")
        .args([
            "add-generic-password",
            "-U",
            "-a",
            "ws-tools",
            "-s",
            &service,
            "-w",
            plain,
        ])
        .output()
        .map_err(|e| format!("调用 security 失败: {}", e))?;
    if !output.status.success() {
        return Err(format!(
            "写入钥匙串失败: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(format!("keychain:{}", key))
}

#[cfg(target_os = "macos")]
fn platform_unprotect(stored: &str) -> Result<String, String> {
    let key = stored
        .strip_prefix("keychain:")
        .ok_or_else(|| "密文格式不是钥匙串引用".to_string())?;
    let service = keychain_service(key);
    let output = std::process::Command::new("security")
        .args([
            "find-generic-password",
            "-a",
            "ws-tools",
            "-s",
            &service,
            "-w",
        ])
        .output()
        .map_err(|e| format!("调用 security 失败: {}", e))?;
    if !output.status.success() {
        return Err("从钥匙串读取密码失败".to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim_end().to_string())
}

#[cfg(target_os = "macos")]
fn platform_forget(stored: &str) {
    if let Some(key) = stored.strip_prefix("keychain:") {
        let service = keychain_service(key);
        let _ = std::process::Command::new("security")
            .args([
                "delete-generic-password",
                "-a",
                "ws-tools",
                "-s",
                &service,
            ])
            .output();
    }
}

// ============================== 其它平台 ==============================

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn platform_protect(_key: &str, _plain: &str) -> Result<String, String> {
    Err("当前平台不支持密码加密存储".to_string())
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn platform_unprotect(_stored: &str) -> Result<String, String> {
    Err("当前平台不支持密码加密存储".to_string())
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn platform_forget(_stored: &str) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip() {
        let data: Vec<u8> = (0u8..=255).collect();
        let hex = to_hex(&data);
        assert_eq!(from_hex(&hex).unwrap(), data);
    }

    #[test]
    fn hex_rejects_bad_input() {
        assert!(from_hex("abc").is_err());
        assert!(from_hex("zz").is_err());
    }

    #[test]
    fn empty_password_is_passthrough() {
        assert_eq!(protect("k", "").unwrap(), "");
        assert_eq!(unprotect("").unwrap(), "");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn dpapi_roundtrip() {
        let cipher = protect("demo", "ExamplePass123").unwrap();
        assert!(cipher.starts_with("dpapi:"));
        assert_eq!(unprotect(&cipher).unwrap(), "ExamplePass123");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn dpapi_supports_chinese() {
        let cipher = protect("demo", "密码-带中文").unwrap();
        assert_eq!(unprotect(&cipher).unwrap(), "密码-带中文");
    }
}
