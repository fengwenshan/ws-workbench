use serde::{Deserialize, Serialize};

/// 老项目没有空间过滤字段时默认「跳过含空格的目录 / 文件」
fn default_true() -> bool {
    true
}

/// 打包排除规则：**命中任一规则的文件/目录不进包**（黑名单）。
///
/// 默认只列开发期产物与打包垃圾，其余文件全部进包：
/// - 隐藏文件/目录（`.git`、`.svn`、`.idea`、`.vscode`、`.DS_Store`…）
/// - 文档目录、文档、样式源码、sourcemap
/// - 测试文件与 `__tests__` 目录
/// - 依赖目录与历史压缩包
///
/// 下划线开头的文件/目录不再默认跳过，按普通文件打包；要排除就在项目里
/// 自己加 `_*`。规则语法与 `.gitignore` 一致，规则为空 = 不排除。
pub const DEFAULT_EXCLUDE: &[&str] = &[
    ".*",
    "docs/**",
    "*.md",
    "*.scss",
    "*.less",
    "*.map",
    "*.test.js",
    "*.spec.js",
    "__tests__",
    "node_modules",
    "*.zip",
];

/// 各版本新增的默认排除规则：(版本号, 规则列表)。
///
/// 迁移时只追加本版本新增的规则（以及显式移除的默认规则），
/// 不碰用户自己加的其它规则。
pub const DEFAULT_EXCLUDE_ADDITIONS: &[(u32, &[&str])] = &[
    // 2026-11-19：测试文件与 __tests__ 目录
    (2, &["*.test.js", "*.spec.js", "__tests__"]),
    // 2026-11-19：文档目录
    (4, &["docs/**"]),
];

/// 各版本移除的默认排除规则：(版本号, 规则列表)。
///
/// 只删这些具体默认规则，用户自己新增的其它规则不受影响。
pub const DEFAULT_EXCLUDE_REMOVALS: &[(u32, &[&str])] = &[(3, &["_*"])];

/// 当前排除规则版本号。以后新增/移除默认规则时递增，老配置由
/// [`migrate_default_exclude`] 按版本增量合并，不动用户自己加的其它规则。
pub const DEFAULT_EXCLUDE_VERSION: u32 = 4;

/// 默认排除规则列表
pub fn default_exclude_rules() -> Vec<String> {
    DEFAULT_EXCLUDE.iter().map(|rule| rule.to_string()).collect()
}

/// 老配置补齐当前版本的默认排除规则。
///
/// 返回是否发生变化。规则为空（老版本没有排除规则字段）或版本落后时，
/// 把缺的默认规则追加进去；用户自己加/删的规则不会被覆盖。
pub fn migrate_default_exclude(rules: &mut Vec<String>, version: &mut u32) -> bool {
    if *version >= DEFAULT_EXCLUDE_VERSION {
        return false;
    }
    let from_version = *version;
    let mut changed = false;

    // 版本 0 是还没有排除规则字段的老配置：补齐完整默认集。
    // 之后的版本只追加该版本新增的规则，避免把用户删掉的旧规则重新塞回去。
    let additions: Vec<&str> = if from_version == 0 {
        DEFAULT_EXCLUDE.iter().copied().collect()
    } else {
        DEFAULT_EXCLUDE_ADDITIONS
            .iter()
            .filter(|(added_in, _)| *added_in > from_version && *added_in <= DEFAULT_EXCLUDE_VERSION)
            .flat_map(|(_, rules)| rules.iter().copied())
            .collect()
    };

    for rule in additions {
        if !rules.iter().any(|existing| existing == rule) {
            rules.push(rule.to_string());
            changed = true;
        }
    }

    // 移除新版本不再想要的默认规则（例如 v3 去掉过宽的 `.*`）
    for (removed_in, removals) in DEFAULT_EXCLUDE_REMOVALS {
        if *removed_in > from_version && *removed_in <= DEFAULT_EXCLUDE_VERSION {
            let before = rules.len();
            rules.retain(|rule| !removals.iter().any(|removed| rule == removed));
            if rules.len() != before {
                changed = true;
            }
        }
    }

    *version = DEFAULT_EXCLUDE_VERSION;
    changed || from_version != DEFAULT_EXCLUDE_VERSION
}

/// 附件元信息。`id` 即磁盘上密文文件的主名（`<id>.wst`）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentMeta {
    pub id: String,
    pub name: String,
    pub size: u64,
    #[serde(default)]
    pub ext: String,
    #[serde(default)]
    pub created_at: i64,
    /// 该附件实际落盘目录（快照上传时的解析结果，便于删除/导出）
    #[serde(default)]
    pub dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectGroup {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub projects: Vec<ProjectConfig>,
    /// 是否为默认分组（默认分组不允许删除，但内部项目可删除）
    #[serde(default)]
    pub is_default: bool,
    #[serde(default)]
    pub created_at: String,
    /// 加密笔记（Markdown）
    #[serde(default)]
    pub note: String,
    /// 附件列表
    #[serde(default)]
    pub attachments: Vec<AttachmentMeta>,
}

/// 只随 config.json 保存的应用设置；业务数据改由 SQLite 承载
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    #[serde(default)]
    pub default_exclude: Vec<String>,
    #[serde(default)]
    pub default_exclude_version: u32,
    /// 上次选中的 VPN 配置
    #[serde(default)]
    pub vpn_active_profile_id: String,
    /// 注册免 UAC 计划任务时用的会话目录
    #[serde(default)]
    pub vpn_worker_session_dir: String,
    /// 全局默认附件上传目录（分组/项目未单独设置时使用）
    #[serde(default)]
    pub default_upload_dir: Option<String>,
    /// 项目树一级顺序：分组 id 与一级项目 id 的混排顺序。
    ///
    /// 分组和「不属于任何分组」的项目都排在一级，像文件管理器里文件夹和文件
    /// 混排；两个数组各自的顺序表达不了交错关系，所以单独存一份 id 序列。
    #[serde(default)]
    pub tree_order: Vec<String>,
    /// 开发者模式：开启后接收内测版（beta）更新；默认关闭，只收正式版
    #[serde(default)]
    pub developer_mode: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        AppSettings {
            default_exclude: default_exclude_rules(),
            default_exclude_version: DEFAULT_EXCLUDE_VERSION,
            vpn_active_profile_id: String::new(),
            vpn_worker_session_dir: String::new(),
            default_upload_dir: None,
            tree_order: vec![],
            developer_mode: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub groups: Vec<ProjectGroup>,
    /// 一级项目（不属于任何分组）
    #[serde(default)]
    pub projects: Vec<ProjectConfig>,
    #[serde(default)]
    pub default_exclude: Vec<String>,
    #[serde(default)]
    pub default_exclude_version: u32,
    /// VPN 配置列表
    #[serde(default)]
    pub vpn_profiles: Vec<crate::vpn::models::VpnProfile>,
    /// VPN 规则集列表
    #[serde(default)]
    pub vpn_rule_sets: Vec<crate::vpn::models::VpnRuleSet>,
    /// 上次选中的 VPN 配置
    #[serde(default)]
    pub vpn_active_profile_id: String,
    /// 注册免 UAC 计划任务时用的会话目录。
    ///
    /// 计划的 `/TR` 把会话目录路径写死了，会话目录一旦回退（用户目录被
    /// 受控文件夹访问/企业策略挡住），旧任务就作废了。这里记一份，
    /// 连接时和当前目录比一下即可，不必每次都去查任务定义。
    #[serde(default)]
    pub vpn_worker_session_dir: String,
    /// 全局默认附件上传目录（分组/项目未单独设置时使用）
    #[serde(default)]
    pub default_upload_dir: Option<String>,
    /// 项目树一级顺序：分组 id 与一级项目 id 的混排顺序（详见 [`AppSettings::tree_order`]）
    #[serde(default)]
    pub tree_order: Vec<String>,
    /// 开发者模式，见 [`AppSettings::developer_mode`]
    #[serde(default)]
    pub developer_mode: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            groups: vec![],
            projects: vec![],
            default_exclude: default_exclude_rules(),
            default_exclude_version: DEFAULT_EXCLUDE_VERSION,
            vpn_profiles: vec![],
            vpn_rule_sets: vec![],
            vpn_active_profile_id: String::new(),
            vpn_worker_session_dir: String::new(),
            default_upload_dir: None,
            tree_order: vec![],
            developer_mode: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub id: String,
    pub name: String,
    pub source_dir: String,
    pub output_dir: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    /// 加密笔记（Markdown）
    #[serde(default)]
    pub note: String,
    /// 附件列表
    #[serde(default)]
    pub attachments: Vec<AttachmentMeta>,
    /// layui 打包排除规则（黑名单）。`None` 表示沿用全局默认规则
    /// （`AppSettings::default_exclude`）；`Some` 为该项目自己的规则。
    /// 空数组在打包器里等价于「不排除、全部打包」，前端会把它归一成 `None`
    /// 回退到全局默认，避免误打包。
    #[serde(default)]
    pub pack_exclude: Option<Vec<String>>,
    /// 打包时跳过路径中含空格的目录（默认跳过；关掉后这类目录里的文件也会打包）
    #[serde(default = "default_true")]
    pub pack_skip_spaced_dirs: bool,
    /// 打包时跳过文件名含空格的文件（默认跳过；关掉后这类文件也会打包）
    #[serde(default = "default_true")]
    pub pack_skip_spaced_files: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackResult {
    pub success: bool,
    pub total_files: u64,
    pub copied_files: u64,
    pub skipped_files: u64,
    pub elapsed_ms: u64,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackProgress {
    pub phase: String,
    pub current: u64,
    pub total: u64,
    pub current_file: String,
    pub percentage: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectValidation {
    pub valid: bool,
    pub source_exists: bool,
    pub file_count: u64,
    pub warnings: Vec<String>,
}

/// 项目类型
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ProjectType {
    Vue2,
    Vue3,
    React,
    Layui,
    SpringBoot,
    Spring,
    Struts2,
    Java,
    Unknown,
}

/// 版本控制类型
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum VcsType {
    Git,
    Svn,
    None,
}

/// 源目录版本控制信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VcsInfo {
    pub vcs_type: VcsType,
    /// 远程仓库地址，未检测到则为空字符串
    #[serde(default)]
    pub url: String,
}


#[cfg(test)]
mod default_exclude_tests {
    use super::*;

    /// 默认排除集覆盖常见 VCS/IDE 目录、文档、样式源码、测试与依赖等
    #[test]
    fn default_rules_cover_dev_artifacts() {
        let rules = default_exclude_rules();
        let has = |rule: &str| rules.iter().any(|item| item == rule);
        for rule in [
            ".*", "docs/**", "*.md", "*.scss", "*.less", "*.map",
            "*.test.js", "*.spec.js", "__tests__", "node_modules", "*.zip",
        ] {
            assert!(has(rule), "默认排除规则缺少 {}", rule);
        }
        assert!(!has("_*"), "下划线开头不再默认排除");
    }

    /// 老项目缺空间过滤字段时默认两个都跳过
    #[test]
    fn project_space_flags_default_to_true() {
        let project: ProjectConfig = serde_json::from_str(
            r#"{"id":"p1","name":"n","source_dir":"/a","output_dir":"/b"}"#,
        )
        .unwrap();
        assert!(project.pack_skip_spaced_dirs);
        assert!(project.pack_skip_spaced_files);
    }

    #[test]
    fn default_rules_have_no_duplicates() {
        let rules = default_exclude_rules();
        let mut sorted = rules.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(rules.len(), sorted.len(), "默认排除规则里有重复项");
    }

    /// 老配置（没有 exclude 字段 / 版本 0）迁移后拿到完整默认集
    #[test]
    fn migration_from_legacy_config_fills_defaults() {
        let mut rules: Vec<String> = Vec::new();
        let mut version = 0;
        assert!(migrate_default_exclude(&mut rules, &mut version));
        assert_eq!(version, DEFAULT_EXCLUDE_VERSION);
        let mut sorted = rules.clone();
        sorted.sort();
        let mut expected = default_exclude_rules();
        expected.sort();
        assert_eq!(sorted, expected);

        // 再跑不重复、不变化
        let before = rules.len();
        assert!(!migrate_default_exclude(&mut rules, &mut version));
        assert_eq!(rules.len(), before);
    }

    /// 版本 1 → 当前版本：补新增规则、移除 `_*`，不复活用户删掉的旧规则
    #[test]
    fn migration_v1_only_adds_new_rules() {
        let mut rules = vec![
            ".*".to_string(),
            "_*".to_string(),
            "*.md".to_string(),
            "custom/**".to_string(),
        ];
        let mut version = 1;
        assert!(migrate_default_exclude(&mut rules, &mut version));
        assert_eq!(version, DEFAULT_EXCLUDE_VERSION);
        assert!(rules.iter().any(|rule| rule == "*.test.js"));
        assert!(rules.iter().any(|rule| rule == "*.spec.js"));
        assert!(rules.iter().any(|rule| rule == "__tests__"));
        assert!(rules.iter().any(|rule| rule == "docs/**"));
        assert!(!rules.iter().any(|rule| rule == "_*"), "`_*` 应被移除");
        assert!(rules.iter().any(|rule| rule == ".*"));
        assert!(
            !rules.iter().any(|rule| rule == "*.scss"),
            "旧版本里已删掉的规则不该被重新塞回来"
        );
    }

    /// 版本 2 → 当前版本：去掉 `_*`、补 `docs/**`，其余规则保持不动
    #[test]
    fn migration_v2_removes_underscore_rule() {
        let mut rules = vec![
            ".*".to_string(),
            "_*".to_string(),
            "*.md".to_string(),
            "custom/**".to_string(),
        ];
        let mut version = 2;
        assert!(migrate_default_exclude(&mut rules, &mut version));
        assert_eq!(version, DEFAULT_EXCLUDE_VERSION);
        assert!(!rules.iter().any(|rule| rule == "_*"), "`_*` 应被移除");
        assert!(rules.iter().any(|rule| rule == "docs/**"));
        assert!(rules.iter().any(|rule| rule == ".*"));
        assert!(rules.iter().any(|rule| rule == "*.md"));
        assert!(rules.iter().any(|rule| rule == "custom/**"));
        assert!(
            !rules.iter().any(|rule| rule == "*.test.js"),
            "版本 2 已经有过测试规则，不应重复追加"
        );
    }

    /// 版本 3 → 4：只补 `docs/**`
    #[test]
    fn migration_v3_adds_docs_dir() {
        let mut rules = vec![".*".to_string(), "*.md".to_string()];
        let mut version = 3;
        assert!(migrate_default_exclude(&mut rules, &mut version));
        assert_eq!(version, DEFAULT_EXCLUDE_VERSION);
        assert!(rules.iter().any(|rule| rule == "docs/**"));
        assert_eq!(rules.len(), 3);
    }

    /// 用户自己删掉的规则不会被重新塞回去（版本已是最新）
    #[test]
    fn migration_respects_user_edits_after_upgrade() {
        let mut rules = vec!["*.scss".to_string(), "*.md".to_string()];
        let mut version = DEFAULT_EXCLUDE_VERSION;
        assert!(!migrate_default_exclude(&mut rules, &mut version));
        assert_eq!(rules, vec!["*.scss", "*.md"]);
    }
}
