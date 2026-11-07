//! macOS 默认应用：LaunchServices + CoreFoundation raw FFI（设计 4.4）。
//!
//! 为什么用 raw FFI：这几个 API 在 Rust 生态没有稳定绑定，且全部标记 deprecated，
//! 但它们仍是当前唯一可以「以编程方式读取并修改默认应用」的方式（设计「已知限制」）。
//! FFI 只在命令层的 `spawn_blocking` 线程里调用，CF 对象只在单次调用内创建与释放，
//! 不跨线程共享。
//!
//! 数据流：
//! 扩展名 → `UTTypeCreatePreferredIdentifierForTag` 得到 UTI → `LSCopyDefaultRoleHandlerForContentType`
//! 得到当前 bundle id；候选 = `LSCopyAllRoleHandlersForContentType` 的 bundle id 集合，
//! 名称与路径优先用 `/Applications`、`~/Applications` 的 `.app` 扫描结果，
//! 系统应用（如预览、Safari）用 `LSCopyApplicationURLsForBundleIdentifier` 反查。
//! 修改走 `LSSetDefaultRoleHandlerForContentType(kLSRolesAll, bundle_id)`。

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_long, c_void};
use std::path::{Path, PathBuf};

use super::{
    dedupe_candidates, display_extension, normalize_extension, sort_candidates, uti_fallback,
    DefaultAppCandidate, DefaultAppEntry, SUPPORTED_EXTENSIONS,
};

// ============================== CoreFoundation / LaunchServices FFI ==============================

type CfType = *const c_void;
type CfString = *const c_void;
type CfArray = *const c_void;
type CfUrl = *const c_void;
type CfIndex = c_long;
type CfStringEncoding = u32;
type CfUrlPathStyle = c_int;
type OsStatus = c_int;
/// `LSRolesMask`：`kLSRolesAll = 0xFFFFFFFF`
type LsRolesMask = u32;

/// `kCFStringEncodingUTF8`
const CF_STRING_ENCODING_UTF8: CfStringEncoding = 0x0800_0100;
/// `kCFURLPOSIXPathStyle`
const CF_URL_POSIX_PATH_STYLE: CfUrlPathStyle = 0;
/// `kLSRolesAll`
const LS_ROLES_ALL: LsRolesMask = 0xFFFF_FFFF;
/// `kUTTagClassFilenameExtension`
const UT_TAG_CLASS_FILENAME_EXTENSION: &str = "public.filename-extension";
/// `LSSetDefaultRoleHandlerForContentType` 成功时返回 noErr(0)
const NO_ERR: OsStatus = 0;

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFStringCreateWithCString(
        allocator: CfType,
        c_str: *const c_char,
        encoding: CfStringEncoding,
    ) -> CfString;
    fn CFStringGetLength(the_string: CfString) -> CfIndex;
    fn CFStringGetMaximumSizeForEncoding(
        length: CfIndex,
        encoding: CfStringEncoding,
    ) -> CfIndex;
    fn CFStringGetCString(
        the_string: CfString,
        buffer: *mut c_char,
        buffer_size: CfIndex,
        encoding: CfStringEncoding,
    ) -> u8;
    fn CFArrayGetCount(the_array: CfArray) -> CfIndex;
    fn CFArrayGetValueAtIndex(the_array: CfArray, index: CfIndex) -> *const c_void;
    fn CFURLCopyFileSystemPath(url: CfUrl, path_style: CfUrlPathStyle) -> CfString;
    fn CFRelease(cf: CfType);
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn UTTypeCreatePreferredIdentifierForTag(
        tag_class: CfString,
        tag: CfString,
        conforming_to_uti: CfString,
    ) -> CfString;
    fn LSCopyDefaultRoleHandlerForContentType(
        content_type: CfString,
        role: LsRolesMask,
    ) -> CfString;
    fn LSCopyAllRoleHandlersForContentType(
        content_type: CfString,
        role: LsRolesMask,
    ) -> CfArray;
    fn LSCopyApplicationURLsForBundleIdentifier(
        bundle_identifier: CfString,
        out_error: *mut CfType,
    ) -> CfArray;
    fn LSSetDefaultRoleHandlerForContentType(
        content_type: CfString,
        role: LsRolesMask,
        handler_bundle_id: CfString,
    ) -> OsStatus;
}

/// 持有 +1 引用的 CoreFoundation 对象（Create/Copy 系列返回），Drop 时 `CFRelease`。
///
/// 只有「自己拥有」的对象才包这一层；`CFArrayGetValueAtIndex` 返回的借用元素
/// 不能包（否则会多做一次 Release）。
struct CfOwned(CfType);

impl CfOwned {
    fn from_raw(raw: CfType) -> Option<Self> {
        (!raw.is_null()).then_some(Self(raw))
    }

    fn as_ptr(&self) -> CfType {
        self.0
    }
}

impl Drop for CfOwned {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) };
        }
    }
}

/// Rust 字符串 → CFString（+1 引用）
fn cf_string(value: &str) -> Option<CfOwned> {
    let c_str = CString::new(value).ok()?;
    let raw = unsafe {
        CFStringCreateWithCString(std::ptr::null(), c_str.as_ptr(), CF_STRING_ENCODING_UTF8)
    };
    CfOwned::from_raw(raw)
}

/// CFString → Rust 字符串（借用，不改变引用计数）
///
/// 先按 UTF-16 长度算 UTF-8 最大字节数再取，避免定长 buffer 截断中文/emoji。
fn cf_string_to_string(raw: CfString) -> Option<String> {
    if raw.is_null() {
        return None;
    }
    let length = unsafe { CFStringGetLength(raw) };
    if length < 0 {
        return None;
    }
    let max_size = unsafe { CFStringGetMaximumSizeForEncoding(length, CF_STRING_ENCODING_UTF8) };
    if max_size < 0 {
        return None;
    }
    let mut buffer = vec![0u8; (max_size as usize).saturating_add(1)];
    let ok = unsafe {
        CFStringGetCString(
            raw,
            buffer.as_mut_ptr().cast::<c_char>(),
            buffer.len() as CfIndex,
            CF_STRING_ENCODING_UTF8,
        )
    };
    if ok == 0 {
        return None;
    }
    // CFStringGetCString 写入了 NUL 结尾的 C 字符串
    let c_str = unsafe { CStr::from_ptr(buffer.as_ptr().cast::<c_char>()) };
    c_str.to_str().ok().map(str::to_string)
}

/// CFArray<CFString> → Vec<String>
fn cf_array_strings(array: CfArray) -> Vec<String> {
    if array.is_null() {
        return Vec::new();
    }
    let count = unsafe { CFArrayGetCount(array) };
    let mut out = Vec::with_capacity(count.max(0) as usize);
    for index in 0..count {
        let value = unsafe { CFArrayGetValueAtIndex(array, index) };
        if value.is_null() {
            continue;
        }
        if let Some(text) = cf_string_to_string(value as CfString) {
            out.push(text);
        }
    }
    out
}

/// CFURL → 文件系统路径
fn cf_url_path(url: CfUrl) -> Option<PathBuf> {
    if url.is_null() {
        return None;
    }
    let raw = unsafe { CFURLCopyFileSystemPath(url, CF_URL_POSIX_PATH_STYLE) };
    let owned = CfOwned::from_raw(raw as CfType)?;
    cf_string_to_string(owned.as_ptr() as CfString).map(PathBuf::from)
}

/// CFArray<CFURL> → Vec<PathBuf>
fn cf_array_url_paths(array: CfArray) -> Vec<PathBuf> {
    if array.is_null() {
        return Vec::new();
    }
    let count = unsafe { CFArrayGetCount(array) };
    let mut out = Vec::new();
    for index in 0..count {
        let value = unsafe { CFArrayGetValueAtIndex(array, index) };
        if value.is_null() {
            continue;
        }
        if let Some(path) = cf_url_path(value as CfUrl) {
            out.push(path);
        }
    }
    out
}

// ============================== LaunchServices 小封装 ==============================

/// 扩展名 → UTI；`UTTypeCreatePreferredIdentifierForTag` 失败时退回静态映射表。
fn content_type_for_extension(extension: &str) -> Option<CfOwned> {
    let tag_class = cf_string(UT_TAG_CLASS_FILENAME_EXTENSION)?;
    let tag = cf_string(extension)?;
    let raw = unsafe {
        UTTypeCreatePreferredIdentifierForTag(tag_class.as_ptr(), tag.as_ptr(), std::ptr::null())
    };
    if let Some(uti) = CfOwned::from_raw(raw) {
        return Some(uti);
    }
    // 兜底：LaunchServices 数据库异常时静态表至少还能让「列表/修改」走通
    uti_fallback(extension).and_then(cf_string)
}

/// 当前默认打开该 UTI 的 bundle id
fn default_handler(content_type: CfString) -> Option<String> {
    let raw = unsafe { LSCopyDefaultRoleHandlerForContentType(content_type, LS_ROLES_ALL) };
    let owned = CfOwned::from_raw(raw as CfType)?;
    cf_string_to_string(owned.as_ptr() as CfString)
}

/// 所有能打开该 UTI 的 bundle id（顺序是 LaunchServices 内部的）
fn all_handlers(content_type: CfString) -> Vec<String> {
    let raw = unsafe { LSCopyAllRoleHandlersForContentType(content_type, LS_ROLES_ALL) };
    let Some(owned) = CfOwned::from_raw(raw as CfType) else {
        return Vec::new();
    };
    cf_array_strings(owned.as_ptr() as CfArray)
}

/// bundle id → 所有已注册副本的路径
fn application_paths(bundle_id: &str) -> Vec<PathBuf> {
    let Some(bundle_id_cf) = cf_string(bundle_id) else {
        return Vec::new();
    };
    let raw = unsafe {
        LSCopyApplicationURLsForBundleIdentifier(bundle_id_cf.as_ptr(), std::ptr::null_mut())
    };
    let Some(owned) = CfOwned::from_raw(raw as CfType) else {
        return Vec::new();
    };
    cf_array_url_paths(owned.as_ptr() as CfArray)
}

// ============================== 应用包扫描 ==============================

/// 一个本机 `.app` 包的元数据（只保留默认应用需要的最小字段）
struct AppBundle {
    bundle_id: String,
    name: String,
    path: PathBuf,
}

fn is_app_bundle(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()) == Some("app")
        && path.is_dir()
        && path.join("Contents").join("Info.plist").is_file()
}

/// 扫 `/Applications` 与 `~/Applications`（含一层子目录，如 Utilities）。
///
/// 与卸载列表的扫描口径一致：不扫 `/System/Applications`，系统应用（预览、Safari）
/// 由 `LSCopyApplicationURLsForBundleIdentifier` 反查名字，不会因为没扫目录而漏掉。
fn scan_app_bundles() -> Vec<AppBundle> {
    let mut roots: Vec<PathBuf> = vec![PathBuf::from("/Applications")];
    if let Some(home) = crate::cleaner::home_dir() {
        roots.push(home.join("Applications"));
    }

    let mut bundles: Vec<AppBundle> = Vec::new();
    let mut seen: Vec<PathBuf> = Vec::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            // 隐藏目录（.localized 等）不是应用
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            if is_app_bundle(&path) {
                push_bundle(&path, &mut bundles, &mut seen);
                continue;
            }
            // 一层子目录（/Applications/Utilities 之类）
            if path.is_dir() {
                let Ok(sub_entries) = std::fs::read_dir(&path) else {
                    continue;
                };
                for sub in sub_entries.flatten() {
                    let sub_path = sub.path();
                    if is_app_bundle(&sub_path) {
                        push_bundle(&sub_path, &mut bundles, &mut seen);
                    }
                }
            }
        }
    }
    bundles
}

fn push_bundle(path: &Path, bundles: &mut Vec<AppBundle>, seen: &mut Vec<PathBuf>) {
    if seen.iter().any(|existing| existing == path) {
        return;
    }
    let Some(info) = crate::cleaner::installer_macos::read_bundle_info(path) else {
        return;
    };
    let Some(bundle_id) = info
        .bundle_id
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
    else {
        return;
    };
    let name = if info.name.trim().is_empty() {
        bundle_id.clone()
    } else {
        info.name
    };
    seen.push(path.to_path_buf());
    bundles.push(AppBundle {
        bundle_id,
        name,
        path: path.to_path_buf(),
    });
}

/// 从 `.app` 路径读应用名（plist 里的 `CFBundleDisplayName` / `CFBundleName`）
fn app_name_from_path(path: &Path) -> Option<String> {
    let info = crate::cleaner::installer_macos::read_bundle_info(path)?;
    let name = info.name.trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// bundle id → 应用名：优先 LaunchServices 反查路径读 plist，取不到就显示 bundle id
fn app_name_for_bundle_id(bundle_id: &str) -> String {
    let bundle_id = bundle_id.trim();
    if bundle_id.is_empty() {
        return "未设置".to_string();
    }
    for path in application_paths(bundle_id) {
        if let Some(name) = app_name_from_path(&path) {
            return name;
        }
    }
    bundle_id.to_string()
}

// ============================== 对外接口 ==============================

/// 首批扩展名的当前默认应用
pub fn list() -> Vec<DefaultAppEntry> {
    SUPPORTED_EXTENSIONS
        .iter()
        .map(|extension| {
            let Some(content_type) = content_type_for_extension(extension) else {
                return DefaultAppEntry {
                    extension: display_extension(extension),
                    current_app_name: "未设置".to_string(),
                    current_app_id: String::new(),
                    can_change: true,
                    detail: format!(
                        "无法识别 .{} 的文件类型；可以试试用「打开方式」重新绑定",
                        extension
                    ),
                };
            };
            let uti = cf_string_to_string(content_type.as_ptr() as CfString).unwrap_or_default();
            let handler = default_handler(content_type.as_ptr() as CfString).unwrap_or_default();
            let current_app_name = if handler.is_empty() {
                "未设置".to_string()
            } else {
                app_name_for_bundle_id(&handler)
            };
            DefaultAppEntry {
                extension: display_extension(extension),
                current_app_name,
                current_app_id: handler,
                can_change: true,
                detail: if uti.is_empty() {
                    "通过 LaunchServices 修改默认应用".to_string()
                } else {
                    format!("UTI: {}；通过 LaunchServices 直接修改", uti)
                },
            }
        })
        .collect()
}

/// 能打开该扩展名的候选应用
pub fn candidates(extension: &str) -> Result<Vec<DefaultAppCandidate>, String> {
    let extension = normalize_extension(extension);
    if extension.is_empty() {
        return Err("扩展名不能为空".to_string());
    }
    let content_type = content_type_for_extension(&extension)
        .ok_or_else(|| format!("无法识别扩展名 .{} 的文件类型", extension))?;
    let uti = content_type.as_ptr() as CfString;
    let handler_ids = all_handlers(uti);
    let current = default_handler(uti).unwrap_or_default();
    let bundles = scan_app_bundles();
    let scanned: HashMap<&str, &AppBundle> = bundles
        .iter()
        .map(|app| (app.bundle_id.as_str(), app))
        .collect();

    let mut candidates: Vec<DefaultAppCandidate> = Vec::new();
    for handler_id in handler_ids {
        let handler_id = handler_id.trim().to_string();
        if handler_id.is_empty() {
            continue;
        }
        // 优先用扫描到的信息（名称 + 真实路径）
        if let Some(app) = scanned.get(handler_id.as_str()) {
            candidates.push(DefaultAppCandidate {
                app_id: handler_id,
                name: app.name.clone(),
                path: app.path.to_string_lossy().to_string(),
            });
            continue;
        }
        // 系统应用或装在其它目录：让 LaunchServices 给出路径与名称
        let path = application_paths(&handler_id).into_iter().next();
        let name = path
            .as_deref()
            .and_then(app_name_from_path)
            .unwrap_or_else(|| handler_id.clone());
        candidates.push(DefaultAppCandidate {
            app_id: handler_id,
            name,
            path: path
                .map(|path| path.to_string_lossy().to_string())
                .unwrap_or_default(),
        });
    }
    let mut candidates = dedupe_candidates(candidates);
    sort_candidates(&mut candidates, &current);
    Ok(candidates)
}

/// 把扩展名的默认应用改成 `app_id`（bundle id）
pub fn set(extension: &str, app_id: &str) -> Result<(), String> {
    let extension = normalize_extension(extension);
    if extension.is_empty() {
        return Err("扩展名不能为空".to_string());
    }
    let app_id = app_id.trim();
    if app_id.is_empty() {
        return Err("应用标识不能为空".to_string());
    }
    let content_type = content_type_for_extension(&extension)
        .ok_or_else(|| format!("无法识别扩展名 .{} 的文件类型", extension))?;
    let handler = cf_string(app_id).ok_or_else(|| format!("无效的应用标识: {}", app_id))?;
    let status = unsafe {
        LSSetDefaultRoleHandlerForContentType(
            content_type.as_ptr() as CfString,
            LS_ROLES_ALL,
            handler.as_ptr() as CfString,
        )
    };
    if status != NO_ERR {
        return Err(format!(
            "系统拒绝了修改（LaunchServices 返回 {}）；请确认「{}」确实能打开 .{} 文件",
            status, app_id, extension
        ));
    }
    Ok(())
}

/// 打开系统设置。macOS 没有独立的「默认应用」面板（macOS 15 起才有部分入口），
/// 所以只把系统设置带到前台；具体类型直接在上面的列表里改。
pub fn reveal_settings() -> Result<(), String> {
    let status = std::process::Command::new("open")
        .arg("-b")
        .arg("com.apple.systempreferences")
        .status()
        .map_err(|error| format!("打开系统设置失败: {}", error))?;
    if status.success() {
        Ok(())
    } else {
        Err("打开系统设置失败（未找到系统设置应用）".to_string())
    }
}

/// macOS 不需要「打开方式」对话框：直接在列表里换即可
pub fn open_with(_extension: &str) -> Result<(), String> {
    Err("macOS 可直接在列表中更换默认应用，不需要打开方式对话框".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_app(dir: &Path, name: &str, bundle_id: &str) -> PathBuf {
        let bundle = dir.join(format!("{}.app", name));
        let contents = bundle.join("Contents");
        std::fs::create_dir_all(&contents).unwrap();
        std::fs::write(
            contents.join("Info.plist"),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>{}</string>
<key>CFBundleName</key><string>{}</string>
</dict></plist>"#,
                bundle_id, name
            ),
        )
        .unwrap();
        bundle
    }

    #[test]
    fn recognizes_app_bundle_layout() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = write_app(dir.path(), "Foo", "com.example.foo");
        assert!(is_app_bundle(&bundle));
        // 目录里没有 Info.plist 的「空壳 .app」不算应用
        let empty = dir.path().join("Empty.app");
        std::fs::create_dir_all(&empty).unwrap();
        assert!(!is_app_bundle(&empty));
        assert!(!is_app_bundle(dir.path()));
    }

    #[test]
    fn reads_app_name_from_bundle_plist() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = write_app(dir.path(), "Bar", "com.example.bar");
        assert_eq!(app_name_from_path(&bundle).as_deref(), Some("Bar"));
    }

    // 真实 LaunchServices 调用：只在人工联调时跑（会读系统默认应用，不修改）
    #[test]
    #[ignore]
    fn lists_real_default_apps() {
        let entries = list();
        assert_eq!(entries.len(), SUPPORTED_EXTENSIONS.len());
        for entry in &entries {
            assert!(!entry.extension.is_empty());
            assert!(entry.can_change);
        }
    }

    #[test]
    #[ignore]
    fn lists_real_candidates_without_changing_anything() {
        let candidates = candidates("pdf").expect("pdf 应该有候选应用");
        assert!(!candidates.is_empty());
        for candidate in &candidates {
            assert!(!candidate.app_id.is_empty());
        }
    }
}
