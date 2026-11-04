//! 文件系统通用操作，统一错误文本，避免各业务各写一套。

use std::path::Path;

/// 递归创建目录；已存在视为成功。
pub fn create_dir_all(path: &Path) -> Result<(), String> {
    std::fs::create_dir_all(path).map_err(|e| format!("创建目录失败（{}）: {}", path.display(), e))
}

/// 读取文本文件；文件不存在返回 `Ok(None)`，其他错误照常返回。
pub fn read_text_if_exists(path: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(content) => Ok(Some(content)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(format!("读取文件失败（{}）: {}", path.display(), err)),
    }
}

/// 写入文本文件，自动补建上级目录。
pub fn write_text(path: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            create_dir_all(parent)?;
        }
    }
    std::fs::write(path, content).map_err(|e| format!("写入文件失败（{}）: {}", path.display(), e))
}

/// 目录是否可写：真实创建再删除探针文件，避免只看权限位。
pub fn is_writable_dir(path: &Path) -> bool {
    if !path.is_dir() {
        return false;
    }
    let probe = path.join(".ws-workbench-write-test");
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}
