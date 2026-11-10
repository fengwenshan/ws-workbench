//! 「系统清理」的 Tauri 命令层：薄封装 + `CleanerState`。
//!
//! 为什么命令都做成 `async` + `spawn_blocking`：枚举注册表 / 算目录大小 /
//! 跑卸载程序都是阻塞操作，直接在普通命令里跑会卡住 WebView 的
//! 主线程（前端表现为整页假死）。进度通过 `cleaner-progress` 事件推给前端。
//!
//! 状态只放进程内存：卸载列表缓存、「扫描中」标记、卸载窗口目标与关联文件
//! 白名单（设计 3.3 / 4.4）。前端不持久化这些数据，重启应用即重新扫描。

use tauri::{AppHandle, Emitter, Manager, State};

use crate::cleaner::installer::{
    EVENT_APPS_CHANGED, EVENT_PROGRESS, EVENT_UNINSTALL_TARGET_CHANGED,
};
use crate::cleaner::models::{
    CleanerProgress, DeleteOutcome, ForceQuitOutcome, InstalledApp, LeftoverItem, RelatedFiles,
    RunningState, UninstallOutcome,
};

pub use crate::cleaner::CleanerState;

/// 卸载窗口 label（`capabilities/default.json` 的 windows 里也要列上）
const UNINSTALL_WINDOW_LABEL: &str = "uninstall";

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

/// 卸载窗口当前目标（列表缓存被清掉后，靠它继续给关联文件扫描提供 bundle 元数据）
fn pending_target(state: &CleanerState, app_id: &str) -> Option<InstalledApp> {
    let guard = state.pending_uninstall.lock().ok()?;
    guard
        .as_ref()
        .filter(|app| app.id == app_id)
        .cloned()
}

/// 列表缓存失效：主窗口收到 `cleaner-apps-changed` 后会重新扫描
fn invalidate_apps_cache(state: &CleanerState) {
    if let Ok(mut cache) = state.apps.lock() {
        *cache = None;
    }
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

/// 检测应用是否仍在运行（只读；卸载前用来决定是否弹二次确认）
#[tauri::command]
pub async fn cleaner_app_running(
    state: State<'_, CleanerState>,
    app_id: String,
) -> Result<RunningState, String> {
    let hint = cached_app(&state, &app_id)
        .ok_or_else(|| "应用信息不存在，请重新扫描列表".to_string())?;
    tauri::async_runtime::spawn_blocking(move || crate::cleaner::running::detect(&hint))
        .await
        .map_err(|err| format!("运行状态检测失败: {}", err))
}

/// 打开（或聚焦）卸载窗口，把目标应用记到 `CleanerState.pending_uninstall`。
///
/// 单例：窗口已存在时 `show + set_focus + set_title`，并广播
/// `cleaner-uninstall-target-changed` 让窗口重新拉目标与清单（设计 4.5）。
#[tauri::command]
pub async fn cleaner_open_uninstall_window(
    app: AppHandle,
    state: State<'_, CleanerState>,
    app_id: String,
) -> Result<(), String> {
    let target = cached_app(&state, &app_id)
        .ok_or_else(|| "应用信息不存在，请重新扫描列表".to_string())?;
    // 命令层兜底：列表过滤之外再拦一次「不能卸载自己」
    if crate::cleaner::installer::is_self_app(&target) {
        return Err("不能卸载正在运行的应用".to_string());
    }
    if let Ok(mut pending) = state.pending_uninstall.lock() {
        *pending = Some(target.clone());
    }
    let title = format!("卸载 {}", target.name);

    if let Some(window) = app.get_webview_window(UNINSTALL_WINDOW_LABEL) {
        let _ = window.show();
        let _ = window.set_focus();
        let _ = window.set_title(&title);
        let _ = app.emit(EVENT_UNINSTALL_TARGET_CHANGED, ());
        return Ok(());
    }

    let mut builder = tauri::WebviewWindowBuilder::new(
        &app,
        UNINSTALL_WINDOW_LABEL,
        tauri::WebviewUrl::App("uninstall.html".into()),
    )
    .title(&title)
    .inner_size(820.0, 560.0)
    .min_inner_size(760.0, 480.0)
    .center()
    .maximizable(false);
    // macOS：隐藏标题、红绿灯浮在内容上（截图 1 的样子）；Windows 用原生标题栏
    #[cfg(target_os = "macos")]
    {
        builder = builder
            .hidden_title(true)
            .title_bar_style(tauri::TitleBarStyle::Overlay);
    }
    let window = builder
        .build()
        .map_err(|err| format!("创建卸载窗口失败: {}", err))?;
    let _ = window.set_focus();
    Ok(())
}

/// 卸载窗口读取当前目标；没有目标时报错（窗口可能在列表刷新后被重新打开）
#[tauri::command]
pub async fn cleaner_uninstall_target(
    state: State<'_, CleanerState>,
) -> Result<InstalledApp, String> {
    let target = state
        .pending_uninstall
        .lock()
        .ok()
        .and_then(|guard| guard.clone());
    target.ok_or_else(|| "没有待卸载的应用".to_string())
}

/// 扫描卸载窗口的三组关联文件，并把本次「存在」的路径写进删除白名单缓存。
#[tauri::command]
pub async fn cleaner_related_files(
    app: AppHandle,
    state: State<'_, CleanerState>,
    app_id: String,
) -> Result<RelatedFiles, String> {
    let hint = cached_app(&state, &app_id)
        .ok_or_else(|| "应用信息不存在，请重新扫描列表".to_string())?;
    if crate::cleaner::installer::is_self_app(&hint) {
        return Err("不能卸载正在运行的应用".to_string());
    }
    let progress_app = app.clone();
    let scan_app_id = app_id.clone();
    let files = tauri::async_runtime::spawn_blocking(move || {
        let emit = move |progress: CleanerProgress| {
            let _ = progress_app.emit(EVENT_PROGRESS, progress);
        };
        crate::cleaner::installer::related_files(&scan_app_id, Some(&hint), &emit)
    })
    .await
    .map_err(|err| format!("关联文件扫描任务失败: {}", err))??;

    // 白名单 = 本次扫描存在的路径（设计 4.4：只允许删这里出现过的路径）
    let paths: Vec<String> = files
        .groups
        .iter()
        .flat_map(|group| group.items.iter())
        .filter(|item| item.exists)
        .map(|item| item.path.clone())
        .collect();
    if let Ok(mut guard) = state.related_paths.lock() {
        *guard = Some((app_id, paths));
    }
    Ok(files)
}

/// 强制退出应用的全部相关进程（先正常退出，超时强杀并轮询到归零）
#[tauri::command]
pub async fn cleaner_force_quit(
    app: AppHandle,
    state: State<'_, CleanerState>,
    app_id: String,
) -> Result<ForceQuitOutcome, String> {
    let hint = cached_app(&state, &app_id)
        .ok_or_else(|| "应用信息不存在，请重新扫描列表".to_string())?;
    if crate::cleaner::installer::is_self_app(&hint) {
        return Err("不能卸载正在运行的应用".to_string());
    }
    let progress_app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let emit = move |progress: CleanerProgress| {
            let _ = progress_app.emit(EVENT_PROGRESS, progress);
        };
        crate::cleaner::running::force_quit(&hint, &emit)
    })
    .await
    .map_err(|err| format!("强制退出任务失败: {}", err))?
}

/// 删除卸载窗口勾选的关联文件。
///
/// 只接受 `cleaner_related_files` 缓存过的路径（app_id 必须匹配）；已不存在的
/// 路径记 skipped（卸载程序可能已经删了）。删除后列表缓存失效并广播刷新。
#[tauri::command]
pub async fn cleaner_delete_related(
    app: AppHandle,
    state: State<'_, CleanerState>,
    app_id: String,
    paths: Vec<String>,
) -> Result<DeleteOutcome, String> {
    // 列表缓存可能已被 `cleaner_uninstall_app` 清掉，用窗口目标兜底
    let hint = cached_app(&state, &app_id).or_else(|| pending_target(&state, &app_id));
    if let Some(target) = hint.as_ref() {
        if crate::cleaner::installer::is_self_app(target) {
            return Err("不能卸载正在运行的应用".to_string());
        }
    }
    let allowed = {
        let guard = state
            .related_paths
            .lock()
            .map_err(|_| "清理状态不可用".to_string())?;
        match guard.as_ref() {
            Some((scanned_app_id, scanned_paths))
                if scanned_app_id == &app_id && !scanned_paths.is_empty() =>
            {
                scanned_paths.clone()
            }
            _ => return Err("清单已过期，请重新扫描".to_string()),
        }
    };

    let progress_app = app.clone();
    let delete_app_id = app_id.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        let emit = move |progress: CleanerProgress| {
            let _ = progress_app.emit(EVENT_PROGRESS, progress);
        };
        crate::cleaner::installer::delete_related(
            &delete_app_id,
            hint.as_ref(),
            &allowed,
            &paths,
            &emit,
        )
    })
    .await
    .map_err(|err| format!("删除任务失败: {}", err))??;

    // 删除后列表缓存失效：主窗口收到事件后 loadApps(true)。
    // 只有全部成功才清掉「窗口目标 + 白名单」，部分失败时保留，允许用户重试
    // （清掉后重试会直接报「清单已过期」，窗口里的重试按钮就没用了）。
    invalidate_apps_cache(&state);
    if outcome.failed.is_empty() {
        if let Ok(mut pending) = state.pending_uninstall.lock() {
            *pending = None;
        }
        if let Ok(mut guard) = state.related_paths.lock() {
            *guard = None;
        }
    }
    let _ = app.emit(EVENT_APPS_CHANGED, ());
    Ok(outcome)
}

/// 卸载指定应用（阻塞到卸载程序退出或 15 分钟超时）。
///
/// `quit_running=true`：先优雅退出应用，退不掉直接返回 `ok=false`，不执行卸载。
/// 新流程传 `quit_running=false`（退出已由 `cleaner_force_quit` 单独完成）。
#[tauri::command]
pub async fn cleaner_uninstall_app(
    app: AppHandle,
    state: State<'_, CleanerState>,
    app_id: String,
    quit_running: bool,
) -> Result<UninstallOutcome, String> {
    let hint = cached_app(&state, &app_id);
    if let Some(target) = hint.as_ref() {
        if crate::cleaner::installer::is_self_app(target) {
            return Err("不能卸载正在运行的应用".to_string());
        }
    }
    let progress_app = app.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        let emit = move |progress: CleanerProgress| {
            let _ = progress_app.emit(EVENT_PROGRESS, progress);
        };
        crate::cleaner::installer::uninstall(&app_id, hint.as_ref(), quit_running, &emit)
    })
    .await
    .map_err(|err| format!("卸载任务失败: {}", err))??;

    if outcome.ok {
        // 列表里已经没有这个应用了：清缓存让主窗口重新扫描
        invalidate_apps_cache(&state);
        let _ = app.emit(EVENT_APPS_CHANGED, ());
        // 注意：这里**不**清 pending_uninstall —— 卸载窗口紧接着要调
        // `cleaner_delete_related`，关联文件重扫还需要目标应用的 bundle 元数据；
        // 由 `cleaner_delete_related` 成功后清理。
    }
    Ok(outcome)
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
    // 会动文件的命令都要先拦「自己」：即使旧入口已不再被主面板使用
    if let Some(target) = hint.as_ref() {
        if crate::cleaner::installer::is_self_app(target) {
            return Err("不能卸载正在运行的应用".to_string());
        }
    }
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
