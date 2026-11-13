//! 「系统清理」后端模块：已安装应用列表 / 卸载 / 残留扫描与删除。
//!
//! 文件划分（设计 7）：
//! - `models`：对前端的数据契约；
//! - `installer`：列表、卸载、残留的统一入口与缓存；
//! - `installer_windows/` / `installer_macos/`：平台实现目录，`mod.rs` 只做门面，
//!   子文件按职责拆（纯解析 / 注册表或 bundle / 卸载 / 残留 / 关联文件）；
//! - `running/` / `trash_guard/`：运行状态与删除安全层，`mod.rs` 只放共享逻辑
//!   与 `cfg` 分派，平台实现分别在 `macos.rs` / `windows.rs`，两平台文件互不引用；
//! - `registry_id` / `paths`：平台无关的字符串与路径判定。
//!
//! 这里只放平台无关的公共小工具与 `CleanerState`，避免平台文件互相引用；
//! 平台代码分离规范见 README「平台代码分离」。

pub mod installer;
pub(crate) mod paths;
pub(crate) mod registry_id;
pub mod related;
pub mod running;
pub mod trash_guard;
// 「系统」工具页新增页签，每个 tab 一个子模块（设计 2026-11-07 第 3 节）
pub mod popup;
pub mod sleep;

// 纯逻辑（过滤/去重/UninstallString 解析）在 test cfg 下也会编进 macOS 测试，
// 此时 Windows 专用部分用不到，单独允许 dead_code 避免噪音警告
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub mod installer_windows;
#[cfg(target_os = "macos")]
pub mod installer_macos;

// 实机冒烟：默认 #[ignore]，只在人工联调时跑（会真实写 ~/Applications 与废纸篓）
#[cfg(all(test, target_os = "macos"))]
mod macos_smoke;
// 实机冒烟（Windows）：写 HKCU 临时卸载项 / LOCALAPPDATA 目录 / 回收站，跑完清理
#[cfg(all(test, target_os = "windows"))]
mod windows_smoke;

pub mod models;
pub use models::*;

use std::sync::Mutex;

/// 安装列表缓存 + 卸载窗口的进程内状态。
///
/// 枚举一次要 1~3 秒（Windows 要读注册表 + 算目录大小，macOS 要遍历 bundle），
/// 切页签不该重扫，所以缓存放在后端；`scanning` 用来拒绝重复触发的并发扫描
/// （前端连点两下「重新扫描」时第二次直接报「正在扫描」）。
#[derive(Default)]
pub struct CleanerState {
    pub apps: Mutex<Option<Vec<InstalledApp>>>,
    pub scanning: Mutex<bool>,
    /// 卸载窗口当前的目标应用（打开窗口时写入；复用窗口换目标时覆盖）
    pub pending_uninstall: Mutex<Option<InstalledApp>>,
    /// 最近一次关联文件扫描的白名单：(app_id, 本次扫描存在的路径)。
    ///
    /// `cleaner_delete_related` 只允许删这个集合里的路径（设计 4.4），
    /// 防止前端传任意路径进来。
    pub related_paths: Mutex<Option<(String, Vec<String>)>>,
}

impl CleanerState {
    pub fn new() -> Self {
        Self::default()
    }
}

// ============================== 公共小工具 ==============================

/// 用户主目录；取不到时返回 None（测试环境与极端受限环境要能退化）。
/// 应用数据目录（弹窗忽略/设置、启动项备份、休眠自动规则都写这里）。
///
/// 优先 `WS_WORKBENCH_DATA_DIR`（测试/调试用），否则按平台给默认位置，
/// 目录不存在会创建。
pub(crate) fn app_data_dir() -> Result<std::path::PathBuf, String> {
    let base = std::env::var_os("WS_WORKBENCH_DATA_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            let home = home_dir()?;
            if cfg!(target_os = "windows") {
                Some(home.join("AppData").join("Roaming").join("ws-workbench"))
            } else {
                Some(
                    home.join("Library")
                        .join("Application Support")
                        .join("ws-workbench"),
                )
            }
        })
        .ok_or_else(|| "取不到应用数据目录".to_string())?;
    std::fs::create_dir_all(&base).map_err(|err| format!("创建数据目录失败：{err}"))?;
    Ok(base)
}

pub(crate) fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
}

/// 目录大小（字节）。
///
/// 递归统计，但带三重上限，避免碰到巨型目录（或符号链接环）时把扫描卡死：
/// - 不跟随符号链接（bundle 里常有指向 /Applications 的软链）；
/// - 深度上限 `max_depth`；
/// - 条目数上限 `max_entries`，到顶就停并返回已统计的量。
pub(crate) fn dir_size_limited(root: &std::path::Path, max_depth: usize, max_entries: u64) -> u64 {
    let walker = walkdir::WalkDir::new(root)
        .follow_links(false)
        .max_depth(max_depth)
        .into_iter()
        .filter_map(|entry| entry.ok());

    let mut total: u64 = 0;
    let mut count: u64 = 0;
    for entry in walker {
        count += 1;
        if count > max_entries {
            break;
        }
        // 只累加普通文件；目录本身在 macOS 上的 st_size 是目录项大小，
        // 加进去会和「文件总和」口径不一致。
        match entry.metadata() {
            Ok(meta) if meta.is_file() => total = total.saturating_add(meta.len()),
            _ => {}
        }
    }
    total
}

/// 文件/目录的大小；文件取字节数，目录递归统计。
#[cfg(target_os = "macos")]
pub(crate) fn path_size(path: &std::path::Path) -> u64 {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() => meta.len(),
        Ok(meta) if meta.is_dir() => dir_size_limited(path, 64, 200_000),
        _ => 0,
    }
}

/// 系统时间转「YYYY-MM-DD」；拿不到就空串（前端会隐藏该列）。
#[cfg(target_os = "macos")]
pub(crate) fn format_date(time: std::time::SystemTime) -> String {
    let datetime: chrono::DateTime<chrono::Utc> = time.into();
    datetime.format("%Y-%m-%d").to_string()
}

/// 在多个 worker 上并行处理 `items`，返回顺序与输入一致。
///
/// 为什么自己写：设计 4.1 要求「最多 8 个 worker」并发取图标/算大小，
/// 而项目里没有 rayon 依赖，为这一处引一个并行库不值得；`std::thread::scope`
/// 足够（闭包里借用的 `emit` 回调在 scope 结束前一直有效）。
pub(crate) fn parallel_map<T, R, F>(items: &[T], workers: usize, f: F) -> Vec<R>
where
    T: Sync,
    R: Send,
    F: Fn(usize, &T) -> R + Sync,
{
    if items.is_empty() {
        return Vec::new();
    }
    let workers = workers.clamp(1, 8).min(items.len());
    let chunk = items.len().div_ceil(workers);
    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for (chunk_index, chunk_items) in items.chunks(chunk).enumerate() {
            let f = &f;
            let start = chunk_index * chunk;
            handles.push(scope.spawn(move || {
                chunk_items
                    .iter()
                    .enumerate()
                    .map(|(offset, item)| f(start + offset, item))
                    .collect::<Vec<R>>()
            }));
        }
        let mut out = Vec::with_capacity(items.len());
        for handle in handles {
            match handle.join() {
                Ok(chunk) => out.extend(chunk),
                // 单个 worker panic 不该让整个扫描失败：丢掉那一段即可，
                // 调用方本来就要对「某项算不出来」做兜底。
                Err(_) => {}
            }
        }
        out
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parallel_map_keeps_input_order() {
        let items: Vec<u32> = (0..20).collect();
        let out = parallel_map(&items, 8, |index, value| (index, *value));
        assert_eq!(out.len(), 20);
        for (i, (index, value)) in out.iter().enumerate() {
            assert_eq!(*index, i);
            assert_eq!(*value, i as u32);
        }
    }

    #[test]
    fn dir_size_counts_nested_files() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-tmp")
            .join("cleaner-dir-size");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a/b")).unwrap();
        std::fs::write(dir.join("a/one.bin"), b"12345").unwrap();
        std::fs::write(dir.join("a/b/two.bin"), b"1234567890").unwrap();
        assert_eq!(dir_size_limited(&dir, 64, 1000), 15);
        // 深度限制：根深度为 0，`a/one.bin` 在深度 2，所以 max_depth=2 只统计到它
        assert_eq!(dir_size_limited(&dir, 2, 1000), 5);
        // 连第一层都到不了时是 0（防御性上限生效，不会无限递归）
        assert_eq!(dir_size_limited(&dir, 1, 1000), 0);
    }

    // `format_date` 只在 macOS 编译（Windows 的日期来自注册表字符串），
    // 测试也只在 macOS 跑，避免 Windows 上整包测试编译失败。
    #[cfg(target_os = "macos")]
    #[test]
    fn formats_date_or_empty() {
        // 2001-09-09T01:46:40Z
        let time = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
        assert_eq!(format_date(time), "2001-09-09");
    }
}
