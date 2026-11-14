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
/// 剪贴板历史（macOS / Windows）
mod clipboard;
/// Hosts 在线编辑（macOS + Windows）
mod hosts;
/// 局域网分享：UDP 组播发现 + 明文 HTTP 文件收发
mod lanshare;
mod packer;
pub mod apps;
mod platform;
mod netinfo;
mod process;
/// 标准数据目录约定与旧版本数据迁移
mod storage;
mod winps;
#[cfg(test)]
pub(crate) mod test_support;
/// 更新包断点续传下载 + 安装
mod updates;
/// 安装包卸载程序读取的路径清单
mod uninstall_info;
pub mod vpn;

use tauri::{
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, Runtime, WindowEvent,
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

/// 主窗口是否已经显示过。
///
/// 启动兜底（8 秒后强制显示主窗口）只兜「从来没显示过」这一种情况：已经显示过
/// 还不可见，只可能是用户自己把窗口收进了托盘，再强制显示就成了「点了关闭，
/// 过几秒窗口又自己弹出来」。
static MAIN_WINDOW_SHOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

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
    }

    const ERROR_ALREADY_EXISTS: i32 = 183;

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
        // 已有实例：优先走事件通道，让第一个实例自己把窗口 show 出来；
        // 它还在冷启动、事件没建好时退回直接改窗口状态。
        if !win_activation::signal() {
            activate_existing_main_window();
        }
        return false;
    }

    // 第一个实例：建事件，监听线程在 setup 里起
    win_activation::create();

    // 句柄故意不关闭：它要活到进程结束，命名 mutex 才会一直存在。
    let _ = handle;
    true
}

/// Windows 上「把已有实例叫到前台」的通道：命名事件 + 第一个实例里的监听线程。
///
/// 为什么不在第二个进程里直接 `ShowWindow`：那样只改了窗口的**系统可见性**，
/// Tauri 自己记的状态还是「隐藏」。用户回到主窗口点关闭时，`CloseRequested` 里
/// 的 `window.hide()` 一看「本来就没显示」就什么都不做——表现就是「点关闭没反应」；
/// 最小化/最大化也会因为两边状态对不上而错乱。
/// 所以第二个进程只负责「敲一下事件」，真正的 show + focus 由第一个实例在
/// 主线程上通过 Tauri 做，两边状态才会一致。
#[cfg(target_os = "windows")]
mod win_activation {
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateEventW(
            attributes: *mut c_void,
            manual_reset: i32,
            initial_state: i32,
            name: *const u16,
        ) -> *mut c_void;
        fn OpenEventW(access: u32, inherit: i32, name: *const u16) -> *mut c_void;
        fn SetEvent(handle: *mut c_void) -> i32;
        fn WaitForSingleObject(handle: *mut c_void, milliseconds: u32) -> u32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }
    #[link(name = "user32")]
    extern "system" {
        fn AllowSetForegroundWindow(process_id: u32) -> i32;
    }

    /// 只用来发通知，不需要查询状态
    const EVENT_MODIFY_STATE: u32 = 0x0002;
    const INFINITE: u32 = 0xFFFF_FFFF;
    /// 允许任意进程抢前台：第二个进程退出后，第一个实例的 SetForegroundWindow
    /// 才不会被 Windows 的前台锁挡掉
    const ASFW_ANY: u32 = 0xFFFF_FFFF;

    static HANDLE: AtomicUsize = AtomicUsize::new(0);

    fn event_name() -> Vec<u16> {
        format!("Local\\{}-activate\0", crate::app_display_name())
            .encode_utf16()
            .collect()
    }

    /// 第一个实例启动时建事件（自动复位，默认无信号）
    pub fn create() {
        let name = event_name();
        let handle = unsafe { CreateEventW(std::ptr::null_mut(), 0, 0, name.as_ptr()) };
        if !handle.is_null() {
            HANDLE.store(handle as usize, Ordering::SeqCst);
        }
    }

    /// 第二个实例：允许对方抢前台后敲事件；第一个实例还没建好事件时返回 false
    pub fn signal() -> bool {
        let name = event_name();
        let handle = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr()) };
        if handle.is_null() {
            return false;
        }
        unsafe {
            AllowSetForegroundWindow(ASFW_ANY);
            SetEvent(handle);
            CloseHandle(handle);
        }
        true
    }

    /// 第一个实例的监听线程：等一次信号（自动复位，可以反复等）
    pub fn wait_once() {
        let handle = HANDLE.load(Ordering::SeqCst) as *mut c_void;
        if handle.is_null() {
            std::thread::sleep(std::time::Duration::from_millis(200));
            return;
        }
        unsafe { WaitForSingleObject(handle, INFINITE) };
    }
}

/// 按「窗口类 `Tauri Window` + 标题精确等于应用名」找主窗口的 HWND。
///
/// 不能用 `FindWindowW(null, 标题)`：实测对**隐藏**（收进托盘）的窗口返回 0，
/// 而「应用在后台，双击快捷方式」恰好就是隐藏状态。枚举则能找到。
/// 快捷面板（`<应用名> - 快捷面板`）、剪贴板面板（`<应用名> - 剪贴板`）标题带后缀，
/// 不会误中。
#[cfg(target_os = "windows")]
fn find_main_window() -> *mut std::ffi::c_void {
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[link(name = "user32")]
    extern "system" {
        fn EnumWindows(
            callback: extern "system" fn(*mut c_void, *mut c_void) -> i32,
            lparam: *mut c_void,
        ) -> i32;
        fn GetClassNameW(hwnd: *mut c_void, buffer: *mut u16, max: i32) -> i32;
        fn GetWindowTextW(hwnd: *mut c_void, buffer: *mut u16, max: i32) -> i32;
    }

    static FOUND: AtomicUsize = AtomicUsize::new(0);
    static TITLE: std::sync::OnceLock<Vec<u16>> = std::sync::OnceLock::new();

    FOUND.store(0, Ordering::SeqCst);
    TITLE.get_or_init(|| app_display_name().encode_utf16().collect());

    extern "system" fn visit(hwnd: *mut c_void, _lparam: *mut c_void) -> i32 {
        let mut class = [0u16; 64];
        let class_len = unsafe { GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32) };
        if class_len <= 0
            || String::from_utf16_lossy(&class[..class_len as usize]) != "Tauri Window"
        {
            return 1;
        }
        let mut title = [0u16; 256];
        let title_len = unsafe { GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32) };
        if title_len <= 0 {
            return 1;
        }
        let expected = TITLE.get().map(|value| value.as_slice()).unwrap_or(&[]);
        if &title[..title_len as usize] == expected {
            FOUND.store(hwnd as usize, Ordering::SeqCst);
            return 0;
        }
        1
    }

    unsafe { EnumWindows(visit, std::ptr::null_mut()) };
    FOUND.load(Ordering::SeqCst) as *mut c_void
}

/// 把窗口恢复并抢到前台。托盘隐藏 / 最小化 / 被别的窗口压着，三种情况都覆盖；
/// `SetForegroundWindow` 单独用常被前台锁挡掉，`SwitchToThisWindow` 兜底。
#[cfg(target_os = "windows")]
fn force_foreground(hwnd: *mut std::ffi::c_void) {
    use std::ffi::c_void;

    #[link(name = "user32")]
    extern "system" {
        fn ShowWindow(hwnd: *mut c_void, command: i32) -> i32;
        fn BringWindowToTop(hwnd: *mut c_void) -> i32;
        fn SetForegroundWindow(hwnd: *mut c_void) -> i32;
        fn SwitchToThisWindow(hwnd: *mut c_void, alt_tab: i32);
    }

    const SW_RESTORE: i32 = 9;
    unsafe {
        ShowWindow(hwnd, SW_RESTORE);
        BringWindowToTop(hwnd);
        SwitchToThisWindow(hwnd, 1);
        SetForegroundWindow(hwnd);
    }
}

/// 兜底路径（事件通道不可用时）：直接改窗口状态把它叫出来。
/// 注意这会绕过 Tauri 的窗口状态，只在命名事件没建好（第一个实例刚启动）时用。
#[cfg(target_os = "windows")]
fn activate_existing_main_window() {
    let hwnd = find_main_window();
    if !hwnd.is_null() {
        force_foreground(hwnd);
    }
}

// 把主窗口重新显示出来（托盘左键、Dock 图标、快捷面板底部「显示主界面」共用）
pub(crate) fn show_main_window<R: Runtime>(app: &AppHandle<R>) {
    MAIN_WINDOW_SHOWN.store(true, std::sync::atomic::Ordering::Relaxed);
    // macOS：托盘面板 / 菜单栏图标触发的点击不会自动激活应用，只 show + set_focus
    // 时窗口会显示在别的应用后面；先激活应用，再显示窗口。
    #[cfg(target_os = "macos")]
    clipboard::activate_app(app);
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// 前端首屏挂载完成：显示主窗口，并预热两个隐藏面板。
///
/// 主窗口在 setup 里以隐藏状态创建（`tauri.conf.json` 的 `visible: false`），
/// 等这里才显示：Windows 冷启动时 WebView2 初始化 + setup 的同步工作要几秒，
/// 先亮一个还没画完的白窗口会被系统标成「未响应」，等页面就绪再显示更稳。
pub(crate) fn on_frontend_ready(app: &AppHandle) {
    show_main_window(app);
    warm_up_hidden_windows(app);
}

// 托盘图标：macOS 用模板图（黑 + 透明），系统会按菜单栏明暗自动反色；
// Windows 直接用应用图标（彩色），白色线条图在浅色任务栏上会看不清。
//
// 开发版整套换成带 dev 标识的变体。macOS 菜单栏这里刻意沿用发布版那张模板图
// （系统按菜单栏明暗染色，开发版的颜色和发布版完全一样），只在箱体正中镂空一个
// 透明小圆点——镂空处透出菜单栏背景，不引入新颜色，18pt 见方、不比发布版宽。
// Windows / Linux 托盘用右下角带 dev 角标的彩色应用图标（浅色任务栏上也认得出）。
#[cfg(target_os = "macos")]
fn tray_icon_image() -> Option<tauri::image::Image<'static>> {
    // 模板图（黑 + 透明），系统按菜单栏明暗自动反色；开发版正中镂空一个点。
    let bytes: &'static [u8] = if is_dev_build() {
        include_bytes!("../icons/tray/tray-dev-template-36.png")
    } else {
        include_bytes!("../icons/tray/tray-template-36.png")
    };
    tauri::image::Image::from_bytes(bytes).ok()
}

#[cfg(target_os = "windows")]
fn tray_icon_image() -> Option<tauri::image::Image<'static>> {
    // Windows 用彩色应用图标（白色线条图在浅色任务栏上看不清）。
    let bytes: &'static [u8] = if is_dev_build() {
        include_bytes!("../icons/dev/32x32-dev.png")
    } else {
        include_bytes!("../icons/32x32.png")
    };
    tauri::image::Image::from_bytes(bytes).ok()
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn tray_icon_image() -> Option<tauri::image::Image<'static>> {
    let bytes: &'static [u8] = if is_dev_build() {
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
    #[cfg(debug_assertions)]
    let bytes: &'static [u8] = include_bytes!("../icons/dev/icon-dev.png");
    #[cfg(not(debug_assertions))]
    let bytes: &'static [u8] = include_bytes!("../icons/icon-source.png");
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
    // 发布包不编入 dev 图标字节；开发版才需要。
    #[cfg(debug_assertions)]
    {
        let Some(window) = app.get_webview_window("main") else {
            return;
        };
        if let Ok(icon) =
            tauri::image::Image::from_bytes(include_bytes!("../icons/dev/128x128@2x-dev.png"))
        {
            let _ = window.set_icon(icon);
        }
    }
    #[cfg(not(debug_assertions))]
    let _ = app;
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
fn webview_data_dir<R: Runtime>(app: &AppHandle<R>) -> std::path::PathBuf {
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

/// 光标是否还停在剪贴板面板窗口里。
///
/// Windows 上拖标题栏会进 SC_MOVE 模态循环，循环一开始窗口先失焦；靠光标位置
/// 区分「在拖面板」和「点了外面」，避免拖拽被失焦收起逻辑掐断。
#[cfg(target_os = "windows")]
fn cursor_inside_panel<R: Runtime>(window: &tauri::Window<R>) -> bool {
    window
        .hwnd()
        .map(|hwnd| clipboard::cursor_inside_hwnd(hwnd.0 as isize))
        .unwrap_or(false)
}

/// 非 Windows：没有 SC_MOVE 这套东西，保持「失焦即收起」。
#[cfg(not(target_os = "windows"))]
fn cursor_inside_panel<R: Runtime>(_window: &tauri::Window<R>) -> bool {
    false
}

/// 创建主窗口。
///
/// 不用 Tauri 的自动创建（tauri.conf.json 里 `create: false`），是为了能在原生层
/// 挂 `on_navigation`：把文件从 Finder 拖进窗口时，WebView 默认会直接导航到
/// file:// 去显示那个文件，整个应用界面被顶替（现场坏掉就是这么来的）。
/// 这里直接拒绝 file:// 导航，页面永远不会被拖进来的文件换掉。
///
/// 主窗口配置是 `visible: false`：创建完先藏着，等前端首屏渲染完调 `app_ready`
/// 再显示（见 `app_ready`）。Windows 冷启动时 WebView2 初始化要一两秒，先亮白窗口
/// 会被系统标成「未响应」。
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
        .data_directory(webview_data_dir(app.handle()))
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

/// 创建托盘快捷面板：柠檬式无边框小窗，创建后保持隐藏。
///
/// 启动时不再创建，主窗口显示后由 `warm_up_hidden_windows` 预热一次；如果用户
/// 抢在预热前点了托盘图标，`ensure_quick_panel` 会现场创建。窗体的显隐/定位在
/// `toggle_quick_panel` 里处理。
fn create_quick_panel<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<tauri::WebviewWindow<R>> {
    let window = tauri::WebviewWindowBuilder::new(
        app,
        QUICK_PANEL_LABEL,
        tauri::WebviewUrl::App("quick.html".into()),
    )
    // 标题不能和主窗口重名：Windows 单实例靠 FindWindowW(标题) 找主窗口，
    // 面板同名时会先被找到，结果第二份进程把快捷面板 ShowWindow 出来（而且
    // 拿不到焦点、点外面也收不起来），用户以为窗口关不掉。
    .title(format!("{} - 快捷面板", app_display_name()))
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
    Ok(window)
}

/// 复用已有快捷面板；第一次用（或预热还没跑到）时就地创建。
///
/// 创建失败只影响面板本身：托盘其它行为照常，不要 panic 打断事件循环。
pub(crate) fn ensure_quick_panel<R: Runtime>(
    app: &AppHandle<R>,
) -> Option<tauri::WebviewWindow<R>> {
    if let Some(window) = app.get_webview_window(QUICK_PANEL_LABEL) {
        return Some(window);
    }
    match create_quick_panel(app) {
        Ok(window) => Some(window),
        Err(err) => {
            eprintln!("[quick-panel] 创建失败: {err}");
            None
        }
    }
}

/// 创建剪贴板历史面板：无边框透明浮层，创建后保持隐藏。
///
/// macOS / Windows 会用到；Linux 不启用（快捷键不注册，`show_panel` 也不会被调用）。
/// 和快捷面板一样不在启动时创建，避免冷启动一口气初始化三个 WebView2。
fn create_clipboard_panel<R: Runtime>(
    app: &AppHandle<R>,
) -> tauri::Result<tauri::WebviewWindow<R>> {
    let window = tauri::WebviewWindowBuilder::new(
        app,
        clipboard::PANEL_LABEL,
        tauri::WebviewUrl::App("clipboard.html".into()),
    )
    // 同快捷面板：标题必须和主窗口区分，否则 Windows 单实例会误激活这个隐藏浮层
    .title(format!("{} - 剪贴板", app_display_name()))
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
    Ok(window)
}

/// 复用已有剪贴板面板；第一次按快捷键时（或预热没跑到）就地创建。
pub(crate) fn ensure_clipboard_panel<R: Runtime>(
    app: &AppHandle<R>,
) -> Option<tauri::WebviewWindow<R>> {
    if let Some(window) = app.get_webview_window(clipboard::PANEL_LABEL) {
        return Some(window);
    }
    match create_clipboard_panel(app) {
        Ok(window) => Some(window),
        Err(err) => {
            eprintln!("[clipboard-panel] 创建失败: {err}");
            None
        }
    }
}

/// 主窗口显示后预热两个隐藏面板。
///
/// 不放在 setup 里：Windows 冷启动时三个 WebView2 一起初始化会把主窗口拖到
/// 被系统判定「未响应」。延后 600ms 建，既不占启动路径，第一次点托盘 /
/// 按剪贴板快捷键时窗口通常也已经在了。
fn warm_up_hidden_windows<R: Runtime>(app: &AppHandle<R>) {
    let handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(600));
        let inner = handle.clone();
        let _ = handle.run_on_main_thread(move || {
            let _ = ensure_quick_panel(&inner);
            let _ = ensure_clipboard_panel(&inner);
        });
    });
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

/// 右键点托盘图标：已显示就收起，否则贴着图标弹出。
///
/// 面板窗口是懒创建的：启动时不再创建，主窗口显示后预热；用户如果在预热前
/// 就点了托盘图标，这里现场建一个再弹。
fn toggle_quick_panel<R: Runtime>(app: &AppHandle<R>, rect: tauri::Rect) {
    let Some(window) = ensure_quick_panel(app) else {
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

/// Windows：取某个点所在显示器的**工作区**（系统已经扣掉任务栏、贴边工具栏
/// 之后的可用矩形，物理像素、左上原点）。
///
/// 面板定位必须用它，不能拿整块屏幕去夹：屏幕最下面那一块正好被任务栏盖住，
/// 面板底边一旦落进去，顶部还在桌面上看得见、底部几行被任务栏压住。托盘图标
/// 本身就在任务栏里，从托盘弹出的面板必然贴着屏幕底边，所以这个位置最容易踩到。
///
/// 任务栏自动隐藏 / 全屏 / 多显示器时 `rcWork` 由系统按当前真实情况算好，比
/// 自己按「屏幕高 - 任务栏高」估要准。
#[cfg(target_os = "windows")]
pub(crate) fn work_area_at(x: i32, y: i32) -> Option<(i32, i32, i32, i32)> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };

    let monitor = unsafe { MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        return None;
    }
    Some((
        info.rcWork.left,
        info.rcWork.top,
        info.rcWork.right - info.rcWork.left,
        info.rcWork.bottom - info.rcWork.top,
    ))
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
    // 优先用系统给的工作区（扣掉任务栏），退回来才是整块显示器
    let screen = work_area_at(anchor.0, anchor.1)
        .or_else(|| {
            app.monitor_from_point(anchor.0 as f64, anchor.1 as f64)
                .ok()
                .flatten()
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
    // 和 quick_panel_placement 一致：Windows 上用扣掉任务栏的工作区
    #[cfg(target_os = "windows")]
    let work_area = work_area_at(anchor.0, anchor.1);
    #[cfg(not(target_os = "windows"))]
    let work_area: Option<(i32, i32, i32, i32)> = None;
    let screen = work_area
        .or_else(|| {
            app.monitor_from_point(anchor.0 as f64, anchor.1 as f64)
                .ok()
                .flatten()
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
            // 前端首屏就绪：显示主窗口（启动时主窗口是隐藏的）
            commands::app_ready,
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
            commands::popup_ignored,
            commands::popup_rules,
            commands::popup_rule_add,
            commands::popup_rule_remove,
            commands::popup_rule_toggle,
            commands::popup_app_toggle,
            commands::popup_rule_apply,
            commands::popup_windows,
            commands::popup_app_meta,
            // 托盘快捷面板：隐藏 / 随页签内容调整高度 / 底部「显示主界面」「退出」
            commands::quick_panel_hide,
            commands::quick_panel_resize,
            commands::quick_panel_show_main,
            commands::quick_panel_quit,
            // 剪贴板历史（macOS / Windows；其它平台不启用）
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
            // 局域网分享（2026-11-13 设计）：快照 / 启停 / 设置 / 接收确认 / 发送 / 清空记录
            commands::lan_share_snapshot,
            commands::lan_share_set_enabled,
            commands::lan_share_set_alias,
            commands::lan_share_set_download_dir,
            commands::lan_share_respond_offer,
            commands::lan_share_send_files,
            // 更新包断点续传：下载 / 安装（前端 useUpdater 调用）
            updates::update_download_resumed,
            updates::update_install_resumed,
        ])
        .setup(|app| {
            // 数据目录迁移必须早于任何 config/db 读取：旧版本的数据目录
            // （产品名目录 / ~/.ws-workbench）统一搬到标准 app_data_dir。
            // 迁移失败只打日志，不阻断启动。
            if let Err(err) = crate::storage::migrate_on_startup(app.handle()) {
                eprintln!("[storage] 旧数据目录迁移失败（不影响启动）: {err}");
            }

            // 启动阶段只建主窗口，而且先隐藏（tauri.conf.json 里 main 是 visible: false）：
            // 前端首屏渲染完会调 `app_ready`，那时再显示。Windows 冷启动时 WebView2
            // 初始化 + setup 里的同步工作要好几秒，先亮一个白窗口会被系统标成
            // 「未响应」；等页面就绪再显示，用户看到的就是能立刻操作的界面。
            create_main_window(app)?;

            // 关机 / 重启前先把 VPN 断开再放行：VPN 连着的时候直接关机，
            // 隧道进程、`/32` 路由和 NRPT 规则都来不及回收，下次开机内网域名
            // 会全部解析不了。Windows 上用 WM_QUERYENDSESSION 拦截（见 shutdown.rs）。
            vpn::shutdown::install(app.handle());

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

            // 弹窗拦截：上次退出时开着就自动恢复，规则立即生效（火绒式的常驻开关）。
            // 启动线程里做，别让磁盘读取和 EnumWindows 占住 setup 主线程。
            #[cfg(target_os = "windows")]
            if cleaner::popup::settings_get().enabled {
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    if let Err(err) = cleaner::popup::start(handle) {
                        eprintln!("弹窗拦截自动开启失败: {err}");
                    }
                });
            }

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

            // 局域网分享：运行期状态（启停 / peer 表 / 待接收请求 / 接收会话）。
            // 冷启动时按设置恢复监听；启动失败只记录原因，不阻断应用启动。
            app.manage(lanshare::LanShareState::from_app(app.handle()));
            {
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    if let Some(state) = handle.try_state::<lanshare::LanShareState>() {
                        state.restore_on_startup(&handle);
                    }
                });
            }

            // 托盘：关掉窗口后应用仍在后台运行。
            //
            // 左键单击图标 = 直接显示主界面；右键 = 弹出快捷面板（面板底部带
            // 「显示主界面 / 退出」两个入口）。这里不再挂原生菜单：托盘一旦有
            // 菜单，右键会先弹系统菜单，快捷面板就出不来了。
            let mut tray = TrayIconBuilder::with_id("main")
                // 不设 tooltip：菜单栏图标悬浮时不要弹出应用名
                .on_tray_icon_event(|tray, event| {
                    let TrayIconEvent::Click {
                        button,
                        button_state: MouseButtonState::Up,
                        rect,
                        ..
                    } = event
                    else {
                        return;
                    };
                    match button {
                        MouseButton::Left => {
                            // 面板开着时左键优先回主界面：先把面板收起来，
                            // 否则 always_on_top 的面板会一直悬在主窗口上面
                            if let Some(window) =
                                tray.app_handle().get_webview_window(QUICK_PANEL_LABEL)
                            {
                                let _ = window.hide();
                            }
                            show_main_window(tray.app_handle());
                        }
                        MouseButton::Right => toggle_quick_panel(tray.app_handle(), rect),
                        _ => {}
                    }
                });
            if let Some(icon) = tray_icon_image() {
                // macOS 菜单栏深浅色由系统决定，模板图交给系统着色；开发版也是模板图
                // （开发标识是镂空点，不依赖颜色），所以这里不按构建区分
                tray = tray.icon(icon).icon_as_template(cfg!(target_os = "macos"));
            }
            tray.build(app)?;

            // 主窗口兜底：前端加载失败（或首屏逻辑异常）时不会调 app_ready，
            // 这里等 8 秒强制显示，避免应用只剩托盘图标、没有任何窗口。
            // 只兜「从来没显示过」：已经显示过还不可见，只可能是用户自己收进了托盘，
            // 再强制显示就会变成「点了关闭，过几秒又自己弹出来」。
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(8));
                let inner = handle.clone();
                let _ = handle.run_on_main_thread(move || {
                    if MAIN_WINDOW_SHOWN.load(std::sync::atomic::Ordering::Relaxed) {
                        return;
                    }
                    if let Some(window) = inner.get_webview_window("main") {
                        if !window.is_visible().unwrap_or(false) {
                            crate::show_main_window(&inner);
                        }
                    }
                });
            });

            // 安装包卸载程序读取的路径清单：启动时先写一份，设置变化时 save_settings 会再写。
            // 写文件可能被杀软 / 受控文件夹访问拖慢，放到后台线程，不占启动路径。
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                let settings = crate::config::load_settings(&handle);
                crate::uninstall_info::refresh(&handle, &settings);
            });

            // 双击快捷方式 / 再跑一次 exe：第二个进程敲事件，这里在主线程上走
            // Tauri 的 show + focus，窗口状态两边一致（关闭、最小化才不会失灵）。
            #[cfg(target_os = "windows")]
            {
                let handle = app.handle().clone();
                std::thread::spawn(move || loop {
                    win_activation::wait_once();
                    let inner = handle.clone();
                    let _ = handle.run_on_main_thread(move || {
                        crate::show_main_window(&inner);
                        let hwnd = find_main_window();
                        if !hwnd.is_null() {
                            force_foreground(hwnd);
                        }
                    });
                });
            }

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
                //
                // 鼠标还在面板里时也不收起：拖标题栏会走 Windows 的 SC_MOVE
                // 模态循环，循环一开始窗口就收到一次失焦事件（日志实测
                // Focused(true) → Focused(false) 紧挨着）。以前这条分支会把
                // 窗口 hide 掉，拖拽当场被掐断，表现就是「点一下头部面板就没了」。
                if window.label() == clipboard::PANEL_LABEL
                    && !clipboard::is_pinned(window.app_handle())
                    && !cursor_inside_panel(window)
                {
                    let _ = window.hide();
                }
            }
            // 点关闭只隐藏到后台，不退出应用；卸载窗口是一次性窗口，点关闭就
            // 正常销毁，下次从列表打开时重新创建并重新扫描关联文件。
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() != crate::commands::UNINSTALL_WINDOW_LABEL {
                    api.prevent_close();
                    if window.label() == clipboard::PANEL_LABEL {
                        // 走统一的收起逻辑：清置顶 + 把焦点还给呼出前的 App
                        clipboard::hide_panel(window.app_handle());
                    } else {
                        let _ = window.hide();
                    }
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

        // macOS 的关机 / 注销不会走 ExitRequested —— AppKit 的 terminate 会直接到
        // applicationWillTerminate → tao 的 LoopDestroyed → RunEvent::Exit。在这里
        // 同步等 worker 把隧道、路由、解析器配置撤掉再让进程退出；loginwindow 会
        // 等本进程退出，所以这几秒就是「先断开再关机」。Windows 同名逻辑在
        // WM_QUERYENDSESSION 里（vpn/shutdown.rs），不在这里重复。
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Exit = event {
            vpn::shutdown::before_exit(app_handle);
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

    /// 真实托盘场景：锚点（托盘图标）就落在任务栏里，比工作区底边还低。
    /// 用系统工作区（3840x2160 屏、底部 72px 任务栏 → 2088 高）时面板底边
    /// 必须停在任务栏上沿以内，不能再按整块屏幕去夹（那时会压到任务栏下面）。
    #[test]
    fn bottom_taskbar_anchor_stays_inside_work_area() {
        let work_area = (0, 0, 3840, 2088);
        let panel = (540u32, 377u32);
        let (x, y) = panel_origin_at((3660, 2124), panel, work_area);
        assert_eq!(y, 2088 - panel.1 as i32 - 8, "底边贴着工作区下沿，留 8px");
        // 托盘图标贴着右边：水平居中会越界，夹到右边缘
        assert_eq!(x, 3840 - panel.0 as i32 - 8);
        assert!(y + panel.1 as i32 <= 2088, "面板不能越过任务栏上沿");
    }
}
