# 本地存储加密（SP2）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让所有落盘数据（`config.json` Store 与文件副本、`localStorage` 标记、VPN 密码）一律以 SM4 密文存储，进程内解密后在界面上展示明文。

**Architecture:** 加密放在 Rust 存储边界 `config.rs` 的四个出入口，内部消费方继续面对明文 `AppConfig`，接口不变。新增 `crypto.rs` 提供「SM4-CBC + 随机 IV + HMAC-SM3 先加密后认证」的信封能力，主密钥（32 字节随机数）交由系统钥匙串（macOS）/ DPAPI（Windows）托管，经 SM3 派生出加密密钥与认证密钥。前端通过新增的 `vault_encrypt` / `vault_decrypt` 命令复用同一套密钥与算法，不引入 `sm-crypto`。

**Tech Stack:** Rust（`sm4`、`cbc`、`sm3`、`hmac`、`base64`、`rand`，均为纯 Rust RustCrypto 系）、Tauri 2 命令、`tauri-plugin-store`、Vue 3 + TypeScript。

**关联设计文档:** `docs/superpowers/specs/2026-09-29-local-storage-encryption-design.md`

**约定：**
- 所有 Rust 命令均在 `src-tauri/` 下执行；所有前端命令均在仓库根目录执行。
- 测试命令：`cargo test`（在 `src-tauri/` 下）；构建校验：`pnpm build`（`vue-tsc --noEmit && vite build`）。
- 每个任务末尾的 `git commit` 步骤**仅在用户明确同意提交后执行**；未获同意时跳过提交、保留工作区改动。
- 每个任务完成后按项目规则重启应用（见 Task 6）。

---

## 文件结构

| 文件 | 动作 | 职责 |
| --- | --- | --- |
| `src-tauri/Cargo.toml` | 修改 | 增加 6 个密码学依赖 |
| `src-tauri/src/crypto.rs` | 新建 | SM4 信封算法 + 主密钥托管 + 对外 `encrypt_str` / `decrypt_str` / `is_encrypted` |
| `src-tauri/src/lib.rs` | 修改 | 声明 `mod crypto;`；注册 `vault_encrypt` / `vault_decrypt`；移除 `vpn_encrypt_password` |
| `src-tauri/src/config.rs` | 修改 | 四个存储出入口套 SM4 信封；历史明文回写升级；历史 VPN 密码迁移 |
| `src-tauri/src/vpn/credential.rs` | 修改 | 暴露 `to_hex` / `from_hex` / `dpapi` 供 `crypto.rs` 复用 |
| `src-tauri/src/vpn/commands.rs` | 修改 | 连接路径去掉 `credential::unprotect`；移除 `vpn_encrypt_password` |
| `src-tauri/src/commands.rs` | 修改 | 新增 `vault_encrypt` / `vault_decrypt` 命令 |
| `src/composables/safeStorage.ts` | 新建 | 前端「加密读写本地键值」封装 |
| `src/composables/useUpdater.ts` | 修改 | `DISMISS_KEY` 改经 `safeStorage` |
| `src/composables/useVpn.ts` | 修改 | 密码直接明文写入 `profile`，去掉 `vpn_encrypt_password` 调用 |
| `.trae/rules/project_rules.md` | 修改 | 写入「本地存储一律加密」项目规范 |

---

## Task 1: 依赖与 `crypto.rs`（算法 + 主密钥托管）

**Files:**
- Modify: `src-tauri/Cargo.toml:26-27`
- Create: `src-tauri/src/crypto.rs`
- Modify: `src-tauri/src/lib.rs:1-9`
- Modify: `src-tauri/src/vpn/credential.rs:10`、`src-tauri/src/vpn/credential.rs:19`、`src-tauri/src/vpn/credential.rs:86`

- [ ] **Step 1: 在 `Cargo.toml` 增加密码学依赖**

在 `src-tauri/Cargo.toml` 的 `[dependencies]` 段落末尾（`tauri-plugin-process = "2.3.1"` 之后、`[features]` 之前）追加：

```toml
base64 = "0.22"
cbc = { version = "0.1", features = ["alloc"] }
hmac = "0.12"
rand = "0.8"
sm3 = "0.4"
sm4 = "0.5"
```

- [ ] **Step 2: 暴露 `credential.rs` 中的十六进制与 DPAPI 能力**

`crypto.rs` 需要复用 `credential.rs` 的十六进制编解码与 DPAPI FFI。做三处最小改动：

`src-tauri/src/vpn/credential.rs:10`，把

```rust
fn to_hex(bytes: &[u8]) -> String {
```

改为

```rust
pub(crate) fn to_hex(bytes: &[u8]) -> String {
```

`src-tauri/src/vpn/credential.rs:19`，把

```rust
fn from_hex(text: &str) -> Result<Vec<u8>, String> {
```

改为

```rust
pub(crate) fn from_hex(text: &str) -> Result<Vec<u8>, String> {
```

`src-tauri/src/vpn/credential.rs:86`，把

```rust
mod dpapi {
```

改为

```rust
pub(crate) mod dpapi {
```

- [ ] **Step 3: 编写 `crypto.rs`（含失败测试先行）**

新建 `src-tauri/src/crypto.rs`：

```rust
//! 本地存储加密：SM4-CBC + 随机 IV + HMAC-SM3（先加密后认证）。
//!
//! 信封格式：`v1.<b64(iv)>.<b64(ct)>.<b64(mac)>`，以 `v1.` 前缀识别密文。
//! 主密钥（32 字节随机数）由系统钥匙串（macOS）/ DPAPI（Windows）托管，
//! 派生出加密密钥 `SM3(master‖"enc")` 与认证密钥 `SM3(master‖"mac")`。

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use cbc::cipher::{block_padding::Pkcs7, BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use hmac::{Hmac, Mac};
use sm3::{Digest, Sm3};
use tauri::AppHandle;

/// 密文信封前缀，同时用作版本号
const PREFIX: &str = "v1.";

/// macOS 钥匙串服务名 / 账号
#[cfg(target_os = "macos")]
const VAULT_SERVICE: &str = "ws-tools-vault";
#[cfg(target_os = "macos")]
const VAULT_ACCOUNT: &str = "ws-tools";

/// Windows 主密钥落盘文件名（内容为 `dpapi:<hex>`）
#[cfg(target_os = "windows")]
const KEY_FILE: &str = ".vault_key";

type Sm4CbcEnc = cbc::Encryptor<sm4::Sm4>;
type Sm4CbcDec = cbc::Decryptor<sm4::Sm4>;
type HmacSm3 = Hmac<Sm3>;

/// 派生后的两把子密钥
///
/// SM4 只接受 16 字节密钥，故加密时取 `enc_key` 的前 16 字节
/// （SM3 摘要左截断），认证仍使用完整的 32 字节 `mac_key`。
#[derive(Clone, Copy)]
struct VaultKeys {
    enc_key: [u8; 32],
    mac_key: [u8; 32],
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
fn encrypt_with_keys(keys: &VaultKeys, plain: &str) -> Result<String, String> {
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
fn decrypt_with_keys(keys: &VaultKeys, envelope: &str) -> Result<String, String> {
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
fn vault_keys(app: &AppHandle) -> Result<VaultKeys, String> {
    let mut guard = VAULT_KEYS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(keys) = guard.as_ref() {
        return Ok(*keys);
    }
    let master = load_or_create_master_key(app)?;
    let keys = VaultKeys {
        enc_key: sm3_of(&[&master, b"enc"]),
        mac_key: sm3_of(&[&master, b"mac"]),
    };
    *guard = Some(keys);
    Ok(keys)
}

/// 加密字符串，返回 `v1.` 信封密文
pub fn encrypt_str(app: &AppHandle, plain: &str) -> Result<String, String> {
    encrypt_with_keys(&vault_keys(app)?, plain)
}

/// 解密 `v1.` 信封密文
pub fn decrypt_str(app: &AppHandle, envelope: &str) -> Result<String, String> {
    decrypt_with_keys(&vault_keys(app)?, envelope)
}

/// 加载或创建主密钥
fn load_or_create_master_key(app: &AppHandle) -> Result<[u8; 32], String> {
    #[cfg(target_os = "macos")]
    {
        let _ = app;
        macos_master_key()
    }
    #[cfg(target_os = "windows")]
    {
        windows_master_key(app)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = app;
        Err("当前平台不支持加密存储".to_string())
    }
}

/// macOS：主密钥存系统钥匙串（明文为十六进制）
#[cfg(target_os = "macos")]
fn macos_master_key() -> Result<[u8; 32], String> {
    let existing = std::process::Command::new("security")
        .args([
            "find-generic-password",
            "-a",
            VAULT_ACCOUNT,
            "-s",
            VAULT_SERVICE,
            "-w",
        ])
        .output()
        .map_err(|e| format!("调用 security 失败: {}", e))?;

    if existing.status.success() {
        let hex = String::from_utf8_lossy(&existing.stdout).trim().to_string();
        return decode_master_hex(&hex);
    }

    let master = random_key();
    let hex = crate::vpn::credential::to_hex(&master);
    let output = std::process::Command::new("security")
        .args([
            "add-generic-password",
            "-U",
            "-a",
            VAULT_ACCOUNT,
            "-s",
            VAULT_SERVICE,
            "-w",
            &hex,
        ])
        .output()
        .map_err(|e| format!("调用 security 失败: {}", e))?;
    if !output.status.success() {
        return Err(format!(
            "写入钥匙串失败: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(master)
}

/// Windows：主密钥经 DPAPI 保护后写入 `<app_data_dir>/.vault_key`
#[cfg(target_os = "windows")]
fn windows_master_key(app: &AppHandle) -> Result<[u8; 32], String> {
    use tauri::Manager;

    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("获取应用数据目录失败: {}", e))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建应用数据目录失败: {}", e))?;
    let path = dir.join(KEY_FILE);

    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let text = text.trim();
            if let Some(hex) = text.strip_prefix("dpapi:") {
                let cipher = crate::vpn::credential::from_hex(hex)?;
                let plain = crate::vpn::credential::dpapi::unprotect(&cipher)?;
                return to_master(&plain);
            }
            if let Ok(master) = decode_master_hex(text) {
                return Ok(master);
            }
            // 文件存在但内容无法解析：绝不能覆盖既有密钥
            return Err("主密钥文件内容无法解析".to_string());
        }
        // 文件不存在时才创建新密钥
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        // 其它 IO 错误（权限/占用等）绝不能覆盖既有密钥，直接报错
        Err(e) => return Err(format!("读取主密钥文件失败: {}", e)),
    }

    let master = random_key();
    let cipher = crate::vpn::credential::dpapi::protect(&master)?;
    std::fs::write(&path, format!("dpapi:{}", crate::vpn::credential::to_hex(&cipher)))
        .map_err(|e| format!("写入主密钥失败: {}", e))?;
    Ok(master)
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
```

- [ ] **Step 4: 在 `lib.rs` 声明 `crypto` 模块**

`src-tauri/src/lib.rs:1-9`，把

```rust
mod models;
mod config;
mod commands;
```

改为

```rust
mod models;
mod config;
mod crypto;
mod commands;
```

- [ ] **Step 5: 运行测试，确认通过**

Run（在 `src-tauri/` 下）: `cargo test crypto::`
Expected: 编译通过，`tests::roundtrip_ascii`、`roundtrip_chinese_and_long`、`same_plaintext_differs_due_to_iv`、`tamper_mac_is_rejected`、`tamper_ciphertext_is_rejected`、`empty_string_roundtrip`、`is_encrypted_prefix` 全部 `ok`。

> 说明：此时 `encrypt_str` / `decrypt_str` / `vault_keys` 等尚无调用方，非测试构建可能出现 `dead_code` 警告；Task 2 接入 `config.rs` 后即消除。若担心噪声，可先执行 `cargo check` 确认只有 `dead_code` 警告、没有其它类型错误。

- [ ] **Step 6: Commit（需用户同意）**

```bash
git add src-tauri/Cargo.toml src-tauri/src/crypto.rs src-tauri/src/lib.rs src-tauri/src/vpn/credential.rs
git commit -m "feat(crypto): add SM4-CBC+HMAC-SM3 vault with keychain/DPAPI master key"
```

---

## Task 2: `config.rs` 存储边界接入加密信封

**Files:**
- Modify: `src-tauri/src/config.rs:24-28`（`load_from_store`）
- Modify: `src-tauri/src/config.rs:31-40`（`save_to_store`）
- Modify: `src-tauri/src/config.rs:43-59`（`load_from_file`）
- Modify: `src-tauri/src/config.rs:124-137`（`save_to_file`）
- Modify: `src-tauri/src/config.rs:140-166`（`load_config`）

- [ ] **Step 1: 改造 `save_to_store`——写密文**

将 `src-tauri/src/config.rs:31-40` 的

```rust
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
```

替换为

```rust
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
```

- [ ] **Step 2: 改造 `load_from_store`——读密文 / 兼容历史明文**

将 `src-tauri/src/config.rs:23-28` 的

```rust
/// 使用 Store 插件加载配置
fn load_from_store(app: &AppHandle) -> Option<AppConfig> {
    let store = app.store("config.json").ok()?;
    let value = store.get(STORE_KEY)?;
    serde_json::from_value::<AppConfig>(value).ok()
}
```

替换为

```rust
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
```

- [ ] **Step 3: 改造 `save_to_file`——写密文**

将 `src-tauri/src/config.rs:123-137` 的

```rust
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
```

替换为

```rust
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
```

- [ ] **Step 4: 改造 `load_from_file`——读密文 / 兼容历史明文**

将 `src-tauri/src/config.rs:42-59` 的

```rust
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
```

替换为

```rust
/// 从文件系统加载配置（兜底）。
///
/// - `Ok(Some((配置, 是否为历史明文)))`：读取成功
/// - `Ok(None)`：所有候选目录都没有可读配置
/// - `Err`：文件存在但无法读取或解密，调用方绝不能覆盖回写
fn load_from_file(app: &AppHandle) -> Result<Option<(AppConfig, bool)>, String> {
    use std::fs;

    let mut saw_unreadable = false;
    // 按优先级尝试多个路径
    let candidates = get_file_candidates(app);
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
```

- [ ] **Step 5: 改造 `load_config`——命中历史明文即回写密文**

将 `src-tauri/src/config.rs:139-166` 的

```rust
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
```

替换为

```rust
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
            // 迁移到 Store，并统一写密文
            let _ = save_to_store(app, &migrated);
            let _ = save_to_file(app, &migrated);
            return migrated;
        }
        // 文件中有密文但无法解密：绝不覆盖，退回内存/默认值
        Err(_) => return load_config_readonly(),
        Ok(None) => {}
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
```

> `load_config` 其余部分（第 3、4 步的内存缓存与默认配置）保持不变。
> 新增的 `load_config_readonly` 专门用于“有数据但读不出”的场景，避免第 4 步的
> `save_config(default)` 把无法解密的既有配置覆盖掉。

- [ ] **Step 6: 编译校验**

Run（在 `src-tauri/` 下）: `cargo check`
Expected: 编译通过，无错误。若出现 `dead_code` 警告指向 `crypto.rs`，属正常（Task 3/4 会消除）。

- [ ] **Step 7: 回归既有单测**

Run（在 `src-tauri/` 下）: `cargo test config::`
Expected: `config::tests` 下四个用例全部 `ok`。

- [ ] **Step 8: Commit（需用户同意）**

```bash
git add src-tauri/src/config.rs
git commit -m "feat(config): encrypt config store and file fallback with SM4 vault"
```

---

## Task 3: VPN 密码路径改造（去字段级加密 + 历史密码迁移）

**Files:**
- Modify: `src-tauri/src/config.rs:245-276`（`migrate_config` 增加 VPN 密码迁移）
- Modify: `src-tauri/src/vpn/commands.rs:396-401`（连接路径去掉 `unprotect`）
- Modify: `src/composables/useVpn.ts:303-308`（`savePassword` 改存明文）

- [ ] **Step 1: 在 `config.rs` 增加 VPN 密码迁移函数**

在 `src-tauri/src/config.rs` 的 `migrate_default_exclude` 函数之后（即 `migrate_config` 之前，约第 243 行位置）插入：

```rust
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
```

- [ ] **Step 2: 在 `migrate_config` 中调用该迁移**

将 `src-tauri/src/config.rs:267-271` 的

```rust
    // 3. 补齐新增的默认排除规则
    if migrate_default_exclude(&mut config) {
        changed = true;
    }

    if changed {
```

替换为

```rust
    // 3. 补齐新增的默认排除规则
    if migrate_default_exclude(&mut config) {
        changed = true;
    }

    // 4. 迁移历史字段级加密的 VPN 密码
    if migrate_vpn_passwords(&mut config) {
        changed = true;
    }

    if changed {
```

- [ ] **Step 3: 连接路径直接使用明文密码**

将 `src-tauri/src/vpn/commands.rs:396-401` 的

```rust
    // 2. 解出密码
    let password = credential::unprotect(&profile.password)
        .map_err(|e| format!("读取已保存的密码失败: {}", e))?;
    if password.is_empty() {
        return Err("这个配置还没有保存密码，请先在配置里填写密码".to_string());
    }
```

替换为

```rust
    // 2. 取密码（password 已是明文，落盘保护由整块配置的 SM4 信封承担）
    // 旧字段级密文若迁移失败会保留 password_encrypted=true，此时绝不能把密文当密码发出去
    if profile.password_encrypted {
        return Err("该配置的旧密码尚未完成迁移，请在 VPN 配置页重新填写密码".to_string());
    }
    let password = profile.password.clone();
    if password.is_empty() {
        return Err("这个配置还没有保存密码，请先在配置里填写密码".to_string());
    }
```

> 守卫说明：迁移失败时 `password_encrypted` 仍为 `true`、`password` 仍是 `keychain:`/`dpapi:` 密文，
> 若直接当明文发送会得到误导性的「认证失败」。此守卫把它转成明确的「请重新填写密码」。

- [ ] **Step 4: 前端 `savePassword` 改为存明文**

将 `src/composables/useVpn.ts:303-308` 的

```ts
  /** 保存密码：明文→密文由后端完成，前端不保存明文 */
  async function savePassword(id: string, plain: string) {
    if (!plain) return
    const cipher = await invoke<string>('vpn_encrypt_password', { key: id, plain })
    await updateProfile(id, { password: cipher, password_encrypted: true })
  }
```

替换为

```ts
  /** 保存密码：明文直接进入配置，整块配置落盘时由后端统一加密 */
  async function savePassword(id: string, plain: string) {
    if (!plain) return
    await updateProfile(id, { password: plain, password_encrypted: false })
  }
```

> 说明：`invoke` 在 `useVpn.ts` 其它命令中仍在使用，保留其 import。

- [ ] **Step 5: 补充 `migrate_vpn_passwords` 单测**

在 `src-tauri/src/config.rs` 的 `#[cfg(test)] mod tests` 中，先加一个构造辅助：

```rust
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
```

再加四个用例（覆盖「明文清标记 / 幂等 / 空串 / 旧密文解不开保留原值」）：

```rust
    #[test]
    fn migrate_vpn_passwords_clears_flag_for_plaintext() {
        let mut config = AppConfig { vpn_profiles: vec![vpn_profile("secret", true)], ..Default::default() };
        assert!(migrate_vpn_passwords(&mut config));
        assert_eq!(config.vpn_profiles[0].password, "secret");
        assert!(!config.vpn_profiles[0].password_encrypted);
    }

    #[test]
    fn migrate_vpn_passwords_is_idempotent() {
        let mut config = AppConfig { vpn_profiles: vec![vpn_profile("secret", true)], ..Default::default() };
        assert!(migrate_vpn_passwords(&mut config));
        assert!(!migrate_vpn_passwords(&mut config));
    }

    #[test]
    fn migrate_vpn_passwords_clears_flag_for_empty() {
        let mut config = AppConfig { vpn_profiles: vec![vpn_profile("", true)], ..Default::default() };
        assert!(migrate_vpn_passwords(&mut config));
        assert_eq!(config.vpn_profiles[0].password, "");
        assert!(!config.vpn_profiles[0].password_encrypted);
    }

    #[test]
    fn migrate_vpn_passwords_keeps_unreadable_legacy() {
        let mut config = AppConfig { vpn_profiles: vec![vpn_profile("keychain:__missing__", true)], ..Default::default() };
        assert!(!migrate_vpn_passwords(&mut config));
        assert_eq!(config.vpn_profiles[0].password, "keychain:__missing__");
        assert!(config.vpn_profiles[0].password_encrypted);
    }
```

Run（在 `src-tauri/` 下）: `cargo test config::`
Expected: `config::tests` 全部 `ok`（含新增四个）。

- [ ] **Step 6: 编译校验（Rust）**

Run（在 `src-tauri/` 下）: `cargo check`
Expected: 编译通过。此时 `credential` 仍被 `vpn_encrypt_password`（Task 4 才移除）使用，故 `vpn/commands.rs:13` 的 import 暂不动。

- [ ] **Step 7: 前端类型检查**

Run（仓库根目录）: `pnpm build`
Expected: `vue-tsc --noEmit` 无类型错误，`vite build` 成功产出。

> 遗留（非本次范围）：迁移成功后旧 macOS 钥匙串条目（`ws-tools-vpn-<key>`）未清理，
> `credential::forget` 仍无人调用。删除须在配置落盘成功之后进行，否则写盘失败会造成密码丢失，
> 故本任务不做，另行处理。

- [ ] **Step 8: Commit（需用户同意）**

```bash
git add src-tauri/src/config.rs src-tauri/src/vpn/commands.rs src/composables/useVpn.ts
git commit -m "feat(vpn): store password in plaintext and migrate legacy encrypted passwords"
```

---

## Task 4: 新增 `vault_encrypt` / `vault_decrypt` 命令并清理旧命令

**Files:**
- Modify: `src-tauri/src/commands.rs:22`（在 `get_config_path` 之后新增命令）
- Modify: `src-tauri/src/lib.rs:2-4`、`src-tauri/src/lib.rs:114`（注册）
- Modify: `src-tauri/src/vpn/commands.rs:13`、`src-tauri/src/vpn/commands.rs:241-245`（移除旧命令）

- [ ] **Step 1: 在 `commands.rs` 新增两个命令**

在 `src-tauri/src/commands.rs` 的 `get_config_path` 命令之后（即第 22 行 `}` 之后）插入：

```rust
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
```

- [ ] **Step 2: 在 `lib.rs` 注册新命令、移除旧命令**

`src-tauri/src/lib.rs:84-129` 的 `generate_handler!` 列表中，将

```rust
            commands::get_config_path,
            commands::get_default_exclude_rules,
```

改为

```rust
            commands::get_config_path,
            commands::vault_encrypt,
            commands::vault_decrypt,
            commands::get_default_exclude_rules,
```

并将

```rust
            vpn::commands::vpn_install_guide,
            vpn::commands::vpn_encrypt_password,
            vpn::commands::vpn_validate_cidr,
```

改为

```rust
            vpn::commands::vpn_install_guide,
            vpn::commands::vpn_validate_cidr,
```

- [ ] **Step 3: 移除 `vpn/commands.rs` 中的旧命令**

将 `src-tauri/src/vpn/commands.rs:241-245` 的

```rust
/// 把明文密码加密后返回，前端只保存密文，界面上不再回显明文
#[tauri::command]
pub fn vpn_encrypt_password(key: String, plain: String) -> Result<String, String> {
    credential::protect(&key, &plain)
}

```

整段删除（连同其后空行）。

- [ ] **Step 4: 清理已不再使用的 `credential` 导入**

将 `src-tauri/src/vpn/commands.rs:13` 的

```rust
use crate::vpn::{credential, detect, elevate, route};
```

改为

```rust
use crate::vpn::{detect, elevate, route};
```

- [ ] **Step 5: 编译校验**

Run（在 `src-tauri/` 下）: `cargo check`
Expected: 编译通过，无 `unused import` / `cannot find` 类错误。

- [ ] **Step 6: 运行全部 Rust 测试**

Run（在 `src-tauri/` 下）: `cargo test`
Expected: 全部 `ok`（含 `crypto::tests`、`config::tests`、`credential::tests`）。

- [ ] **Step 7: Commit（需用户同意）**

```bash
git add src-tauri/src/commands.rs src-tauri/src/lib.rs src-tauri/src/vpn/commands.rs
git commit -m "feat(vault): expose vault_encrypt/vault_decrypt and drop vpn_encrypt_password"
```

---

## Task 5: 前端 `safeStorage` 封装与 `useUpdater` 改造

**Files:**
- Create: `src/composables/safeStorage.ts`
- Modify: `src/composables/useUpdater.ts:1-3`、`src/composables/useUpdater.ts:59-61`、`src/composables/useUpdater.ts:72-80`

- [ ] **Step 1: 新建 `src/composables/safeStorage.ts`**

```ts
import { invoke } from '@tauri-apps/api/core'

/** 后端密文信封前缀，与 Rust crypto.rs 的 PREFIX 保持一致 */
const ENCRYPTED_PREFIX = 'v1.'

/**
 * 读取本地键值：密文经后端解密后返回明文字符串；不存在返回 null。
 * 历史明文值会被直接采用，并异步回写为密文。
 */
export async function safeGet(key: string): Promise<string | null> {
  if (typeof localStorage === 'undefined') return null
  let raw: string | null
  try {
    raw = localStorage.getItem(key)
  } catch {
    // 隐私模式下 localStorage 可能抛 SecurityError，按「无值」处理
    return null
  }
  if (raw === null) return null

  if (!raw.trimStart().startsWith(ENCRYPTED_PREFIX)) {
    // 历史明文：直接采用，并异步升级为密文
    void safeSet(key, raw)
    return raw
  }

  try {
    return await invoke<string>('vault_decrypt', { cipher: raw })
  } catch (err) {
    console.error(`[safeStorage] 解密 ${key} 失败`, err)
    return null
  }
}

/**
 * 写入本地键值：经后端加密后以密文落盘。
 * 加密不可用时降级为明文写入并打日志，不阻断功能。
 */
export async function safeSet(key: string, value: string): Promise<void> {
  if (typeof localStorage === 'undefined') return
  try {
    const cipher = await invoke<string>('vault_encrypt', { plain: value })
    localStorage.setItem(key, cipher)
  } catch (err) {
    console.error(`[safeStorage] 加密 ${key} 失败，降级为明文`, err)
    try {
      localStorage.setItem(key, value)
    } catch {
      // localStorage 不可用时忽略，仅内存生效
    }
  }
}
```

- [ ] **Step 2: `useUpdater.ts` 引入 `safeStorage`**

将 `src/composables/useUpdater.ts:1-3` 的

```ts
import { ref, computed } from 'vue'
import { check } from '@tauri-apps/plugin-updater'
import { relaunch } from '@tauri-apps/plugin-process'
```

替换为

```ts
import { ref, computed } from 'vue'
import { check } from '@tauri-apps/plugin-updater'
import { relaunch } from '@tauri-apps/plugin-process'
import { safeGet, safeSet } from './safeStorage'
```

- [ ] **Step 3: `useUpdater.ts` 改初始读取为异步加密读取**

将 `src/composables/useUpdater.ts:59-61` 的

```ts
  const dismissedVersion = ref<string>(
    typeof localStorage !== 'undefined' ? localStorage.getItem(DISMISS_KEY) || '' : ''
  )
```

替换为

```ts
  const dismissedVersion = ref<string>('')

  async function loadDismissed() {
    dismissedVersion.value = (await safeGet(DISMISS_KEY)) || ''
  }
  void loadDismissed()
```

- [ ] **Step 4: `useUpdater.ts` 改写入为加密写入**

将 `src/composables/useUpdater.ts:71-80` 的

```ts
  /** 忽略当前提示的版本（记入本地，重启后不再提示该版本） */
  function dismissCurrent() {
    if (!info.value) return
    dismissedVersion.value = info.value.version
    try {
      localStorage.setItem(DISMISS_KEY, info.value.version)
    } catch {
      // localStorage 不可用时忽略，仅内存生效
    }
  }
```

替换为

```ts
  /** 忽略当前提示的版本（记入本地，重启后不再提示该版本） */
  function dismissCurrent() {
    if (!info.value) return
    dismissedVersion.value = info.value.version
    void safeSet(DISMISS_KEY, info.value.version)
  }
```

- [ ] **Step 5: 前端类型检查与构建**

Run（仓库根目录）: `pnpm build`
Expected: `vue-tsc --noEmit` 无类型错误，`vite build` 成功。

- [ ] **Step 6: 确认 `localStorage` 已无直接明文读写**

Run: 在仓库内搜索 `src/` 下的 `localStorage.`
Expected: 仅剩 `src/composables/safeStorage.ts` 中的两处（`getItem` / `setItem`），`useUpdater.ts` 不再直接出现 `localStorage`。

- [ ] **Step 7: Commit（需用户同意）**

```bash
git add src/composables/safeStorage.ts src/composables/useUpdater.ts
git commit -m "feat(storage): route localStorage access through encrypted safeStorage"
```

---

## Task 6: 写入项目规范并重启验收

**Files:**
- Modify: `.trae/rules/project_rules.md`

- [ ] **Step 1: 追加「本地存储一律加密」规范**

在 `.trae/rules/project_rules.md` 末尾追加：

```markdown
## 本地存储一律加密

凡写入本地磁盘的数据（配置文件、`localStorage` 等）必须密文存储，展示时解密。

- Rust 侧：新增持久化字段必须经 `crypto::encrypt_str` / `crypto::decrypt_str`（`src-tauri/src/crypto.rs`），不得直接明文落盘。
- 前端侧：读写本地键值必须经 `safeStorage` 封装（`src/composables/safeStorage.ts`），不得直接调用 `localStorage`。
- 新增配置读写入口时，须复用 `config.rs` 的加密信封边界，不要在业务代码里各自加解密。
```

- [ ] **Step 2: 重启应用**

先结束残留的 vite / tauri 进程，再重新拉起（macOS）：

```bash
pkill -f "ws-tools" ; pkill -f "vite" ; pkill -f "target/debug/ws-tools"
pnpm exec tauri dev
```

> 注意：不要用 `pnpm tauri dev`（`package.json` 的 `tauri` 脚本会解析成 `tauri dev dev`）。

- [ ] **Step 3: 按验收标准逐项核对**

Run/Check:
1. 打开 `<app_data_dir>/config.json`，内容应为 `v1.` 开头的一行信封密文，肉眼不可读。
2. 界面中项目分组、VPN 配置与规则、应用路径均正常展示为明文、可正常操作。
3. 若此前存在明文配置，首次启动后 `config.json` 已被自动重写为密文。
4. macOS 钥匙串中存在 `ws-tools-vault` / `ws-tools` 条目：

   ```bash
   security find-generic-password -a ws-tools -s ws-tools-vault -w
   ```

   能取回十六进制主密钥；重启应用后仍能正确解密。
5. 手工把 `config.json` 里密文的任意一个字符改掉后重启：应用不崩溃，日志报「数据已被篡改」并走兜底。
6. 前端 `localStorage` 中更新提示标记（键 `ws-tools:dismissed-update-version`）为 `v1.` 密文。
7. 旧版 `keychain:` / `dpapi:` 密码的 VPN 配置升级后可正常连接，`config.json` 中不再出现这两个前缀。

Expected: 以上 7 项全部满足。

- [ ] **Step 4: Commit（需用户同意）**

```bash
git add .trae/rules/project_rules.md
git commit -m "docs(rules): require encryption for all local storage"
```

> 注意：`.trae/` 被仓库 `.gitignore:44` 忽略，提交前需确认是否强制加入（`git add -f .trae/rules/project_rules.md`）或维持仅本地生效。

---

## 自检（Self-Review）

**1. Spec 覆盖对照**

| Spec 章节 | 对应任务 |
| --- | --- |
| §3 密钥管理（钥匙串 / DPAPI、密钥分离） | Task 1（`macos_master_key` / `windows_master_key` / `sm3_of`） |
| §4 算法与信封格式 | Task 1（`encrypt_with_keys` / `decrypt_with_keys`） |
| §5.1 新增 `crypto.rs` | Task 1 |
| §5.2 `config.rs` 四出入口 | Task 2 |
| §5.3 `vault_encrypt` / `vault_decrypt` + 注册 | Task 4 |
| §5.4 VPN 密码路径改造 | Task 3 |
| §6 前端 `safeStorage` + `useUpdater` | Task 5 |
| §7 迁移与兼容（明文升级、旧 VPN 密码、解密失败降级） | Task 2 Step 5（明文回写）、Task 3 Step 1-2（旧密码迁移）、Task 2 Step 4（密文解密失败 `continue` 走下一候选） |
| §8 验收标准 | Task 6 Step 3 |
| §9 项目规范 | Task 6 Step 1 |
| §10 风险与已知问题 | 记录于设计文档；`get_config_path` 的既有缺陷按约定不修 |
| §11 实施顺序 | Task 1→6 顺序一致 |

**2. Placeholder 扫描**：无 `TBD` / `TODO` / 「稍后实现」/「类似 Task N」等占位；每个代码步骤均给出完整代码。

**3. 类型与命名一致性核对**
- `crypto::encrypt_str(app: &AppHandle, plain: &str)` / `decrypt_str(app: &AppHandle, envelope: &str)` / `is_encrypted(s: &str)`：Task 2、Task 4 调用签名一致。
- `load_from_store` / `load_from_file` 统一返回 `Option<(AppConfig, bool)>`：Task 2 中定义与使用一致。
- `migrate_vpn_passwords(config: &mut AppConfig) -> bool`：Task 3 定义与 `migrate_config` 调用一致。
- `credential::to_hex` / `from_hex` / `dpapi::{protect,unprotect}`：Task 1 暴露、`crypto.rs`（macOS/Windows 分支）使用一致。
- 前端命令名 `vault_encrypt` / `vault_decrypt` 与参数名 `plain` / `cipher`：Task 4（Rust）与 Task 5（`safeStorage.ts` invoke）一致。
- `safeGet` / `safeSet`：Task 5 定义与 `useUpdater.ts` 调用一致。

**4. 已知取舍**
- Task 1 完成后、Task 2 之前，`crypto.rs` 的对外函数暂无调用方，非测试构建可能出现 `dead_code` 警告，属预期，Task 2 消除。
- 前端无单测框架（`package.json` 未配置 `test` 脚本），前端任务以 `pnpm build`（`vue-tsc` 类型检查）+ Task 6 运行时验收作为验证手段。
- Windows 分支（DPAPI + `.vault_key`）在 macOS 开发机上不参与编译，需在 Windows 环境补验；本计划已保证其代码与既有 `credential.rs` DPAPI FFI 保持一致。
