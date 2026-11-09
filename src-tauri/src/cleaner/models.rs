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

/// 应用运行状态（`cleaner_app_running` 返回值）。
///
/// `processes` 是主进程 + 该应用 bundle/安装目录内的 helper 进程数（按 pid 去重）；
/// `names` 已去重，前端只展示前几个，避免弹窗被一长串渲染进程撑爆。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RunningState {
    pub running: bool,
    pub processes: usize,
    pub names: Vec<String>,
}
