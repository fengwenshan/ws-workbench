//! Hosts 写入前备份。
//!
//! 备份目录：`<data_dir>/hosts-backups/`（`crate::cleaner::app_data_dir()`），
//! 文件名 `<unix_ms>_hosts`，最多保留 [`MAX_BACKUPS`] 份。
//!
//! 在线编辑不再维护环境配置（`hosts.json`），这里只负责备份的创建 / 列表 / 读取 / 清理。

use std::path::{Path, PathBuf};

use super::{format_ms, now_ms, HostsBackup, MAX_BACKUPS};

const BACKUP_DIR: &str = "hosts-backups";
const BACKUP_SUFFIX: &str = "_hosts";

// ============================== 路径 ==============================

fn data_dir() -> Result<PathBuf, String> {
    crate::cleaner::app_data_dir()
}

/// 备份目录（`hosts_backups` 列表 / 写前备份共用）。
pub fn backups_dir() -> Result<PathBuf, String> {
    Ok(data_dir()?.join(BACKUP_DIR))
}

// ============================== 备份 ==============================

/// 把即将被覆盖的内容备份到 `<data_dir>/hosts-backups/<unix_ms>_hosts`，返回新备份。
///
/// 同毫秒内多次写入时给毫秒数 +1 直到文件名不冲突，保证 id 唯一。
pub fn create_backup(content: &[u8]) -> Result<HostsBackup, String> {
    let dir = backups_dir()?;
    create_backup_in(&dir, content, now_ms())
}

fn create_backup_in(dir: &Path, content: &[u8], stamp: u64) -> Result<HostsBackup, String> {
    std::fs::create_dir_all(dir).map_err(|err| format!("创建备份目录失败：{err}"))?;
    let mut ms = stamp;
    let path = loop {
        let candidate = dir.join(format!("{ms}{BACKUP_SUFFIX}"));
        if !candidate.exists() {
            break candidate;
        }
        ms += 1;
    };
    std::fs::write(&path, content).map_err(|err| format!("备份 hosts 失败：{err}"))?;
    prune_backups_in(dir)?;
    Ok(HostsBackup {
        id: format!("{ms}{BACKUP_SUFFIX}"),
        created_at: format_ms(ms),
        size: content.len() as u64,
    })
}

/// 列备份（新→旧）。
pub fn list_backups() -> Result<Vec<HostsBackup>, String> {
    let dir = backups_dir()?;
    list_backups_in(&dir)
}

fn list_backups_in(dir: &Path) -> Result<Vec<HostsBackup>, String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(format!("读取备份目录失败：{err}")),
    };
    let mut backups: Vec<(u64, HostsBackup)> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(ms) = backup_stamp(&name) else {
            continue;
        };
        let size = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
        backups.push((
            ms,
            HostsBackup {
                id: name,
                created_at: format_ms(ms),
                size,
            },
        ));
    }
    backups.sort_by(|left, right| right.0.cmp(&left.0));
    Ok(backups.into_iter().map(|(_, backup)| backup).collect())
}

/// 读一份备份的原始字节（`hosts_restore` 用）。
pub fn read_backup(backup_id: &str) -> Result<Vec<u8>, String> {
    if backup_stamp(backup_id).is_none() {
        return Err("备份 id 不合法".to_string());
    }
    let path = backups_dir()?.join(backup_id);
    std::fs::read(&path).map_err(|err| format!("读取备份失败：{err}"))
}

/// 清理旧备份，只保留最新的 [`MAX_BACKUPS`] 份。
fn prune_backups_in(dir: &Path) -> Result<(), String> {
    let mut backups: Vec<(u64, PathBuf)> = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("读取备份目录失败：{err}")),
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if let Some(ms) = backup_stamp(&name) {
            backups.push((ms, entry.path()));
        }
    }
    if backups.len() <= MAX_BACKUPS {
        return Ok(());
    }
    backups.sort_by_key(|(ms, _)| *ms);
    let remove_count = backups.len() - MAX_BACKUPS;
    for (_, path) in backups.into_iter().take(remove_count) {
        let _ = std::fs::remove_file(path);
    }
    Ok(())
}

/// 备份文件名 → unix 毫秒；不符合 `<数字>_hosts` 的文件一律忽略（不是本应用写的）。
fn backup_stamp(name: &str) -> Option<u64> {
    let digits = name.strip_suffix(BACKUP_SUFFIX)?;
    if digits.is_empty() || !digits.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    digits.parse::<u64>().ok()
}

// ============================== 单测 ==============================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn creates_lists_and_reads_backups() {
        let dir = tempdir().expect("tempdir");
        let first = create_backup_in(dir.path(), b"127.0.0.1 localhost\n", 1_700_000_000_000)
            .expect("create first");
        assert_eq!(first.id, "1700000000000_hosts");
        assert_eq!(first.size, 20);

        let second = create_backup_in(dir.path(), b"::1 localhost\n", 1_700_000_000_000)
            .expect("create second");
        assert_eq!(second.id, "1700000000001_hosts", "同毫秒要自动 +1 去重");

        let listed = list_backups_in(dir.path()).expect("list");
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].id, second.id, "新→旧排序");
        assert_eq!(listed[1].id, first.id);

        let bytes = std::fs::read(dir.path().join(&first.id)).expect("read backup");
        assert_eq!(bytes, b"127.0.0.1 localhost\n");
    }

    #[test]
    fn rejects_bad_backup_ids() {
        for id in ["", "_hosts", "abc_hosts", "../hosts", "123_hosts/../x"] {
            assert!(backup_stamp(id).is_none(), "不该接受 {id:?}");
        }
        assert_eq!(backup_stamp("1700000000000_hosts"), Some(1_700_000_000_000));
    }

    #[test]
    fn prunes_to_max_backups() {
        let dir = tempdir().expect("tempdir");
        for index in 0..(MAX_BACKUPS + 5) {
            create_backup_in(
                dir.path(),
                b"127.0.0.1 localhost\n",
                1_700_000_000_000 + index as u64,
            )
            .expect("create");
        }
        let listed = list_backups_in(dir.path()).expect("list");
        assert_eq!(listed.len(), MAX_BACKUPS);
        assert_eq!(
            listed[0].id,
            format!("{}_hosts", 1_700_000_000_000 + (MAX_BACKUPS as u64 + 4)),
            "保留最新的一份"
        );
    }

    #[test]
    fn missing_dir_lists_empty() {
        let dir = tempdir().expect("tempdir");
        let missing = dir.path().join("nope");
        assert!(list_backups_in(&missing).expect("list").is_empty());
    }
}
