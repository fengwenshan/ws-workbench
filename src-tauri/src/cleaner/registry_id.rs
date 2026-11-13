//! 注册表条目身份解析：平台无关的字符串约定。
//!
//! 残留/关联清单里的注册表条目用 `win:{hive}|{key_path}` 表示，删除编排层
//! (`installer`) 和 Windows 平台实现 (`installer_windows`) 都会用到。
//! 这里只做字符串拆分与校验，不碰 `winreg`，所以两个平台的测试机都能跑单测；
//! 真正读写注册表的代码留在 `trash_guard` 的 `#[cfg(target_os = "windows")]`
//! 实现和 `installer_windows` 里。

/// 拆开的注册表条目 id 前缀（与 `installer_windows` 的 app id 约定一致）
pub(crate) const HIVE_HKLM: &str = "HKLM";
pub(crate) const HIVE_HKLM_WOW64: &str = "HKLM-WOW64";
pub(crate) const HIVE_HKCU: &str = "HKCU";

/// `win:{hive}|{key_path}` → (hive 令牌, 键路径)。注册表类残留的路径格式。
pub fn split_registry_id(path: &str) -> Option<(&str, &str)> {
    let rest = path.strip_prefix("win:")?;
    let (hive, key_path) = rest.split_once('|')?;
    if key_path.trim().is_empty() {
        return None;
    }
    Some((hive, key_path))
}

/// 校验注册表键路径：只允许 `SOFTWARE` 之下，不许 `..`、不许空段。
///
/// 返回规范化后的路径（统一反斜杠、去尾部反斜杠），调用方拿它去删。
pub fn validate_registry_key_path(key_path: &str) -> Result<String, String> {
    let normalized = key_path.trim().trim_end_matches('\\').replace('/', "\\");
    let lower = normalized.to_ascii_lowercase();
    if !lower.starts_with("software\\") || lower.len() <= "software\\".len() {
        return Err(format!("拒绝删除 SOFTWARE 之外的注册表键: {}", key_path));
    }
    if normalized
        .split('\\')
        .any(|segment| segment == ".." || segment.is_empty())
    {
        return Err(format!("非法注册表键路径: {}", key_path));
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_well_formed_registry_ids() {
        assert_eq!(
            split_registry_id("win:HKLM|SOFTWARE\\Foo\\Bar"),
            Some(("HKLM", "SOFTWARE\\Foo\\Bar"))
        );
        assert_eq!(
            split_registry_id("win:HKCU|SOFTWARE\\Foo"),
            Some(("HKCU", "SOFTWARE\\Foo"))
        );
    }

    #[test]
    fn rejects_malformed_registry_ids() {
        assert_eq!(split_registry_id("HKLM|SOFTWARE\\Foo"), None);
        assert_eq!(split_registry_id("win:HKLM"), None);
        assert_eq!(split_registry_id("win:HKLM|"), None);
        // 无前缀但含 `|` 也不算合法条目
        assert_eq!(split_registry_id("/Applications/Foo.app"), None);
    }

    #[test]
    fn validates_only_software_subtrees() {
        assert_eq!(
            validate_registry_key_path("SOFTWARE\\Vendor\\App"),
            Ok("SOFTWARE\\Vendor\\App".to_string())
        );
        // 斜杠归一、尾部反斜杠去掉
        assert_eq!(
            validate_registry_key_path("SOFTWARE/Vendor/App\\"),
            Ok("SOFTWARE\\Vendor\\App".to_string())
        );
        assert!(validate_registry_key_path("SYSTEM\\CurrentControlSet").is_err());
        assert!(validate_registry_key_path("SOFTWARE").is_err());
        assert!(validate_registry_key_path("SOFTWARE\\Foo\\..\\Bar").is_err());
        assert!(validate_registry_key_path("SOFTWARE\\\\Foo").is_err());
    }
}
