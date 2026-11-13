use std::path::Path;

/// 从 Info.plist 读出来的 bundle 元数据
#[derive(Debug, Clone, Default)]
pub struct BundleInfo {
    pub bundle_id: Option<String>,
    pub name: String,
    pub version: String,
    /// `CFBundleExecutable`：界面暂不展示，留给后续按可执行文件名精确匹配进程
    #[allow(dead_code)]
    pub executable: Option<String>,
}

/// 读 `Contents/Info.plist`。解析失败 / 文件不存在返回 None（有的「应用」只是
/// 一个空壳目录，列表里应该跳过而不是报错）。
pub fn read_bundle_info(bundle: &Path) -> Option<BundleInfo> {
    let plist_path = bundle.join("Contents").join("Info.plist");
    let value = plist::Value::from_file(&plist_path).ok()?;
    let dict = value.as_dictionary()?;
    let get = |key: &str| {
        dict.get(key)
            .and_then(|value| value.as_string())
            .map(str::to_string)
            .filter(|text| !text.is_empty())
    };
    let stem_name = bundle
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_else(|| "未知应用".to_string());
    let name = get("CFBundleDisplayName")
        .or_else(|| get("CFBundleName"))
        .unwrap_or(stem_name);
    let version = get("CFBundleShortVersionString")
        .or_else(|| get("CFBundleVersion"))
        .unwrap_or_default();
    Some(BundleInfo {
        bundle_id: get("CFBundleIdentifier"),
        name,
        version,
        executable: get("CFBundleExecutable"),
    })
}

/// 应用是否允许卸载。
///
/// 路径在 `/System` 下、或 bundle id 以 `com.apple.` 开头 → 不可卸载。
/// 本版本不扫 `/System`，但判定逻辑留着：前端可能拿到旧缓存，SIP 应用
/// 即使被列出来也不能给卸载入口。
pub fn is_uninstallable(bundle: &Path) -> bool {
    if bundle.starts_with("/System") {
        return false;
    }
    match read_bundle_info(bundle) {
        Some(info) => !info
            .bundle_id
            .as_deref()
            .is_some_and(|id| id.starts_with("com.apple.")),
        // plist 读不出来时按「非系统路径即可卸载」处理，交给 trash 去报真实错误
        None => true,
    }
}
