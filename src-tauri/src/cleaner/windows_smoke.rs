//! Windows 实机冒烟测试（默认 `#[ignore]`）。
//!
//! 与 `macos_smoke.rs` 对称：单测只覆盖纯逻辑（过滤/去重/`UninstallString` 解析），
//! 而「列表 → 残留扫描 → 删进回收站 → 卸载 → 注册表键删除」这条链路要读真实注册表、
//! 起真实卸载进程、走系统回收站，只有真机才能验证。本文件把这条链路跑一遍，
//! 并且**所有写入都用测试唯一的名字**，结束（含断言失败）时清理。
//!
//! 为什么需要它：本文件对应的验收项原先只是 `TODO(platform-verify:windows)` 注释
//! （设计文档 9.2 的 Windows 清单第 1~3 条）。macOS 侧早就有 `macos_smoke.rs`，
//! Windows 侧一直没有对应物，所以「枚举是否和应用和功能一致」「MSI 条目会不会把
//! System32 当成程序文件」「残留能不能真的删进回收站」全靠人工看一眼。
//!
//! 手动执行（`CLEANER_SMOKE=1` 是保护：不给就不跑，避免误删）：
//! ```text
//! CLEANER_SMOKE=1 cargo test --manifest-path src-tauri/Cargo.toml --lib \
//!   -- --ignored --test-threads=1 cleaner::windows_smoke
//! ```
//!
//! 需要 UAC 才能验证的项（提权批量删除、HKLM 键删除、真实 MSI 卸载）**不在这里**：
//! 自动化测试不能替用户点「是」，那些仍按项目规则保留 `TODO(platform-verify:windows)`。

use std::path::{Path, PathBuf};

use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY};
use winreg::RegKey;

use super::models::{CleanerProgress, InstalledApp, LeftoverItem};
use super::{installer, trash_guard};

const UNINSTALL_BASE: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";

fn emit(_progress: CleanerProgress) {}

/// 冒烟开关：没设 `CLEANER_SMOKE=1` 就直接返回（`#[ignore]` 之外的第二道保险）。
fn enabled() -> bool {
    if std::env::var("CLEANER_SMOKE").ok().as_deref() == Some("1") {
        return true;
    }
    eprintln!("未设置 CLEANER_SMOKE=1，跳过 Windows 实机冒烟（这是有意的保护）");
    false
}

/// 先确认本进程真的能往用户目录写东西，否则后面每一条断言都会以「拒绝访问」
/// 失败，看起来像产品 bug，其实是被安全软件拦了。
///
/// 现场：火绒 / Defender 之类会把**没放行过的**新编译二进制挡住（交互式弹窗在
/// 非交互会话里没人点「允许」，于是直接拒绝）。这类失败与 cleaner 逻辑无关，
/// 所以这里直接给出可照做的处理办法，而不是留给后面某行 `unwrap` 报错。
fn require_writable_user_dir() {
    let base = PathBuf::from(std::env::var_os("LOCALAPPDATA").expect("LOCALAPPDATA"));
    let probe = base.join(format!("wsworkbench-smoke-write-probe-{}", unique()));
    match std::fs::create_dir_all(&probe) {
        Ok(()) => {
            let _ = std::fs::remove_dir_all(&probe);
        }
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => panic!(
            "写入 {} 被拒绝（{}）：多半是本机安全软件（火绒/Defender 等）拦了没放行过的测试二进制。\n\
             处理：在**交互式终端**里重跑本用例，弹窗时选「允许」；或把 \
             `src-tauri\\target\\debug\\deps\\ws_workbench_lib-*.exe` 加入信任。\n\
             这条限制与被测的 cleaner 代码无关。",
            probe.display(),
            error
        ),
        Err(error) => panic!("创建测试目录失败：{}（{}）", probe.display(), error),
    }
}

/// 本轮测试的唯一后缀：pid + 秒级时间戳 + 自增序号。
///
/// 加序号是因为 `cargo test` 默认并发跑用例，同一秒里启动的多个用例若只靠
/// pid + 时间戳会拿到同一个后缀，进而在 HKCU / AppData 里互相踩。
fn unique() -> String {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{}-{}-{}", std::process::id(), secs, seq)
}

fn hkcu() -> RegKey {
    RegKey::predef(HKEY_CURRENT_USER)
}

/// 在 HKCU 的 Uninstall 键下建一条记录，返回它的 app id。
///
/// 只在 HKCU 建：写 HKLM 需要管理员，而「能不能枚举到 per-user 安装的应用」正是
/// 验收清单第 1 条要抽查的那一类。
fn create_app(key_name: &str, values: &[(&str, Value)]) -> String {
    let path = format!(r"{}\{}", UNINSTALL_BASE, key_name);
    let (key, _) = hkcu()
        .create_subkey(&path)
        .unwrap_or_else(|err| panic!("创建 HKCU\\{} 失败: {}", path, err));
    for (name, value) in values {
        match value {
            Value::Text(text) => key
                .set_value(name, text)
                .unwrap_or_else(|err| panic!("写 {} 失败: {}", name, err)),
            Value::Dword(number) => key
                .set_value(name, number)
                .unwrap_or_else(|err| panic!("写 {} 失败: {}", name, err)),
        }
    }
    format!("win:HKCU|{}", path)
}

/// 注册表值：字符串或 DWORD（`EstimatedSize` / `SystemComponent` 现场两种都有）。
enum Value {
    Text(String),
    Dword(u32),
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Value::Text(value.to_string())
    }
}

impl From<u32> for Value {
    fn from(value: u32) -> Self {
        Value::Dword(value)
    }
}

/// 测试期间创建的东西：Drop 时（含断言失败）逐条清掉，避免污染本机。
struct Cleanup {
    registry_keys: Vec<String>,
    paths: Vec<PathBuf>,
    recycle_prefixes: Vec<String>,
}

impl Cleanup {
    fn new() -> Self {
        Self {
            registry_keys: Vec::new(),
            paths: Vec::new(),
            recycle_prefixes: Vec::new(),
        }
    }

    fn key(&mut self, path: impl Into<String>) {
        self.registry_keys.push(path.into());
    }

    fn path(&mut self, path: impl Into<PathBuf>) {
        self.paths.push(path.into());
    }

    fn recycle(&mut self, prefix: impl Into<String>) {
        self.recycle_prefixes.push(prefix.into());
    }
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        for path in &self.registry_keys {
            remove_registry_key(hkcu(), path);
        }
        for path in &self.paths {
            if path.is_dir() {
                let _ = std::fs::remove_dir_all(path);
            } else if path.exists() {
                let _ = std::fs::remove_file(path);
            }
        }
        for prefix in &self.recycle_prefixes {
            purge_recycle_bin_prefix(prefix);
        }
    }
}

/// 删一个 HKCU 下的键（含子键），失败只打日志：清理不该盖掉真正的断言错误。
fn remove_registry_key(root: RegKey, path: &str) {
    let Some((parent_path, name)) = path.rsplit_once('\\') else {
        return;
    };
    let Ok(parent) = root.open_subkey_with_flags(parent_path, winreg::enums::KEY_ALL_ACCESS) else {
        return;
    };
    if let Err(error) = parent.delete_subkey_all(name) {
        eprintln!("[smoke] 清理注册表键 {} 失败: {}", path, error);
    }
}

/// 尽力清掉本次测试写进回收站的条目（按唯一前缀匹配）。
///
/// 回收站内容属于系统 UI：枚举/删除失败都不算功能失败，只打日志 —— 与 macOS 侧
/// 「读不到 `~/.Trash` 就跳过」的处理一致。
fn purge_recycle_bin_prefix(prefix: &str) {
    let script = format!(
        "$ErrorActionPreference='SilentlyContinue'; \
         $bin = (New-Object -ComObject Shell.Application).Namespace(10); \
         foreach ($item in @($bin.Items())) {{ \
           if ($item.Name -like '{prefix}*') {{ Remove-Item -LiteralPath $item.Path -Recurse -Force }} \
         }}",
        prefix = prefix.replace('\'', "''")
    );
    let output = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output();
    match output {
        Ok(result) if !result.status.success() => eprintln!(
            "[smoke] 清空回收站条目失败（不影响功能结论）: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        ),
        Err(error) => eprintln!("[smoke] 起 PowerShell 清空回收站失败（不影响功能结论）: {}", error),
        _ => {}
    }
}

/// 从列表里按 app id 找我们造的那条
fn find_app<'a>(apps: &'a [InstalledApp], app_id: &str) -> &'a InstalledApp {
    apps.iter()
        .find(|app| app.id == app_id)
        .unwrap_or_else(|| panic!("列表里没有扫到 {}，共 {} 条", app_id, apps.len()))
}

fn find_leftover<'a>(items: &'a [LeftoverItem], path: &Path) -> &'a LeftoverItem {
    items
        .iter()
        .find(|item| Path::new(&item.path) == path)
        .unwrap_or_else(|| {
            panic!(
                "残留扫描漏了 {}\n实际扫到: {:?}",
                path.display(),
                items.iter().map(|item| item.path.clone()).collect::<Vec<_>>()
            )
        })
}

// ============================== 冒烟 1：列表与注册表一致 ==============================

/// 直接数一遍某个视图下「有 DisplayName」的条目，作为列表的对照。
fn raw_uninstall_count(root: &RegKey, view: u32) -> usize {
    let Ok(key) = root.open_subkey_with_flags(UNINSTALL_BASE, KEY_READ | view) else {
        return 0;
    };
    key.enum_keys()
        .flatten()
        .filter(|name| {
            let Ok(sub) = key.open_subkey_with_flags(name, KEY_READ | view) else {
                return false;
            };
            sub.get_value::<String, _>("DisplayName")
                .map(|value| !value.trim().is_empty())
                .unwrap_or(false)
        })
        .count()
}

/// 验收清单第 1 条：三处 Uninstall 键（HKLM 64 位 / Wow6432Node / HKCU）都要读到，
/// 32 位与 per-user 条目不能漏，名称/版本不乱码。
#[test]
#[ignore = "实机冒烟：读真实注册表并起 PowerShell 取图标，需 CLEANER_SMOKE=1"]
fn windows_list_matches_registry_views() {
    if !enabled() {
        return;
    }
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let raw = [
        ("HKLM", raw_uninstall_count(&hklm, KEY_WOW64_64KEY)),
        ("HKLM-WOW64", raw_uninstall_count(&hklm, KEY_WOW64_32KEY)),
        ("HKCU", raw_uninstall_count(&hkcu(), 0)),
    ];
    eprintln!(
        "[smoke] 注册表原始条目：HKLM={} Wow6432Node={} HKCU={}",
        raw[0].1, raw[1].1, raw[2].1
    );
    assert!(
        raw[0].1 > 0,
        "HKLM 64 位视图读不到任何卸载条目，注册表枚举通道没通"
    );

    let apps = installer::list_apps(&emit).expect("读取已安装列表失败");
    assert!(!apps.is_empty(), "列表不应该为空");
    let raw_total: usize = raw.iter().map(|(_, count)| count).sum();
    assert!(
        apps.len() <= raw_total,
        "过滤后的列表（{}）不该比注册表原始条目（{}）还多",
        apps.len(),
        raw_total
    );
    eprintln!("[smoke] 列表：{} 条（原始 {} 条）", apps.len(), raw_total);

    // 每条都要能回溯到合法键路径，且字段不能是乱码/空白
    for app in &apps {
        let key_path = app
            .id
            .strip_prefix("win:")
            .and_then(|rest| rest.split_once('|'))
            .map(|(hive, path)| (hive, path))
            .unwrap_or_else(|| panic!("app id 格式不对: {}", app.id));
        assert!(
            ["HKLM", "HKLM-WOW64", "HKCU"].contains(&key_path.0),
            "未知 hive: {}",
            app.id
        );
        assert!(
            crate::cleaner::registry_id::validate_registry_key_path(key_path.1).is_ok(),
            "键路径应落在 SOFTWARE 之下: {}",
            key_path.1
        );
        assert_eq!(app.name, app.name.trim(), "名称应已去空白: {:?}", app.name);
        assert!(!app.name.is_empty(), "名称不能为空: {}", app.id);
        assert!(
            app.name.chars().count() == app.name.chars().filter(|c| !c.is_control()).count(),
            "名称里不应有控制字符: {:?}",
            app.name
        );
        assert!(
            ["msi", "exe", "force"].contains(&app.uninstall_kind.as_str()),
            "未知的卸载类型 {}（{}）",
            app.uninstall_kind,
            app.name
        );
        if !app.install_date.is_empty() {
            assert!(
                app.install_date.len() == 10
                    && app.install_date.as_bytes()[4] == b'-'
                    && app.install_date.as_bytes()[7] == b'-',
                "安装日期格式应为 YYYY-MM-DD，实际 {}（{}）",
                app.install_date,
                app.name
            );
        }
        if !app.version.is_empty() {
            assert!(
                !app.version.contains(char::is_control),
                "版本号里不应有控制字符: {:?}",
                app.version
            );
        }
    }

    // 32 位视图：注册表里确实有 32 位条目时，列表里必须能看到 Wow6432Node 来源的条目。
    // （同名同版本会在去重时合并成 HKLM 那条，所以这里只做「至少见过一条」的检查。）
    if raw[1].1 > 0 {
        let wow64 = apps
            .iter()
            .filter(|app| app.id.starts_with("win:HKLM-WOW64|"))
            .count();
        assert!(
            wow64 > 0,
            "注册表 Wow6432Node 有 {} 条，但列表里一条 HKLM-WOW64 都没有：32 位视图没枚举到",
            raw[1].1
        );
        eprintln!("[smoke] 32 位视图条目：{} 条", wow64);
    } else {
        eprintln!("[smoke] 本机 Wow6432Node 没有卸载条目，跳过 32/64 视图覆盖检查");
    }

    // 去重：同名同版本只该留一条
    let mut seen = std::collections::HashSet::new();
    for app in &apps {
        let key = (app.name.to_lowercase(), app.version.to_lowercase());
        assert!(seen.insert(key), "同名同版本没有去重: {} {}", app.name, app.version);
    }
}

// ============================== 冒烟 2：MSI 条目不把系统目录当程序文件 ==============================

/// 验收清单第 2 条的前半段（`related.rs` 的 TODO）：`UninstallString` 是
/// `System32\msiexec.exe` 的 MSI 条目，卸载窗口的「程序文件」组里**不能**出现
/// `C:\Windows\System32` 或 `Program Files` 根目录。
#[test]
#[ignore = "实机冒烟：在 HKCU 建临时卸载项并跑真实关联扫描，需 CLEANER_SMOKE=1"]
fn windows_msi_entry_never_offers_system_dirs() {
    if !enabled() {
        return;
    }
    require_writable_user_dir();
    let uniq = unique();
    let mut cleanup = Cleanup::new();

    // (a) 只有 UninstallString 的 MSI 条目：没有任何安装位置信息
    let msi_key = format!("wsworkbench-smoke-msi-{}", uniq);
    let msi_id = create_app(
        &msi_key,
        &[
            (
                "DisplayName",
                format!("wsworkbench smoke msi {}", uniq).as_str().into(),
            ),
            (
                "UninstallString",
                r"MsiExec.exe /I{00000000-0000-0000-0000-000000000000}".into(),
            ),
            (
                "DisplayIcon",
                format!(r"{}\System32\shell32.dll,3", std::env::var("WINDIR").expect("WINDIR"))
                    .as_str()
                    .into(),
            ),
        ],
    );
    cleanup.key(format!(r"{}\{}", UNINSTALL_BASE, msi_key));

    // (b) 卸载程序在 System32 里的 exe 条目：位置缺失时才会走「卸载程序目录」兜底
    let exe_key = format!("wsworkbench-smoke-exe-{}", uniq);
    let exe_id = create_app(
        &exe_key,
        &[
            (
                "DisplayName",
                format!("wsworkbench smoke exe {}", uniq).as_str().into(),
            ),
            (
                "UninstallString",
                format!(
                    r#""{}\System32\cmd.exe" /c exit 0"#,
                    std::env::var("WINDIR").expect("WINDIR")
                )
                .as_str()
                .into(),
            ),
        ],
    );
    cleanup.key(format!(r"{}\{}", UNINSTALL_BASE, exe_key));

    let apps = installer::list_apps(&emit).expect("读取已安装列表失败");
    let msi_app = find_app(&apps, &msi_id);
    assert_eq!(msi_app.uninstall_kind, "msi", "MsiExec 条目应识别为 msi");
    let exe_app = find_app(&apps, &exe_id);
    assert_eq!(exe_app.uninstall_kind, "exe", "cmd.exe 条目应识别为 exe");

    let windir = std::env::var("WINDIR").expect("WINDIR");
    let forbidden = |path: &str| -> bool {
        let normalized = path.replace('/', "\\").to_lowercase();
        let windir = windir.replace('/', "\\").to_lowercase();
        normalized == windir
            || normalized.starts_with(&format!("{}\\", windir))
            || ["program files", "program files (x86)", "programdata"]
                .iter()
                .any(|root| normalized.trim_end_matches('\\') == *root)
    };

    for (app, label) in [(msi_app, "MSI"), (exe_app, "exe")] {
        let files = installer::related_files(&app.id, Some(app), &emit).expect("关联文件扫描失败");
        eprintln!(
            "[smoke] {} 关联文件：{} 组 / {} 项",
            label,
            files.groups.len(),
            files.item_count
        );
        for group in &files.groups {
            for item in &group.items {
                assert!(
                    !forbidden(&item.path),
                    "{} 条目的「{}」组把系统目录列成了程序文件: {}",
                    label,
                    group.label,
                    item.path
                );
                assert!(
                    !item.path.to_lowercase().contains(r"\windows\system32"),
                    "{} 条目的关联文件里出现了 System32: {}",
                    label,
                    item.path
                );
            }
        }
        // 程序文件组要么没有，要么只有应用自己的目录：不能出现 6GB 的系统目录
        if let Some(program) = files.groups.iter().find(|group| group.id == "program") {
            assert!(
                program.items.is_empty(),
                "{} 条目没有安装位置，程序文件组却列了 {:?}",
                label,
                program.items.iter().map(|item| item.path.clone()).collect::<Vec<_>>()
            );
        }
    }
}

// ============================== 冒烟 3：残留扫描 + 真删进回收站 ==============================

/// 验收清单第 3 条（`leftovers.rs` 的 TODO）：卸载后残留扫描能列出安装目录 /
/// AppData / 开始菜单与桌面快捷方式，并把勾选项真的删进回收站。
#[test]
#[ignore = "实机冒烟：会真实写入 LOCALAPPDATA/APPDATA/桌面并删除进回收站，需 CLEANER_SMOKE=1"]
fn windows_leftover_scan_and_recycle_smoke() {
    if !enabled() {
        return;
    }
    require_writable_user_dir();
    let uniq = unique();
    let mut cleanup = Cleanup::new();
    let name = format!("wsworkbench smoke {}", uniq);
    let publisher = "WsworkbenchSmoke";

    let local = PathBuf::from(std::env::var_os("LOCALAPPDATA").expect("LOCALAPPDATA"));
    let appdata = PathBuf::from(std::env::var_os("APPDATA").expect("APPDATA"));
    let home = super::home_dir().expect("HOME / USERPROFILE");

    // 安装目录（high + 默认勾选）+ 两类用户数据 + 两个快捷方式
    let install_dir = local.join(format!("wsworkbench-smoke-{}", uniq));
    let programs_dir = local.join("Programs").join(&name);
    let appdata_dir = appdata.join(&name);
    let start_menu_lnk = appdata
        .join(r"Microsoft\Windows\Start Menu\Programs")
        .join(format!("{}.lnk", name));
    let desktop_lnk = home.join("Desktop").join(format!("{}.lnk", name));
    for dir in [&install_dir, &programs_dir, &appdata_dir] {
        std::fs::create_dir_all(dir).expect("创建测试目录");
        std::fs::write(dir.join("payload.bin"), b"cleaner-smoke").expect("写测试文件");
        cleanup.path(dir);
    }
    for lnk in [&start_menu_lnk, &desktop_lnk] {
        if let Some(parent) = lnk.parent() {
            std::fs::create_dir_all(parent).expect("创建快捷方式目录");
        }
        // .lnk 内容不重要：扫描只看扩展名 + 文件名，删除走回收站
        std::fs::write(lnk, b"cleaner-smoke-shortcut").expect("写快捷方式");
        cleanup.path(lnk);
    }
    cleanup.recycle(format!("wsworkbench-smoke-{}", uniq));
    cleanup.recycle(name.clone());

    let key_name = format!("wsworkbench-smoke-leftover-{}", uniq);
    let app_id = create_app(
        &key_name,
        &[
            ("DisplayName", name.as_str().into()),
            ("DisplayVersion", "9.9.9".into()),
            ("Publisher", publisher.into()),
            ("InstallLocation", install_dir.to_string_lossy().as_ref().into()),
            ("EstimatedSize", 4096u32.into()),
            ("InstallDate", "20240105".into()),
            (
                "UninstallString",
                format!(
                    r#""{}\System32\cmd.exe" /c exit 0"#,
                    std::env::var("WINDIR").expect("WINDIR")
                )
                .as_str()
                .into(),
            ),
        ],
    );
    cleanup.key(format!(r"{}\{}", UNINSTALL_BASE, key_name));
    // 注册表残留：`SOFTWARE\<Publisher>\<Name>`（HKCU 普通权限即可删）
    let residual_key = format!(r"SOFTWARE\{}\{}", publisher, name);
    let (residual, _) = hkcu()
        .create_subkey(&residual_key)
        .expect("创建注册表残留键");
    residual
        .set_value("Smoke", &"cleaner-smoke")
        .expect("写残留值");
    drop(residual);
    cleanup.key(residual_key.clone());

    // 1) 列表：字段解析正确
    let apps = installer::list_apps(&emit).expect("读取已安装列表失败");
    let app = find_app(&apps, &app_id);
    assert_eq!(app.name, name);
    assert_eq!(app.version, "9.9.9");
    assert_eq!(app.publisher, publisher);
    assert_eq!(app.install_location, install_dir.to_string_lossy());
    assert_eq!(app.size_bytes, 4096 * 1024, "EstimatedSize 是 KB");
    assert_eq!(app.install_date, "2024-01-05", "InstallDate 应转成 YYYY-MM-DD");
    eprintln!("[smoke] 列表命中：{} / {} / {} B", app.name, app.version, app.size_bytes);

    // 2) 残留扫描：安装目录 / 用户数据 / 快捷方式 / 注册表残留都要命中
    let leftovers = installer::scan_leftovers(&app_id, Some(app), &emit).expect("残留扫描失败");
    let install_item = find_leftover(&leftovers, &install_dir);
    assert_eq!(install_item.kind, "install_dir");
    assert_eq!(install_item.confidence, "high");
    assert!(install_item.selected, "安装目录应默认勾选");
    assert!(!install_item.requires_elevation, "per-user 目录不需要提权");

    let programs_item = find_leftover(&leftovers, &programs_dir);
    assert_eq!(programs_item.kind, "install_dir");
    assert!(programs_item.confidence == "high" && programs_item.selected);

    let appdata_item = find_leftover(&leftovers, &appdata_dir);
    assert_eq!(appdata_item.kind, "app_data");
    assert_eq!(appdata_item.confidence, "medium");
    assert!(!appdata_item.selected, "用户数据默认不勾选");

    for lnk in [&start_menu_lnk, &desktop_lnk] {
        let item = find_leftover(&leftovers, lnk);
        assert_eq!(item.kind, "shortcut");
        assert_eq!(item.confidence, "high");
        assert!(item.selected, "快捷方式应默认勾选: {}", lnk.display());
    }

    let registry_item = leftovers
        .iter()
        .find(|item| item.path == format!("win:HKCU|{}", residual_key))
        .unwrap_or_else(|| {
            panic!(
                "残留扫描漏了注册表项 {}\n实际扫到: {:?}",
                residual_key,
                leftovers.iter().map(|item| item.path.clone()).collect::<Vec<_>>()
            )
        });
    assert_eq!(registry_item.kind, "registry");
    assert!(!registry_item.requires_elevation, "HKCU 键不需要提权");
    assert!(!registry_item.selected, "注册表残留默认不勾选");
    eprintln!("[smoke] 残留扫描：{} 条", leftovers.len());

    // 3) 删除：普通权限的文件进回收站、HKCU 键直接删
    let selected: Vec<String> = [
        &install_dir,
        &programs_dir,
        &appdata_dir,
        &start_menu_lnk,
        &desktop_lnk,
    ]
    .iter()
    .map(|path| path.to_string_lossy().to_string())
    .chain(std::iter::once(format!("win:HKCU|{}", residual_key)))
    .collect();
    let outcome =
        installer::delete_leftovers(&app_id, Some(app), &selected, &emit).expect("删除残留失败");
    assert!(
        outcome.failed.is_empty(),
        "删除失败：{:?}",
        outcome.failed
    );
    assert_eq!(outcome.trashed, selected.len(), "实际结果: {:?}", outcome);
    for path in [&install_dir, &programs_dir, &appdata_dir, &start_menu_lnk, &desktop_lnk] {
        assert!(!path.exists(), "{} 应已移出原位置", path.display());
    }
    assert!(
        hkcu().open_subkey(&residual_key).is_err(),
        "注册表残留键应已删除"
    );
    eprintln!("[smoke] 删除：{} 项（含回收站与注册表键）", outcome.trashed);
}

// ============================== 冒烟 4：卸载结果映射 ==============================

/// 验收清单第 2 条的后半段（`uninstall.rs` 的 TODO）：真实跑 exe 卸载器、
/// 正确解析退出码、没有卸载命令时走强制移除。
///
/// 真实 MSI 卸载（`/I` → `/X` 打真包）与 UAC 取消**不在这里**：前者要装一个真实
/// MSI 应用，后者要人点「否」，都不能由自动化代替。
#[test]
#[ignore = "实机冒烟：会真实起卸载进程并删 HKCU 项，需 CLEANER_SMOKE=1"]
fn windows_uninstall_outcomes_smoke() {
    if !enabled() {
        return;
    }
    require_writable_user_dir();
    let uniq = unique();
    let mut cleanup = Cleanup::new();
    let cmd_exe = format!(r"{}\System32\cmd.exe", std::env::var("WINDIR").expect("WINDIR"));

    // (a) 退出码 0：成功
    let ok_key = format!("wsworkbench-smoke-uninstall-ok-{}", uniq);
    let ok_id = create_app(
        &ok_key,
        &[
            ("DisplayName", format!("wsworkbench smoke ok {}", uniq).as_str().into()),
            ("UninstallString", format!(r#""{}" /c exit 0"#, cmd_exe).as_str().into()),
        ],
    );
    cleanup.key(format!(r"{}\{}", UNINSTALL_BASE, ok_key));
    let ok_outcome = installer::uninstall(&ok_id, None, false, &emit).expect("卸载调用失败");
    assert!(ok_outcome.ok, "退出码 0 应判定成功: {}", ok_outcome.message);
    assert_eq!(ok_outcome.exit_code, Some(0));
    assert!(!ok_outcome.elevated, "HKCU 条目不应提权");
    eprintln!("[smoke] 退出码 0：{}", ok_outcome.message);

    // (b) 非 0 退出码：失败但不误报成功
    let fail_key = format!("wsworkbench-smoke-uninstall-fail-{}", uniq);
    let fail_id = create_app(
        &fail_key,
        &[
            ("DisplayName", format!("wsworkbench smoke fail {}", uniq).as_str().into()),
            ("UninstallString", format!(r#""{}" /c exit 7"#, cmd_exe).as_str().into()),
        ],
    );
    cleanup.key(format!(r"{}\{}", UNINSTALL_BASE, fail_key));
    let fail_outcome = installer::uninstall(&fail_id, None, false, &emit).expect("卸载调用失败");
    assert!(!fail_outcome.ok, "退出码 7 不能判定成功");
    assert_eq!(fail_outcome.exit_code, Some(7));
    assert!(
        fail_outcome.message.contains("7"),
        "失败文案应带退出码: {}",
        fail_outcome.message
    );
    eprintln!("[smoke] 退出码 7：{}", fail_outcome.message);

    // (c) 没有卸载命令 → 强制移除注册表条目
    let force_key = format!("wsworkbench-smoke-uninstall-force-{}", uniq);
    let force_path = format!(r"{}\{}", UNINSTALL_BASE, force_key);
    let force_id = create_app(
        &force_key,
        &[("DisplayName", format!("wsworkbench smoke force {}", uniq).as_str().into())],
    );
    let force_outcome = installer::uninstall(&force_id, None, false, &emit).expect("卸载调用失败");
    assert!(force_outcome.ok, "强制移除应成功: {}", force_outcome.message);
    assert!(
        force_outcome.message.contains("移除注册表条目"),
        "文案应说明只移除了注册表条目: {}",
        force_outcome.message
    );
    assert!(
        hkcu().open_subkey(&force_path).is_err(),
        "强制移除后注册表条目应已删除"
    );

    // (d) 提权删除的白名单闸门在生成脚本之前拦住，**不会弹 UAC**
    let windir = PathBuf::from(std::env::var("WINDIR").expect("WINDIR"));
    let rejected = trash_guard::delete_elevated(
        &[windir.join("System32").join("notepad.exe")],
        &[("HKLM".to_string(), r"SYSTEM\CurrentControlSet\Control".to_string())],
    );
    assert_eq!(rejected.trashed, 0, "白名单外的目标不能删除");
    assert_eq!(rejected.failed.len(), 2, "文件与注册表键都应被拒绝: {:?}", rejected);
    for failure in &rejected.failed {
        assert!(
            failure.error.contains("白名单") || failure.error.contains("SOFTWARE"),
            "拒绝原因应说明白名单/范围: {}",
            failure.error
        );
    }
    eprintln!("[smoke] 提权删除的白名单拒绝：{} 项", rejected.failed.len());
}

// ============================== 冒烟 5：硬拒名单与注册表键删除 ==============================

/// `trash_guard/windows.rs` 的硬拒名单与注册表键删除（设计 4.4 / 4.5）。
///
/// HKLM 的键删除与 WOW6432Node 视图隔离需要管理员，不在这里验证。
#[test]
#[ignore = "实机冒烟：会建删 HKCU 临时键，需 CLEANER_SMOKE=1"]
fn windows_trash_guard_deny_list_and_key_delete() {
    if !enabled() {
        return;
    }
    require_writable_user_dir();
    let uniq = unique();
    let mut cleanup = Cleanup::new();
    let windir = PathBuf::from(std::env::var("WINDIR").expect("WINDIR"));
    let local = PathBuf::from(std::env::var_os("LOCALAPPDATA").expect("LOCALAPPDATA"));
    let home = super::home_dir().expect("HOME / USERPROFILE");

    // 1) 硬拒：系统目录、公共父目录本身、主目录、盘符根都不能删
    for denied in [
        windir.clone(),
        windir.join("System32"),
        windir.join("System32").join("drivers").join("etc").join("hosts"),
        PathBuf::from(r"C:\Program Files\Common Files"),
        PathBuf::from(r"C:\Program Files (x86)\Common Files"),
        home.clone(),
        PathBuf::from(r"C:\"),
    ] {
        assert!(
            !trash_guard::is_allowed(&denied),
            "{} 不该被允许删除",
            denied.display()
        );
    }
    // 2) 白名单内：本地 AppData 里的应用目录可以删
    let allowed = local.join(format!("wsworkbench-smoke-allowed-{}", uniq));
    std::fs::create_dir_all(&allowed).expect("创建测试目录");
    cleanup.path(&allowed);
    assert!(
        trash_guard::is_allowed(&allowed),
        "{} 应允许删除（在 LOCALAPPDATA 之下）",
        allowed.display()
    );

    // 3) 注册表键删除：HKCU 下的键要能连子键一起删掉，且不误删同名的兄弟键
    let base = format!(r"SOFTWARE\WsworkbenchSmoke\{}", uniq);
    let target = format!(r"{}\target", base);
    let sibling = format!(r"{}\keep", base);
    for path in [&target, &sibling] {
        let (key, _) = hkcu().create_subkey(path).expect("创建测试键");
        let (child, _) = key.create_subkey("child").expect("创建子键");
        child.set_value("v", &1u32).expect("写值");
    }
    cleanup.key(base.clone());
    trash_guard::delete_registry_key("HKCU", &target).expect("删除注册表键失败");
    assert!(
        hkcu().open_subkey(&target).is_err(),
        "目标键（含子键）应已删除"
    );
    assert!(
        hkcu().open_subkey(&sibling).is_ok(),
        "同级的兄弟键不能被连带删除"
    );

    // 4) 越界拒绝：SOFTWARE 根、SYSTEM 子树、未知 hive 都不能删。
    //
    // 这里**不能**用 HKLM\SOFTWARE\... 当反例：管理员身份下它会被真的删掉。
    for (hive, path) in [
        ("HKCU", "SOFTWARE"),
        ("HKCU", r"SYSTEM\CurrentControlSet\Services"),
        ("BADHIVE", r"SOFTWARE\WsworkbenchSmoke"),
    ] {
        let error = trash_guard::delete_registry_key(hive, path)
            .expect_err(&format!("{}|{} 应该被拒绝", hive, path));
        assert!(
            !error.is_empty(),
            "{}|{} 的拒绝原因不能是空串",
            hive,
            path
        );
    }
    eprintln!("[smoke] 硬拒名单 / 注册表键删除：通过");

    // 收尾：`SOFTWARE\WsworkbenchSmoke` 这个容器根键也是本用例造的，连它一起删掉，
    // 否则每跑一次就留下一个空壳。先删本轮的 `\uniq`（Drop 里也会再试一次），再用
    // 非递归的 `delete_subkey` 删根：万一真有别人的同名数据，它会失败而不是连坐。
    remove_registry_key(hkcu(), &base);
    if let Err(error) = hkcu().delete_subkey(r"SOFTWARE\WsworkbenchSmoke") {
        eprintln!("[smoke] 容器根键未删除（可能还有别的数据）: {}", error);
    }
}
