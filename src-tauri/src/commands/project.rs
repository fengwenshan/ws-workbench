//! 项目打包、校验、类型与版本控制识别命令。

use tauri::AppHandle;

use crate::models::*;
use crate::packer;

#[tauri::command]
pub async fn pack_project(
    app: AppHandle,
    project: ProjectConfig,
    default_exclude: Vec<String>,
) -> Result<PackResult, String> {
    packer::pack_project(&app, &project, &default_exclude)
}

#[tauri::command]
pub async fn pack_to_zip(
    app: AppHandle,
    project: ProjectConfig,
    default_exclude: Vec<String>,
) -> Result<PackResult, String> {
    packer::pack_to_zip(&app, &project, &default_exclude)
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
