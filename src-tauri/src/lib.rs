// Windows 的 MSVC 链接器会输出创建 .lib/.exp 的信息，属于正常链接产物，
// 不是代码问题；这里按 rustc 的建议把它们当作信息性输出忽略。
#![allow(linker_messages)]

mod models;
mod config;
mod db;
mod crypto;
mod filecrypto;
mod util;
mod commands;
mod cleaner;
/// 剪贴板历史（macOS）
mod clipboard;
/// Hosts 在线编辑（macOS + Windows）
mod hosts;
mod packer;
pub mod apps;
mod platform;
mod netinfo;
mod process;
mod winps;
#[cfg(test)]
pub(crate) mod test_support;
/// 更新包断点续传下载 + 安装
mod updates;
/// 安装包卸载程序读取的路径清单
mod uninstall_info;
pub mod vpn;

use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, Runtime, WindowEvent,
};

/// 应用基础名（发布版显示名）
pub(crate) const APP_BASE_NAME: &str = "ws-workbench";

/// 是否开发构建：`pnpm exec tauri dev` 和本地 debug 二进制都是 true。
pub(crate) fn is_dev_build() -> bool {
    cfg!(debug_assertions)
}

/// 当前构建的应用显示名：开发版拼 `-dev`，和安装版同机同时运行时能一眼区分。
pub(crate) fn app_display_name() -> String {
    if is_dev_build() {
        format!("{APP_BASE_NAME}-dev")
    } else {
        APP_BASE_NAME.to_string()
    }
}

/// 需要按构建区分开的目录后缀：发布版空串，开发版 `-dev`。
///
/// 主要给 WebView2 用户数据目录用：开发版和安装版各用各的，避免两个进程
/// 锁同一个用户数据目录。
fn build_dir_suffix() -> &'static str {
    if is_dev_build() {
        "-dev"
    } else {
        ""
    }
}

/// 托盘快捷面板的窗口 label（前端 `/quick.html`）
pub const QUICK_PANEL_LABEL: &str = "quick-panel";
/// 面板的逻辑尺寸，和 `QuickPanel.vue` 的布局保持一致。
///
/// 高度只是启动时的默认值：面板挂载后前端会按当前页签的内容调
/// `quick_panel_resize`，VPN / 休眠两个页签各自贴合内容，不留大片空白。
const QUICK_PANEL_WIDTH: f64 = 360.0;
const QUICK_PANEL_HEIGHT: f64 = 320.0;
/// 自适应高度的上下限，避免内容异常时窗口被算得过小或铺满屏幕。
const QUICK_PANEL_MIN_HEIGHT: f64 = 160.0;
const QUICK_PANEL_MAX_HEIGHT: f64 = 480.0;

/// Windows 上只允许一个 GUI 实例。
///
/// 便携包很容易被重复双击：每个实例都会建自己的托盘图标和 worker，用户点
/// 其中一个托盘的“退出”后，另一个实例仍然握着 exe/worker/openconnect，
/// 解压目录就会报“文件已在另一程序打开”。用命名 mutex 做单实例，第二个
/// 进程直接把已有窗口叫到前台后退出。worker 在 `main.rs` 里已经提前分流，
/// 不会走到这里，所以不会误伤提权 worker。
#[cfg(target_os = "windows")]
fn ensure_single_instance() -> bool {
    use std::ffi::c_void;

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateMutexW(
            attributes: *mut c_void,
            initial_owner: i32,
            name: *const u16,
        ) -> *mut c_void;
        fn FindWindowW(class_name: *const u16, window_name: *const u16) -> *mut c_void;
        fn ShowWindow(window: *mut c_void, command: i32) -> i32;
        fn SetForegroundWindow(window: *mut c_void) -> i32;
    }

    const ERROR_ALREADY_EXISTS: i32 = 183;
    const SW_RESTORE: i32 = 9;

    let name: Vec<u16> = format!("Local\\{}-single-instance\0", app_display_name())
        .encode_utf16()
        .collect();
    let handle = unsafe { CreateMutexW(std::ptr::null_mut(), 0, name.as_ptr()) };
    if handle.is_null() {
        // 创建 mutex 都失败时不要阻止用户启动；单实例只是防重复，
        // 不应该变成“应用打不开”的新问题。
        return true;
    }

    if std::io::Error::last_os_error().raw_os_error() == Some(ERROR_ALREADY_EXISTS) {
        // 已有实例：尽量把它从托盘/最小化状态叫出来，然后第二个进程退出。
        let title: Vec<u16> = format!("{}\0", app_display_name()).encode_utf16().collect();
        let window = unsafe { FindWindowW(std::ptr::null(), title.as_ptr()) };
        if !window.is_null() {
            unsafe {
                ShowWindow(window, SW_RESTORE);
                SetForegroundWindow(window);
            }
        }
        return false;
    }

    // 句柄故意不关闭：它要活到进程结束，命名 mutex 才会一直存在。
    let _ = handle;
    true
}

// 把主窗口重新显示出来（托盘菜单、Dock 图标、快捷面板里的「打开主窗口」进入）
pub(crate) fn show_main_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

// 托盘图标：macOS 用模板图（黑 + 透明），系统会按菜单栏明暗自动反色；
// Windows 直接用应用图标（彩色），白色线条图在浅色任务栏上会看不清。
//
// 开发版整套换成带 dev 标识的变体。macOS 菜单栏这里刻意沿用发布版那张模板图
// （系统按菜单栏明暗染色，开发版的颜色和发布版完全一样），只在箱体正中镂空一个
// 透明小圆点——镂空处透出菜单栏背景，不引入新颜色，18pt 见方、不比发布版宽。
// Windows / Linux 托盘用右下角带 dev 角标的彩色应用图标（浅色任务栏上也认得出）。
fn tray_icon_image() -> Option<tauri::image::Image<'static>> {
    let dev = is_dev_build();
    let bytes: &'static [u8] = if cfg!(target_os = "macos") {
        if dev {
            // 开发版：模板图 + 正中镂空点，仍交给系统着色
            include_bytes!("../icons/tray/tray-dev-template-36.png")
        } else {
            include_bytes!("../icons/tray/tray-template-36.png")
        }
    } else if cfg!(target_os = "windows") {
        if dev {
            include_bytes!("../icons/dev/32x32-dev.png")
        } else {
            include_bytes!("../icons/32x32.png")
        }
    } else if dev {
        // Linux 托盘：带 dev 角标的彩色应用图标
        include_bytes!("../icons/dev/32x32-dev.png")
    } else {
        include_bytes!("../icons/tray/tray-white-48.png")
    };
    tauri::image::Image::from_bytes(bytes).ok()
}

// macOS 的 Dock 图标来自 .app 包里的 icns；`tauri dev` 跑的是裸二进制，
// 没有 bundle，Dock 会退回系统那个黑底「exec」占位图标。
// 启动时显式设一次 applicationIconImage，dev 和安装包的观感就一致了。
// 开发版这里换成右下角带 dev 角标的图：Dock 里和生产版一眼分开，
// 而不是只靠磁贴上的小字标题。
#[cfg(target_os = "macos")]
fn set_dock_icon() {
    use objc2::{AllocAnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let bytes: &'static [u8] = if is_dev_build() {
        include_bytes!("../icons/dev/icon-dev.png")
    } else {
        include_bytes!("../icons/icon-source.png")
    };
    let data = NSData::with_bytes(bytes);
    let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    // SAFETY: setup 在主线程执行；NSApplication 只允许主线程访问
    unsafe { app.setApplicationIconImage(Some(&image)) };
}

// 开发版把主窗口图标也换成带角标的那张：Windows 的任务栏 / Alt-Tab 读的是
// 窗口图标，不会跟着托盘图标走；macOS 上 set_icon 是 no-op，Dock 由
// set_dock_icon 负责，所以这里不用按平台分支。
fn set_dev_window_icon<R: Runtime>(app: &AppHandle<R>) {
    if !is_dev_build() {
        return;
    }
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    if let Ok(icon) =
        tauri::image::Image::from_bytes(include_bytes!("../icons/dev/128x128@2x-dev.png"))
    {
        let _ = window.set_icon(icon);
    }
}

// macOS 顶部菜单栏汉化（Tauri 默认菜单是英文的）
#[cfg(target_os = "macos")]
fn setup_app_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    use tauri::menu::{Menu, PredefinedMenuItem, Submenu};

    let name = app_display_name();

    let app_menu = Submenu::with_items(
        app,
        &name,
        true,
        &[
            &PredefinedMenuItem::about(app, Some(&format!("关于 {name}")), None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::services(app, Some("服务"))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, Some(&format!("隐藏 {name}")))?,
            &PredefinedMenuItem::hide_others(app, Some("隐藏其他"))?,
            &PredefinedMenuItem::show_all(app, Some("显示全部"))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::quit(app, Some(&format!("退出 {name}")))?,
        ],
    )?;

    // 编辑菜单：macOS 的 ⌘X/⌘C/⌘V/⌘A 依赖这些标准菜单项派发到第一响应者，
    // 自定义菜单时必须补回，否则输入框里无法剪切/复制/粘贴/全选。
    let edit_menu = Submenu::with_items(
        app,
        "编辑",
        true,
        &[
            &PredefinedMenuItem::undo(app, Some("撤销"))?,
            &PredefinedMenuItem::redo(app, Some("重做"))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, Some("剪切"))?,
            &PredefinedMenuItem::copy(app, Some("拷贝"))?,
            &PredefinedMenuItem::paste(app, Some("粘贴"))?,
            &PredefinedMenuItem::select_all(app, Some("全选"))?,
        ],
    )?;

    let window_menu = Submenu::with_items(
        app,
        "窗口",
        true,
        &[
            &PredefinedMenuItem::minimize(app, Some("最小化"))?,
            &PredefinedMenuItem::maximize(app, Some("缩放"))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::bring_all_to_front(app, Some("前置全部窗口"))?,
        ],
    )?;
    // 让系统把「窗口」当成标准窗口菜单（由系统补上窗口列表与平铺项）
    window_menu.set_as_windows_menu_for_nsapp()?;

    app.set_menu(Menu::with_items(app, &[&app_menu, &edit_menu, &window_menu])?)?;
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn setup_app_menu<R: Runtime>(_app: &AppHandle<R>) -> tauri::Result<()> {
    Ok(())
}

/// 选一个能真正建出来的 WebView2 用户数据目录。
///
/// 不要用默认目录（`%LOCALAPPDATA%\EBWebView`）：其它 WebView2 应用会共用，
/// 浏览器版本或锁不一致时创建环境会报「拒绝访问」。优先应用数据目录；在受限
/// 环境（受控文件夹访问、企业策略、沙箱）下退到可执行文件旁，最后才是临时目录。
fn webview_data_dir(app: &tauri::App) -> std::path::PathBuf {
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(base) = app.path().app_local_data_dir() {
        candidates.push(base.join(format!("webview{}", build_dir_suffix())));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push(parent.join(format!("webview-data{}", build_dir_suffix())));
        }
    }
    candidates.push(
        std::env::temp_dir().join(format!("{}-webview{}", APP_BASE_NAME, build_dir_suffix())),
    );

    for dir in &candidates {
        if std::fs::create_dir_all(dir).is_ok() {
            return dir.clone();
        }
    }
    // 全都建不出来时仍返回首选路径，让 WebView2 自己报出更具体的错误。
    candidates.remove(0)
}

/// 创建主窗口。
///
/// 不用 Tauri 的自动创建（tauri.conf.json 里 `create: false`），是为了能在原生层
/// 挂 `on_navigation`：把文件从 Finder 拖进窗口时，WebView 默认会直接导航到
/// file:// 去显示那个文件，整个应用界面被顶替（现场坏掉就是这么来的）。
/// 这里直接拒绝 file:// 导航，页面永远不会被拖进来的文件换掉。
fn create_main_window(app: &mut tauri::App) -> tauri::Result<()> {
    let config = app
        .config()
        .app
        .windows
        .iter()
        .find(|window| window.label == "main")
        .cloned()
        .expect("tauri.conf.json 里缺少 main 窗口配置");

    let builder = tauri::WebviewWindowBuilder::from_config(app.handle(), &config)?;
    // Windows 上隐藏系统标题栏，改由前端画标题栏和最小化/最大化/关闭按钮；
    // macOS 继续用 Overlay 的原生红绿灯，Linux 也保留系统边框。
    #[cfg(target_os = "windows")]
    let builder = builder.decorations(false);
    let window = builder
        .data_directory(webview_data_dir(app))
        .on_navigation(|url| {
            // 只拦本地文件导航；dev 的 http://localhost 和打包后的 tauri:// 都放行
            url.scheme() != "file"
        })
        .build()?;
    // 开发版的窗口标题也带 -dev：Windows 单实例靠窗口标题找已有实例，
    // 任务栏/窗口列表里也能一眼看出是开发版
    #[cfg(debug_assertions)]
    let _ = window.set_title(&app_display_name());
    #[cfg(not(debug_assertions))]
    let _ = window;
    Ok(())
}

/// 创建托盘快捷面板：柠檬式无边框小窗，启动时建好但保持隐藏。
///
/// 提前创建而不是每次点击现建，是为了第一次点托盘图标就能立刻出内容；
/// 窗体的显隐/定位在 `toggle_quick_panel` 里处理。
fn create_quick_panel(app: &mut tauri::App) -> tauri::Result<()> {
    let window = tauri::WebviewWindowBuilder::new(
        app.handle(),
        QUICK_PANEL_LABEL,
        tauri::WebviewUrl::App("quick.html".into()),
    )
    .title(app_display_name())
    // 和主窗口共用同一个可写的 WebView2 数据目录；不设时 WebView2 会退回
    // `%LOCALAPPDATA%\com.ws.workbench`，在受限环境 / 受控文件夹访问下会
    // 直接以「拒绝访问」让整个 setup hook 失败。
    .data_directory(webview_data_dir(app))
    .inner_size(QUICK_PANEL_WIDTH, QUICK_PANEL_HEIGHT)
    .resizable(false)
    .maximizable(false)
    .minimizable(false)
    .decorations(false)
    // 透明窗口 + CSS 圆角/描边，才能做出柠檬那种悬浮卡片的样子
    .transparent(true)
    .shadow(false)
    // macOS：第一次点击就落到面板上，不用先激活应用再点第二下
    .accept_first_mouse(true)
    .skip_taskbar(true)
    .always_on_top(true)
    .visible(false)
    .build()?;
    // dev 下把面板窗口的实际地址/尺寸打出来，方便确认 quick.html 是否被正确加载
    #[cfg(debug_assertions)]
    eprintln!(
        "[quick-panel] created url={:?} size={:?} visible={:?}",
        window.url().map(|url| url.to_string()),
        window.outer_size(),
        window.is_visible()
    );
    // release 下不打日志，显式消费掉绑定，避免 unused_variables
    #[cfg(not(debug_assertions))]
    let _ = window;
    Ok(())
}

/// 创建剪贴板历史面板：无边框透明浮层，启动时建好、默认隐藏。
///
/// macOS / Windows 都创建；Linux 不启用。
/// 显示位置 / 焦点在 `clipboard::show_panel` 里处理。
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn create_clipboard_panel(app: &mut tauri::App) -> tauri::Result<()> {
    let window = tauri::WebviewWindowBuilder::new(
        app.handle(),
        clipboard::PANEL_LABEL,
        tauri::WebviewUrl::App("clipboard.html".into()),
    )
    .title(app_display_name())
    // 同上：剪贴板浮层也必须走可写的数据目录，否则第二个 WebView 创建失败
    .data_directory(webview_data_dir(app))
    .inner_size(clipboard::PANEL_WIDTH, clipboard::PANEL_HEIGHT)
    .resizable(false)
    .maximizable(false)
    .minimizable(false)
    .decorations(false)
    // 透明窗口 + CSS 圆角/投影，和托盘快捷面板保持同一套观感
    .transparent(true)
    .shadow(false)
    .accept_first_mouse(true)
    .skip_taskbar(true)
    .always_on_top(true)
    .visible(false)
    .build()?;
    #[cfg(debug_assertions)]
    eprintln!(
        "[clipboard-panel] created url={:?} size={:?} visible={:?}",
        window.url().map(|url| url.to_string()),
        window.outer_size(),
        window.is_visible()
    );
    // release 下不打日志，显式消费掉绑定，避免 unused_variables
    #[cfg(not(debug_assertions))]
    let _ = window;
    Ok(())
}

/// 面板最近一次因失焦被收起的时间（unix 毫秒）。
///
/// 点托盘图标关闭面板时，macOS 可能先发失焦收起、再发点击事件；没有这个时间戳，
/// 点击事件会看到「已隐藏」又立刻把面板弹回来，看起来像关不掉。
static QUICK_PANEL_BLUR_HIDDEN_AT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 面板收起后 250ms 内的托盘点击视为「关闭动作的余波」，不再重新弹出
const QUICK_PANEL_TOGGLE_GRACE_MS: u64 = 250;

fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// 左键点托盘图标：已显示就收起，否则贴着图标弹出。
fn toggle_quick_panel<R: Runtime>(app: &AppHandle<R>, rect: tauri::Rect) {
    let Some(window) = app.get_webview_window(QUICK_PANEL_LABEL) else {
        return;
    };
    if window.is_visible().unwrap_or(false) {
        let _ = window.hide();
        return;
    }
    // 刚被失焦收起：这次点击就是刚才那下，不要再弹出来
    let hidden_at = QUICK_PANEL_BLUR_HIDDEN_AT.load(std::sync::atomic::Ordering::Relaxed);
    if hidden_at != 0 && unix_millis().saturating_sub(hidden_at) < QUICK_PANEL_TOGGLE_GRACE_MS {
        return;
    }

    let Some(placement) = quick_panel_placement(app, &window, rect)
        .or_else(|| fallback_placement(app, &window, rect))
    else {
        // 连兜底都拿不到屏幕信息：保持窗口原位置直接弹，至少不会卡住交互
        let _ = window.show();
        let _ = window.set_focus();
        return;
    };
    let (x, y) = panel_origin_at(placement.anchor, placement.panel, placement.screen);
    // 打开状态下前端会按当前页签内容改高度，记下锚点供重新定位
    if let Ok(mut anchor) = QUICK_PANEL_ANCHOR.lock() {
        *anchor = Some(placement);
    }

    #[cfg(debug_assertions)]
    eprintln!(
        "[quick-panel] logical={} anchor={:?} screen={:?} panel={:?} origin=({x},{y})",
        placement.logical, placement.anchor, placement.screen, placement.panel
    );

    if placement.logical {
        // macOS：x/y 是 AppKit 逻辑点，交给 tao 按窗口所在屏的缩放比换算
        let _ = window.set_position(tauri::LogicalPosition::new(x as f64, y as f64));
    } else {
        let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
    }
    let _ = window.show();
    let _ = window.set_focus();
}

/// 面板落点：锚点 + 显示器范围 + 面板尺寸，三者单位一致（由 `logical` 说明）。
#[derive(Clone, Copy)]
struct PanelPlacement {
    anchor: (i32, i32),
    screen: (i32, i32, i32, i32),
    panel: (u32, u32),
    /// `true` = AppKit 逻辑点（macOS），`false` = 物理像素（Windows / 兜底路径）。
    logical: bool,
}

/// 面板最近一次弹出时的锚点（托盘图标中心 + 所在显示器范围）。
///
/// 面板打开状态下切换页签会改高度，需要按同一锚点重新计算位置：菜单栏下方
/// 顶边不动、任务栏上方底边不动，否则窗口会顺着高度变化往上/往下长歪。
static QUICK_PANEL_ANCHOR: std::sync::Mutex<Option<PanelPlacement>> =
    std::sync::Mutex::new(None);

impl PanelPlacement {
    /// 面板是否朝下展开（锚点在屏幕上半部分：macOS 菜单栏 / 顶部任务栏）。
    ///
    /// 朝下时改高度只要让窗口从顶边往下长即可，不需要重算位置；朝上（底部
    /// 任务栏）才需要按锚点重新定位，把底边固定在任务栏上方。
    fn expands_below(&self) -> bool {
        self.anchor.1 < self.screen.1 + self.screen.3 / 2
    }
}

/// 面板当前的实际逻辑尺寸。
///
/// 高度会被前端改成当前页签内容的高度，所以定位不能用启动时的常量。
#[cfg(target_os = "macos")]
fn quick_panel_logical_size<R: Runtime>(
    window: &tauri::WebviewWindow<R>,
) -> Option<(u32, u32)> {
    let scale = window.scale_factor().ok()?;
    let size = window.inner_size().ok()?;
    Some((
        (size.width as f64 / scale).round() as u32,
        (size.height as f64 / scale).round() as u32,
    ))
}

/// macOS：直接用 AppKit / CoreGraphics 取「逻辑点」坐标。
///
/// 为什么不用 `AppHandle::cursor_position()` 加 `Monitor`：tao 0.35 在 macOS 上
/// - `cursor_position()` 把 `CGDisplay::main().pixels_high()`（物理像素高）和
///   `NSEvent::mouseLocation()`（逻辑点、主屏左下原点）混算再乘缩放比；
/// - `Monitor::position/size` 把已经是逻辑点的 `CGDisplayBounds` 又乘了一遍
///   backingScaleFactor，Retina 上等于放大 2 倍。
///
/// 结果就是锚点/屏幕范围全混了单位：Retina 上算出来的 x 会落在面板左边很远，
/// 判断「图标在屏幕右半边」也会失效（屏幕宽度被算成两倍）。这里统一用逻辑点：
/// `NSEvent::mouseLocation()` 取光标，`CGDisplayBounds`（同样是逻辑点、主屏左上
/// 原点）找显示器范围，最后用 `set_position(LogicalPosition)`。
#[cfg(target_os = "macos")]
fn quick_panel_placement<R: Runtime>(
    _app: &AppHandle<R>,
    window: &tauri::WebviewWindow<R>,
    _rect: tauri::Rect,
) -> Option<PanelPlacement> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSEvent, NSScreen};

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGPoint {
        x: f64,
        y: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGSize {
        width: f64,
        height: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGRect {
        origin: CGPoint,
        size: CGSize,
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGMainDisplayID() -> u32;
        fn CGDisplayBounds(display: u32) -> CGRect;
        fn CGGetActiveDisplayList(max_displays: u32, displays: *mut u32, count: *mut u32) -> i32;
    }

    // AppKit 的鼠标位置只能主线程读；托盘点击回调本来就在主线程
    let mtm = MainThreadMarker::new()?;

    // 菜单栏高度：frame 减掉 visibleFrame（后者不含菜单栏/程序坞）
    let menu_bar_height = NSScreen::mainScreen(mtm).map_or(0.0, |screen| {
        let frame = screen.frame();
        let visible = screen.visibleFrame();
        (frame.size.height - (visible.origin.y + visible.size.height)).max(0.0)
    });

    let mouse = NSEvent::mouseLocation();
    let primary = unsafe { CGDisplayBounds(CGMainDisplayID()) };
    // AppKit 的 y 以主屏底边为原点，翻成左上原点后与 CGDisplayBounds 同一套坐标
    let cursor = (
        mouse.x.round() as i32,
        (primary.origin.y + primary.size.height - mouse.y).round() as i32,
    );
    // 托盘图标在菜单栏里：锚点压到菜单栏下沿，面板顶边才不会钻到菜单栏下面
    let anchor = (
        cursor.0,
        cursor.1.max(menu_bar_height.round() as i32),
    );

    let mut ids = [0u32; 16];
    let mut count = 0u32;
    let status = unsafe { CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut count) };
    if status != 0 {
        return None;
    }
    let screen = ids[..count as usize].iter().find_map(|display| {
        let bounds = unsafe { CGDisplayBounds(*display) };
        let (x, y) = (
            bounds.origin.x.round() as i32,
            bounds.origin.y.round() as i32,
        );
        let (w, h) = (
            bounds.size.width.round() as i32,
            bounds.size.height.round() as i32,
        );
        (anchor.0 >= x && anchor.0 < x + w && anchor.1 >= y && anchor.1 < y + h)
            .then_some((x, y, w, h))
    })?;

    Some(PanelPlacement {
        anchor,
        screen,
        panel: quick_panel_logical_size(window)
            .unwrap_or((QUICK_PANEL_WIDTH as u32, QUICK_PANEL_HEIGHT as u32)),
        logical: true,
    })
}

/// Windows：托盘 rect / 光标 / 显示器都已经是物理像素，直接用。
#[cfg(target_os = "windows")]
fn quick_panel_placement<R: Runtime>(
    app: &AppHandle<R>,
    window: &tauri::WebviewWindow<R>,
    rect: tauri::Rect,
) -> Option<PanelPlacement> {
    let scale = window.scale_factor().ok()?;
    let panel = window.outer_size().ok()?;
    // 光标一定落在图标上；拿不到光标时退回托盘 rect 的中心
    let tray_pos = rect.position.to_physical::<i32>(scale);
    let tray_size = rect.size.to_physical::<u32>(scale);
    let anchor = app.cursor_position().ok().map_or(
        (
            tray_pos.x + tray_size.width as i32 / 2,
            tray_pos.y + tray_size.height as i32 / 2,
        ),
        |cursor| (cursor.x as i32, cursor.y as i32),
    );
    let monitor = app
        .monitor_from_point(anchor.0 as f64, anchor.1 as f64)
        .ok()
        .flatten();
    let screen = monitor
        .map(|monitor| {
            let position = monitor.position();
            let size = monitor.size();
            (
                position.x,
                position.y,
                size.width as i32,
                size.height as i32,
            )
        })
        .unwrap_or((0, 0, 1920, 1080));
    Some(PanelPlacement {
        anchor,
        screen,
        panel: (panel.width, panel.height),
        logical: false,
    })
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn quick_panel_placement<R: Runtime>(
    _app: &AppHandle<R>,
    _window: &tauri::WebviewWindow<R>,
    _rect: tauri::Rect,
) -> Option<PanelPlacement> {
    None
}

/// 兜底：平台坐标拿不到时退回 Tauri 的物理像素（Retina 上可能偏，但不至于不弹）。
fn fallback_placement<R: Runtime>(
    app: &AppHandle<R>,
    window: &tauri::WebviewWindow<R>,
    rect: tauri::Rect,
) -> Option<PanelPlacement> {
    let scale = window.scale_factor().ok()?;
    let panel = window.outer_size().unwrap_or_else(|_| {
        tauri::PhysicalSize::new(
            (QUICK_PANEL_WIDTH * scale).round() as u32,
            (QUICK_PANEL_HEIGHT * scale).round() as u32,
        )
    });
    let tray_pos = rect.position.to_physical::<i32>(scale);
    let tray_size = rect.size.to_physical::<u32>(scale);
    let anchor = app.cursor_position().ok().map_or(
        (
            tray_pos.x + tray_size.width as i32 / 2,
            tray_pos.y + tray_size.height as i32 / 2,
        ),
        |cursor| (cursor.x as i32, cursor.y as i32),
    );
    let monitor = app
        .monitor_from_point(anchor.0 as f64, anchor.1 as f64)
        .ok()
        .flatten();
    let screen = monitor
        .map(|monitor| {
            let position = monitor.position();
            let size = monitor.size();
            (
                position.x,
                position.y,
                size.width as i32,
                size.height as i32,
            )
        })
        .unwrap_or((0, 0, 1920, 1080));
    Some(PanelPlacement {
        anchor,
        screen,
        panel: (panel.width, panel.height),
        logical: false,
    })
}

/// 面板与图标之间的间隙（与 `panel_origin_at` 的坐标单位一致：逻辑点或物理像素）
const QUICK_PANEL_GAP: i32 = 8;

/// 计算面板左上角（单位由调用方决定：macOS 是逻辑点，Windows 是物理像素）。
///
/// 纯函数，单测覆盖菜单栏/底部任务栏、左右边缘夹取、多显示器负坐标、超小屏幕：
/// - 锚点（图标中心）在屏幕上半部分（macOS 菜单栏 / 顶部任务栏）时面板朝下，否则朝上；
/// - 水平方向「居中于图标」（对齐 Lemon 那种托盘面板：图标悬在面板上方中间），
///   靠边时再夹进显示器范围，所以贴右边缘时会退化成右对齐；
/// - 最后把结果夹进显示器范围，避免越界或被任务栏盖住。
fn panel_origin_at(
    anchor: (i32, i32),
    panel: (u32, u32),
    screen: (i32, i32, i32, i32),
) -> (i32, i32) {
    const MARGIN: i32 = 8;
    let (anchor_x, anchor_y) = anchor;
    let (panel_w, panel_h) = (panel.0 as i32, panel.1 as i32);
    let (screen_x, screen_y, screen_w, screen_h) = screen;

    let below = anchor_y < screen_y + screen_h / 2;
    let mut x = anchor_x - panel_w / 2;
    let mut y = if below {
        anchor_y + QUICK_PANEL_GAP
    } else {
        anchor_y - panel_h - QUICK_PANEL_GAP
    };

    // 屏幕比面板还小时 clamp 会 panic，先算出不会越界上限再夹
    let max_x = (screen_x + screen_w - panel_w - MARGIN).max(screen_x + MARGIN);
    x = x.min(max_x).max(screen_x + MARGIN);
    let max_y = (screen_y + screen_h - panel_h - MARGIN).max(screen_y + MARGIN);
    y = y.min(max_y).max(screen_y + MARGIN);
    (x, y)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(target_os = "windows")]
    if !ensure_single_instance() {
        return;
    }

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        // 剪贴板历史：官方剪贴板读写插件 + 全局快捷键插件（Command+Shift+V）
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    // 只处理按下，松开不再触发；面板已显示时再按也不收起（用户会连按确认复制成功）
                    if event.state() == tauri_plugin_global_shortcut::ShortcutState::Pressed {
                        clipboard::on_hotkey(app);
                    }
                })
                .build(),
        )
        .register_asynchronous_uri_scheme_protocol("wst-img", |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            let uri = request.uri().clone();
            std::thread::spawn(move || {
                let response = commands::serve_note_image(&app, &uri);
                responder.respond(response);
            });
        })
        .register_asynchronous_uri_scheme_protocol("wsclip", |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            let uri = request.uri().clone();
            std::thread::spawn(move || {
                let response = commands::serve_clipboard_image(&app, &uri);
                responder.respond(response);
            });
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_config,
            commands::save_config,
            commands::get_config_path,
            commands::storage_info,
            commands::vault_encrypt,
            commands::vault_decrypt,
            commands::get_default_include_rules,
            commands::pack_project,
            commands::pack_to_zip,
            commands::validate_project,
            commands::detect_project_type,
            commands::detect_vcs,
            commands::open_dir,
            commands::open_file,
            commands::list_open_with_apps,
            commands::refresh_open_with_apps,
            commands::open_with,
            commands::deepseek_plugin_status,
            commands::deepseek_plugin_update,
            commands::open_deepseek_workspace,
            commands::open_deepseek_app,
            commands::resource_list,
            commands::resource_install,
            commands::resource_uninstall,
            commands::check_dir,
            commands::app_name,
            commands::dev_build,
            commands::get_platform_info,
            commands::get_network_info,
            commands::resolve_upload_dir,
            commands::ensure_dir,
            commands::encrypt_upload_file,
            commands::decrypt_export_file,
            commands::remove_attachment_file,
            commands::stat_attachment,
            commands::encrypt_upload_bytes,
            vpn::commands::vpn_detect_connectors,
            vpn::commands::vpn_import_openconnect,
            vpn::commands::vpn_remove_managed_openconnect,
            vpn::commands::vpn_install_guide,
            vpn::commands::vpn_validate_rule,
            vpn::commands::vpn_preview_rules,
            vpn::commands::vpn_elevation_info,
            vpn::commands::vpn_setup_elevation,
            vpn::commands::vpn_remove_elevation,
            vpn::commands::vpn_prepare_update,
            vpn::commands::vpn_connect,
            vpn::commands::vpn_set_proxy_mode,
            vpn::commands::vpn_push_rules,
            vpn::commands::vpn_conn_identities,
            vpn::commands::vpn_switch_profile,
            vpn::commands::vpn_disconnect,
            vpn::commands::vpn_get_snapshot,
            vpn::commands::vpn_read_logs,
            vpn::commands::vpn_clear_logs,
            vpn::commands::vpn_open_session_dir,
            vpn::commands::vpn_session_dir_info,
            // 系统清理（设计 3.2）：列表/卸载/残留/内存 七个命令
            commands::cleaner_list_apps,
            commands::cleaner_app_running,
            commands::cleaner_uninstall_app,
            commands::cleaner_scan_leftovers,
            commands::cleaner_delete_leftovers,
            // 软件卸载独立窗口（2026-11-10 设计 3.1）：
            // 打开窗口 / 读目标 / 关联文件 / 强制退出 / 白名单删除
            commands::cleaner_open_uninstall_window,
            commands::cleaner_uninstall_target,
            commands::cleaner_related_files,
            commands::cleaner_force_quit,
            commands::cleaner_delete_related,
            // 「系统」页新增页签：屏幕休眠 / 弹窗拦截（仅 Windows）
            commands::keepawake_status,
            commands::keepawake_start,
            commands::keepawake_stop,
            commands::sleep_triggers_state,
            commands::sleep_triggers_set_armed,
            commands::sleep_triggers_add,
            commands::sleep_triggers_remove,
            commands::sleep_triggers_set_enabled,
            commands::popup_status,
            commands::popup_settings_get,
            commands::popup_settings_set,
            commands::popup_start,
            commands::popup_stop,
            commands::popup_recent,
            commands::popup_clear,
            commands::popup_dismiss,
            commands::popup_ignore,
            // 托盘快捷面板：隐藏 / 随页签内容调整高度
            commands::quick_panel_hide,
            commands::quick_panel_resize,
            // 剪贴板历史（macOS）
            commands::clipboard_page,
            commands::clipboard_search,
            commands::clipboard_stats,
            commands::clipboard_copy,
            commands::clipboard_copy_image,
            commands::clipboard_delete,
            commands::clipboard_clear,
            commands::clipboard_export_image,
            commands::clipboard_limits,
            commands::clipboard_set_shortcut,
            commands::clipboard_set_pinned,
            commands::clipboard_panel_hide,
            // Hosts 在线编辑
            commands::hosts_status,
            commands::hosts_save,
            commands::hosts_backups,
            commands::hosts_restore,
            commands::hosts_open_file_dir,
            // 更新包断点续传：下载 / 安装（前端 useUpdater 调用）
            updates::update_download_resumed,
            updates::update_install_resumed,
        ])
        .setup(|app| {
            // 先建主窗口：窗口配置在 tauri.conf.json 里，但 create: false 交给这里
            create_main_window(app)?;
            // 托盘快捷面板：同样提前建好、默认隐藏，点托盘图标时再定位显示
            create_quick_panel(app)?;
            // 剪贴板面板：macOS / Windows 启用，提前建好、默认隐藏
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            create_clipboard_panel(app)?;

            // dev 模式（裸二进制）Dock 会显示系统占位图标，这里补上真实图标
            #[cfg(target_os = "macos")]
            set_dock_icon();
            // 开发版的任务栏 / 窗口图标同样换成带 dev 角标的那张
            set_dev_window_icon(app.handle());
            // macOS 顶部菜单栏汉化
            setup_app_menu(app.handle())?;

            // VPN 运行期状态：会话目录必须主进程和提权 worker 都能读写。
            //
            // 这里不能自己拼 app_data_dir 了事 —— 那个目录在受控文件夹访问、
            // 企业策略或受限令牌下会拒绝写入，而 app_data_dir() 本身不会报错，
            // 于是失败会一路推迟到用户点「连接」时才以「拒绝访问」爆出来。
            // 统一走 session_dir()，它带真实写入探测和逐级回退。
            let session = match vpn::commands::session_dir(app.handle()) {
                Ok(dir) => vpn::session::VpnSession::new(dir),
                Err(err) => {
                    // 连临时目录都写不了：不阻断启动，但把原因打出来，
                    // 免得后面只看到一句没有上下文的「拒绝访问」
                    eprintln!("VPN 会话目录不可用: {}", err);
                    vpn::session::VpnSession::new(
                        std::env::temp_dir().join(format!("ws-workbench-vpn{}", build_dir_suffix())),
                    )
                }
            };
            if let Err(err) = session.ensure() {
                eprintln!("VPN 会话目录初始化失败: {}", err);
            }
            app.manage(vpn::commands::VpnState::new(session));

            // 系统清理的列表缓存与扫描标记（设计 3.3）：只有进程内状态，无需落盘
            app.manage(cleaner::CleanerState::new());

            // 剪贴板历史：运行时状态（变化序列号 / 采集锁 / 当前快捷键）；
            // 采集线程与全局快捷键在 macOS / Windows 启用
            app.manage(clipboard::ClipboardRuntime::new());
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            {
                clipboard::register_saved_shortcut(app.handle());
                clipboard::start_poller(app.handle().clone());
            }

            // 轮询会话目录里的状态文件，把 VPN 状态与日志推给前端
            vpn::commands::start_status_poller(app.handle().clone());

            // 托盘：关掉窗口后应用仍在后台运行，从托盘菜单重新打开或退出。
            // 菜单只保留必要入口：显示 / 检查更新 / 退出，页面里的动作都在主界面里。
            // 开发版最上面再压一行禁用的构建标识，和安装版的菜单区分开。
            let build_tag = MenuItem::with_id(
                app,
                "build-tag",
                format!("{}（开发版）", app_display_name()),
                false,
                None::<&str>,
            )?;
            let show = MenuItem::with_id(app, "show", "显示主界面", true, None::<&str>)?;
            let check_update =
                MenuItem::with_id(app, "check-update", "检查更新", true, None::<&str>)?;
            let separator = PredefinedMenuItem::separator(app)?;
            let dev_separator = PredefinedMenuItem::separator(app)?;
            let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let mut items: Vec<&dyn tauri::menu::IsMenuItem<tauri::Wry>> = Vec::new();
            if is_dev_build() {
                items.push(&build_tag);
                items.push(&dev_separator);
            }
            items.push(&show);
            items.push(&check_update);
            items.push(&separator);
            items.push(&quit);
            let menu = Menu::with_items(app, &items)?;

            let mut tray = TrayIconBuilder::with_id("main")
                // 不设 tooltip：菜单栏图标悬浮时不要弹出应用名
                .menu(&menu)
                // 左键单击弹快捷面板，不弹菜单；菜单只在右键时出现
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => show_main_window(app),
                    "check-update" => {
                        // 检查更新要用户看得见进度，先把主界面叫出来，
                        // 再让前端自己去 check()：事件只负责触发，不传版本信息，
                        // 避免 Rust 侧和前端各查一次、出现两份不一致的结论。
                        show_main_window(app);
                        let _ = app.emit("tray-check-update", ());
                    }
                    "quit" => {
                        // 退出就是整个应用和它拉起的进程都结束：先给 worker 发停止
                        // 请求（写 stop.flag），然后立刻退出主进程。
                        //
                        // 不再在这里等 worker 收尾：worker 有两条独立兜底——main.pid
                        // 里记的主进程消失、以及心跳过期——会自己杀掉 openconnect、
                        // 还原路由后退出。等它只会让托盘和任务栏图标迟迟不消失，
                        // 体感上就是「点了退出没反应」。
                        vpn::commands::shutdown(app);
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        rect,
                        ..
                    } = event
                    {
                        toggle_quick_panel(tray.app_handle(), rect);
                    }
                });
            if let Some(icon) = tray_icon_image() {
                // macOS 菜单栏深浅色由系统决定，模板图交给系统着色；开发版也是模板图
                // （开发标识是镂空点，不依赖颜色），所以这里不按构建区分
                tray = tray.icon(icon).icon_as_template(cfg!(target_os = "macos"));
            }
            tray.build(app)?;
            // 安装包卸载程序读取的路径清单：启动时先写一份，设置变化时 save_settings 会再写。
            let settings = crate::config::load_settings(app.handle());
            crate::uninstall_info::refresh(app.handle(), &settings);
            Ok(())
        })
        .on_window_event(|window, event| {
            // 快捷面板失焦就收起：点桌面/别的应用/主窗口都算「点外面」
            if let WindowEvent::Focused(false) = event {
                if window.label() == QUICK_PANEL_LABEL {
                    let _ = window.hide();
                    QUICK_PANEL_BLUR_HIDDEN_AT.store(
                        unix_millis(),
                        std::sync::atomic::Ordering::Relaxed,
                    );
                }
                // 置顶（贴屏幕）时失焦不收起，用户点别的应用面板也留在屏幕上
                if window.label() == clipboard::PANEL_LABEL
                    && !clipboard::is_pinned(window.app_handle())
                {
                    let _ = window.hide();
                }
            }
            // 点关闭只隐藏到后台，不退出应用
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                if window.label() == clipboard::PANEL_LABEL {
                    // 走统一的收起逻辑：清置顶 + 把焦点还给呼出前的 App
                    clipboard::hide_panel(window.app_handle());
                } else {
                    let _ = window.hide();
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app_handle, event| {
        // macOS 点 Dock 图标时把窗口重新显示出来
        // 注意：RunEvent::Reopen 只存在于 macOS，其他平台编译时需要整段排除
        #[cfg(target_os = "macos")]
        {
            // 启动完成后再设一次 Dock 图标：磁贴是应用启动后才画出来的
            if let tauri::RunEvent::Ready = event {
                set_dock_icon();
            }
            if let tauri::RunEvent::Reopen {
                has_visible_windows,
                ..
            } = event
            {
                if !has_visible_windows {
                    show_main_window(app_handle);
                }
            }
        }

        // 退出（Cmd+Q / 菜单退出 / 系统关机）时也要让提权 worker 收尾：
        // 只处理窗口关闭是不够的，那条路径会隐藏到托盘、不退出进程。
        if let tauri::RunEvent::ExitRequested { .. } = event {
            vpn::commands::shutdown(app_handle);
        }

        // 非 macOS 平台没有 Reopen 事件，这里显式忽略参数避免 unused 警告
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (app_handle, event);
        }
    });
}

#[cfg(test)]
mod quick_panel_tests {
    use super::{panel_origin_at, PanelPlacement, QUICK_PANEL_HEIGHT, QUICK_PANEL_WIDTH};

    const MENUBAR_SCREEN: (i32, i32, i32, i32) = (0, 0, 1920, 1080);
    const PANEL: (u32, u32) = (QUICK_PANEL_WIDTH as u32, QUICK_PANEL_HEIGHT as u32);
    const GAP: i32 = 8;
    /// 居中时面板左边 = 图标中心 - 半个面板宽
    const HALF_PANEL: i32 = QUICK_PANEL_WIDTH as i32 / 2;

    /// macOS 菜单栏图标：面板挂在图标下方，水平居中于图标
    #[test]
    fn centers_below_top_menubar_icon() {
        let (x, y) = panel_origin_at((1000, 10), PANEL, MENUBAR_SCREEN);
        assert_eq!(x, 1000 - HALF_PANEL);
        assert_eq!(y, 10 + GAP);
    }

    /// Windows 底部任务栏图标：面板朝上，同样水平居中
    #[test]
    fn centers_above_bottom_taskbar_icon() {
        let (x, y) = panel_origin_at((1500, 1044), PANEL, MENUBAR_SCREEN);
        assert_eq!(x, 1500 - HALF_PANEL);
        assert_eq!(y, 1044 - PANEL.1 as i32 - GAP);
    }

    /// 切换页签后内容变高：菜单栏下方是顶边贴着图标往下长，y 不变
    #[test]
    fn taller_panel_keeps_top_edge_below_menubar() {
        // 214 / 256 是当前 VPN / 休眠页签的实测高度，这里当代表值用；
        // 断言的是几何关系，不依赖具体高度
        let short = (QUICK_PANEL_WIDTH as u32, 214);
        let tall = (QUICK_PANEL_WIDTH as u32, 256);
        let (_, short_y) = panel_origin_at((1000, 10), short, MENUBAR_SCREEN);
        let (_, tall_y) = panel_origin_at((1000, 10), tall, MENUBAR_SCREEN);
        assert_eq!(short_y, 10 + GAP);
        assert_eq!(tall_y, 10 + GAP);
    }

    /// 切换页签后内容变高：底部任务栏上方是底边不动、往上长，不会压到任务栏
    #[test]
    fn taller_panel_keeps_bottom_edge_above_taskbar() {
        // 214 / 256 是当前 VPN / 休眠页签的实测高度，这里当代表值用；
        // 断言的是几何关系，不依赖具体高度
        let short = (QUICK_PANEL_WIDTH as u32, 214);
        let tall = (QUICK_PANEL_WIDTH as u32, 256);
        let anchor = (1500, 1044);
        let (_, short_y) = panel_origin_at(anchor, short, MENUBAR_SCREEN);
        let (_, tall_y) = panel_origin_at(anchor, tall, MENUBAR_SCREEN);
        assert_eq!(short_y + short.1 as i32 + GAP, anchor.1);
        assert_eq!(tall_y + tall.1 as i32 + GAP, anchor.1);
        assert_eq!(short_y - tall_y, tall.1 as i32 - short.1 as i32);
    }

    /// 菜单栏 / 底部任务栏：判断面板朝下还是朝上，决定改高度时要不要重新定位
    #[test]
    fn knows_which_side_panel_expands() {
        let below = PanelPlacement {
            anchor: (1000, 10),
            screen: MENUBAR_SCREEN,
            panel: PANEL,
            logical: true,
        };
        let above = PanelPlacement {
            anchor: (1500, 1044),
            screen: MENUBAR_SCREEN,
            panel: PANEL,
            logical: false,
        };
        assert!(below.expands_below());
        assert!(!above.expands_below());
    }

    /// 贴近屏幕左右边缘：基准位置被夹进显示器，不能有一半跑到屏幕外
    #[test]
    fn clamps_into_screen_edges() {
        let (left_x, _) = panel_origin_at((2, 10), PANEL, MENUBAR_SCREEN);
        assert_eq!(left_x, 8, "左边距不足时贴住 8px 安全边");

        let (right_x, _) = panel_origin_at((1918, 10), PANEL, MENUBAR_SCREEN);
        assert_eq!(right_x, 1920 - PANEL.0 as i32 - 8);
    }

    /// 多显示器：负坐标的副屏按它自己的范围夹，而不是按 0 点
    #[test]
    fn handles_negative_monitor_coordinates() {
        let screen = (-1920, 0, 1920, 1080);
        let (x, y) = panel_origin_at((-1000, 10), PANEL, screen);
        assert_eq!(x, -1000 - HALF_PANEL);
        assert_eq!(y, 10 + GAP);
    }

    /// 显示器比面板还小时不能 panic，结果停在左上角安全边
    #[test]
    fn tiny_screen_does_not_panic() {
        let (x, y) = panel_origin_at((100, 10), PANEL, (0, 0, 320, 300));
        assert_eq!(x, 8);
        assert_eq!(y, 8);
    }
}
