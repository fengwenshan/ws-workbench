//! 二进制流式加密容器 `WST1`：用于加密附件大文件，内存占用近似常数。
//!
//! 容器结构（小端序）：
//! ```text
//! Header（明文，纳入 MAC 覆盖）
//!   magic     "WST1"                     4 B
//!   version   0x01                       1 B
//!   flags     0x00（保留）               1 B
//!   orig_size 原始字节数 u64             8 B
//! Body（若干帧，顺序排列）
//!   frame := plain_len u32(4B) | iv 16B | cipher SM4-CBC(PKCS7) 变长
//! Trailer
//!   end_mark  0x00000000（plain_len=0）   4 B
//!   mac       HMAC-SM3(header ‖ frames) 32 B
//! ```
//!
//! 密钥复用 [`crate::crypto::vault_keys`]：SM4 key = `enc_key[0..16]`，
//! HMAC key = `mac_key`（全 32B）。

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;

use cbc::cipher::{block_padding::Pkcs7, BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use hmac::{Hmac, Mac};
use rand::RngCore;
use sm3::Sm3;
use tauri::AppHandle;

use crate::crypto::{vault_keys, VaultKeys};

const MAGIC: &[u8; 4] = b"WST1";
const VERSION: u8 = 0x01;
const FLAGS: u8 = 0x00;
const HEADER_LEN: usize = 14;
const IV_LEN: usize = 16;
const BLOCK: usize = 16;
const MAC_LEN: usize = 32;
/// 每帧明文块大小：1 MiB
const CHUNK: usize = 1024 * 1024;

type Sm4CbcEnc = cbc::Encryptor<sm4::Sm4>;
type Sm4CbcDec = cbc::Decryptor<sm4::Sm4>;
type HmacSm3 = Hmac<Sm3>;

/// 尽量读满缓冲区，直到填满或遇到 EOF
fn read_chunk<R: Read>(reader: &mut R, buf: &mut [u8]) -> Result<usize, String> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(format!("读取源文件失败: {}", e)),
        }
    }
    Ok(filled)
}

/// 精确读满缓冲区，不足则报错
fn read_exact<R: Read>(reader: &mut R, buf: &mut [u8], what: &str) -> Result<(), String> {
    reader
        .read_exact(buf)
        .map_err(|e| format!("读取{}失败: {}", what, e))
}

fn init_mac(key: &[u8]) -> Result<HmacSm3, String> {
    HmacSm3::new_from_slice(key).map_err(|e| format!("初始化 HMAC 失败: {}", e))
}

/// 核心加密：把 `reader` 全部内容按 `WST1` 帧格式写入 `writer`
///
/// `orig_size` 为原始字节数，写入文件头并参与 MAC；
/// `on_progress(done, total)` 在每帧写完后回调。
fn encrypt_core<R: Read, W: Write>(
    keys: &VaultKeys,
    reader: &mut R,
    writer: &mut W,
    orig_size: u64,
    on_progress: &mut dyn FnMut(u64, u64),
) -> Result<(), String> {
    let mut mac = init_mac(&keys.mac_key)?;

    let mut header = Vec::with_capacity(HEADER_LEN);
    header.extend_from_slice(MAGIC);
    header.push(VERSION);
    header.push(FLAGS);
    header.extend_from_slice(&orig_size.to_le_bytes());
    writer
        .write_all(&header)
        .map_err(|e| format!("写入文件头失败: {}", e))?;
    mac.update(&header);

    let mut written: u64 = 0;
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = read_chunk(reader, &mut buf)?;
        if n == 0 {
            break;
        }
        let mut iv = [0u8; IV_LEN];
        rand::thread_rng().fill_bytes(&mut iv);

        let cipher = Sm4CbcEnc::new_from_slices(&keys.enc_key[..16], &iv)
            .map_err(|e| format!("初始化 SM4 失败: {}", e))?
            .encrypt_padded_vec_mut::<Pkcs7>(&buf[..n]);

        let len_bytes = (n as u32).to_le_bytes();
        writer
            .write_all(&len_bytes)
            .and_then(|_| writer.write_all(&iv))
            .and_then(|_| writer.write_all(&cipher))
            .map_err(|e| format!("写入加密数据失败: {}", e))?;

        mac.update(&len_bytes);
        mac.update(&iv);
        mac.update(&cipher);

        written += n as u64;
        on_progress(written, orig_size);
    }

    // 结束标记（不纳入 MAC：MAC 只覆盖 header ‖ frames）
    writer
        .write_all(&0u32.to_le_bytes())
        .map_err(|e| format!("写入结束标记失败: {}", e))?;

    let tag = mac.finalize().into_bytes();
    writer
        .write_all(&tag)
        .map_err(|e| format!("写入校验码失败: {}", e))?;
    writer.flush().map_err(|e| format!("刷新目标失败: {}", e))?;

    Ok(())
}

/// 核心解密：从 `reader` 读取 `WST1`，明文写入 `writer`，末尾校验 HMAC-SM3
///
/// 校验失败返回 `Err`；调用方据此丢弃已写入的内容（文件场景丢弃临时文件）。
fn decrypt_core<R: Read, W: Write>(
    keys: &VaultKeys,
    reader: &mut R,
    writer: &mut W,
    on_progress: &mut dyn FnMut(u64, u64),
) -> Result<(), String> {
    let mut mac = init_mac(&keys.mac_key)?;

    let mut header = [0u8; HEADER_LEN];
    read_exact(reader, &mut header, "文件头")?;
    if &header[0..4] != MAGIC {
        return Err("不是有效的 WST1 文件".to_string());
    }
    if header[4] != VERSION {
        return Err(format!("不支持的 WST1 版本: {}", header[4]));
    }
    mac.update(&header);

    let mut orig = [0u8; 8];
    orig.copy_from_slice(&header[6..14]);
    let total = u64::from_le_bytes(orig);

    let mut written: u64 = 0;
    loop {
        let mut len_bytes = [0u8; 4];
        read_exact(reader, &mut len_bytes, "帧长度")?;
        let plain_len = u32::from_le_bytes(len_bytes) as usize;
        if plain_len == 0 {
            break;
        }
        if plain_len > CHUNK {
            return Err("帧长度非法".to_string());
        }

        let mut iv = [0u8; IV_LEN];
        read_exact(reader, &mut iv, "帧 IV")?;

        let cipher_len = (plain_len / BLOCK + 1) * BLOCK;
        let mut cipher = vec![0u8; cipher_len];
        read_exact(reader, &mut cipher, "帧密文")?;

        mac.update(&len_bytes);
        mac.update(&iv);
        mac.update(&cipher);

        let plain = Sm4CbcDec::new_from_slices(&keys.enc_key[..16], &iv)
            .map_err(|e| format!("初始化 SM4 失败: {}", e))?
            .decrypt_padded_vec_mut::<Pkcs7>(&cipher)
            .map_err(|_| "文件损坏或无法解密".to_string())?;
        if plain.len() != plain_len {
            return Err("密文长度不匹配".to_string());
        }
        writer
            .write_all(&plain)
            .map_err(|e| format!("写入明文失败: {}", e))?;

        written += plain_len as u64;
        on_progress(written, total);
    }

    writer
        .flush()
        .map_err(|e| format!("刷新明文失败: {}", e))?;

    let mut tag = [0u8; MAC_LEN];
    read_exact(reader, &mut tag, "校验码")?;
    mac.verify_slice(&tag)
        .map_err(|_| "文件损坏或无法解密".to_string())?;

    Ok(())
}

/// 流式加密文件：`src` → `dst`（`WST1` 格式）
///
/// `on_progress(done, total)` 在每帧写完后回调，用于推送进度事件。
pub fn encrypt_file_stream(
    app: &AppHandle,
    src: &Path,
    dst: &Path,
    on_progress: &mut dyn FnMut(u64, u64),
) -> Result<(), String> {
    let keys = vault_keys(app)?;

    let orig_size = std::fs::metadata(src)
        .map_err(|e| format!("读取源文件信息失败: {}", e))?
        .len();

    let file = File::open(src).map_err(|e| format!("打开源文件失败: {}", e))?;
    let mut reader = BufReader::with_capacity(CHUNK, file);
    let out = File::create(dst).map_err(|e| format!("创建目标文件失败: {}", e))?;
    let mut writer = BufWriter::with_capacity(CHUNK, out);

    encrypt_core(&keys, &mut reader, &mut writer, orig_size, on_progress)
}

/// 流式解密 `WST1` 文件：`src` → `dst`
///
/// 边读边解密写入同目录临时文件，末尾 MAC 校验通过后再落位，
/// 校验失败则丢弃临时文件，绝不产出损坏内容。
pub fn decrypt_file_stream(
    app: &AppHandle,
    src: &Path,
    dst: &Path,
    on_progress: &mut dyn FnMut(u64, u64),
) -> Result<(), String> {
    let keys = vault_keys(app)?;

    let file = File::open(src).map_err(|e| format!("打开密文失败: {}", e))?;
    let mut reader = BufReader::with_capacity(CHUNK, file);

    let parent = dst
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| Path::new(".").to_path_buf());
    let tmp = tempfile::NamedTempFile::new_in(&parent)
        .map_err(|e| format!("创建临时文件失败: {}", e))?;
    let mut writer = BufWriter::with_capacity(CHUNK, tmp.as_file());

    decrypt_core(&keys, &mut reader, &mut writer, on_progress)?;
    drop(writer);

    // 目标已存在时先移除，保证 persist 可覆盖（Windows 上 rename 不覆盖）
    match std::fs::remove_file(dst) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("写入目标文件失败: {}", e)),
    }
    tmp.persist(dst)
        .map_err(|e| format!("写入目标文件失败: {}", e))?;

    Ok(())
}

/// 内存加密：明文 `data` → `WST1` 密文字节（全程不落盘，无明文临时文件）
pub fn encrypt_bytes(app: &AppHandle, data: &[u8]) -> Result<Vec<u8>, String> {
    let keys = vault_keys(app)?;
    encrypt_bytes_with_keys(&keys, data)
}

/// 内存解密：`WST1` 密文字节 → 明文（末尾 HMAC-SM3 校验失败即报错）
pub fn decrypt_bytes(app: &AppHandle, blob: &[u8]) -> Result<Vec<u8>, String> {
    let keys = vault_keys(app)?;
    decrypt_bytes_with_keys(&keys, blob)
}

fn encrypt_bytes_with_keys(keys: &VaultKeys, data: &[u8]) -> Result<Vec<u8>, String> {
    let mut src = data;
    let mut out = Vec::with_capacity(data.len() + HEADER_LEN + BLOCK + MAC_LEN + 8);
    encrypt_core(keys, &mut src, &mut out, data.len() as u64, &mut |_, _| {})?;
    Ok(out)
}

fn decrypt_bytes_with_keys(keys: &VaultKeys, blob: &[u8]) -> Result<Vec<u8>, String> {
    let mut src = blob;
    let mut out = Vec::new();
    decrypt_core(keys, &mut src, &mut out, &mut |_, _| {})?;
    Ok(out)
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

    #[test]
    fn roundtrip_empty() {
        let k = keys();
        let blob = encrypt_bytes_with_keys(&k, b"").unwrap();
        assert!(blob.starts_with(MAGIC));
        assert_eq!(decrypt_bytes_with_keys(&k, &blob).unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn roundtrip_small() {
        let k = keys();
        let data = b"hello-\xe4\xb8\xad\xe6\x96\x87".to_vec();
        let blob = encrypt_bytes_with_keys(&k, &data).unwrap();
        assert_ne!(blob, data);
        assert_eq!(decrypt_bytes_with_keys(&k, &blob).unwrap(), data);
    }

    #[test]
    fn roundtrip_multi_frame() {
        let k = keys();
        let data: Vec<u8> = (0..(CHUNK + 1234)).map(|i| (i % 251) as u8).collect();
        let blob = encrypt_bytes_with_keys(&k, &data).unwrap();
        assert_eq!(decrypt_bytes_with_keys(&k, &blob).unwrap(), data);
    }

    #[test]
    fn tampered_cipher_fails() {
        let k = keys();
        let mut blob = encrypt_bytes_with_keys(&k, b"secret payload").unwrap();
        blob[HEADER_LEN + 4 + IV_LEN] ^= 0x01;
        assert!(decrypt_bytes_with_keys(&k, &blob).is_err());
    }

    #[test]
    fn wrong_key_fails() {
        let a = keys();
        let b = VaultKeys {
            enc_key: [1u8; 32],
            mac_key: [2u8; 32],
        };
        let blob = encrypt_bytes_with_keys(&a, b"secret payload").unwrap();
        assert!(decrypt_bytes_with_keys(&b, &blob).is_err());
    }

    #[test]
    fn bad_magic_fails() {
        let k = keys();
        let mut blob = encrypt_bytes_with_keys(&k, b"x").unwrap();
        blob[0] = b'X';
        assert!(decrypt_bytes_with_keys(&k, &blob).is_err());
    }

    #[test]
    fn oversized_frame_len_fails() {
        let k = keys();
        let mut blob = encrypt_bytes_with_keys(&k, b"secret payload").unwrap();
        blob[HEADER_LEN..HEADER_LEN + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decrypt_bytes_with_keys(&k, &blob).is_err());
    }
}
