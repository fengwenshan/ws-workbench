//! 卸载窗口的「关联文件清单」组装（设计 4.1 / 4.2）。
//!
//! 这里只放平台无关的部分：kind → 分组映射、分组顺序与中文标签、去重、
//! 默认勾选、合计大小、`last_used` 文案与平台分派；具体候选（macOS 的
//! bundle 副本 / `mac_leftover_candidates`，Windows 的安装目录 / 残留扫描）
//! 由 `installer_macos` / `installer_windows` 生成，沿用平台文件避免跨文件
//! 调用私有逻辑。

use std::collections::HashSet;

use super::models::{CleanerProgress, InstalledApp, RelatedFileGroup, RelatedFileItem, RelatedFiles};

/// 分组 id（与前端 `RelatedFileGroup.id` 一致）
pub const GROUP_PROGRAM: &str = "program";
pub const GROUP_SUPPORT: &str = "support";
pub const GROUP_CACHE: &str = "cache";

/// 分组固定顺序：程序文件 → 支持文件 → 缓存文件（设计 2）
pub const GROUP_ORDER: [&str; 3] = [GROUP_PROGRAM, GROUP_SUPPORT, GROUP_CACHE];

/// 默认勾选的 kind：只包含「删掉不影响设置/账号/文件」的清理项，以及卸载本体。
///
/// 用户数据（`app_data` / `preference` / `container` / `launch_agent` / `registry` /
/// `shortcut` 等）默认**不勾选**：卸载默认保留设置、登录状态、插件配置，只有用户
/// 显式切到「彻底删除」或手动勾选才清理。缓存/日志/崩溃报告删了可以重建，保持默认勾选。
pub const DEFAULT_SELECTED_KINDS: &[&str] = &[
    "app_bundle",
    "install_dir",
    "cache",
    "log",
    "crash_report",
];

/// 单个条目是否默认勾选：低置信度一律不勾，且只勾 [`DEFAULT_SELECTED_KINDS`]。
pub fn default_selected(kind: &str, confidence: &str) -> bool {
    confidence != "low" && DEFAULT_SELECTED_KINDS.contains(&kind)
}

/// 分组中文标签
pub fn label_for_group(id: &str) -> &'static str {
    match id {
        GROUP_PROGRAM => "程序文件",
        GROUP_SUPPORT => "支持文件",
        GROUP_CACHE => "缓存文件",
        _ => "其它文件",
    }
}

/// kind → 分组（设计 4.2）：
/// - `app_bundle` / `install_dir` → 程序文件；
/// - `cache` / `log` / `crash_report` → 缓存文件；
/// - 其余（app_data/preference/container/launch_agent/registry/shortcut）→ 支持文件。
pub fn group_of_kind(kind: &str) -> &'static str {
    match kind {
        "app_bundle" | "install_dir" => GROUP_PROGRAM,
        "cache" | "log" | "crash_report" => GROUP_CACHE,
        _ => GROUP_SUPPORT,
    }
}

/// 置信度排序权重：high 在前、low 在最后（组内稳定顺序）
fn confidence_rank(confidence: &str) -> u8 {
    match confidence {
        "high" => 0,
        "medium" => 1,
        _ => 2,
    }
}

/// 把平台生成的条目组装成对前端的清单。
///
/// 规则（设计 3 / 4.2）：
/// - 按路径去重（保留先出现的那条，平台侧已按高置信度优先生成）；
/// - `selected = default_selected(kind, confidence)`：只默认勾选缓存/日志/崩溃报告
///   与卸载本体，用户数据默认保留；
/// - 分组顺序固定，空组剔除；
/// - 组内按置信度再按路径排序；
/// - 合计大小取所有条目（含默认不勾选的 low）。
pub(crate) fn build_related_files(
    app_id: &str,
    items: Vec<RelatedFileItem>,
    last_used: String,
) -> RelatedFiles {
    let mut seen: HashSet<String> = HashSet::new();
    let mut items: Vec<RelatedFileItem> = items
        .into_iter()
        .filter(|item| seen.insert(item.path.clone()))
        .collect();
    for item in items.iter_mut() {
        // 默认勾选规则（设计 4.2/4.4）：只勾缓存/日志/崩溃报告与卸载本体；
        // 设置、账号、插件配置等用户数据默认保留，必须由用户显式选择（前端
        // 「彻底删除」或手动勾选）。低置信度条目同样不勾。
        item.selected = default_selected(&item.kind, &item.confidence);
    }

    let total_bytes = items
        .iter()
        .fold(0u64, |sum, item| sum.saturating_add(item.size_bytes));
    let item_count = items.len();

    let groups: Vec<RelatedFileGroup> = GROUP_ORDER
        .iter()
        .filter_map(|group_id| {
            let mut group_items: Vec<RelatedFileItem> = items
                .iter()
                .filter(|item| group_of_kind(&item.kind) == *group_id)
                .cloned()
                .collect();
            if group_items.is_empty() {
                return None;
            }
            group_items.sort_by(|left, right| {
                confidence_rank(&left.confidence)
                    .cmp(&confidence_rank(&right.confidence))
                    .then_with(|| left.path.cmp(&right.path))
            });
            Some(RelatedFileGroup {
                id: group_id.to_string(),
                label: label_for_group(group_id).to_string(),
                items: group_items,
            })
        })
        .collect();

    RelatedFiles {
        app_id: app_id.to_string(),
        groups,
        total_bytes,
        item_count,
        last_used,
    }
}

/// 「使用情况」文案（平台无关）：`mdls` 原始时间 → 今天 / 7 天内 / 30 天内 /
/// 最近未使用；解析失败返回空串（前端隐藏该行）。
///
/// `now` 由调用方注入：生产环境传本地当前时间，单测传固定时间，不依赖时区。
///
/// 只有 macOS 的关联文件扫描会读 `mdls` 时间；Windows 侧没有这个文案，
/// 但单测仍要在所有平台跑这段纯逻辑。
#[cfg(any(target_os = "macos", test))]
pub fn last_used_text_from(raw: &str, now: chrono::DateTime<chrono::FixedOffset>) -> String {
    let Ok(parsed) = chrono::DateTime::parse_from_str(raw.trim(), "%Y-%m-%d %H:%M:%S %z") else {
        return String::new();
    };
    let days = now.signed_duration_since(parsed).num_days();
    if days < 0 {
        // 系统时间异常 / 未来时间：宁可不展示
        return String::new();
    }
    match days {
        0 => "今天使用过",
        1..=7 => "7天内使用过",
        8..=30 => "30天内使用过",
        _ => "最近未使用",
    }
    .to_string()
}

/// 扫描关联文件（平台分派）。`hint` 是列表缓存里的应用信息：应用包已被移走后
/// 仍能靠它拿到 bundle id / 名字继续扫（设计 4.2）。
pub fn related_files(
    app_id: &str,
    hint: Option<&InstalledApp>,
    emit: &dyn Fn(CleanerProgress),
) -> Result<RelatedFiles, String> {
    #[cfg(target_os = "macos")]
    {
        return super::installer_macos::related_files(app_id, hint, emit);
    }
    #[cfg(target_os = "windows")]
    {
        return super::installer_windows::related_files(app_id, hint, emit);
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (app_id, hint, emit);
        Err("当前平台不支持关联文件扫描".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(path: &str, kind: &str, confidence: &str, size: u64) -> RelatedFileItem {
        RelatedFileItem {
            path: path.to_string(),
            name: path.rsplit('/').next().unwrap_or(path).to_string(),
            kind: kind.to_string(),
            size_bytes: size,
            is_dir: kind != "preference",
            version: String::new(),
            confidence: confidence.to_string(),
            requires_elevation: false,
            exists: true,
            selected: false,
        }
    }

    #[test]
    fn kind_maps_to_expected_group() {
        assert_eq!(group_of_kind("app_bundle"), GROUP_PROGRAM);
        assert_eq!(group_of_kind("install_dir"), GROUP_PROGRAM);
        assert_eq!(group_of_kind("app_data"), GROUP_SUPPORT);
        assert_eq!(group_of_kind("preference"), GROUP_SUPPORT);
        assert_eq!(group_of_kind("container"), GROUP_SUPPORT);
        assert_eq!(group_of_kind("launch_agent"), GROUP_SUPPORT);
        assert_eq!(group_of_kind("registry"), GROUP_SUPPORT);
        assert_eq!(group_of_kind("shortcut"), GROUP_SUPPORT);
        assert_eq!(group_of_kind("cache"), GROUP_CACHE);
        assert_eq!(group_of_kind("log"), GROUP_CACHE);
        assert_eq!(group_of_kind("crash_report"), GROUP_CACHE);
        assert_eq!(label_for_group(GROUP_PROGRAM), "程序文件");
        assert_eq!(label_for_group(GROUP_SUPPORT), "支持文件");
        assert_eq!(label_for_group(GROUP_CACHE), "缓存文件");
    }

    #[test]
    fn build_related_files_groups_and_defaults() {
        let files = build_related_files(
            "mac:/Applications/Foo.app",
            vec![
                item("/Applications/Foo.app", "app_bundle", "high", 100),
                item("/Users/me/Library/Caches/com.example.foo", "cache", "high", 30),
                item("/Users/me/Library/Caches/com.example.foo/extra", "cache", "low", 7),
                item("/Users/me/Library/Preferences/com.example.foo.plist", "preference", "medium", 3),
                // 重复路径只保留先出现的（保留 high）
                item("/Applications/Foo.app", "app_bundle", "low", 999),
            ],
            "今天使用过".to_string(),
        );

        assert_eq!(files.item_count, 4, "去重后应剩 4 条");
        assert_eq!(files.total_bytes, 140, "合计大小取所有条目（含 low）");
        assert_eq!(files.last_used, "今天使用过");
        assert_eq!(
            files.groups.iter().map(|group| group.id.as_str()).collect::<Vec<_>>(),
            vec!["program", "support", "cache"],
            "分组顺序固定且空组剔除"
        );

        let program = &files.groups[0];
        assert_eq!(program.label, "程序文件");
        assert_eq!(program.items.len(), 1);
        assert!(program.items[0].selected, "程序文件（卸载本体）默认勾选");

        let support = &files.groups[1];
        assert_eq!(support.items.len(), 1);
        assert!(!support.items[0].selected, "用户数据（设置/账号等）默认不勾选");

        let cache = &files.groups[2];
        assert_eq!(cache.items.len(), 2);
        // 组内 high 在前、low 在后
        assert_eq!(cache.items[0].confidence, "high");
        assert!(cache.items[0].selected);
        assert_eq!(cache.items[1].confidence, "low");
        assert!(!cache.items[1].selected, "low 默认不勾选");
    }

    #[test]
    fn default_selection_keeps_user_data() {
        // 卸载本体与可重建的清理项默认勾选
        for (kind, confidence) in [
            ("app_bundle", "high"),
            ("install_dir", "medium"),
            ("cache", "high"),
            ("log", "medium"),
            ("crash_report", "high"),
        ] {
            assert!(default_selected(kind, confidence), "{kind} 应默认勾选");
        }
        // 用户数据默认保留：设置、账号、插件配置、启动项、注册表、快捷方式
        for kind in [
            "app_data",
            "preference",
            "container",
            "launch_agent",
            "registry",
            "shortcut",
        ] {
            assert!(!default_selected(kind, "high"), "{kind} 不应默认勾选");
        }
        // 低置信度一律不勾
        for kind in DEFAULT_SELECTED_KINDS {
            assert!(!default_selected(kind, "low"), "{kind} low 不应默认勾选");
        }
    }

    #[test]
    fn last_used_text_maps_days() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-10T12:00:00+08:00").unwrap();
        assert_eq!(
            last_used_text_from("2026-11-10 09:00:00 +0800", now),
            "今天使用过"
        );
        assert_eq!(
            last_used_text_from("2026-11-05 09:00:00 +0800", now),
            "7天内使用过"
        );
        assert_eq!(
            last_used_text_from("2026-10-20 09:00:00 +0800", now),
            "30天内使用过"
        );
        assert_eq!(
            last_used_text_from("2026-01-01 09:00:00 +0800", now),
            "最近未使用"
        );
        // mdls 未索引 / 解析失败：不展示
        assert_eq!(last_used_text_from("(null)", now), "");
        assert_eq!(last_used_text_from("not a date", now), "");
    }

    #[test]
    fn build_related_files_omits_empty_groups() {
        let files = build_related_files(
            "app",
            vec![item("/Users/me/Library/Caches/x", "cache", "high", 1)],
            String::new(),
        );
        assert_eq!(files.groups.len(), 1);
        assert_eq!(files.groups[0].id, GROUP_CACHE);
        assert!(files.last_used.is_empty());
    }
}
