fn main() {
    // 开发构建把 macOS 的应用包名也设成 ws-workbench-dev。
    //
    // Dock 磁贴名本来就等于可执行文件名（bin target 已叫 ws-workbench-dev），这里补的是
    // tauri-codegen 嵌进二进制 __TEXT,__info_plist 的 CFBundleName（默认取 productName
    // = ws-workbench）。注意它**改不了菜单栏左上角的应用名**——那个是 AppKit 按应用
    // 身份画的，实测注入 CFBundleName 后 `otool` 里确实是 ws-workbench-dev，菜单栏
    // 仍显示 ws-workbench；所以开发版的可辨识标识靠图标角标、窗口标题、托盘与标题栏的 dev 标签。
    // 这里只是让系统信息、关于面板里读 CFBundleName 的地方跟开发版一致。
    // release profile 不注入，发布产物（.app 的 Info.plist、安装包）仍然是 ws-workbench。
    let debug = std::env::var("PROFILE").as_deref() == Ok("debug");
    if debug {
        inject_dev_bundle_name();
    }

    // Windows 的开发构建把 exe 里嵌的图标也换成带 dev 角标的那张：
    // 资源管理器里的文件图标、桌面快捷方式、任务栏固定项都读 exe 资源，
    // 运行时 `window.set_icon` 只改窗口/任务栏，改不到文件图标。
    let windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    let attributes = if debug && windows {
        tauri_build::Attributes::new().windows_attributes(
            tauri_build::WindowsAttributes::new().window_icon_path("icons/dev/icon-dev.ico"),
        )
    } else {
        tauri_build::Attributes::new()
    };
    tauri_build::try_build(attributes).expect("tauri-build 失败")
}

/// 把 `bundle.macOS.bundleName` 合并进 `TAURI_CONFIG`。
///
/// `tauri dev -c ...` 传的覆盖项也在同一个环境变量里（CLI 会透传），所以是合并
/// 而不是覆盖，别把用户/脚本传进来的配置冲掉。
fn inject_dev_bundle_name() {
    const DEV_APP_NAME: &str = "ws-workbench-dev";

    let mut patch: serde_json::Value = std::env::var("TAURI_CONFIG")
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_else(|| serde_json::json!({}));

    merge(
        &mut patch,
        serde_json::json!({ "bundle": { "macOS": { "bundleName": DEV_APP_NAME } } }),
    );

    println!("cargo:rustc-env=TAURI_CONFIG={patch}");
}

/// 递归合并：对象逐键合并，其余类型直接覆盖（和 tauri 内部给 TAURI_CONFIG 用的
/// json_patch::merge 语义一致）。
fn merge(base: &mut serde_json::Value, patch: serde_json::Value) {
    match (base, patch) {
        (serde_json::Value::Object(base_map), serde_json::Value::Object(patch_map)) => {
            for (key, value) in patch_map {
                merge(base_map.entry(key).or_insert(serde_json::Value::Null), value);
            }
        }
        (base, patch) => *base = patch,
    }
}
