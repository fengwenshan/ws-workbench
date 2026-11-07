//! 默认应用：常用文件类型的当前默认打开应用、候选应用与更换（设计 4.4）。
//!
//! 平台差异很大，所以按平台拆文件：
//! - `defaultapps_macos`：LaunchServices（UTType + `LSCopy/LSSetDefaultRoleHandlerForContentType`），
//!   可以真实修改；候选应用来自本机 `/Applications`、`~/Applications` 的 `.app` 包；
//! - `defaultapps_windows`：注册表只读展示（`UserChoice` → ProgId → `shell\open\command`）。
//!   Win8+ 的 `UserChoice` 带哈希签名，第三方写入会被系统重置（设计「非目标」），
//!   所以只提供「打开方式」对话框和「系统设置」入口，**不静默写注册表**。
//!
//! 平台无关的纯逻辑（扩展名归一化、UTI 兜底表、候选排序、Windows 关联解析）
//! 都放在这里或 `defaultapps_windows` 的纯逻辑段，保证单测在 macOS 上也能跑。

// 子模块文件放在 defaultapps.rs 同级（不是 defaultapps/ 目录），所以显式给 `#[path]`。
// 这样 `cleaner/mod.rs` 只需要一行 `pub mod defaultapps;`，平台文件由本模块自己声明。
#[cfg(target_os = "macos")]
#[path = "defaultapps_macos.rs"]
pub mod defaultapps_macos;
// 纯逻辑（关联解析 / 名称优先级 / 详情文案）在 test cfg 下也会编进 macOS 测试，
// 此时 Windows 专用部分用不到，单独允许 dead_code 避免噪音警告
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
#[path = "defaultapps_windows.rs"]
pub mod defaultapps_windows;

use serde::{Deserialize, Serialize};

/// 首批支持的扩展名（不带点、小写；设计 4.4）
pub const SUPPORTED_EXTENSIONS: &[&str] = &[
    "txt", "md", "pdf", "png", "jpg", "mp4", "mp3", "zip", "html", "json", "csv", "docx", "xlsx",
    "pptx",
];

/// 列表的一行：扩展名 → 当前默认应用。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefaultAppEntry {
    /// 展示用扩展名，带点：`.txt`
    pub extension: String,
    /// 当前默认应用名；没有关联时为空串 / 「未设置」
    pub current_app_name: String,
    /// macOS 是 bundle id，Windows 是注册表 ProgId；没有关联时空串
    pub current_app_id: String,
    /// 是否能在本应用内直接修改（macOS true；Windows false，系统限制）
    pub can_change: bool,
    /// 备注：UTI / ProgId / 引导说明等
    pub detail: String,
}

/// 一个候选应用（macOS：能打开该 UTI 的 bundle）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefaultAppCandidate {
    /// macOS 是 bundle id；Windows 当前留空（没有可靠候选来源）
    pub app_id: String,
    pub name: String,
    /// `.app` 包路径；Windows 为空串
    pub path: String,
}

/// 扩展名归一化：去空白、去通配前缀 `*.`、去所有前导点、小写。
///
/// 前端展示带点（`.txt`），命令参数允许带点或不带点，统一在这里收敛。
pub fn normalize_extension(raw: &str) -> String {
    let trimmed = raw.trim();
    let trimmed = trimmed.strip_prefix("*.").unwrap_or(trimmed);
    trimmed
        .trim_start_matches('.')
        .trim()
        .to_ascii_lowercase()
}

/// 是否属于首批支持的类型
#[allow(dead_code)]
pub fn is_supported_extension(extension: &str) -> bool {
    let normalized = normalize_extension(extension);
    SUPPORTED_EXTENSIONS.contains(&normalized.as_str())
}

/// 展示用扩展名（带点）；空输入返回空串
pub fn display_extension(extension: &str) -> String {
    let normalized = normalize_extension(extension);
    if normalized.is_empty() {
        String::new()
    } else {
        format!(".{}", normalized)
    }
}

/// 静态 UTI 映射表兜底（UTTypeCreatePreferredIdentifierForTag 不可用时用）。
///
/// 只覆盖首批类型；不在表里的扩展名返回 None，由调用方决定是报错还是显示
/// 「无法识别的文件类型」。
#[allow(dead_code)]
pub fn uti_fallback(extension: &str) -> Option<&'static str> {
    let normalized = normalize_extension(extension);
    let uti = match normalized.as_str() {
        "txt" => "public.plain-text",
        "md" => "net.daringfireball.markdown",
        "pdf" => "com.adobe.pdf",
        "png" => "public.png",
        "jpg" | "jpeg" => "public.jpeg",
        "mp4" => "public.mpeg-4",
        "mp3" => "public.mp3",
        "zip" => "public.zip-archive",
        "html" | "htm" => "public.html",
        "json" => "public.json",
        "csv" => "public.comma-separated-values-text",
        "docx" => "org.openxmlformats.wordprocessingml.document",
        "xlsx" => "org.openxmlformats.spreadsheetml.sheet",
        "pptx" => "org.openxmlformats.presentationml.presentation",
        _ => return None,
    };
    Some(uti)
}

/// 候选应用排序：当前默认的排最前，其余按名称（忽略大小写）再按 app id。
///
/// 候选来自 LaunchServices 的 handler 数组，顺序是系统内部的（通常是注册顺序），
/// 直接展示会让「当前默认」淹没在中间，所以统一排序。
#[allow(dead_code)]
pub fn sort_candidates(candidates: &mut [DefaultAppCandidate], current_app_id: &str) {
    let current = current_app_id.trim();
    candidates.sort_by(|left, right| {
        let left_current = !current.is_empty() && left.app_id == current;
        let right_current = !current.is_empty() && right.app_id == current;
        // true 先于 false：当前默认的排最前
        right_current
            .cmp(&left_current)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            .then_with(|| left.app_id.cmp(&right.app_id))
    });
}

/// 同一 bundle id 只保留一条（`/Applications` 与 `~/Applications` 可能存在重复副本）。
#[allow(dead_code)]
pub fn dedupe_candidates(candidates: Vec<DefaultAppCandidate>) -> Vec<DefaultAppCandidate> {
    let mut seen: Vec<String> = Vec::new();
    let mut kept: Vec<DefaultAppCandidate> = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        if candidate.app_id.trim().is_empty() || seen.contains(&candidate.app_id) {
            continue;
        }
        seen.push(candidate.app_id.clone());
        kept.push(candidate);
    }
    kept
}

/// 列出首批扩展名的当前默认应用
pub fn list() -> Result<Vec<DefaultAppEntry>, String> {
    #[cfg(target_os = "macos")]
    {
        return Ok(defaultapps_macos::list());
    }
    #[cfg(target_os = "windows")]
    {
        return Ok(defaultapps_windows::list());
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Err("当前平台不支持默认应用查询".to_string())
    }
}

/// 能打开该扩展名的候选应用。
///
/// macOS 返回真实候选；Windows 没有可靠来源（系统不允许第三方枚举关联候选），
/// 返回空列表，前端引导用户走「打开方式」对话框。
pub fn candidates(extension: &str) -> Result<Vec<DefaultAppCandidate>, String> {
    let normalized = normalize_extension(extension);
    if normalized.is_empty() {
        return Err("扩展名不能为空".to_string());
    }
    #[cfg(target_os = "macos")]
    {
        return defaultapps_macos::candidates(&normalized);
    }
    #[cfg(target_os = "windows")]
    {
        let _ = normalized;
        Ok(Vec::new())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = normalized;
        Err("当前平台不支持更换默认应用".to_string())
    }
}

/// 把扩展名的默认应用改成指定应用（macOS LaunchServices）。
pub fn set(extension: &str, app_id: &str) -> Result<(), String> {
    let normalized = normalize_extension(extension);
    if normalized.is_empty() {
        return Err("扩展名不能为空".to_string());
    }
    if app_id.trim().is_empty() {
        return Err("应用标识不能为空".to_string());
    }
    #[cfg(target_os = "macos")]
    {
        return defaultapps_macos::set(&normalized, app_id.trim());
    }
    #[cfg(target_os = "windows")]
    {
        let _ = (normalized, app_id);
        Err(
            "Windows 不允许第三方静默修改默认应用（UserChoice 有系统哈希保护），\
             请用「打开方式」对话框勾选「始终使用」或在系统设置里修改"
                .to_string(),
        )
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (normalized, app_id);
        Err("当前平台不支持更换默认应用".to_string())
    }
}

/// 打开系统的「默认应用」设置页
pub fn reveal_settings() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        return defaultapps_macos::reveal_settings();
    }
    #[cfg(target_os = "windows")]
    {
        return defaultapps_windows::reveal_settings();
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Err("当前平台不支持打开默认应用设置".to_string())
    }
}

/// 用指定扩展名调起系统的「打开方式」对话框（Windows 专用）
pub fn open_with(extension: &str) -> Result<(), String> {
    let normalized = normalize_extension(extension);
    if normalized.is_empty() {
        return Err("扩展名不能为空".to_string());
    }
    #[cfg(target_os = "macos")]
    {
        return defaultapps_macos::open_with(&normalized);
    }
    #[cfg(target_os = "windows")]
    {
        return defaultapps_windows::open_with(&normalized);
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = normalized;
        Err("当前平台不支持打开方式对话框".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(app_id: &str, name: &str) -> DefaultAppCandidate {
        DefaultAppCandidate {
            app_id: app_id.to_string(),
            name: name.to_string(),
            path: format!("/Applications/{}.app", name),
        }
    }

    #[test]
    fn normalizes_extension_with_or_without_dot() {
        assert_eq!(normalize_extension(".TXT"), "txt");
        assert_eq!(normalize_extension(" .Mp4 "), "mp4");
        assert_eq!(normalize_extension("json"), "json");
        assert_eq!(normalize_extension("*.PDF"), "pdf");
        assert_eq!(normalize_extension("...Md"), "md");
        assert_eq!(normalize_extension(""), "");
        assert_eq!(normalize_extension("   "), "");
        // 展示时统一带点
        assert_eq!(display_extension("TXT"), ".txt");
        assert_eq!(display_extension(".pdf"), ".pdf");
        assert_eq!(display_extension("  "), "");
    }

    #[test]
    fn supported_extension_matches_normalized_form() {
        for extension in SUPPORTED_EXTENSIONS {
            assert!(is_supported_extension(extension), "{} 应受支持", extension);
            assert!(is_supported_extension(&format!(".{}", extension)));
            assert!(is_supported_extension(&extension.to_uppercase()));
        }
        assert!(!is_supported_extension("exe"));
        assert!(!is_supported_extension(""));
    }

    #[test]
    fn uti_fallback_covers_every_supported_extension() {
        for extension in SUPPORTED_EXTENSIONS {
            assert!(
                uti_fallback(extension).is_some(),
                "{} 缺少 UTI 兜底映射",
                extension
            );
        }
        assert_eq!(uti_fallback(".PDF"), Some("com.adobe.pdf"));
        assert_eq!(uti_fallback("jpg"), Some("public.jpeg"));
        assert_eq!(uti_fallback("exe"), None);
    }

    #[test]
    fn sort_puts_current_first_then_name() {
        let mut candidates = vec![
            candidate("com.example.zeta", "zeta"),
            candidate("com.example.current", "Current App"),
            candidate("com.example.alpha", "alpha"),
        ];
        sort_candidates(&mut candidates, "com.example.current");
        assert_eq!(candidates[0].app_id, "com.example.current");
        assert_eq!(candidates[1].name, "alpha");
        assert_eq!(candidates[2].name, "zeta");

        // 没有当前默认时按名称排，不应 panic
        let mut candidates = vec![candidate("b", "Beta"), candidate("a", "alpha")];
        sort_candidates(&mut candidates, "");
        assert_eq!(candidates[0].name, "alpha");
        assert_eq!(candidates[1].name, "Beta");
    }

    #[test]
    fn dedupe_keeps_first_bundle_id_and_skips_empty() {
        let candidates = dedupe_candidates(vec![
            candidate("com.example.foo", "Foo"),
            candidate("com.example.foo", "Foo 副本"),
            candidate("", "无 bundle id"),
            candidate("com.example.bar", "Bar"),
        ]);
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].name, "Foo");
        assert_eq!(candidates[1].app_id, "com.example.bar");
    }
}
