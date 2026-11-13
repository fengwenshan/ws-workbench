//! 数据存储位置：标准数据仓库约定 + 旧版本数据目录自动迁移。
//!
//! 约定：所有业务数据都放在 Tauri 的 `app_data_dir` 里
//! （macOS `~/Library/Application Support/<identifier>`，Windows
//! `%APPDATA%\<identifier>`），数据库、`attachments/`、`clipboard-attachments/`、
//! `vpn/` 都在这个目录下；安装包卸载程序按 `uninstall-info.txt` 找到同一位置。
//!
//! 旧版本可能把数据写在产品名目录（`.../ws-workbench`）或主目录点目录
//! （`~/.ws-workbench`），启动时自动迁移到标准目录。平台差异只在旧目录候选
//! 上，因此按平台拆文件（`macos.rs` / `windows.rs`），`mod.rs` 只做分派和
//! 平台无关的搬运逻辑。

use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod unsupported;

#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(target_os = "windows")]
use windows as platform;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
use unsupported as platform;

/// 判定「目录里已有用户数据」的标记：命中任意一个才迁移。
///
/// 只认业务数据，不认 `uninstall-info.txt`、日志等可再生成的文件。
pub(crate) const DATA_MARKERS: &[&str] = &[
    "config.json",
    "ws-workbench.db",
    "attachments",
    "clipboard-attachments",
    "vpn",
];

/// 启动时迁移旧数据目录；失败只打日志，不阻断启动。
pub fn migrate_on_startup(app: &AppHandle) -> Result<(), String> {
    let home = app
        .path()
        .home_dir()
        .map_err(|e| format!("读取用户主目录失败: {e}"))?;
    // 标准数据目录；app_data_dir 不可写时 `storage_dir` 会回退到别的候选，
    // 迁移目标跟着实际存储目录走，避免把数据搬到一个不能用位置。
    let target = crate::db::storage_dir(app)?;
    let legacy = platform::legacy_dirs(&home, &target);
    migrate_into(&target, &legacy)
}

/// 把 `legacy_dirs` 里的用户数据搬进 `target`，旧目录搬空后删除。
///
/// 规则：
/// - 只迁移 [`DATA_MARKERS`] 命中的条目；
/// - `target` 里已存在的同名条目不覆盖（保留新数据），旧目录里对应条目原样
///   留下，整个旧目录因此不删，只打日志；
/// - 迁移成功后删除旧目录；删除失败只记录，不影响启动；
/// - 幂等：搬完即删旧目录，后续启动不会重复搬。
pub(crate) fn migrate_into(target: &Path, legacy_dirs: &[PathBuf]) -> Result<(), String> {
    crate::util::fs::create_dir_all(target)?;
    for legacy in legacy_dirs {
        if legacy == target || !legacy.is_dir() {
            continue;
        }
        if !has_data(legacy) {
            continue;
        }
        let mut moved = 0usize;
        for marker in DATA_MARKERS {
            let source = legacy.join(marker);
            let dest = target.join(marker);
            if !source.exists() || dest.exists() {
                continue;
            }
            move_path(&source, &dest)?;
            moved += 1;
        }
        if moved == 0 {
            eprintln!(
                "[storage] 旧数据目录未迁移（标准目录已有同名数据）: {}",
                legacy.display()
            );
            continue;
        }
        // 旧目录里还有冲突条目（目标已有同名数据）就保留，避免误删新旧任意一份；
        // 数据标记都搬空了才删除旧目录。
        let leftover = DATA_MARKERS
            .iter()
            .any(|marker| legacy.join(marker).exists());
        if leftover {
            eprintln!(
                "[storage] 旧数据目录保留（标准目录已有同名数据）: {}",
                legacy.display()
            );
            continue;
        }
        match std::fs::remove_dir_all(legacy) {
            Ok(()) => eprintln!(
                "[storage] 已迁移旧数据目录: {} -> {}",
                legacy.display(),
                target.display()
            ),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => eprintln!(
                "[storage] 旧数据目录未完全清理（{}）: {}",
                legacy.display(),
                err
            ),
        }
    }
    Ok(())
}

fn has_data(dir: &Path) -> bool {
    DATA_MARKERS.iter().any(|marker| dir.join(marker).exists())
}

/// 移动文件或目录；跨盘 `rename` 会失败，退化为「复制到目标 → 删源」。
fn move_path(source: &Path, dest: &Path) -> Result<(), String> {
    match std::fs::rename(source, dest) {
        Ok(()) => Ok(()),
        Err(rename_err) => {
            if let Err(copy_err) = copy_path(source, dest) {
                // 复制到一半失败：清掉目标里的半成品，下次启动还能重试。
                let _ = remove_path(dest);
                return Err(format!(
                    "复制旧数据失败（{} → {}）: {}（重命名错误: {}）",
                    source.display(),
                    dest.display(),
                    copy_err,
                    rename_err
                ));
            }
            if let Err(remove_err) = remove_path(source) {
                return Err(format!(
                    "复制成功但删除旧数据失败（{} → {}）: {}（重命名错误: {}）",
                    source.display(),
                    dest.display(),
                    remove_err,
                    rename_err
                ));
            }
            Ok(())
        }
    }
}

/// 递归复制（保留文件内容；权限不保证复制，数据目录按当前用户读写即可）。
fn copy_path(source: &Path, dest: &Path) -> Result<(), String> {
    if source.is_dir() {
        crate::util::fs::create_dir_all(dest)?;
        let entries = std::fs::read_dir(source)
            .map_err(|err| format!("读取目录失败（{}）: {}", source.display(), err))?;
        for entry in entries {
            let entry = entry.map_err(|err| format!("读取目录项失败（{}）: {}", source.display(), err))?;
            copy_path(&entry.path(), &dest.join(entry.file_name()))?;
        }
        return Ok(());
    }
    if let Some(parent) = dest.parent() {
        crate::util::fs::create_dir_all(parent)?;
    }
    std::fs::copy(source, dest)
        .map(|_| ())
        .map_err(|err| format!("复制失败（{} → {}）: {}", source.display(), dest.display(), err))
}

fn remove_path(path: &Path) -> Result<(), String> {
    if path.is_dir() {
        std::fs::remove_dir_all(path).map_err(|err| format!("删除目录失败（{}）: {}", path.display(), err))
    } else {
        std::fs::remove_file(path).map_err(|err| format!("删除文件失败（{}）: {}", path.display(), err))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_file(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn migrates_legacy_data_into_empty_target() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("app-data");
        let legacy = root.path().join("legacy");
        write_file(&legacy.join("config.json"), "{}");
        write_file(&legacy.join("ws-workbench.db"), "db");
        write_file(&legacy.join("attachments/a.wst"), "wst");
        write_file(&legacy.join("clipboard-attachments/b.wst"), "wst");
        write_file(&legacy.join("uninstall-info.txt"), "version=1");

        migrate_into(&target, &[legacy.clone()]).unwrap();

        assert_eq!(std::fs::read_to_string(target.join("config.json")).unwrap(), "{}");
        assert_eq!(std::fs::read_to_string(target.join("ws-workbench.db")).unwrap(), "db");
        assert_eq!(std::fs::read_to_string(target.join("attachments/a.wst")).unwrap(), "wst");
        assert_eq!(
            std::fs::read_to_string(target.join("clipboard-attachments/b.wst")).unwrap(),
            "wst"
        );
        // 非数据文件不迁移；旧目录搬空后删除
        assert!(!target.join("uninstall-info.txt").exists());
        assert!(!legacy.exists(), "旧目录应在迁移成功后删除");
    }

    #[test]
    fn keeps_newer_target_and_leftover_legacy_on_conflict() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("app-data");
        let legacy = root.path().join("legacy");
        write_file(&target.join("ws-workbench.db"), "new");
        write_file(&legacy.join("ws-workbench.db"), "old");
        write_file(&legacy.join("config.json"), "{}");

        migrate_into(&target, &[legacy.clone()]).unwrap();

        // 同名数据库不覆盖，缺的配置照常迁移
        assert_eq!(std::fs::read_to_string(target.join("ws-workbench.db")).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(target.join("config.json")).unwrap(), "{}");
        // 有冲突条目就保留旧目录，避免误删新旧任意一份
        assert!(legacy.join("ws-workbench.db").exists());
        assert!(!legacy.join("config.json").exists());
    }

    #[test]
    fn skips_dirs_without_user_data() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("app-data");
        let legacy = root.path().join("legacy");
        write_file(&legacy.join("uninstall-info.txt"), "version=1");
        write_file(&legacy.join("notes.txt"), "not our data");

        migrate_into(&target, &[legacy.clone()]).unwrap();

        assert!(legacy.exists(), "没有业务数据的旧目录不应被迁移或删除");
        assert!(!target.join("uninstall-info.txt").exists());
    }
}
