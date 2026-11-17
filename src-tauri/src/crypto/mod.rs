//! 本地存储加密：SM4-CBC + 随机 IV + HMAC-SM3（先加密后认证）。
//!
//! 信封格式：`v1.<b64(iv)>.<b64(ct)>.<b64(mac)>`，以 `v1.` 前缀识别密文。
//! 主密钥（32 字节随机数）由系统钥匙串（macOS）/ DPAPI（Windows）托管，
//! 派生出加密密钥 `SM3(master‖"enc")` 与认证密钥 `SM3(master‖"mac")`。
//!
//! 平台差异只有「主密钥放哪、怎么托管」，按 `vpn/platform/` 的规矩拆在
//! `macos.rs` / `windows.rs` 里；本文件只放信封算法、子密钥派生与对外入口。
//!
//! 一条硬约束：**主密钥必须和它加密的数据待在同一个目录**。数据目录在用户
//! 目录不可写（沙箱、企业策略、杀软）时会回退到别的候选目录，密钥要是还留在
//! 原处，读出来全是「数据已被篡改」、写盘又全部失败，界面表现为「新建配置
//! 成功但弹窗不关、重启后配置不见」。Windows 侧按数据目录取密钥，见
//! `crypto/windows.rs`。

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use cbc::cipher::{block_padding::Pkcs7, BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use hmac::{Hmac, Mac};
use sm3::{Digest, Sm3};
use tauri::AppHandle;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod unsupported;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
use unsupported as platform;
#[cfg(target_os = "windows")]
use windows as platform;

/// 密文信封前缀，同时用作版本号
const PREFIX: &str = "v1.";

type Sm4CbcEnc = cbc::Encryptor<sm4::Sm4>;
type Sm4CbcDec = cbc::Decryptor<sm4::Sm4>;
type HmacSm3 = Hmac<Sm3>;

/// 派生后的两把子密钥
///
/// SM4 只接受 16 字节密钥，故加密时取 `enc_key` 的前 16 字节
/// （SM3 摘要左截断），认证仍使用完整的 32 字节 `mac_key`。
#[derive(Clone, Copy)]
pub(crate) struct VaultKeys {
    pub(crate) enc_key: [u8; 32],
    pub(crate) mac_key: [u8; 32],
}

/// 进程内缓存，避免每次读写都去访问钥匙串 / DPAPI
static VAULT_KEYS: std::sync::Mutex<Option<VaultKeys>> = std::sync::Mutex::new(None);

/// 是否为密文信封（以 `v1.` 开头）
pub fn is_encrypted(s: &str) -> bool {
    s.trim_start().starts_with(PREFIX)
}

/// 用 SM3 把若干字节片段串接后摘要成 32 字节密钥
fn sm3_of(parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = Sm3::new();
    for part in parts {
        hasher.update(part);
    }
    let out = hasher.finalize();
    let mut key = [0u8; 32];
    key.copy_from_slice(&out);
    key
}

/// 由主密钥派生出加解密用的两把子密钥
fn keys_from_master(master: &[u8; 32]) -> VaultKeys {
    VaultKeys {
        enc_key: sm3_of(&[master, b"enc"]),
        mac_key: sm3_of(&[master, b"mac"]),
    }
}

/// 生成 32 字节随机数
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn random_key() -> [u8; 32] {
    use rand::RngCore;
    let mut key = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut key);
    key
}

/// 校验字节长度是否为 32 字节并转成定长数组
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn to_master(bytes: &[u8]) -> Result<[u8; 32], String> {
    if bytes.len() != 32 {
        return Err("主密钥长度不合法".to_string());
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(bytes);
    Ok(key)
}

/// 把十六进制主密钥文本解析成 32 字节
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn decode_master_hex(hex: &str) -> Result<[u8; 32], String> {
    to_master(&crate::vpn::credential::from_hex(hex)?)
}

/// 加密封装（给定子密钥，纯函数，便于单测）
pub(crate) fn encrypt_with_keys(keys: &VaultKeys, plain: &str) -> Result<String, String> {
    use rand::RngCore;

    let mut iv = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut iv);

    let ct = Sm4CbcEnc::new_from_slices(&keys.enc_key[..16], &iv)
        .map_err(|e| format!("初始化 SM4 失败: {}", e))?
        .encrypt_padded_vec_mut::<Pkcs7>(plain.as_bytes());

    let mut mac = HmacSm3::new_from_slice(&keys.mac_key)
        .map_err(|e| format!("初始化 HMAC 失败: {}", e))?;
    mac.update(&iv);
    mac.update(&ct);
    let tag = mac.finalize().into_bytes();

    Ok(format!(
        "{}{}.{}.{}",
        PREFIX,
        B64.encode(iv),
        B64.encode(&ct),
        B64.encode(tag)
    ))
}

/// 解密封装（给定子密钥，纯函数，便于单测）
pub(crate) fn decrypt_with_keys(keys: &VaultKeys, envelope: &str) -> Result<String, String> {
    let body = envelope
        .trim()
        .strip_prefix(PREFIX)
        .ok_or_else(|| "不是有效的密文信封".to_string())?;

    let mut parts = body.split('.');
    let iv_b64 = parts.next().ok_or_else(|| "密文格式不完整".to_string())?;
    let ct_b64 = parts.next().ok_or_else(|| "密文格式不完整".to_string())?;
    let mac_b64 = parts.next().ok_or_else(|| "密文格式不完整".to_string())?;
    if parts.next().is_some() {
        return Err("密文格式不完整".to_string());
    }

    let iv = B64.decode(iv_b64).map_err(|e| format!("IV 解码失败: {}", e))?;
    let ct = B64.decode(ct_b64).map_err(|e| format!("密文解码失败: {}", e))?;
    let tag = B64.decode(mac_b64).map_err(|e| format!("校验码解码失败: {}", e))?;
    if iv.len() != 16 {
        return Err("IV 长度不合法".to_string());
    }

    let mut mac = HmacSm3::new_from_slice(&keys.mac_key)
        .map_err(|e| format!("初始化 HMAC 失败: {}", e))?;
    mac.update(&iv);
    mac.update(&ct);
    mac.verify_slice(&tag).map_err(|_| "数据已被篡改".to_string())?;

    let plain = Sm4CbcDec::new_from_slices(&keys.enc_key[..16], &iv)
        .map_err(|e| format!("初始化 SM4 失败: {}", e))?
        .decrypt_padded_vec_mut::<Pkcs7>(&ct)
        .map_err(|_| "解密失败（填充不合法）".to_string())?;

    String::from_utf8(plain).map_err(|e| format!("明文解码失败: {}", e))
}

/// 取得（必要时创建）进程内缓存的两把子密钥
///
/// 整个「检查-创建-写入缓存」过程持有同一把锁，避免并发首启时
/// 生成两把不同的主密钥导致数据不可解。
pub(crate) fn vault_keys(app: &AppHandle) -> Result<VaultKeys, String> {
    let mut guard = VAULT_KEYS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(keys) = guard.as_ref() {
        return Ok(*keys);
    }
    let master = platform::load_or_create_master_key(app)?;
    let keys = keys_from_master(&master);
    *guard = Some(keys);
    Ok(keys)
}

/// 加密字符串，返回 `v1.` 信封密文
pub fn encrypt_str(app: &AppHandle, plain: &str) -> Result<String, String> {
    let k = vault_keys(app)?;
    encrypt_with_keys(&k, plain)
}

/// 解密 `v1.` 信封密文
pub fn decrypt_str(app: &AppHandle, envelope: &str) -> Result<String, String> {
    let k = vault_keys(app)?;
    decrypt_with_keys(&k, envelope)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys() -> VaultKeys {
        VaultKeys {
            enc_key: [7u8; 32],
            mac_key: [9u8; 32],
        }
    }

    /// 明文 → 密文 → 明文 往返一致
    #[test]
    fn roundtrip_ascii() {
        let k = keys();
        let env = encrypt_with_keys(&k, "hello").unwrap();
        assert!(env.starts_with("v1."));
        assert_eq!(decrypt_with_keys(&k, &env).unwrap(), "hello");
    }

    /// 中文与长文本往返一致
    #[test]
    fn roundtrip_chinese_and_long() {
        let k = keys();
        let text = "密码-带中文".repeat(1000);
        let env = encrypt_with_keys(&k, &text).unwrap();
        assert_eq!(decrypt_with_keys(&k, &env).unwrap(), text);
    }

    /// 随机 IV 使同文两次加密结果不同
    #[test]
    fn same_plaintext_differs_due_to_iv() {
        let k = keys();
        let a = encrypt_with_keys(&k, "same").unwrap();
        let b = encrypt_with_keys(&k, "same").unwrap();
        assert_ne!(a, b);
    }

    /// 篡改认证码后校验失败
    #[test]
    fn tamper_mac_is_rejected() {
        let k = keys();
        let env = encrypt_with_keys(&k, "data").unwrap();
        let idx = env.rfind('.').unwrap();
        let mut chars: Vec<char> = env.chars().collect();
        let mac_start = idx + 1;
        chars[mac_start] = if chars[mac_start] == 'A' { 'B' } else { 'A' };
        let tampered: String = chars.into_iter().collect();
        assert_eq!(
            decrypt_with_keys(&k, &tampered).unwrap_err(),
            "数据已被篡改"
        );
    }

    /// 篡改密文后解密失败
    #[test]
    fn tamper_ciphertext_is_rejected() {
        let k = keys();
        let env = encrypt_with_keys(&k, "data").unwrap();
        let first_dot = env.find('.').unwrap();
        let mut chars: Vec<char> = env.chars().collect();
        let ct_start = first_dot + 1;
        chars[ct_start] = if chars[ct_start] == 'A' { 'B' } else { 'A' };
        let tampered: String = chars.into_iter().collect();
        assert!(decrypt_with_keys(&k, &tampered).is_err());
    }

    /// 空串可正常往返
    #[test]
    fn empty_string_roundtrip() {
        let k = keys();
        let env = encrypt_with_keys(&k, "").unwrap();
        assert_eq!(decrypt_with_keys(&k, &env).unwrap(), "");
    }

    /// 前缀判定
    #[test]
    fn is_encrypted_prefix() {
        assert!(is_encrypted("v1.abc.def.ghi"));
        assert!(is_encrypted("  v1.abc.def.ghi"));
        assert!(!is_encrypted("{\"groups\":[]}"));
        assert!(!is_encrypted(""));
    }
}
