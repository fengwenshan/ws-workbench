mod models;
mod config;
mod crypto;
mod commands;
mod packer;
pub mod apps;
mod platform;
mod process;
mod winps;
pub mod vpn;

use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, Runtime, WindowEvent,
};

/// 托盘菜单里点击 VPN 相关项时推给前端的事件名
pub const EVENT_VPN_TRAY: &str = "vpn-tray";

// 把主窗口重新显示出来（从托盘左键、托盘菜单、Dock 图标进入）
fn show_main_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

// macOS 顶部菜单栏汉化（Tauri 默认菜单是英文的）
#[cfg(target_os = "macos")]
fn setup_app_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    use tauri::menu::{Menu, PredefinedMenuItem, Submenu};

    let name = app.package_info().name.clone();

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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .invoke_handler(tauri::generate_handler![
            commands::get_config,
            commands::save_config,
            commands::get_config_path,
            commands::vault_encrypt,
            commands::vault_decrypt,
            commands::get_default_exclude_rules,
            commands::pack_project,
            commands::pack_to_zip,
            commands::validate_project,
            commands::detect_project_type,
            commands::detect_vcs,
            commands::open_dir,
            commands::open_parent_dir,
            commands::open_in_vscode,
            commands::open_in_idea,
            commands::open_in_trae,
            commands::open_in_terminal,
            commands::get_platform_info,
            commands::list_app_locations,
            commands::set_app_path,
            commands::clear_app_path,
            commands::import_app,
            commands::remove_managed_app,
            commands::refresh_app_search,
            commands::open_with_app_id,
            vpn::commands::vpn_detect_connectors,
            vpn::commands::vpn_connector_version,
            vpn::commands::vpn_import_openconnect,
            vpn::commands::vpn_remove_managed_openconnect,
            vpn::commands::vpn_managed_openconnect_path,
            vpn::commands::vpn_install_guide,
            vpn::commands::vpn_validate_cidr,
            vpn::commands::vpn_check_rule_overlaps,
            vpn::commands::vpn_elevation_info,
            vpn::commands::vpn_setup_elevation,
            vpn::commands::vpn_remove_elevation,
            vpn::commands::vpn_connect,
            vpn::commands::vpn_disconnect,
            vpn::commands::vpn_get_snapshot,
            vpn::commands::vpn_read_logs,
            vpn::commands::vpn_clear_logs,
            vpn::commands::vpn_read_learn_report,
            vpn::commands::vpn_clear_learn_report,
            vpn::commands::vpn_open_session_dir,
            vpn::commands::vpn_session_dir_info,
        ])
        .setup(|app| {
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
                    vpn::session::VpnSession::new(std::env::temp_dir().join("ws-tools-vpn"))
                }
            };
            if let Err(err) = session.ensure() {
                eprintln!("VPN 会话目录初始化失败: {}", err);
            }
            app.manage(vpn::commands::VpnState::new(session));

            // 轮询会话目录里的状态文件，把 VPN 状态与日志推给前端
            vpn::commands::start_status_poller(app.handle().clone());

            // 托盘：关掉窗口后应用仍在后台运行，从托盘菜单重新打开或退出
            let show = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
            let vpn_connect =
                MenuItem::with_id(app, "vpn-connect", "连接 VPN", true, None::<&str>)?;
            let vpn_disconnect =
                MenuItem::with_id(app, "vpn-disconnect", "断开 VPN", true, None::<&str>)?;
            let vpn_mode =
                MenuItem::with_id(app, "vpn-mode", "切换分流模式", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let separator = PredefinedMenuItem::separator(app)?;
            let menu = Menu::with_items(
                app,
                &[
                    &show,
                    &separator,
                    &vpn_connect,
                    &vpn_disconnect,
                    &vpn_mode,
                    &separator,
                    &quit,
                ],
            )?;

            let mut tray = TrayIconBuilder::with_id("main")
                .tooltip("ws-tools")
                .menu(&menu)
                // 左键单击直接打开客户端，不弹菜单；菜单只在右键时出现
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => show_main_window(app),
                    // VPN 的配置与规则都在前端，托盘只负责把动作转发过去
                    "vpn-connect" | "vpn-disconnect" | "vpn-mode" => {
                        show_main_window(app);
                        let _ = app.emit(EVENT_VPN_TRAY, event.id.as_ref().to_string());
                    }
                    "quit" => {
                        // 退出前先请求 worker 收尾，避免留下无人回收的路由
                        vpn::commands::shutdown(app);
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        show_main_window(tray.app_handle());
                    }
                });
            if let Some(icon) = app.default_window_icon().cloned() {
                tray = tray.icon(icon);
            }
            tray.build(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            // 点关闭只隐藏到后台，不退出应用
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app_handle, event| {
        // macOS 点 Dock 图标时把窗口重新显示出来
        // 注意：RunEvent::Reopen 只存在于 macOS，其他平台编译时需要整段排除
        #[cfg(target_os = "macos")]
        {
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

        // 非 macOS 平台没有 Reopen 事件，这里显式忽略参数避免 unused 警告
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (app_handle, event);
        }
    });
}
