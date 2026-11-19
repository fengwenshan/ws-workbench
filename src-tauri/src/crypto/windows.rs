//! Windows 的主密钥托管：DPAPI 保护后写成 `<数据目录>/.vault_key`。
//!
//! 硬约束是**主密钥必须和它加密的数据待在同一个目录**。数据目录不一定是
//! `app_data_dir`：用户目录被沙箱、企业策略或杀软锁死时，`config::data_dir`
//! 会回退到项目内 `.config`，数据库、附件、设置全都跟着回退。旧实现按「第一个
//! 存在 `.vault_key` 的候选目录」取密钥，数据却按「第一个可写目录」落盘，
//! 两者一旦分家，读出来全是「数据已被篡改」、写盘又全部失败 —— 现场表现就是
//! 「新建 VPN 配置看起来成功了，但弹窗不关，重启后配置不见了」。
//!
//! 所以这里先定数据目录，再定密钥：
//!
//! 1. 数据目录自带 `.vault_key` 且能解开数据目录里的密文 → 直接用；
//! 2. 否则找别的候选目录里的旧密钥，谁真能解开数据目录里的密文就用谁，
//!    并把它复制进数据目录（从此密钥与数据绑在一起）；
//! 3. 数据目录里没有密文（全新安装）→ 沿用旧密钥，没有旧密钥就新建；
//! 4. 数据目录里有密文，却没有任何密钥能解开 → 报错，**绝不新建密钥**：
//!    新建只会把旧数据变成永久读不出来的垃圾。
//!
//! 平台验证状态：本文件只在 Windows 上编译，Windows CI（test-windows）验证；
//! `choose_key` 的决策是纯逻辑，单测在文件末尾。

use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager};

use super::{decode_master_hex, decrypt_with_keys, keys_from_master, random_key, to_master};

/// 主密钥文件名（内容为 `dpapi:<hex>`）
const KEY_FILE: &str = ".vault_key";

/// 数据目录里出现这些条目就说明「里面已经有加密数据了」
const ENCRYPTED_MARKERS: &[&str] = &["config.json", "ws-workbench.db"];
/// 附件目录里只要有文件就同样是加密数据（WST1 容器）
const ENCRYPTED_DIRS: &[&str] = &["attachments", "clipboard-attachments"];

/// 读完数据目录、定下该用哪把主密钥（必要时新建）
pub(super) fn load_or_create_master_key(app: &AppHandle) -> Result<[u8; 32], String> {
    let data_dir = crate::config::data_dir(app)?;
    let key_path = data_dir.join(KEY_FILE);
    // 自带的密钥文件读不出来时直接报错：绝不能覆盖既有密钥
    let local = parse_master_key_file(&key_path)?;
    let legacy = legacy_keys(app, &data_dir);
    let choice = choose_key(local, &legacy, has_encrypted_data(&data_dir), |master| {
        key_fits(&data_dir, master)
    })?;

    match choice {
        KeyChoice::Local(master) => Ok(master),
        KeyChoice::Adopt(from, master) => {
            write_master_key_file(&key_path, &master)?;
            eprintln!(
                "[vault] 主密钥已跟随数据目录（{} → {}）",
                from.display(),
                key_path.display()
            );
            Ok(master)
        }
        KeyChoice::Fresh => {
            let master = random_key();
            write_master_key_file(&key_path, &master)?;
            Ok(master)
        }
    }
}

/// 选定的主密钥来源
#[derive(Debug)]
enum KeyChoice {
    /// 数据目录自带的密钥
    Local([u8; 32]),
    /// 从别的目录接管：来源文件 + 密钥
    Adopt(PathBuf, [u8; 32]),
    /// 全新安装：在数据目录里新建
    Fresh,
}

/// 纯决策：给定数据目录自带的密钥、别处的旧密钥、数据目录里有没有密文，
/// 以及「这把密钥能不能解开数据目录里的密文」的判定，选出该用的密钥。
///
/// `fits` 返回 `None` 表示没有可验证的密文（还没有设置文件、或设置是历史明文）。
fn choose_key(
    local: Option<[u8; 32]>,
    legacy: &[(PathBuf, [u8; 32])],
    has_data: bool,
    fits: impl Fn(&[u8; 32]) -> Option<bool>,
) -> Result<KeyChoice, String> {
    // 数据目录自带的密钥：验不过才算可疑，没东西可验就信任它
    if let Some(master) = local {
        if fits(&master) != Some(false) {
            return Ok(KeyChoice::Local(master));
        }
    }

    // 别处的旧密钥里，谁能解开就用谁（数据目录回退时代的主密钥）
    let mut verdict_known = false;
    for (path, master) in legacy {
        match fits(master) {
            Some(true) => return Ok(KeyChoice::Adopt(path.clone(), *master)),
            Some(false) => verdict_known = true,
            None => {}
        }
    }

    // 带着密钥却验不过：数据不是它加密的，换一把只会更糟
    if local.is_some() {
        return Err(
            "数据目录里的密文不是它的 .vault_key 加密的（主密钥与数据不匹配）。\
             请把原来那份 .vault_key 放回数据目录后重试"
                .to_string(),
        );
    }

    // 数据目录里还没有密文：沿用旧密钥，别让旧附件/旧设置变成孤儿
    if !has_data {
        if let Some((path, master)) = legacy.first() {
            return Ok(KeyChoice::Adopt(path.clone(), *master));
        }
        return Ok(KeyChoice::Fresh);
    }

    // 有密文但一句都验不了（例如只剩数据库、设置还没落盘）：沿用旧密钥，
    // 至少保证密钥与数据同目录，旧数据还有机会被认出来
    if !verdict_known {
        if let Some((path, master)) = legacy.first() {
            return Ok(KeyChoice::Adopt(path.clone(), *master));
        }
    }

    Err(
        "数据目录里已有加密数据，却找不到能解开它的主密钥（.vault_key）。\
         不会新建密钥覆盖它，请恢复原来的 .vault_key 后重试"
            .to_string(),
    )
}

/// 其它候选目录里能读出来的旧密钥
fn legacy_keys(app: &AppHandle, data_dir: &Path) -> Vec<(PathBuf, [u8; 32])> {
    let mut out = Vec::new();
    for dir in candidate_dirs(app) {
        if dir == data_dir {
            continue;
        }
        let path = dir.join(KEY_FILE);
        match parse_master_key_file(&path) {
            Ok(Some(master)) => out.push((path, master)),
            Ok(None) => {}
            // 旧密钥文件损坏时跳过：这里是在找候选，不是要覆盖它
            Err(err) => eprintln!(
                "[vault] 跳过无法解析的旧主密钥（{}）: {}",
                path.display(),
                err
            ),
        }
    }
    out
}

/// 可能存过主密钥的目录：标准数据目录优先，再按回退顺序
fn candidate_dirs(app: &AppHandle) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(dir) = app.path().app_data_dir() {
        dirs.push(dir);
    }
    for (_, dir) in crate::config::storage_candidates(app) {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs
}

/// 数据目录里有没有用主密钥加密过的数据
fn has_encrypted_data(dir: &Path) -> bool {
    if ENCRYPTED_MARKERS.iter().any(|name| dir.join(name).is_file()) {
        return true;
    }
    ENCRYPTED_DIRS.iter().any(|name| {
        let path = dir.join(name);
        path.is_dir()
            && std::fs::read_dir(&path)
                .map(|mut entries| entries.next().is_some())
                .unwrap_or(false)
    })
}

/// 数据目录里的密文能不能被这把主密钥解开。
///
/// 只拿 `config.json` 做验证就够了：它在设置第一次落盘时就存在，几乎必然可验。
/// 返回 `None` 表示没有可验证的密文。
fn key_fits(dir: &Path, master: &[u8; 32]) -> Option<bool> {
    let text = crate::util::fs::read_text_if_exists(&dir.join("config.json")).ok()??;
    let envelope = settings_envelope(&text)?;
    Some(decrypt_with_keys(&keys_from_master(master), &envelope).is_ok())
}

/// 从 `config.json` 里取出设置密文。
///
/// 两种格式都可能遇到：`tauri-plugin-store` 写的是 `{"app_config":"v1..."}`，
/// 文件兜底写的是裸信封 `v1...`。
fn settings_envelope(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if super::is_encrypted(trimmed) {
        return Some(trimmed.to_string());
    }
    let value: serde_json::Value = serde_json::from_str(trimmed).ok()?;
    value
        .get("app_config")
        .and_then(|value| value.as_str())
        .filter(|value| super::is_encrypted(value))
        .map(|value| value.to_string())
}

/// 解析主密钥文件；文件不存在时返回 None（由调用方决定是否新建）
fn parse_master_key_file(path: &Path) -> Result<Option<[u8; 32]>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let text = text.trim();
            if let Some(hex) = text.strip_prefix("dpapi:") {
                let cipher = crate::vpn::credential::from_hex(hex)?;
                let plain = crate::vpn::credential::dpapi::unprotect(&cipher)?;
                return Ok(Some(to_master(&plain)?));
            }
            if let Ok(master) = decode_master_hex(text) {
                return Ok(Some(master));
            }
            // 文件存在但内容无法解析：绝不能覆盖既有密钥
            Err("主密钥文件内容无法解析".to_string())
        }
        // 文件不存在时才允许新建
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        // 其它 IO 错误（权限/占用等）绝不能覆盖既有密钥，直接报错
        Err(e) => Err(format!("读取主密钥文件失败: {}", e)),
    }
}

/// 把主密钥经 DPAPI 保护后写进指定文件
fn write_master_key_file(path: &Path, master: &[u8; 32]) -> Result<(), String> {
    let cipher = crate::vpn::credential::dpapi::protect(master)?;
    let text = format!("dpapi:{}", crate::vpn::credential::to_hex(&cipher));
    crate::util::fs::write_text(path, &text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn legacy(path: &str, byte: u8) -> (PathBuf, [u8; 32]) {
        (PathBuf::from(path), [byte; 32])
    }

    fn local(byte: u8) -> [u8; 32] {
        [byte; 32]
    }

    /// 数据目录自带的密钥能解开密文：直接用它，不去找旧密钥
    #[test]
    fn prefers_local_key_that_fits() {
        let legacy = vec![legacy(r"C:\old\.vault_key", 2)];
        let choice = choose_key(Some(local(1)), &legacy, true, |master| {
            Some(master == &local(1))
        })
        .unwrap();
        assert!(matches!(choice, KeyChoice::Local(master) if master == local(1)));
    }

    /// 数据目录没有密钥，旧密钥能解开它的密文：接管旧密钥（密钥跟着数据走）
    #[test]
    fn adopts_legacy_key_that_fits() {
        let legacy = vec![legacy(r"C:\old\.vault_key", 2)];
        let choice = choose_key(None, &legacy, true, |master| Some(master == &local(2))).unwrap();
        match choice {
            KeyChoice::Adopt(path, master) => {
                assert_eq!(path, PathBuf::from(r"C:\old\.vault_key"));
                assert_eq!(master, local(2));
            }
            other => panic!("应接管旧密钥，实际是 {:?}", other),
        }
    }

    /// 数据目录里没有密文：沿用旧密钥，免得旧附件/旧设置变成孤儿
    #[test]
    fn adopts_legacy_key_when_data_dir_is_empty() {
        let legacy = vec![legacy(r"C:\old\.vault_key", 2)];
        let choice = choose_key(None, &legacy, false, |_| None).unwrap();
        assert!(matches!(choice, KeyChoice::Adopt(_, master) if master == local(2)));
    }

    /// 数据目录没有密钥、别处也没有：新建
    #[test]
    fn creates_key_for_fresh_install() {
        let choice = choose_key(None, &[], false, |_| None).unwrap();
        assert!(matches!(choice, KeyChoice::Fresh));
    }

    /// 已有加密数据却找不到任何能解开的密钥：报错，不新建密钥
    #[test]
    fn refuses_to_orphan_data_without_key() {
        let err = choose_key(None, &[], true, |_| None).unwrap_err();
        assert!(err.contains("找不到能解开它"), "错误文案要能定位问题: {err}");
    }

    /// 旧密钥验不过（数据不是它加密的）：宁可报错也不接管，否则数据永久读不出来
    #[test]
    fn refuses_legacy_key_that_cannot_decrypt_data() {
        let legacy = vec![legacy(r"C:\old\.vault_key", 2)];
        let err = choose_key(None, &legacy, true, |_| Some(false)).unwrap_err();
        assert!(err.contains("找不到能解开它"), "错误文案要能定位问题: {err}");
    }

    /// 自带密钥解不开数据、旧密钥也解不开：报错，别假装能用
    #[test]
    fn reports_key_data_mismatch() {
        let legacy = vec![legacy(r"C:\old\.vault_key", 2)];
        let err = choose_key(Some(local(1)), &legacy, true, |_| Some(false)).unwrap_err();
        assert!(err.contains("不匹配"), "错误文案要能定位问题: {err}");
    }

    /// 数据目录里是历史明文设置（验不了）：信任自带的密钥
    #[test]
    fn trusts_local_key_when_nothing_to_verify() {
        let choice = choose_key(Some(local(1)), &[], true, |_| None).unwrap();
        assert!(matches!(choice, KeyChoice::Local(master) if master == local(1)));
    }

    /// store 格式（JSON 包一层）和裸信封都要能取出来
    #[test]
    fn reads_settings_envelope_in_both_formats() {
        let bare = "v1.aXY=.Y3Q=.bWFj";
        assert_eq!(settings_envelope(bare).unwrap(), bare);
        let wrapped = format!(r#"{{"app_config":"{}"}}"#, bare);
        assert_eq!(settings_envelope(&wrapped).unwrap(), bare);
        assert!(settings_envelope("{}").is_none());
        assert!(settings_envelope(r#"{"app_config":"plain"}"#).is_none());
    }
}
