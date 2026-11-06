//! 「系统清理」的对外数据模型（设计文档 3.1）。
//!
//! 全部字段由前端直接消费，因此：
//! - serde 字段保持 snake_case（与 `OpenWithApp` 等现有类型一致）；
//! - 不为内部实现方便而增删字段，改契约要先改设计文档。
//!
//! 这些类型同时要能被 tauri 命令序列化，所以统一 `Serialize + Deserialize +
//! Clone + Debug`，部分类型带 `Default` 方便测试与「未知字段」兜底。

use serde::{Deserialize, Serialize};

/// 已安装应用（软件卸载列表的一行）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InstalledApp {
    /// "win:{hive}|{key_path}" / "mac:{bundle_path}"
    pub id: String,
    pub name: String,
    pub version: String,
    pub publisher: String,
    pub install_location: String,
    /// 0 表示未知
    pub size_bytes: u64,
    /// `data:image/png;base64,...`
    pub icon: Option<String>,
    /// "registry" | "applications" | "system"
    pub source: String,
    /// 系统应用 / SIP 应用为 false
    pub can_uninstall: bool,
    /// "msi" | "exe" | "app_bundle" | "system" | "force"
    pub uninstall_kind: String,
    /// YYYY-MM-DD；未知为空串
    pub install_date: String,
    /// 备注：安装日期、注册表键、bundle id 等
    pub detail: String,
}

/// 一次卸载执行的结果
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UninstallOutcome {
    pub ok: bool,
    /// 实际执行的命令（展示给用户）
    pub command: String,
    /// 拿不到时为 None
    pub exit_code: Option<i32>,
    pub elevated: bool,
    pub message: String,
}

/// 一条残留（文件或注册表项）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LeftoverItem {
    pub path: String,
    /// install_dir/app_data/registry/cache/preference/log/
    /// launch_agent/shortcut/container/receipt
    pub kind: String,
    pub size_bytes: u64,
    pub is_dir: bool,
    /// "high" | "medium" | "low"
    pub confidence: String,
    /// 默认勾选：high
    pub selected: bool,
    pub requires_elevation: bool,
    pub exists: bool,
}

/// 批量删除的结果
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeleteOutcome {
    pub trashed: usize,
    pub failed: Vec<DeleteFailure>,
}

/// 单条删除失败
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeleteFailure {
    pub path: String,
    pub error: String,
}

/// 按应用聚合后的内存占用
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryApp {
    /// bundle id / 应用目录 / exe 路径
    pub id: String,
    pub name: String,
    pub exe: String,
    pub icon: Option<String>,
    pub process_count: usize,
    /// 该应用所有进程 RSS 之和（字节）
    pub memory: u64,
    pub pids: Vec<u32>,
    /// "app" | "process"
    pub kind: String,
}

/// 一次内存采样
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemorySnapshot {
    pub total: u64,
    pub used: u64,
    pub available: u64,
    pub used_percent: f32,
    pub swap_total: u64,
    pub swap_used: u64,
    /// 按 memory 降序
    pub apps: Vec<MemoryApp>,
    /// epoch 毫秒
    pub sampled_at: i64,
}

/// 结束进程的结果
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QuitOutcome {
    /// 成功发出优雅退出请求的数量
    pub quit: usize,
    /// 强制结束的数量
    pub forced: usize,
    /// 失败的进程标识（应用名 / pid）
    pub failed: Vec<String>,
}

/// 深度释放的结果
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReleaseOutcome {
    pub ok: bool,
    pub before_used: u64,
    pub after_used: u64,
    /// after 用系统重采样，不夸张
    pub freed: u64,
    pub message: String,
    /// 原理说明，直接展示在 UI
    pub note: String,
}

/// 扫描/卸载进度，经 `cleaner-progress` 事件推给前端
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CleanerProgress {
    /// "list_apps" | "scan_leftovers" | "uninstall"
    pub task: String,
    /// "scanning" | "loading_icons" | "sizing" | "running" | "waiting" | "done"
    pub phase: String,
    pub current: usize,
    pub total: usize,
    pub message: String,
}

impl CleanerProgress {
    pub fn new(task: &str, phase: &str, current: usize, total: usize, message: impl Into<String>) -> Self {
        Self {
            task: task.to_string(),
            phase: phase.to_string(),
            current,
            total,
            message: message.into(),
        }
    }
}
