//! 「弹窗拦截」页签（仅 Windows）的命令入口。

use std::collections::HashMap;

use crate::cleaner::popup::{
    self, PopupAppMeta, PopupHit, PopupRule, PopupRuleAddResult, PopupRuleDraft, PopupSettings,
    PopupStatus, PopupWindow,
};

#[tauri::command]
pub fn popup_status() -> PopupStatus {
    popup::status()
}

/// 开启弹窗拦截（同时把总开关持久化，下次启动自动恢复）
#[tauri::command]
pub async fn popup_start(app: tauri::AppHandle) -> Result<PopupStatus, String> {
    tauri::async_runtime::spawn_blocking(move || popup::start(app))
        .await
        .map_err(|err| format!("开启弹窗拦截失败：{err}"))?
}

/// 停止检测线程（join 最多几百毫秒，别占主线程）
#[tauri::command]
pub async fn popup_stop() -> Result<PopupStatus, String> {
    tauri::async_runtime::spawn_blocking(popup::stop)
        .await
        .map_err(|err| format!("关闭弹窗拦截失败：{err}"))?
}

#[tauri::command]
pub fn popup_recent() -> Vec<PopupHit> {
    popup::recent()
}

#[tauri::command]
pub fn popup_clear() {
    popup::clear();
}

#[tauri::command]
pub fn popup_dismiss(id: u64, action: String) -> Result<(), String> {
    popup::dismiss(id, &action)
}

/// 按进程名加入 / 移出忽略名单（白名单），返回最新名单
#[tauri::command]
pub fn popup_ignore(process: String, ignored: bool) -> Result<Vec<String>, String> {
    popup::ignore(&process, ignored)
}

#[tauri::command]
pub fn popup_ignored() -> Vec<String> {
    popup::ignored()
}

/// 拦截设置（智能拦截开关 / 动作 / 阈值 / 每分钟上限）
#[tauri::command]
pub fn popup_settings_get() -> PopupSettings {
    popup::settings_get()
}

/// 保存拦截设置；后端会把越界值收进安全范围后返回最终值
#[tauri::command]
pub fn popup_settings_set(settings: PopupSettings) -> Result<PopupSettings, String> {
    popup::settings_set(settings)
}

#[tauri::command]
pub fn popup_rules() -> Vec<PopupRule> {
    popup::rules_list()
}

/// 新增拦截规则；`applyNow` 时顺便处理当前已经开着的同类窗口
#[tauri::command]
pub async fn popup_rule_add(
    app: tauri::AppHandle,
    draft: PopupRuleDraft,
    apply_now: bool,
) -> Result<PopupRuleAddResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let result = popup::rule_add(draft)?;
        if apply_now {
            // 拦截线程没开时保存规则即可，不要把「立即处理」的失败抛给用户
            let _ = popup::rule_apply(&result.id, &app);
        }
        Ok(result)
    })
    .await
    .map_err(|err| format!("保存拦截规则失败：{err}"))?
}

#[tauri::command]
pub fn popup_rule_remove(id: String) -> Result<Vec<PopupRule>, String> {
    popup::rule_remove(&id)
}

#[tauri::command]
pub fn popup_rule_toggle(id: String, enabled: bool) -> Result<Vec<PopupRule>, String> {
    popup::rule_set_enabled(&id, enabled)
}

/// 按应用整体启用 / 停用（规则列表里每个软件一行开关）
#[tauri::command]
pub fn popup_app_toggle(process: String, enabled: bool) -> Result<Vec<PopupRule>, String> {
    popup::rule_set_app_enabled(&process, enabled)
}

/// 对已保存的规则立即执行一次（窗口选择器里「立即拦截」用）
#[tauri::command]
pub async fn popup_rule_apply(
    app: tauri::AppHandle,
    rule_id: String,
) -> Result<usize, String> {
    tauri::async_runtime::spawn_blocking(move || popup::rule_apply(&rule_id, &app))
        .await
        .map_err(|err| format!("执行拦截规则失败：{err}"))?
}

/// 当前可见的顶层窗口（「从窗口添加规则」用）
#[tauri::command]
pub async fn popup_windows() -> Vec<PopupWindow> {
    tauri::async_runtime::spawn_blocking(popup::windows)
        .await
        .unwrap_or_default()
}

/// 应用展示名 + 图标（取 exe 的版本信息，第一次可能等 1~2 秒）
#[tauri::command]
pub async fn popup_app_meta(paths: Vec<String>) -> HashMap<String, PopupAppMeta> {
    tauri::async_runtime::spawn_blocking(move || popup::app_meta(paths))
        .await
        .unwrap_or_default()
}
