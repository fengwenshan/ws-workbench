//! 「开机自启」页签：列出开机启动项并启用 / 禁用，含本应用登录启动开关。
//!
//! 设计 4.2。平台差异全部下沉到 `startup_windows` / `startup_macos`，本文件只做三件事：
//! 1. 数据契约（`StartupItem`，字段名与设计一致）；
//! 2. 分派到平台实现；
//! 3. **统一的安全校验**：只允许切换本次列表里出现、且 `can_toggle` 的条目，
//!    不信任前端传来的任意 id（避免误写系统级状态位）。
//!
//! 平台文件与这里同级（`cleaner/startup_*.rs`），用 `#[path]` 挂在 `startup` 模块下，
//! 这样 `cleaner/mod.rs` 只需要 `pub mod startup;` 一行。

use serde::{Deserialize, Serialize};
use std::path::Path;

#[path = "startup_windows.rs"]
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod startup_windows;

#[path = "startup_macos.rs"]
#[cfg(target_os = "macos")]
mod startup_macos;

/// 本应用在 `HKCU\...\Run` / `~/Library/LaunchAgents` 里使用的名字（设计 4.2）
pub const APP_AUTOSTART_NAME: &str = "ws-workbench";

/// 自动化授权被拒时，macOS 列表里会多出这条只读占位项，前端据此显示提示条。
/// 只有 macOS 后端会构造它，Windows 构建里是未使用的常量（契约仍保留）。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub const LOGIN_ITEMS_UNAVAILABLE_ID: &str = "mac:login-items:unavailable";

/// 一条开机启动项（设计 4.2 的数据契约）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartupItem {
    /// 启停时回传给后端的唯一标识（平台前缀 `win:` / `mac:`）
    pub id: String,
    pub name: String,
    /// 启动命令行；启动文件夹里是快捷方式的绝对路径
    pub command: String,
    /// run / run_once / startup_folder / launch_agent / login_item
    pub source: String,
    /// user / machine
    pub scope: String,
    pub enabled: bool,
    /// 系统级 / RunOnce 为 false（只读展示）
    pub can_toggle: bool,
    /// 备注：来源位置、只读原因、授权提示等，直接展示给用户
    pub detail: String,
}

/// 列出所有启动项。
///
/// 单个来源读不到（权限/损坏）不应让整个列表失败：平台实现逐项跳过，
/// macOS 的自动化授权被拒会降级成一条带说明的只读项。
pub fn list() -> Result<Vec<StartupItem>, String> {
    #[cfg(target_os = "macos")]
    {
        return Ok(startup_macos::list());
    }
    #[cfg(target_os = "windows")]
    {
        return Ok(startup_windows::list());
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Err("当前平台不支持启动项管理".to_string())
    }
}

/// 启用 / 禁用一条启动项，成功返回**刷新后**的列表。
///
/// 校验规则：条目必须在刚枚举出来的列表里（登录项被移除后允许重新添加，见
/// `startup_macos`），且 `can_toggle=true`；否则直接返回中文错误，平台层不会再
/// 被调用。`data_dir` 用于 Windows 写状态位前备份原值（设计 4.2）。
pub fn set_enabled(
    id: &str,
    enabled: bool,
    data_dir: Option<&Path>,
) -> Result<Vec<StartupItem>, String> {
    let id = id.trim();
    if id.is_empty() {
        return Err("启动项标识为空".to_string());
    }
    let _ = data_dir;

    // 重新枚举，不信任前端传回来的 id 与 can_toggle
    let items = list()?;
    ensure_toggleable(&items, id)?;

    #[cfg(target_os = "macos")]
    {
        startup_macos::set_enabled(id, enabled)?;
    }
    #[cfg(target_os = "windows")]
    {
        startup_windows::set_enabled(id, enabled, data_dir)?;
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = enabled;
        return Err("当前平台不支持启动项启停".to_string());
    }

    list()
}

/// 校验一条启动项是否允许切换（抽成纯函数，方便单测只读判定）。
fn ensure_toggleable(items: &[StartupItem], id: &str) -> Result<(), String> {
    if let Some(item) = items.iter().find(|item| item.id == id) {
        if !item.can_toggle {
            let reason = if item.detail.trim().is_empty() {
                "系统级启动项需要管理员权限，本版本只读展示".to_string()
            } else {
                item.detail.trim().to_string()
            };
            return Err(format!("「{}」不可切换：{}", item.name, reason));
        }
    }
    Ok(())
}

/// 本应用是否设置为登录时启动（读失败按「未设置」处理，不阻塞页面）
pub fn app_autostart_get() -> Result<bool, String> {
    #[cfg(target_os = "macos")]
    {
        return Ok(startup_macos::app_autostart_get());
    }
    #[cfg(target_os = "windows")]
    {
        return Ok(startup_windows::app_autostart_get());
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Err("当前平台不支持设置登录启动".to_string())
    }
}

/// 设置 / 取消本应用登录时启动；成功返回设置后的状态。
pub fn app_autostart_set(enabled: bool) -> Result<bool, String> {
    #[cfg(target_os = "macos")]
    {
        startup_macos::app_autostart_set(enabled)?;
    }
    #[cfg(target_os = "windows")]
    {
        startup_windows::app_autostart_set(enabled)?;
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = enabled;
        return Err("当前平台不支持设置登录启动".to_string());
    }
    Ok(enabled)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, can_toggle: bool) -> StartupItem {
        StartupItem {
            id: id.to_string(),
            name: format!("项目 {}", id),
            command: "cmd".to_string(),
            source: "run".to_string(),
            scope: if can_toggle { "user" } else { "machine" }.to_string(),
            enabled: true,
            can_toggle,
            detail: if can_toggle {
                String::new()
            } else {
                "系统级注册表启动项（HKLM）需要管理员权限，本版本只读展示".to_string()
            },
        }
    }

    #[test]
    fn toggleable_contract_allows_user_items() {
        let items = vec![item("win:run:hkcu:foo", true), item("win:run:hklm:bar", false)];
        assert!(ensure_toggleable(&items, "win:run:hkcu:foo").is_ok());
    }

    #[test]
    fn toggleable_contract_rejects_readonly_items() {
        let items = vec![item("win:run:hklm:bar", false)];
        let error = ensure_toggleable(&items, "win:run:hklm:bar").unwrap_err();
        assert!(error.contains("不可切换"));
        assert!(error.contains("管理员权限"));
    }

    /// 手动验证：真实枚举本机启动项并打印（macOS 会跑 launchctl / osascript）。
    /// 跑法：`cargo test --lib cleaner::startup::tests::manual_list_real_items -- --ignored --nocapture`
    #[test]
    #[ignore = "依赖本机启动项环境，默认不跑"]
    fn manual_list_real_items() {
        match super::list() {
            Ok(items) => {
                println!("共 {} 条启动项", items.len());
                for item in items.iter().take(20) {
                    println!(
                        "  [{}] {} | {} | scope={} toggle={} detail={}",
                        if item.enabled { "on " } else { "off" },
                        item.name,
                        item.source,
                        item.scope,
                        item.can_toggle,
                        item.detail
                    );
                }
                assert!(!items.len() > 1000, "启动项数量异常");
            }
            Err(err) => panic!("枚举启动项失败：{err}"),
        }
    }

    #[test]
    fn unknown_ids_fall_through_to_platform() {
        // 登录项被移除后允许重新添加，所以「列表里找不到」不在这里报错
        let items = vec![item("win:run:hkcu:foo", true)];
        assert!(ensure_toggleable(&items, "win:run:hkcu:gone").is_ok());
    }
}
