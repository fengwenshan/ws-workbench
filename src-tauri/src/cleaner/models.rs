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
    /// 跳过数：卸载程序已删掉、或扫描后消失的路径不算失败（设计 3 节）
    #[serde(default)]
    pub skipped: usize,
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

// ============================== 卸载窗口：关联文件清单（设计 3 节） ==============================

/// 关联文件分组：program / support / cache
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RelatedFileGroup {
    /// "program" | "support" | "cache"
    pub id: String,
    /// 「程序文件」|「支持文件」|「缓存文件」
    pub label: String,
    pub items: Vec<RelatedFileItem>,
}

/// 一条关联文件（文件、目录或 Windows 注册表项）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RelatedFileItem {
    /// 文件/目录绝对路径；Windows 注册表项为 `win:{hive}|{key_path}`
    pub path: String,
    /// 展示名（应用名 / 目录名 / 注册表键名）
    pub name: String,
    /// app_bundle/app_data/cache/preference/container/launch_agent/log/
    /// crash_report/install_dir/registry/shortcut
    pub kind: String,
    /// 目录按 `path_size` 限深统计
    pub size_bytes: u64,
    pub is_dir: bool,
    /// 仅 app_bundle；未知为空串
    pub version: String,
    /// high / medium / low
    pub confidence: String,
    pub requires_elevation: bool,
    pub exists: bool,
    /// 默认勾选：confidence != "low"（由 `related::build_related_files` 统一赋值）
    pub selected: bool,
}

/// 卸载窗口的三组关联文件清单
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RelatedFiles {
    pub app_id: String,
    pub groups: Vec<RelatedFileGroup>,
    /// 所有条目合计
    pub total_bytes: u64,
    pub item_count: usize,
    /// 「今天使用过」/「7天内使用过」/「30天内使用过」/「最近未使用」/ ""
    pub last_used: String,
}

/// 强制退出结果（设计 3 节）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ForceQuitOutcome {
    /// true = 所有相关进程已退出
    pub ok: bool,
    /// 结束时仍在运行的进程数（ok 时为 0）
    pub processes: usize,
    pub names: Vec<String>,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn related_files_serializes_snake_case() {
        let files = RelatedFiles {
            app_id: "mac:/Applications/Foo.app".to_string(),
            groups: vec![RelatedFileGroup {
                id: "program".to_string(),
                label: "程序文件".to_string(),
                items: vec![RelatedFileItem {
                    path: "/Applications/Foo.app".to_string(),
                    name: "Foo".to_string(),
                    kind: "app_bundle".to_string(),
                    size_bytes: 1024,
                    is_dir: true,
                    version: "1.0.0".to_string(),
                    confidence: "high".to_string(),
                    requires_elevation: false,
                    exists: true,
                    selected: true,
                }],
            }],
            total_bytes: 1024,
            item_count: 1,
            last_used: "今天使用过".to_string(),
        };
        let json = serde_json::to_string(&files).unwrap();
        assert!(json.contains("\"size_bytes\""), "实际: {json}");
        assert!(json.contains("\"last_used\""), "实际: {json}");
        assert!(json.contains("\"item_count\""), "实际: {json}");
        assert!(json.contains("\"requires_elevation\""), "实际: {json}");
        assert!(!json.contains("sizeBytes"), "不应出现 camelCase: {json}");
        assert!(!json.contains("lastUsed"), "不应出现 camelCase: {json}");
    }

    #[test]
    fn delete_outcome_skipped_defaults_to_zero_and_is_optional() {
        // 旧前端/旧数据里没有 skipped 字段时反序列化不能失败（serde default）
        let outcome: DeleteOutcome = serde_json::from_str(r#"{"trashed":2,"failed":[]}"#).unwrap();
        assert_eq!(outcome.trashed, 2);
        assert_eq!(outcome.skipped, 0);
    }
}
