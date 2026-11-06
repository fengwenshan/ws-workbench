//! 「系统清理」的 Tauri 命令层：薄封装 + `CleanerState`。
//!
//! 为什么命令都做成 `async` + `spawn_blocking`：枚举注册表 / 算目录大小 /
//! 跑卸载程序 / 扫内存都是阻塞操作，直接在普通命令里跑会卡住 WebView 的
//! 主线程（前端表现为整页假死）。进度通过 `cleaner-progress` 事件推给前端。
//!
//! 状态只放进程内存：卸载列表缓存与「扫描中」标记（设计 3.3）。前端不持久化
//! 这些数据，重启应用即重新扫描。

use tauri::{AppHandle, Emitter, State};

use crate::cleaner::installer::EVENT_PROGRESS;
use crate::cleaner::models::{
    CleanerProgress, DeleteOutcome, InstalledApp, LeftoverItem, MemorySnapshot, QuitOutcome,
    ReleaseOutcome, UninstallOutcome,
};

pub use crate::cleaner::CleanerState;

/// 从列表缓存里取一条应用信息：卸载/残留扫描在 app id 之外还需要名字等信息，
/// 而 `UninstallString`/bundle 元数据在应用被删掉后就取不到了，只能靠缓存兜底。
fn cached_app(state: &CleanerState, app_id: &str) -> Option<InstalledApp> {
    let cache = state.apps.lock().ok()?;
    cache
        .as_ref()?
        .iter()
        .find(|app| app.id == app_id)
        .cloned()
}

/// 列出已安装应用。`refresh=false` 且有缓存时直接返回；扫描中重复调用报错。
#[tauri::command]
pub async fn cleaner_list_apps(
    app: AppHandle,
    state: State<'_, CleanerState>,
    refresh: bool,
) -> Result<Vec<InstalledApp>, String> {
    if !refresh {
        if let Ok(cache) = state.apps.lock() {
            if let Some(apps) = cache.as_ref() {
                return Ok(apps.clone());
            }
        }
    }

    {
        let mut scanning = state
            .scanning
            .lock()
            .map_err(|_| "扫描状态不可用".to_string())?;
        if *scanning {
            return Err("正在扫描，请稍候".to_string());
        }
        *scanning = true;
    }

    let progress_app = app.clone();
    let joined = tauri::async_runtime::spawn_blocking(move || {
        let emit = move |progress: CleanerProgress| {
            let _ = progress_app.emit(EVENT_PROGRESS, progress);
        };
        crate::cleaner::installer::list_apps(&emit)
    })
    .await;

    // 不管扫描成功失败都要复位标记，否则一次失败之后页面再也刷不出来
    if let Ok(mut scanning) = state.scanning.lock() {
        *scanning = false;
    }
    let apps = joined.map_err(|err| format!("扫描任务失败: {}", err))??;
    if let Ok(mut cache) = state.apps.lock() {
        *cache = Some(apps.clone());
    }
    Ok(apps)
}

/// 卸载指定应用（阻塞到卸载程序退出或 15 分钟超时）
#[tauri::command]
pub async fn cleaner_uninstall_app(
    app: AppHandle,
    state: State<'_, CleanerState>,
    app_id: String,
) -> Result<UninstallOutcome, String> {
    let hint = cached_app(&state, &app_id);
    let progress_app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let emit = move |progress: CleanerProgress| {
            let _ = progress_app.emit(EVENT_PROGRESS, progress);
        };
        crate::cleaner::installer::uninstall(&app_id, hint.as_ref(), &emit)
    })
    .await
    .map_err(|err| format!("卸载任务失败: {}", err))?
}

/// 扫描残留（卸载后调用；也允许卸载前预览）
#[tauri::command]
pub async fn cleaner_scan_leftovers(
    app: AppHandle,
    state: State<'_, CleanerState>,
    app_id: String,
) -> Result<Vec<LeftoverItem>, String> {
    let hint = cached_app(&state, &app_id);
    let progress_app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let emit = move |progress: CleanerProgress| {
            let _ = progress_app.emit(EVENT_PROGRESS, progress);
        };
        crate::cleaner::installer::scan_leftovers(&app_id, hint.as_ref(), &emit)
    })
    .await
    .map_err(|err| format!("残留扫描任务失败: {}", err))?
}

/// 删除勾选的残留（白名单校验后进废纸篓；需提权的合并成一次授权）
#[tauri::command]
pub async fn cleaner_delete_leftovers(
    app: AppHandle,
    state: State<'_, CleanerState>,
    app_id: String,
    paths: Vec<String>,
) -> Result<DeleteOutcome, String> {
    let hint = cached_app(&state, &app_id);
    let progress_app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let emit = move |progress: CleanerProgress| {
            let _ = progress_app.emit(EVENT_PROGRESS, progress);
        };
        crate::cleaner::installer::delete_leftovers(&app_id, hint.as_ref(), &paths, &emit)
    })
    .await
    .map_err(|err| format!("删除任务失败: {}", err))?
}

/// 内存快照（前端按 2 秒轮询）
#[tauri::command]
pub async fn cleaner_memory_snapshot() -> Result<MemorySnapshot, String> {
    tauri::async_runtime::spawn_blocking(crate::cleaner::memory::snapshot)
        .await
        .map_err(|err| format!("内存采样任务失败: {}", err))?
}

/// 结束所选应用进程（优雅退出优先；`force=true` 为第二次确认后的强制结束）
#[tauri::command]
pub async fn cleaner_quit_apps(app_ids: Vec<String>, force: bool) -> Result<QuitOutcome, String> {
    tauri::async_runtime::spawn_blocking(move || crate::cleaner::memory::quit_apps(&app_ids, force))
        .await
        .map_err(|err| format!("结束进程任务失败: {}", err))?
}

/// 深度释放（需要时弹一次系统授权；取消授权返回 ok=false 而不是错误）
#[tauri::command]
pub async fn cleaner_deep_release() -> Result<ReleaseOutcome, String> {
    tauri::async_runtime::spawn_blocking(crate::cleaner::memory::deep_release)
        .await
        .map_err(|err| format!("深度释放任务失败: {}", err))?
}
