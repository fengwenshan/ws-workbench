//! Windows 启动项：注册表 `Run` / `RunOnce`、用户/公共启动文件夹与 `StartupApproved` 状态位。
//!
//! 设计 4.2。分两层：
//!
//! - **纯逻辑**（`StartupApproved` 12 字节编解码、FILETIME 反解、过滤/去重/只读判定、
//!   Run 值命令行解析）不依赖 winreg，macOS 上 `cargo test` 也能覆盖；
//! - **注册表 / 文件系统实现**在文件末尾的 `#[cfg(target_os = "windows")] mod win_impl`。
//!
//! 安全约束：
//! - 禁用只写 `Explorer\StartupApproved` 的状态位，**不删除 Run 值**；
//! - 写状态位前把「原状态位 + 原 Run 值」追加备份到 `<data_dir>/startup-backup.json`；
//! - HKLM（含 WOW6432Node 视图）/ 公共启动文件夹 / `RunOnce` 一律只读；
//! - 本应用自启写 / 删 `HKCU\...\Run\ws-workbench`。

use std::collections::{HashMap, HashSet};
use std::path::Path;

use super::StartupItem;

/// `Explorer\StartupApproved\{Run,Run32,StartupFolder}` 的状态位：值为名 → 原始 12 字节
pub type ApprovedState = HashMap<String, Vec<u8>>;

/// `StartupApproved` 值的固定长度（设计 4.2）
pub const STARTUP_APPROVED_LEN: usize = 12;
/// 1601-01-01 到 1970-01-01 的秒数（FILETIME 纪元偏移）
const FILETIME_EPOCH_OFFSET_SECS: i64 = 11_644_473_600;
/// FILETIME 每秒的 tick 数
const FILETIME_TICKS_PER_SEC: u64 = 10_000_000;

// ============================== 枚举与来源 ==============================

/// 注册表根 / 视图
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WinHive {
    /// HKCU（用户级，可切换）
    Hkcu,
    /// HKLM 的 64 位视图（系统级，只读）
    Hklm,
    /// HKLM 的 32 位视图（`WOW6432Node`，系统级，只读）
    Hklm32,
}

impl WinHive {
    pub fn token(self) -> &'static str {
        match self {
            WinHive::Hkcu => "hkcu",
            WinHive::Hklm => "hklm",
            WinHive::Hklm32 => "hklm32",
        }
    }
}

/// 启动项来源类别
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WinSection {
    /// `...\CurrentVersion\Run`
    Run,
    /// `...\CurrentVersion\RunOnce`（只读）
    RunOnce,
    /// 用户 / 公共启动文件夹
    StartupFolder,
}

impl WinSection {
    pub fn token(self) -> &'static str {
        match self {
            WinSection::Run => "run",
            WinSection::RunOnce => "run_once",
            WinSection::StartupFolder => "startup_folder",
        }
    }

    /// 对前端暴露的 source
    pub fn source(self) -> &'static str {
        self.token()
    }

    /// 注册表里的字样（`StartupApproved` 的子键名也用它）
    pub fn registry_name(self) -> &'static str {
        match self {
            WinSection::Run => "Run",
            WinSection::RunOnce => "RunOnce",
            WinSection::StartupFolder => "StartupFolder",
        }
    }
}

/// `StartupApproved` 所在的注册表根
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ApprovedHive {
    Hkcu,
    Hklm,
}

impl ApprovedHive {
    pub fn token(self) -> &'static str {
        match self {
            ApprovedHive::Hkcu => "hkcu",
            ApprovedHive::Hklm => "hklm",
        }
    }
}

/// `StartupApproved` 的子键：Run / Run32 / StartupFolder
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ApprovedSection {
    Run,
    Run32,
    StartupFolder,
}

impl ApprovedSection {
    pub fn token(self) -> &'static str {
        match self {
            ApprovedSection::Run => "run",
            ApprovedSection::Run32 => "run32",
            ApprovedSection::StartupFolder => "startup_folder",
        }
    }

    pub fn registry_name(self) -> &'static str {
        match self {
            ApprovedSection::Run => "Run",
            ApprovedSection::Run32 => "Run32",
            ApprovedSection::StartupFolder => "StartupFolder",
        }
    }
}

/// scope：HKCU / 用户启动文件夹为 user，HKLM 为 machine
pub fn scope_of(hive: WinHive) -> &'static str {
    match hive {
        WinHive::Hkcu => "user",
        WinHive::Hklm | WinHive::Hklm32 => "machine",
    }
}

/// 一条启动项对应的 `StartupApproved` 位置。
///
/// `RunOnce` 没有状态位（系统执行一次后自行删除），返回 None。
pub fn approved_target(
    hive: WinHive,
    section: WinSection,
) -> Option<(ApprovedHive, ApprovedSection)> {
    match (hive, section) {
        // HKCU Run → HKCU\...\StartupApproved\Run
        (WinHive::Hkcu, WinSection::Run) => Some((ApprovedHive::Hkcu, ApprovedSection::Run)),
        // HKLM 64 位 Run → HKLM\...\StartupApproved\Run
        (WinHive::Hklm, WinSection::Run) => Some((ApprovedHive::Hklm, ApprovedSection::Run)),
        // HKLM 32 位 Run（WOW6432Node）→ HKLM\...\StartupApproved\Run32
        (WinHive::Hklm32, WinSection::Run) => Some((ApprovedHive::Hklm, ApprovedSection::Run32)),
        // 用户启动文件夹 → HKCU\...\StartupApproved\StartupFolder
        (WinHive::Hkcu, WinSection::StartupFolder) => {
            Some((ApprovedHive::Hkcu, ApprovedSection::StartupFolder))
        }
        // 公共启动文件夹 → HKLM\...\StartupApproved\StartupFolder
        (WinHive::Hklm, WinSection::StartupFolder) => {
            Some((ApprovedHive::Hklm, ApprovedSection::StartupFolder))
        }
        _ => None,
    }
}

/// `StartupApproved` 映射键（纯字符串，方便跨平台单测与序列化）
pub fn approved_key(hive: ApprovedHive, section: ApprovedSection, name: &str) -> String {
    format!("{}|{}|{}", hive.token(), section.token(), name)
}

// ============================== StartupApproved 编解码 ==============================

/// 一条 `StartupApproved` 状态位解出来的结果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupApproved {
    /// 启用（含「值缺失 / 全 0 / 非法短值」等缺省情况）
    Enabled,
    /// 禁用；`filetime` 为禁用时写入的 FILETIME（老系统可能没写）
    Disabled { filetime: Option<u64> },
}

impl StartupApproved {
    pub fn is_enabled(self) -> bool {
        matches!(self, StartupApproved::Enabled)
    }

    pub fn disabled_filetime(self) -> Option<u64> {
        match self {
            StartupApproved::Disabled { filetime } => filetime,
            StartupApproved::Enabled => None,
        }
    }

    /// 禁用时间（unix 秒）；没有有效 FILETIME 时为 None
    pub fn disabled_at_unix(self) -> Option<i64> {
        self.disabled_filetime().and_then(filetime_to_unix_seconds)
    }
}

/// 解 12 字节 `StartupApproved` 值（设计 4.2）：
///
/// - `02 00 00 00` + 8 字节 0 → 启用；
/// - `03 00 00 00` + 8 字节 FILETIME → 禁用；
/// - 值缺失 / 长度不足 12 / 全 0 → **启用**。
///
/// 容错策略刻意偏向「启用」：把启用项误标成禁用会让用户以为自启被关了，
/// 而把禁用项误标成启用只是显示不准，不会造成任何写入。
pub fn decode_startup_approved(raw: &[u8]) -> StartupApproved {
    if raw.len() < STARTUP_APPROVED_LEN {
        return StartupApproved::Enabled;
    }
    match raw[0] {
        // 0x00：老系统 / 手写值；0x02：Task Manager 写入的「启用」
        0x00 | 0x02 => StartupApproved::Enabled,
        // 0x03：Task Manager 的「禁用」；0x04 / 0x06 是策略/组策略写过的变体，
        // 同样按禁用处理（都比「误判为启用」更接近真实意图）
        _ => {
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(&raw[4..STARTUP_APPROVED_LEN]);
            let filetime = u64::from_le_bytes(bytes);
            StartupApproved::Disabled {
                filetime: normalize_filetime(filetime),
            }
        }
    }
}

/// 生成 12 字节 `StartupApproved` 值：启用写 `02`，禁用写 `03` + FILETIME。
///
/// `now_filetime` 由调用方传入（纯函数，单测可给固定值）。禁用时若为 0，
/// 后 8 字节保持全 0 —— Windows 只认首字节，能正常识别为禁用。
pub fn encode_startup_approved(enabled: bool, now_filetime: u64) -> [u8; STARTUP_APPROVED_LEN] {
    let mut out = [0u8; STARTUP_APPROVED_LEN];
    out[0] = if enabled { 0x02 } else { 0x03 };
    if !enabled {
        out[4..].copy_from_slice(&now_filetime.to_le_bytes());
    }
    out
}

/// FILETIME 的 0 / 全 1 是「没有时间」的哨兵值
fn normalize_filetime(filetime: u64) -> Option<u64> {
    if filetime == 0 || filetime == u64::MAX {
        None
    } else {
        Some(filetime)
    }
}

/// unix 秒 → FILETIME（100ns tick，1601 纪元）；负值按 0 处理
pub fn unix_seconds_to_filetime(seconds: i64) -> u64 {
    if seconds < 0 {
        return 0;
    }
    (seconds as u64)
        .saturating_add(FILETIME_EPOCH_OFFSET_SECS as u64)
        .saturating_mul(FILETIME_TICKS_PER_SEC)
}

/// FILETIME → unix 秒；0 / 全 1 返回 None
pub fn filetime_to_unix_seconds(filetime: u64) -> Option<i64> {
    if filetime == 0 || filetime == u64::MAX {
        return None;
    }
    let seconds = (filetime / FILETIME_TICKS_PER_SEC) as i64;
    Some(seconds.saturating_sub(FILETIME_EPOCH_OFFSET_SECS))
}

// ============================== 一条候选启动项 ==============================

/// 枚举阶段的一条启动项（还没和 `StartupApproved` 状态合并）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WinStartupEntry {
    pub hive: WinHive,
    pub section: WinSection,
    /// 展示名（注册表值名 / 启动文件夹文件名去掉扩展名）
    pub name: String,
    /// 注册表值名 / 启动文件夹里的完整文件名：写状态位和生成 id 都用它
    pub key_name: String,
    pub command: String,
    /// 对应的 `StartupApproved` 映射键；`RunOnce` 为 None
    pub approved_key: Option<String>,
    /// 来源位置说明（只读原因会追加在后面）
    pub detail: String,
}

impl WinStartupEntry {
    /// 注册表 `Run` / `RunOnce` 条目
    pub fn registry(hive: WinHive, section: WinSection, value_name: &str, command: &str) -> Self {
        Self {
            hive,
            section,
            name: value_name.to_string(),
            key_name: value_name.to_string(),
            command: command.to_string(),
            approved_key: approved_target(hive, section)
                .map(|(hive, section)| approved_key(hive, section, value_name)),
            detail: registry_detail(hive, section),
        }
    }

    /// 启动文件夹条目（`file_name` 带 `.lnk` / `.exe` 后缀）
    pub fn startup_folder(
        hive: WinHive,
        dir: &Path,
        file_name: &str,
        command: String,
    ) -> Self {
        let name = Path::new(file_name)
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_string())
            .filter(|stem| !stem.is_empty())
            .unwrap_or_else(|| file_name.to_string());
        Self {
            hive,
            section: WinSection::StartupFolder,
            name,
            key_name: file_name.to_string(),
            command,
            approved_key: approved_target(hive, WinSection::StartupFolder)
                .map(|(hive, section)| approved_key(hive, section, file_name)),
            detail: format!("启动文件夹：{}", dir.display()),
        }
    }

    /// 前端回传的唯一标识：`win:<section>:<hive>:<key_name>`
    pub fn id(&self) -> String {
        format!(
            "win:{}:{}:{}",
            self.section.token(),
            self.hive.token(),
            self.key_name
        )
    }
}

/// 注册表位置说明（UI 的 title / detail 用）
pub fn registry_detail(hive: WinHive, section: WinSection) -> String {
    let base = match hive {
        WinHive::Hkcu => r"HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion",
        WinHive::Hklm => r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion",
        WinHive::Hklm32 => r"HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion",
    };
    format!(r"{}\{}", base, section.registry_name())
}

// ============================== id 解析 ==============================

/// `win:<section>:<hive>:<key_name>` 解析结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedItem {
    pub hive: WinHive,
    pub section: WinSection,
    /// 注册表值名 / 启动文件夹文件名（可含 `:`，所以只在前三段切分）
    pub key_name: String,
}

/// 解析 id；格式不对返回 None（调用方给中文错误）
pub fn parse_item_id(id: &str) -> Option<ParsedItem> {
    let rest = id.strip_prefix("win:")?;
    let mut parts = rest.splitn(3, ':');
    let section = match parts.next()? {
        "run" => WinSection::Run,
        "run_once" => WinSection::RunOnce,
        "startup_folder" => WinSection::StartupFolder,
        _ => return None,
    };
    let hive = match parts.next()? {
        "hkcu" => WinHive::Hkcu,
        "hklm" => WinHive::Hklm,
        "hklm32" => WinHive::Hklm32,
        _ => return None,
    };
    let key_name = parts.next()?;
    if key_name.is_empty() {
        return None;
    }
    Some(ParsedItem {
        hive,
        section,
        key_name: key_name.to_string(),
    })
}

// ============================== 只读判定 / 过滤 / 合并 ==============================

/// 只读原因；None 表示该条目允许启用 / 禁用。
///
/// 规则（设计 4.2）：HKLM（两个视图）、公共启动文件夹、`RunOnce` 全部只读；
/// 只有 HKCU `Run` 与用户启动文件夹允许切换。
pub fn readonly_reason(hive: WinHive, section: WinSection) -> Option<&'static str> {
    match (section, hive) {
        (WinSection::RunOnce, _) => Some("RunOnce 条目在下次登录执行一次后由系统删除，本版本只读展示"),
        (WinSection::StartupFolder, WinHive::Hkcu) => None,
        (WinSection::StartupFolder, _) => {
            Some("公共启动文件夹需要管理员权限，本版本只读展示")
        }
        (WinSection::Run, WinHive::Hkcu) => None,
        (WinSection::Run, _) => {
            Some("系统级注册表启动项（HKLM）需要管理员权限，本版本只读展示")
        }
    }
}

pub fn can_toggle(hive: WinHive, section: WinSection) -> bool {
    readonly_reason(hive, section).is_none()
}

/// 是否展示这一条：名字和启动命令都非空才展示
/// （空值多半是卸载残留 / 安装器写坏的键，展示出来只会让人困惑）
pub fn should_show(entry: &WinStartupEntry) -> bool {
    !entry.name.trim().is_empty()
        && !entry.key_name.trim().is_empty()
        && !entry.command.trim().is_empty()
}

/// 同一 id 只留第一条（Run/RunOnce 枚举可能因为视图重叠读到重复项）
pub fn dedupe(entries: Vec<WinStartupEntry>) -> Vec<WinStartupEntry> {
    let mut seen: HashSet<String> = HashSet::new();
    entries
        .into_iter()
        .filter(|entry| seen.insert(entry.id()))
        .collect()
}

/// 合并「枚举结果 + StartupApproved 状态」生成前端契约，按 scope / 名称排序。
///
/// 状态位缺省视为启用（设计 4.2）；只读条目的原因追加到 detail 里。
pub fn build_items(entries: Vec<WinStartupEntry>, approved: &ApprovedState) -> Vec<StartupItem> {
    let mut items: Vec<StartupItem> = Vec::new();
    for entry in dedupe(entries.into_iter().filter(should_show).collect()) {
        let readonly = readonly_reason(entry.hive, entry.section);
        let approved_state = entry
            .approved_key
            .as_ref()
            .and_then(|key| approved.get(key))
            .map(|raw| decode_startup_approved(raw));
        // 没有状态位（RunOnce / 从未被 Task Manager 写过）→ 启用
        let enabled = approved_state.map(|state| state.is_enabled()).unwrap_or(true);
        let mut detail = match readonly {
            Some(reason) => format!("{}；{}", entry.detail, reason),
            None => entry.detail.clone(),
        };
        // 禁用时把时间也展示出来：用户能对照「我是什么时候关掉的」
        if let Some(stamp) = approved_state
            .and_then(|state| state.disabled_at_unix())
            .and_then(format_unix_local)
        {
            detail = format!("{}；禁用时间：{}", detail, stamp);
        }
        items.push(StartupItem {
            id: entry.id(),
            name: entry.name.clone(),
            command: entry.command.clone(),
            source: entry.section.source().to_string(),
            scope: scope_of(entry.hive).to_string(),
            enabled,
            can_toggle: readonly.is_none(),
            detail,
        });
    }
    items.sort_by(|left, right| {
        scope_rank(&left.scope)
            .cmp(&scope_rank(&right.scope))
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            .then_with(|| left.id.cmp(&right.id))
    });
    items
}

/// unix 秒 → 本地时间字符串（Windows 状态位里的 FILETIME 反解后用于 detail）
fn format_unix_local(unix_seconds: i64) -> Option<String> {
    let datetime = chrono::DateTime::from_timestamp(unix_seconds, 0)?;
    Some(
        datetime
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
            .to_string(),
    )
}

fn scope_rank(scope: &str) -> u8 {
    match scope {
        "user" => 0,
        _ => 1,
    }
}

// ============================== Run 值命令行解析 ==============================

/// Run 值解析出来的命令
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedRunCommand {
    pub program: String,
    pub args: Vec<String>,
}

impl ParsedRunCommand {
    /// 给用户看的命令行（只在有空格时补引号）
    pub fn display(&self) -> String {
        let mut parts = vec![quote_if_needed(&self.program)];
        parts.extend(self.args.iter().map(|arg| quote_if_needed(arg)));
        parts.join(" ")
    }
}

fn quote_if_needed(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.contains(' ') || trimmed.contains('\t') {
        format!("\"{}\"", trimmed)
    } else {
        trimmed.to_string()
    }
}

/// 按 `CommandLineToArgvW` 规则解析 Run 值里的命令行。
///
/// Run 值常见形态（设计 4.2）：
/// - `"C:\Program Files\Foo\foo.exe" --minimized`
/// - `C:\Program Files\Foo\foo.exe`（没加引号，系统会按空格拆，属于启动项自己的问题）
/// - `rundll32.exe "C:\a b\hook.dll",Entry`
///
/// 解析失败（空值 / 只有空 token）返回 None。
pub fn parse_run_command(raw: &str) -> Option<ParsedRunCommand> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut tokens = crate::cleaner::installer_windows::split_windows_args(trimmed).into_iter();
    let program = tokens.next()?;
    if program.trim().is_empty() {
        return None;
    }
    Some(ParsedRunCommand {
        program,
        args: tokens.collect(),
    })
}

/// 本应用写进 Run 的值：可执行文件路径（含空格时加引号）
pub fn format_autostart_command(exe_path: &str) -> String {
    let trimmed = exe_path.trim().trim_matches('"');
    if trimmed.contains(' ') || trimmed.contains('\t') {
        format!("\"{}\"", trimmed)
    } else {
        trimmed.to_string()
    }
}

// ============================== Windows 实现（注册表 / 启动文件夹） ==============================

#[cfg(target_os = "windows")]
mod win_impl {
    use std::path::{Path, PathBuf};

    use serde::{Deserialize, Serialize};
    use winreg::enums::{
        HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE, KEY_WOW64_32KEY,
        KEY_WOW64_64KEY, REG_BINARY, REG_EXPAND_SZ, REG_SZ,
    };
    use winreg::{RegKey, RegValue};

    use super::*;
    use crate::cleaner::startup::{StartupItem, APP_AUTOSTART_NAME};

    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const RUN_ONCE_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\RunOnce";
    const STARTUP_APPROVED_KEY: &str =
        r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved";
    const STARTUP_FOLDER: &str = r"Microsoft\Windows\Start Menu\Programs\Startup";
    /// 写状态位前的备份文件（设计 4.2：带时间戳、可回滚）
    const BACKUP_FILE: &str = "startup-backup.json";
    const MAX_BACKUP_ENTRIES: usize = 200;

    /// 备份文件里的一条记录
    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct BackupEntry {
        /// RFC3339 时间戳
        at: String,
        item_id: String,
        /// 注册表键 / 启动文件夹
        location: String,
        key_name: String,
        /// 写入前的 StartupApproved 原值（REG_BINARY 原始字节；缺失为 None）
        previous_approved: Option<Vec<u8>>,
        /// 写入前的 Run 原值 / 快捷方式路径（防御性备份，便于人工回滚）
        previous_command: Option<String>,
    }

    /// hive → (注册表根, 注册表视图)
    fn hive_root(hive: WinHive) -> (RegKey, u32) {
        match hive {
            WinHive::Hkcu => (RegKey::predef(HKEY_CURRENT_USER), 0),
            WinHive::Hklm => (RegKey::predef(HKEY_LOCAL_MACHINE), KEY_WOW64_64KEY),
            WinHive::Hklm32 => (RegKey::predef(HKEY_LOCAL_MACHINE), KEY_WOW64_32KEY),
        }
    }

    /// StartupApproved 所在根 → (注册表根, 视图)
    fn approved_root(hive: ApprovedHive) -> (RegKey, u32) {
        match hive {
            ApprovedHive::Hkcu => (RegKey::predef(HKEY_CURRENT_USER), 0),
            // Run32 是 StartupApproved 下的普通子键，用 64 位视图读才不会被重定向
            ApprovedHive::Hklm => (RegKey::predef(HKEY_LOCAL_MACHINE), KEY_WOW64_64KEY),
        }
    }

    fn run_key_path(section: WinSection) -> &'static str {
        match section {
            WinSection::Run => RUN_KEY,
            WinSection::RunOnce => RUN_ONCE_KEY,
            // 启动文件夹不是注册表键；调用方只对 Run/RunOnce 调用
            WinSection::StartupFolder => RUN_KEY,
        }
    }

    /// REG_SZ / REG_EXPAND_SZ → String；其他类型返回 None
    fn reg_string_value(value: &RegValue) -> Option<String> {
        match value.vtype {
            REG_SZ | REG_EXPAND_SZ => {
                let units: Vec<u16> = value
                    .bytes
                    .chunks_exact(2)
                    .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
                    .collect();
                let text = String::from_utf16_lossy(&units);
                let trimmed = text.trim_end_matches('\0').trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                }
            }
            _ => None,
        }
    }

    // ------------------------------ 枚举 ------------------------------

    /// 读一条 Run / RunOnce 键下的所有值
    fn read_run_key_entries(hive: WinHive, section: WinSection) -> Vec<WinStartupEntry> {
        let (root, view) = hive_root(hive);
        let Ok(key) = root.open_subkey_with_flags(run_key_path(section), KEY_READ | view) else {
            // 某个视图打不开（极少数受限环境）不应该让整个列表失败
            return Vec::new();
        };
        let mut entries = Vec::new();
        for item in key.enum_values().flatten() {
            let (value_name, raw) = item;
            if value_name.trim().is_empty() {
                continue;
            }
            let Some(raw_command) = reg_string_value(&raw) else {
                continue;
            };
            // 解析后重新拼一遍展示用命令行：引号/参数按 Windows 规则规整，
            // 但写状态位时仍然只依据注册表值名，不碰原值
            let command = parse_run_command(&raw_command)
                .map(|parsed| parsed.display())
                .unwrap_or(raw_command);
            entries.push(WinStartupEntry::registry(
                hive,
                section,
                &value_name,
                &command,
            ));
        }
        entries
    }

    /// 用户与公共启动文件夹
    fn startup_folders() -> Vec<(WinHive, PathBuf)> {
        let mut folders = Vec::new();
        if let Some(appdata) = std::env::var_os("APPDATA") {
            folders.push((WinHive::Hkcu, PathBuf::from(appdata).join(STARTUP_FOLDER)));
        }
        if let Some(program_data) =
            std::env::var_os("ProgramData").or_else(|| std::env::var_os("ALLUSERSPROFILE"))
        {
            folders.push((WinHive::Hklm, PathBuf::from(program_data).join(STARTUP_FOLDER)));
        }
        folders
    }

    fn read_folder_entries(hive: WinHive, dir: &Path) -> Vec<WinStartupEntry> {
        let Ok(read_dir) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut entries = Vec::new();
        for entry in read_dir.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let file_name = entry.file_name().to_string_lossy().to_string();
            if file_name.starts_with('.') || file_name.eq_ignore_ascii_case("desktop.ini") {
                continue;
            }
            let lower = file_name.to_ascii_lowercase();
            if !(lower.ends_with(".lnk") || lower.ends_with(".exe")) {
                continue;
            }
            entries.push(WinStartupEntry::startup_folder(
                hive,
                dir,
                &file_name,
                path.to_string_lossy().to_string(),
            ));
        }
        entries
    }

    /// 读全部 StartupApproved 状态位
    fn read_approved() -> ApprovedState {
        let mut state = ApprovedState::new();
        let stores = [
            (ApprovedHive::Hkcu, ApprovedSection::Run),
            (ApprovedHive::Hkcu, ApprovedSection::StartupFolder),
            (ApprovedHive::Hklm, ApprovedSection::Run),
            (ApprovedHive::Hklm, ApprovedSection::Run32),
            (ApprovedHive::Hklm, ApprovedSection::StartupFolder),
        ];
        for (approved_hive, approved_section) in stores {
            let (root, view) = approved_root(approved_hive);
            let path = format!(
                r"{}\{}",
                STARTUP_APPROVED_KEY,
                approved_section.registry_name()
            );
            let Ok(key) = root.open_subkey_with_flags(&path, KEY_READ | view) else {
                continue;
            };
            for item in key.enum_values().flatten() {
                let (name, raw) = item;
                if raw.vtype == REG_BINARY {
                    state.insert(
                        approved_key(approved_hive, approved_section, &name),
                        raw.bytes,
                    );
                }
            }
        }
        state
    }

    /// 启动项列表
    pub fn list() -> Vec<StartupItem> {
        let approved = read_approved();
        let mut entries = Vec::new();
        // 注册表：HKCU Run/RunOnce、HKLM 64 位、HKLM 32 位（WOW6432Node）
        for hive in [WinHive::Hkcu, WinHive::Hklm, WinHive::Hklm32] {
            entries.extend(read_run_key_entries(hive, WinSection::Run));
            entries.extend(read_run_key_entries(hive, WinSection::RunOnce));
        }
        // 启动文件夹：用户 + 公共
        for (hive, dir) in startup_folders() {
            entries.extend(read_folder_entries(hive, &dir));
        }
        build_items(entries, &approved)
    }

    // ------------------------------ 启停 ------------------------------

    /// 切换一条启动项的启用状态（只写状态位，不删启动项）
    pub fn set_enabled(
        id: &str,
        enabled: bool,
        data_dir: Option<&Path>,
    ) -> Result<(), String> {
        let parsed = parse_item_id(id).ok_or_else(|| format!("无效的启动项标识: {}", id))?;
        if !can_toggle(parsed.hive, parsed.section) {
            return Err(format!(
                "「{}」是只读启动项，本版本不修改",
                parsed.key_name
            ));
        }
        match parsed.section {
            WinSection::Run => set_registry_run(&parsed, enabled, data_dir),
            WinSection::StartupFolder => set_startup_folder(&parsed, enabled, data_dir),
            WinSection::RunOnce => Err("RunOnce 条目只读展示，不支持启停".to_string()),
        }
    }

    /// 读某个 StartupApproved 值的原始字节（备份用）
    fn read_approved_raw(
        approved_hive: ApprovedHive,
        approved_section: ApprovedSection,
        name: &str,
    ) -> Option<Vec<u8>> {
        let (root, view) = approved_root(approved_hive);
        let path = format!(
            r"{}\{}",
            STARTUP_APPROVED_KEY,
            approved_section.registry_name()
        );
        let key = root.open_subkey_with_flags(&path, KEY_READ | view).ok()?;
        let raw = key.get_raw_value(name).ok()?;
        if raw.vtype == REG_BINARY {
            Some(raw.bytes)
        } else {
            None
        }
    }

    /// 写 StartupApproved 状态位（必要时创建 StartupApproved 子键）。
    ///
    /// `root` 必须是 `StartupApproved` 所在根（当前只有 HKCU 可写）。
    fn write_approved(
        root: &RegKey,
        approved_section: ApprovedSection,
        name: &str,
        enabled: bool,
    ) -> Result<(), String> {
        let path = format!(
            r"{}\{}",
            STARTUP_APPROVED_KEY,
            approved_section.registry_name()
        );
        let (key, _) = root
            .create_subkey_with_flags(&path, KEY_WRITE)
            .map_err(|err| format!("打开启动状态注册表项失败: {}", err))?;
        let raw = encode_startup_approved(enabled, now_filetime());
        let value = RegValue {
            bytes: raw.to_vec(),
            vtype: REG_BINARY,
        };
        key.set_raw_value(name, &value)
            .map_err(|err| format!("写入启动项状态失败: {}", err))
    }

    fn set_registry_run(
        parsed: &ParsedItem,
        enabled: bool,
        data_dir: Option<&Path>,
    ) -> Result<(), String> {
        let root = RegKey::predef(HKEY_CURRENT_USER);
        let run_key = root
            .open_subkey_with_flags(RUN_KEY, KEY_READ | KEY_WRITE)
            .map_err(|err| format!("打开注册表启动项失败: {}", err))?;
        // 先确认值还在：不存在的条目不该凭空写状态位
        let current = run_key
            .get_raw_value(&parsed.key_name)
            .map_err(|_| "启动项不存在（可能已被删除）".to_string())?;
        let previous_command = reg_string_value(&current);

        let backup = BackupEntry {
            at: chrono::Utc::now().to_rfc3339(),
            item_id: format!(
                "win:{}:{}:{}",
                parsed.section.token(),
                parsed.hive.token(),
                parsed.key_name
            ),
            location: registry_detail(parsed.hive, parsed.section),
            key_name: parsed.key_name.clone(),
            previous_approved: read_approved_raw(
                ApprovedHive::Hkcu,
                ApprovedSection::Run,
                &parsed.key_name,
            ),
            previous_command,
        };
        append_backup(data_dir, &backup)?;

        write_approved(&root, ApprovedSection::Run, &parsed.key_name, enabled)
    }

    fn set_startup_folder(
        parsed: &ParsedItem,
        enabled: bool,
        data_dir: Option<&Path>,
    ) -> Result<(), String> {
        let Some((_, dir)) = startup_folders()
            .into_iter()
            .find(|(hive, _)| *hive == parsed.hive)
        else {
            return Err("找不到启动文件夹".to_string());
        };
        let path = dir.join(&parsed.key_name);
        if !path.is_file() {
            return Err(format!("启动项不存在（可能已被删除）: {}", path.display()));
        }

        let backup = BackupEntry {
            at: chrono::Utc::now().to_rfc3339(),
            item_id: format!(
                "win:{}:{}:{}",
                parsed.section.token(),
                parsed.hive.token(),
                parsed.key_name
            ),
            location: format!("启动文件夹：{}", dir.display()),
            key_name: parsed.key_name.clone(),
            previous_approved: read_approved_raw(
                ApprovedHive::Hkcu,
                ApprovedSection::StartupFolder,
                &parsed.key_name,
            ),
            previous_command: Some(path.to_string_lossy().to_string()),
        };
        append_backup(data_dir, &backup)?;

        let root = RegKey::predef(HKEY_CURRENT_USER);
        write_approved(
            &root,
            ApprovedSection::StartupFolder,
            &parsed.key_name,
            enabled,
        )
    }

    /// 写状态位前把原值追加进 `<data_dir>/startup-backup.json`
    fn append_backup(data_dir: Option<&Path>, entry: &BackupEntry) -> Result<(), String> {
        // 没有可写目录就拒绝修改：设计 4.2 要求「写前必须能回滚」
        let Some(dir) = data_dir else {
            return Err(
                "没有可写的本地存储目录，已取消修改（启动项状态需要先备份）".to_string(),
            );
        };
        crate::util::fs::create_dir_all(dir)?;
        let path = dir.join(BACKUP_FILE);
        let mut entries: Vec<BackupEntry> = crate::util::fs::read_text_if_exists(&path)?
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        entries.push(entry.clone());
        if entries.len() > MAX_BACKUP_ENTRIES {
            entries.drain(0..entries.len() - MAX_BACKUP_ENTRIES);
        }
        let text = serde_json::to_string_pretty(&entries)
            .map_err(|err| format!("序列化启动项备份失败: {}", err))?;
        crate::util::fs::write_text(&path, &text)
    }

    fn now_filetime() -> u64 {
        let seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs() as i64)
            .unwrap_or(0);
        unix_seconds_to_filetime(seconds)
    }

    // ------------------------------ 本应用登录启动 ------------------------------

    /// 本应用是否写在 `HKCU\...\Run\ws-workbench` 里
    pub fn app_autostart_get() -> bool {
        let Ok(key) = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(RUN_KEY, KEY_READ)
        else {
            return false;
        };
        key.get_value::<String, _>(APP_AUTOSTART_NAME)
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
    }

    pub fn app_autostart_set(enabled: bool) -> Result<(), String> {
        let root = RegKey::predef(HKEY_CURRENT_USER);
        if !enabled {
            // 键不存在 = 本来就没设置，按成功处理
            let Ok(key) = root.open_subkey_with_flags(RUN_KEY, KEY_READ | KEY_WRITE) else {
                return Ok(());
            };
            match key.delete_value(APP_AUTOSTART_NAME) {
                Ok(()) => return Ok(()),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(err) => return Err(format!("删除登录启动项失败: {}", err)),
            }
        }

        let exe = std::env::current_exe()
            .map_err(|err| format!("获取当前程序路径失败: {}", err))?;
        let command = format_autostart_command(&exe.to_string_lossy());
        let (key, _) = root
            .create_subkey_with_flags(RUN_KEY, KEY_WRITE)
            .map_err(|err| format!("打开注册表启动项失败: {}", err))?;
        key.set_value(APP_AUTOSTART_NAME, &command)
            .map_err(|err| format!("写入登录启动项失败: {}", err))?;
        Ok(())
    }
}

#[cfg(target_os = "windows")]
pub use win_impl::{app_autostart_get, app_autostart_set, list, set_enabled};

// ============================== 单测（纯逻辑，macOS 也能跑） ==============================

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- StartupApproved 编解码 ----------

    #[test]
    fn decodes_enabled_state() {
        let mut raw = [0u8; 12];
        raw[0] = 0x02;
        let state = decode_startup_approved(&raw);
        assert!(state.is_enabled());
        assert_eq!(state.disabled_filetime(), None);

        // 全 0（从未被 Task Manager 写过）也按启用
        assert!(decode_startup_approved(&[0u8; 12]).is_enabled());
    }

    #[test]
    fn decodes_disabled_state_with_filetime() {
        // 2026-01-01T00:00:00Z
        let unix_seconds = 1_767_225_600i64;
        let filetime = unix_seconds_to_filetime(unix_seconds);
        let raw = encode_startup_approved(false, filetime);
        assert_eq!(raw[0], 0x03);

        let state = decode_startup_approved(&raw);
        assert!(!state.is_enabled());
        assert_eq!(state.disabled_filetime(), Some(filetime));
        // FILETIME 反解回 unix 秒必须一致
        assert_eq!(state.disabled_at_unix(), Some(unix_seconds));
        assert_eq!(filetime_to_unix_seconds(filetime), Some(unix_seconds));
    }

    #[test]
    fn decode_tolerates_short_and_invalid_values() {
        // 长度不足 12：容错为启用（宁可不误标禁用）
        assert!(decode_startup_approved(&[]).is_enabled());
        assert!(decode_startup_approved(&[0x03]).is_enabled());
        assert!(decode_startup_approved(&[0x03, 0, 0, 0, 0, 0]).is_enabled());

        // 非 02 的首字节按禁用处理，没有有效 FILETIME 时为 None
        let mut strategy = [0u8; 12];
        strategy[0] = 0x06;
        let state = decode_startup_approved(&strategy);
        assert!(!state.is_enabled());
        assert_eq!(state.disabled_filetime(), None);
        assert_eq!(state.disabled_at_unix(), None);
    }

    #[test]
    fn encodes_roundtrip() {
        let enabled = encode_startup_approved(true, 0);
        assert_eq!(enabled[0], 0x02);
        assert_eq!(&enabled[1..], &[0u8; 11]);
        assert!(decode_startup_approved(&enabled).is_enabled());

        let disabled = encode_startup_approved(false, 0);
        assert_eq!(disabled[0], 0x03);
        assert!(matches!(
            decode_startup_approved(&disabled),
            StartupApproved::Disabled { filetime: None }
        ));
    }

    #[test]
    fn filetime_helpers_handle_sentinels() {
        assert_eq!(unix_seconds_to_filetime(0), FILETIME_EPOCH_OFFSET_SECS as u64 * FILETIME_TICKS_PER_SEC);
        assert_eq!(unix_seconds_to_filetime(-1), 0);
        assert_eq!(filetime_to_unix_seconds(0), None);
        assert_eq!(filetime_to_unix_seconds(u64::MAX), None);
    }

    // ---------- 过滤 / 去重 / 只读判定 ----------

    fn entry(hive: WinHive, section: WinSection, name: &str, command: &str) -> WinStartupEntry {
        match section {
            WinSection::StartupFolder => WinStartupEntry::startup_folder(
                hive,
                Path::new(r"C:\Users\me\Start Menu\Programs\Startup"),
                name,
                command.to_string(),
            ),
            _ => WinStartupEntry::registry(hive, section, name, command),
        }
    }

    #[test]
    fn filters_empty_entries() {
        assert!(!should_show(&entry(WinHive::Hkcu, WinSection::Run, "  ", "cmd")));
        assert!(!should_show(&entry(WinHive::Hkcu, WinSection::Run, "Foo", "  ")));
        assert!(should_show(&entry(WinHive::Hkcu, WinSection::Run, "Foo", "C:\\Foo\\foo.exe")));
    }

    #[test]
    fn dedupes_same_item_id() {
        let entries = vec![
            entry(WinHive::Hkcu, WinSection::Run, "Foo", "C:\\Foo\\foo.exe"),
            entry(WinHive::Hkcu, WinSection::Run, "Foo", "C:\\Foo\\foo.exe"),
            entry(WinHive::Hklm, WinSection::Run, "Foo", "C:\\Foo\\foo.exe"),
        ];
        let approved = ApprovedState::new();
        let items = build_items(entries, &approved);
        assert_eq!(items.len(), 2);
        assert!(items.iter().any(|item| item.scope == "user"));
        assert!(items.iter().any(|item| item.scope == "machine"));
    }

    #[test]
    fn readonly_judgement() {
        // 可切换：HKCU Run / 用户启动文件夹
        assert!(can_toggle(WinHive::Hkcu, WinSection::Run));
        assert!(can_toggle(WinHive::Hkcu, WinSection::StartupFolder));
        // 只读：HKLM 两个视图、公共启动文件夹、RunOnce
        assert!(!can_toggle(WinHive::Hklm, WinSection::Run));
        assert!(!can_toggle(WinHive::Hklm32, WinSection::Run));
        assert!(!can_toggle(WinHive::Hklm, WinSection::StartupFolder));
        assert!(!can_toggle(WinHive::Hkcu, WinSection::RunOnce));
        assert!(!can_toggle(WinHive::Hklm, WinSection::RunOnce));
        assert!(readonly_reason(WinHive::Hklm32, WinSection::Run)
            .unwrap()
            .contains("管理员权限"));
        assert!(readonly_reason(WinHive::Hkcu, WinSection::RunOnce)
            .unwrap()
            .contains("RunOnce"));
    }

    #[test]
    fn build_items_applies_approved_state() {
        let hkcu_key = approved_key(ApprovedHive::Hkcu, ApprovedSection::Run, "Foo");
        let mut approved = ApprovedState::new();
        approved.insert(hkcu_key, encode_startup_approved(false, 0).to_vec());

        let items = build_items(
            vec![
                entry(WinHive::Hkcu, WinSection::Run, "Foo", "C:\\Foo\\foo.exe"),
                entry(WinHive::Hkcu, WinSection::Run, "Bar", "C:\\Bar\\bar.exe"),
            ],
            &approved,
        );
        let foo = items.iter().find(|item| item.name == "Foo").unwrap();
        assert!(!foo.enabled);
        assert!(foo.can_toggle);
        let bar = items.iter().find(|item| item.name == "Bar").unwrap();
        assert!(bar.enabled, "没有状态位时缺省视为启用");
    }

    #[test]
    fn item_id_roundtrip_keeps_colons_in_value_name() {
        let id = format!("win:run:hkcu:{}", r"App: with: colons");
        let parsed = parse_item_id(&id).unwrap();
        assert_eq!(parsed.hive, WinHive::Hkcu);
        assert_eq!(parsed.section, WinSection::Run);
        assert_eq!(parsed.key_name, r"App: with: colons");

        let folder = parse_item_id("win:startup_folder:hkcu:OneDrive.lnk").unwrap();
        assert_eq!(folder.section, WinSection::StartupFolder);
        assert_eq!(folder.key_name, "OneDrive.lnk");

        assert!(parse_item_id("win:run:bogus:Foo").is_none());
        assert!(parse_item_id("hkcu:Foo").is_none());
        assert!(parse_item_id("").is_none());
    }

    #[test]
    fn folder_entry_display_name_strips_extension() {
        let entry = entry(
            WinHive::Hkcu,
            WinSection::StartupFolder,
            "OneDrive.lnk",
            r"C:\Users\me\Start Menu\Programs\Startup\OneDrive.lnk",
        );
        assert_eq!(entry.name, "OneDrive");
        assert_eq!(entry.key_name, "OneDrive.lnk");
        assert_eq!(entry.id(), "win:startup_folder:hkcu:OneDrive.lnk");
    }

    // ---------- Run 值命令解析 ----------

    #[test]
    fn parses_quoted_run_command() {
        let parsed =
            parse_run_command(r#""C:\Program Files\Foo\foo.exe" --minimized --title="hello world""#)
                .unwrap();
        assert_eq!(parsed.program, r"C:\Program Files\Foo\foo.exe");
        assert_eq!(parsed.args, vec!["--minimized", "--title=hello world"]);
        assert_eq!(
            parsed.display(),
            r#""C:\Program Files\Foo\foo.exe" --minimized "--title=hello world""#
        );
    }

    #[test]
    fn parses_unquoted_run_command() {
        let parsed = parse_run_command(r"C:\Foo\foo.exe /background").unwrap();
        assert_eq!(parsed.program, r"C:\Foo\foo.exe");
        assert_eq!(parsed.args, vec!["/background"]);
    }

    #[test]
    fn rejects_empty_run_command() {
        assert!(parse_run_command("").is_none());
        assert!(parse_run_command("   ").is_none());
    }

    #[test]
    fn formats_autostart_command_for_current_exe() {
        assert_eq!(
            format_autostart_command(r"C:\Program Files\ws-workbench\ws-workbench.exe"),
            r#""C:\Program Files\ws-workbench\ws-workbench.exe""#
        );
        assert_eq!(
            format_autostart_command(r"C:\ws-workbench.exe"),
            r"C:\ws-workbench.exe"
        );
    }
}
