//! 项目打包、校验、类型与版本控制识别命令。

use tauri::AppHandle;

use crate::models::*;
use crate::packer;

#[tauri::command]
pub async fn pack_project(
    app: AppHandle,
    project: ProjectConfig,
    default_include: Vec<String>,
) -> Result<PackResult, String> {
    packer::pack_project(&app, &project, &default_include)
}

#[tauri::command]
pub async fn pack_to_zip(
    app: AppHandle,
    project: ProjectConfig,
    default_include: Vec<String>,
) -> Result<PackResult, String> {
    packer::pack_to_zip(&app, &project, &default_include)
}

#[tauri::command]
pub fn validate_project(project: ProjectConfig) -> ProjectValidation {
    packer::validate_project(&project)
}

#[tauri::command]
pub fn detect_project_type(source_dir: String) -> ProjectType {
    packer::detect_project_type(&source_dir)
}

#[tauri::command]
pub fn detect_vcs(source_dir: String) -> VcsInfo {
    packer::detect_vcs(&source_dir)
}

/// 快速搜索用：批量读取项目的 Git / SVN 信息（顺序与入参一一对应）。
///
/// 单独开一个异步命令而不是让前端并发调用 `detect_vcs`：同步命令跑在主线程上，
/// 项目一多（SVN 还要等一次 `svn info`）会把界面卡住。这里放进阻塞线程池，
/// 单个目录读失败只影响它自己的结果，不会中断整批。
#[tauri::command]
pub async fn detect_vcs_many(source_dirs: Vec<String>) -> Vec<VcsInfo> {
    tauri::async_runtime::spawn_blocking(move || {
        source_dirs
            .iter()
            .map(|dir| packer::detect_vcs(dir))
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default()
}
