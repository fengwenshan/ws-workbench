//! 内存采样与「按应用聚合」主流程（平台无关部分）。
//!
//! 采样是只读操作：进程内维护一个 `sysinfo::System` 单例，每次采样只刷新
//! 内存与进程内存字段，不常驻后台线程（设计 5.3）。
//!
//! 进程 → 应用的身份判定由平台模块提供（macOS 找 `.app` 祖先，Windows 找产品
//! 安装目录）；本文件只负责聚合、排序、图标与结果模型转换，这样聚合逻辑可以
//! 用假数据单测，不必真的去采样系统。

use std::sync::Mutex;

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use super::models::{MemoryApp, MemorySnapshot, QuitOutcome, ReleaseOutcome};

/// 一条原始进程采样（平台无关，方便单测构造假数据）
#[derive(Debug, Clone)]
pub struct ProcSample {
    pub pid: u32,
    pub name: String,
    pub exe: String,
    pub memory: u64,
}

/// sysinfo 的 System 建一次要读不少 /proc（macOS 上是 sysctl），复用比重建快；
/// 内存页签每 2 秒采样一次，重建会明显增加开销。
static SYSTEM: Mutex<Option<System>> = Mutex::new(None);

/// 一次采样：总览数值 + 原始进程列表
struct Sampled {
    total: u64,
    used: u64,
    available: u64,
    swap_total: u64,
    swap_used: u64,
    samples: Vec<ProcSample>,
}

fn sample_processes() -> Result<Sampled, String> {
    let mut guard = SYSTEM.lock().map_err(|_| "内存采样初始化失败".to_string())?;
    if guard.is_none() {
        *guard = Some(System::new().map_err(|err| format!("内存采样初始化失败: {}", err))?);
    }
    let Some(system) = guard.as_mut() else {
        return Err("内存采样初始化失败".to_string());
    };

    system.refresh_memory();
    // exe 要 OnlyIfNotSet：进程的可执行文件路径不会变，但 `nothing()` 下
    // 默认是 Never，不显式打开的话所有进程都会因为 exe 为空被跳过
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_memory()
            .with_exe(UpdateKind::OnlyIfNotSet),
    );

    let mut samples: Vec<ProcSample> = Vec::new();
    for (pid, process) in system.processes() {
        let Some(exe) = process.exe() else {
            // 已删除的可执行文件 / 内核进程：没有 exe 就无法归属，跳过
            continue;
        };
        let exe = exe.to_string_lossy().to_string();
        if exe.is_empty() {
            continue;
        }
        let mut name = process.name().to_string_lossy().to_string();
        if name.is_empty() {
            name = std::path::Path::new(&exe)
                .file_stem()
                .map(|stem| stem.to_string_lossy().to_string())
                .unwrap_or_default();
        }
        samples.push(ProcSample {
            pid: pid.as_u32(),
            name,
            exe,
            memory: process.memory(),
        });
    }

    Ok(Sampled {
        total: system.total_memory(),
        used: system.used_memory(),
        available: system.available_memory(),
        swap_total: system.total_swap(),
        swap_used: system.used_swap(),
        samples,
    })
}

/// 把进程按应用身份聚合。
///
/// `identity` 由平台模块提供，测试里可以注入假函数；kind 用「id 是否等于 exe
/// 路径」判断：能找到应用容器（.app / 产品目录）时 id 是容器路径，找不到时
/// id 就是 exe 本身，对应 kind="process"。
pub fn group_processes_with<F>(samples: &[ProcSample], identity: F) -> Vec<MemoryApp>
where
    F: Fn(&ProcSample) -> (String, String),
{
    use std::collections::HashMap;

    // id → (展示名, 进程列表)；保持插入顺序不影响最终排序（按内存降序）
    let mut grouped: HashMap<String, (String, Vec<&ProcSample>)> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for sample in samples {
        let (id, name) = identity(sample);
        if id.is_empty() {
            continue;
        }
        match grouped.get_mut(&id) {
            Some((_, list)) => list.push(sample),
            None => {
                order.push(id.clone());
                grouped.insert(id, (name, vec![sample]));
            }
        }
    }

    let mut apps: Vec<MemoryApp> = order
        .into_iter()
        .filter_map(|id| {
            let (name, list) = grouped.remove(&id)?;
            let memory = list
                .iter()
                .fold(0u64, |sum, sample| sum.saturating_add(sample.memory));
            let kind = if list.first().map(|sample| sample.exe == id).unwrap_or(false) {
                "process"
            } else {
                "app"
            };
            let mut pids: Vec<u32> = list.iter().map(|sample| sample.pid).collect();
            pids.sort_unstable();
            Some(MemoryApp {
                id,
                name,
                exe: list.first().map(|sample| sample.exe.clone()).unwrap_or_default(),
                icon: None,
                process_count: pids.len(),
                memory,
                pids,
                kind: kind.to_string(),
            })
        })
        .collect();
    apps.sort_by(|left, right| right.memory.cmp(&left.memory));
    apps
}

/// 平台身份函数
#[cfg(target_os = "macos")]
fn platform_identity(sample: &ProcSample) -> (String, String) {
    super::memory_macos::identity(sample)
}

#[cfg(target_os = "windows")]
fn platform_identity(sample: &ProcSample) -> (String, String) {
    super::memory_windows::identity(sample)
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn platform_identity(sample: &ProcSample) -> (String, String) {
    (sample.exe.clone(), sample.name.clone())
}

/// 给聚合结果补图标（平台实现，取不到就是 None）
fn attach_icons(apps: &mut [MemoryApp]) {
    #[cfg(target_os = "macos")]
    {
        for app in apps {
            app.icon = super::memory_macos::icon_for(&app.id);
        }
    }
    #[cfg(target_os = "windows")]
    {
        let icons = super::memory_windows::icons_for(apps);
        for app in apps {
            app.icon = icons.get(&app.id).cloned().flatten();
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        for app in apps {
            app.icon = None;
        }
    }
}

/// 单次采样（设计 3.2 的 `cleaner_memory_snapshot`）
pub fn snapshot() -> Result<MemorySnapshot, String> {
    let sampled = sample_processes()?;
    let mut apps = group_processes_with(&sampled.samples, platform_identity);
    attach_icons(&mut apps);
    let used_percent = if sampled.total > 0 {
        (sampled.used as f64 / sampled.total as f64 * 100.0) as f32
    } else {
        0.0
    };
    Ok(MemorySnapshot {
        total: sampled.total,
        used: sampled.used,
        available: sampled.available,
        used_percent,
        swap_total: sampled.swap_total,
        swap_used: sampled.swap_used,
        apps,
        sampled_at: chrono::Utc::now().timestamp_millis(),
    })
}

/// 只聚合、不取图标（结束进程时用：图标对后台操作没意义，而 sips 很慢）
fn snapshot_apps() -> Result<Vec<MemoryApp>, String> {
    let sampled = sample_processes()?;
    Ok(group_processes_with(&sampled.samples, platform_identity))
}

/// 结束所选应用（设计 5.4 的 `cleaner_quit_apps`）。
///
/// `force=false` 走优雅退出（macOS terminate / Windows CloseMainWindow），
/// `force=true` 是前端的第二次确认后强制结束；前端负责「等 3 秒重新采样再问」。
pub fn quit_apps(app_ids: &[String], force: bool) -> Result<QuitOutcome, String> {
    let apps = snapshot_apps()?;
    let selected: Vec<MemoryApp> = apps
        .into_iter()
        .filter(|app| app_ids.iter().any(|id| id == &app.id))
        .collect();
    if selected.is_empty() {
        return Ok(QuitOutcome::default());
    }

    #[cfg(target_os = "macos")]
    {
        return Ok(super::memory_macos::quit_apps(&selected, force));
    }
    #[cfg(target_os = "windows")]
    {
        return Ok(super::memory_windows::quit_apps(&selected, force));
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (selected, force);
        Err("当前平台不支持结束进程".to_string())
    }
}

/// 深度释放的原理说明（直接展示在 UI，所以按平台分开写）
fn release_note() -> String {
    #[cfg(target_os = "macos")]
    {
        return "purge 只清可回收的文件缓存，不释放应用已 malloc 的内存，收益有限".to_string();
    }
    #[cfg(target_os = "windows")]
    {
        return "清空各进程工作集与系统待机列表，被清掉的页仍在磁盘/页面文件里，系统会按需换入".to_string();
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        "当前平台不支持深度释放".to_string()
    }
}

/// 深度释放（设计 5.5 的 `cleaner_deep_release`）。
///
/// 释放前后各采样一次（释放后等 1 秒再采，给系统回收的时间），freed 用差值
/// 而不是「宣称释放了多少」—— 系统内存本来就在波动，只报可观测量。
pub fn deep_release() -> Result<ReleaseOutcome, String> {
    let before = snapshot()?.used;
    let note = release_note();

    let platform_result = platform_deep_release();
    if let Err(message) = platform_result {
        return Ok(ReleaseOutcome {
            ok: false,
            before_used: before,
            after_used: before,
            freed: 0,
            message,
            note,
        });
    }

    std::thread::sleep(std::time::Duration::from_secs(1));
    let after = snapshot()?.used;
    let freed = before.saturating_sub(after);
    Ok(ReleaseOutcome {
        ok: true,
        before_used: before,
        after_used: after,
        freed,
        message: format!("已释放 {}", format_bytes(freed)),
        note,
    })
}

fn platform_deep_release() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        return super::memory_macos::deep_release();
    }
    #[cfg(target_os = "windows")]
    {
        return super::memory_windows::deep_release();
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Err("当前平台不支持深度释放".to_string())
    }
}

/// 字节数转人类可读（GB/MB/KB），UI 与提示文案共用
pub fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let value = bytes as f64;
    if value >= GB {
        format!("{:.2} GB", value / GB)
    } else if value >= MB {
        format!("{:.1} MB", value / MB)
    } else if value >= KB {
        format!("{:.0} KB", value / KB)
    } else {
        format!("{} B", bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(pid: u32, name: &str, exe: &str, memory: u64) -> ProcSample {
        ProcSample {
            pid,
            name: name.to_string(),
            exe: exe.to_string(),
            memory,
        }
    }

    #[test]
    fn groups_processes_by_app_identity() {
        let procs = vec![
            sample(
                1,
                "chrome",
                "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
                100,
            ),
            sample(
                2,
                "chrome",
                "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
                50,
            ),
            sample(3, "foo", "/opt/foo/bin/foo", 10),
        ];
        // 用假 identity 固定归属：前两个进程属于同一个应用，第三个独立
        let apps = group_processes_with(&procs, |sample| {
            if sample.exe.contains("Chrome.app") {
                (
                    "/Applications/Google Chrome.app".to_string(),
                    "Google Chrome".to_string(),
                )
            } else {
                (sample.exe.clone(), sample.name.clone())
            }
        });
        assert_eq!(apps.len(), 2);
        assert_eq!(apps[0].memory, 150);
        assert_eq!(apps[0].process_count, 2);
        assert_eq!(apps[0].pids, vec![1, 2]);
        assert_eq!(apps[0].kind, "app");
        // 独立 exe 成项是 process
        assert_eq!(apps[1].kind, "process");
        assert_eq!(apps[1].id, "/opt/foo/bin/foo");
    }

    #[test]
    fn empty_identity_is_skipped_and_zero_memory_sorted_last() {
        let procs = vec![
            sample(1, "tiny", "/a/tiny", 0),
            sample(2, "big", "/a/big", 4096),
            sample(3, "skip", "/a/skip", 8192),
        ];
        let apps = group_processes_with(&procs, |sample| {
            if sample.name == "skip" {
                (String::new(), String::new())
            } else {
                (sample.exe.clone(), sample.name.clone())
            }
        });
        assert_eq!(apps.len(), 2);
        assert_eq!(apps[0].name, "big");
        assert_eq!(apps[1].name, "tiny");
    }

    #[test]
    fn formats_bytes_for_ui() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1500), "1 KB");
        assert_eq!(format_bytes(2 * 1024 * 1024), "2.0 MB");
        assert_eq!(format_bytes(3 * 1024 * 1024 * 1024), "3.00 GB");
    }
}
